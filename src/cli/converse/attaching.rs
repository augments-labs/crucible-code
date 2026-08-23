//! Turning a path somebody typed into a file the model can look at.
//!
//! A word in a prompt that names a workspace file of an attachable kind is
//! attached without asking, because naming it *is* the choice — the user typed
//! the path, and no tool went looking for it. What that leaves to decide is
//! whether the file can be sent at all, and every answer of no is a sentence
//! the user reads while they can still act on it.

use std::fs;
use std::io::{Cursor, Write as _};
use std::path::{Path, PathBuf};

use crucible_core::{Attachment, CEILING, Provider, Workspace, kind, written};
use crucible_runner::Runner;
use crucible_tui::{Renderer, Row, Terminal, TerminalError, fold};
use sha2::{Digest as _, Sha256};

use crate::cli::attachable;
use crate::cli::draw;
use crate::cli::style::Style;

/// What a prompt turned out to be carrying.
#[cfg(test)]
pub(super) struct Attaching {
    /// The files it named that may be sent, in the order it named them.
    pub(super) attachments: Box<[Attachment]>,
    /// What is said about the files it named that may not be.
    pub(super) refusals: Vec<String>,
}

/// Reads the prompt for files, and decides about each one.
#[cfg(test)]
pub(super) fn attaching(
    workspace: &Workspace,
    provider: &dyn Provider,
    model: &str,
    prompt: &str,
) -> Attaching {
    let mut attachments = Vec::new();
    let mut refusals = Vec::new();

    for word in names(prompt) {
        match decide(workspace, provider, model, &word, None) {
            Named::Attached(one) => attachments.push(one),
            Named::Refused(said) => refusals.push(said),
            Named::Nothing => {}
        }
    }

    Attaching {
        attachments: attachments.into_boxed_slice(),
        refusals,
    }
}

/// What the prompt is sending, with whatever it could not send said first.
///
/// Said before the turn starts and left on the screen, because a file that did
/// not go is something the reader acts on once the answer is in — not
/// something to show while they wait and take away again. Wrapped rather than
/// clipped: every one of these sentences keeps the next move in its second
/// half, and clipping is what would cut it off.
pub(super) fn beside<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &Runner,
    workspace: &Workspace,
    prompt: &str,
    style: Style,
) -> Result<Box<[Attachment]>, TerminalError> {
    let imported = runner.session().id().map(|id| {
        runner
            .session()
            .path()
            .with_file_name("attachments")
            .join(id.as_str())
    });
    let mut attachments = Vec::new();
    let mut refusals = Vec::new();

    for word in names(prompt) {
        match decide(
            workspace,
            runner.provider(),
            runner.model(),
            &word,
            imported.as_deref(),
        ) {
            Named::Attached(one) => attachments.push(one),
            Named::Refused(said) => refusals.push(said),
            Named::Nothing => {}
        }
    }
    let attachments = attachments.into_boxed_slice();

    // Before the refusals, because this is the line's own block closing over
    // what went with it, and a refusal is the next thing to read rather than
    // part of it.
    draw::attached(renderer, &attachments, workspace, style)?;

    for said in &refusals {
        let columns = renderer.columns();
        let rows: Vec<Row> = fold(&format!("! {said}"), columns)
            .into_iter()
            .map(|row| Row::new().then(crucible_tui::Slot::Plain, row))
            .collect();

        renderer.settle()?;
        renderer.present(&rows)?;
    }

    Ok(attachments)
}

/// Imports the image on the operating-system clipboard and names it for the prompt.
pub(super) fn clipboard(runner: &Runner) -> Result<String, String> {
    let Some(id) = runner.session().id() else {
        return Err("this session has nowhere durable to keep a clipboard image".to_owned());
    };
    let image = arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.get_image())
        .map_err(|_| "the clipboard does not hold a readable image".to_owned())?;
    let Some(expected) = image
        .width
        .checked_mul(image.height)
        .and_then(|pixels| pixels.checked_mul(4))
    else {
        return Err("the clipboard image is too large to measure".to_owned());
    };
    if image.bytes.len() != expected {
        return Err("the clipboard image has incomplete RGBA pixels".to_owned());
    }
    let width =
        u32::try_from(image.width).map_err(|_| "the clipboard image is too wide".to_owned())?;
    let height =
        u32::try_from(image.height).map_err(|_| "the clipboard image is too tall".to_owned())?;
    let pixels = image::RgbaImage::from_raw(width, height, image.bytes.into_owned())
        .ok_or_else(|| "the clipboard image has invalid RGBA pixels".to_owned())?;
    let mut bytes = Cursor::new(Vec::new());
    pixels
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|problem| format!("the clipboard image could not be encoded: {problem}"))?;
    let bytes = bytes.into_inner();
    if bytes.len() > CEILING {
        return Err(format!(
            "the clipboard image is larger than the {} MB one attachment may be",
            CEILING / (1024 * 1024)
        ));
    }

    let hash = <[u8; 32]>::from(Sha256::digest(&bytes));
    let directory = runner
        .session()
        .path()
        .with_file_name("attachments")
        .join(id.as_str());
    let path = import(&directory, "png", hash, &bytes)
        .map_err(|problem| format!("the clipboard image could not be imported: {problem}"))?;
    let path = written(&path);
    if path.contains(['\'', '"']) {
        return Err(
            "the clipboard attachment path contains a quote and cannot be put in the prompt"
                .to_owned(),
        );
    }

    Ok(format!("'{path}'"))
}

/// What one word in a prompt turned out to be.
enum Named {
    /// A file to send.
    Attached(Attachment),
    /// A file that will not be sent, and the sentence saying why.
    Refused(String),
    /// Not a file at all. Most words are this, and so is `main.rs`: an
    /// extension no model reads as anything but text is a word in a sentence,
    /// and refusing it would be noise on every prompt that mentions a file.
    Nothing,
}

/// Everything that has to hold before a file is put in front of a model.
///
/// The order is the order a person can act in. What the protocol cannot spell
/// is nothing they can do anything about; what the model cannot read they fix
/// with `/model`; what is too large they fix with a smaller copy. Reading the
/// file comes last, because the three answers above it cost nothing.
fn decide(
    workspace: &Workspace,
    provider: &dyn Provider,
    model: &str,
    word: &str,
    imported: Option<&Path>,
) -> Named {
    let Some(kind) = kind(word) else {
        return Named::Nothing;
    };
    let source = match workspace.existing(word) {
        Ok(path) => Source::Workspace(path),
        Err(_) if Path::new(word).is_absolute() => {
            let Ok(path) = Path::new(word).canonicalize() else {
                return Named::Nothing;
            };
            Source::External(path)
        }
        Err(_) => return Named::Nothing,
    };
    let Some(size) = sized(source.path()) else {
        return Named::Nothing;
    };

    // The intersection, taken in halves. `attachable` is the whole of it, and
    // asking the provider first is the only thing that lets a refusal say
    // which side of it said no — which is the difference between a sentence
    // somebody can act on and one they cannot.
    if !provider.spells().contains(kind.modality) {
        return Named::Refused(format!(
            "{word} is not attached: crucible's {} requests have no shape for {}. Nothing you \
             type changes that — a later release adds the shape.",
            provider.name(),
            kind.spoken(),
        ));
    }
    let Some(accepts) = attachable(provider, model) else {
        return Named::Refused(format!(
            "{word} is not attached: this build has no entry for {model}, so it does not know \
             what that model reads. That is not a refusal. /model names one it knows.",
        ));
    };
    if !accepts.contains(kind.modality) {
        return Named::Refused(format!(
            "{word} is not attached: {model} does not read {}. /model picks one that does.",
            kind.spoken(),
        ));
    }

    if size > CEILING as u64 {
        return Named::Refused(format!(
            "{word} is larger than the {} MB one attachment may be, so it is not attached. A \
             smaller copy of it would be.",
            CEILING / (1024 * 1024),
        ));
    }

    let Ok(bytes) = fs::read(source.path()) else {
        return Named::Nothing;
    };
    if !(kind.confirms)(&bytes) {
        return Named::Refused(format!(
            "{word} is not attached: it is named .{0} and its bytes are not a {0}. Rename it to \
             what it is.",
            kind.extension,
        ));
    }

    let hash = <[u8; 32]>::from(Sha256::digest(&bytes));
    let path = match source {
        Source::Workspace(path) => path.as_path().to_owned(),
        Source::External(_) => {
            let Some(directory) = imported else {
                return Named::Refused(format!(
                    "{word} is outside the workspace and this session has nowhere durable to import it."
                ));
            };
            match import(directory, kind.extension, hash, &bytes) {
                Ok(path) => path,
                Err(problem) => {
                    return Named::Refused(format!(
                        "{word} could not be imported for this session: {problem}"
                    ));
                }
            }
        }
    };

    Named::Attached(Attachment {
        path: written(&path).into_boxed_str(),
        modality: kind.modality,
        media_type: kind.media_type.into(),
        hash,
    })
}

/// A file already under agent reach, or one the user alone selected.
enum Source {
    Workspace(crucible_core::WorkspacePath),
    External(PathBuf),
}

impl Source {
    fn path(&self) -> &Path {
        match self {
            Self::Workspace(path) => path.as_path(),
            Self::External(path) => path,
        }
    }
}

/// Copies user-selected bytes somewhere a resumed session can still find them.
fn import(
    directory: &Path,
    extension: &str,
    hash: [u8; 32],
    bytes: &[u8],
) -> Result<PathBuf, std::io::Error> {
    crucible_privacy::directory(directory).map_err(crucible_privacy::PrivacyError::into_io)?;
    let name = format!("{}.{extension}", hex(&hash));
    let destination = directory.join(name);

    match crucible_privacy::create_write(&destination) {
        Ok(mut file) => {
            if let Err(problem) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                drop(file);
                let _ = fs::remove_file(&destination);
                return Err(problem);
            }
            crucible_privacy::sync_parent(&destination)
                .map_err(crucible_privacy::PrivacyError::into_io)?;
        }
        Err(problem) if problem.kind() == std::io::ErrorKind::AlreadyExists => {
            if !fs::read(&destination).is_ok_and(|existing| existing == bytes) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "the content-addressed destination holds different bytes",
                ));
            }
        }
        Err(problem) => return Err(problem.into_io()),
    }

    Ok(destination)
}

/// Lowercase hexadecimal, used as the stable name of imported bytes.
fn hex(hash: &[u8; 32]) -> String {
    let mut out = String::with_capacity(hash.len() * 2);
    for byte in hash {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

/// Prompt words, with a quoted path kept whole instead of split at its spaces.
fn names(prompt: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut one = String::new();
    let mut quote = None;

    for character in prompt.chars() {
        match (quote, character) {
            (Some(open), close) if open == close => {
                quote = None;
                if !one.is_empty() {
                    names.push(std::mem::take(&mut one));
                }
            }
            (None, '\'' | '"') if one.is_empty() => quote = Some(character),
            (None, character) if character.is_whitespace() => {
                if !one.is_empty() {
                    names.push(std::mem::take(&mut one));
                }
            }
            (Some(_) | None, character) => one.push(character),
        }
    }
    if !one.is_empty() {
        names.push(one);
    }

    names
}

/// How large the named file is, or `None` where it is not a regular file.
fn sized(path: &Path) -> Option<u64> {
    let about = fs::metadata(path).ok()?;
    about.is_file().then_some(about.len())
}

#[cfg(test)]
mod tests;
