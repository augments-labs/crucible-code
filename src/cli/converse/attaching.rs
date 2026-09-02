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

use crucible_core::{
    Attachment, CEILING, Modalities, Provider, SessionId, Workspace, kind, written,
};
use crucible_runner::Runner;
use crucible_tui::{Renderer, Row, Terminal, TerminalError, fold};
use sha2::{Digest as _, Sha256};

use crate::cli::draw;
use crate::cli::style::Style;

use super::Held;

/// Refreshes the durable image store after a command may replace the session.
pub(super) fn refresh_store(held: &mut Held<'_>, runner: &Runner) {
    let store = runner
        .session()
        .id()
        .cloned()
        .map(|id: SessionId| (runner.session().path().to_owned(), id));
    if held.attachment_store != store {
        // A platform clipboard connection has no session identity itself, but
        // dropping it here prevents a long-lived handle from crossing a session
        // reset and keeps retry semantics simple after a display reconnect.
        held.clipboard = None;
    }
    held.attachment_store = store;
}

/// What a prompt turned out to be carrying.
#[cfg(test)]
pub(super) struct Attaching {
    /// The files it named that may be sent, in the order it named them.
    pub(super) attachments: Box<[Attachment]>,
    /// What is said about the files it named that may not be.
    pub(super) refusals: Vec<String>,
}

/// One prompt, beside the pasted images its `[Image #N]` markers can name.
///
/// One value because the two are read together everywhere a prompt is read:
/// a marker is a word standing for the Nth path here, and a prompt handed
/// over without the list is one whose markers name nothing.
#[derive(Clone, Copy)]
pub(super) struct Sent<'a> {
    /// What was typed.
    pub(super) prompt: &'a str,
    /// The paths pasted so far, `[Image #1]` first.
    pub(super) images: &'a [Box<str>],
}

/// Who the prompt is going to, and what this session says they can be given.
///
/// The two halves of "can this file be sent" travel together because a refusal
/// has to name which of them said no. `reads` is the model's half as the
/// session itself holds it — the same answer the next request carries — and
/// `None` there is nobody having said what the model reads rather than a model
/// that reads nothing.
#[derive(Clone, Copy)]
pub(super) struct Asking<'a> {
    /// The protocol a request is written to.
    pub(super) provider: &'a dyn Provider,
    /// The model it names, as that provider spells it.
    pub(super) model: &'a str,
    /// What the model reads, where the session was told.
    pub(super) reads: Option<Modalities>,
}

impl<'a> Asking<'a> {
    /// What the session in hand is asking, read off the runner driving it.
    pub(super) fn of(runner: &'a Runner) -> Self {
        Self {
            provider: runner.provider(),
            model: runner.model(),
            reads: runner.reads(),
        }
    }
}

/// Reads the prompt for files, and decides about each one.
#[cfg(test)]
pub(super) fn attaching(
    workspace: &Workspace,
    asking: Asking<'_>,
    sent: Sent<'_>,
    imported: Option<&Path>,
) -> Attaching {
    let (attachments, refusals) = gathered(workspace, asking, sent, imported);

    Attaching {
        attachments: attachments.into_boxed_slice(),
        refusals,
    }
}

/// Everything one prompt is sending: the files its words name, and the images
/// its `[Image #N]` markers point back at.
///
/// One function under both readers of a prompt, because the two must agree on
/// what a prompt carries or the tested answer is not the shipped one. An image
/// attached twice — marked twice, or marked and named — goes once: the model
/// would be handed the same bytes beside themselves.
fn gathered(
    workspace: &Workspace,
    asking: Asking<'_>,
    sent: Sent<'_>,
    imported: Option<&Path>,
) -> (Vec<Attachment>, Vec<String>) {
    let Sent { prompt, images } = sent;
    let mut attachments: Vec<Attachment> = Vec::new();
    let mut refusals = Vec::new();
    let mut one = |named: Named, attachments: &mut Vec<Attachment>| match named {
        Named::Attached(one) => {
            if !attachments.iter().any(|have| have.hash == one.hash) {
                attachments.push(one);
            }
        }
        Named::Refused(said) => refusals.push(said),
        Named::Nothing => {}
    };

    for word in names(prompt) {
        one(decide(workspace, asking, &word, imported), &mut attachments);
    }
    for mark in marked(prompt) {
        let Some(path) = mark.checked_sub(1).and_then(|at| images.get(at)) else {
            continue;
        };
        one(decide(workspace, asking, path, imported), &mut attachments);
    }

    (attachments, refusals)
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
    sent: Sent<'_>,
    style: Style,
) -> Result<Box<[Attachment]>, TerminalError> {
    let imported = runner.session().id().map(|id| {
        runner
            .session()
            .path()
            .with_file_name("attachments")
            .join(id.as_str())
    });
    let (attachments, refusals) =
        gathered(workspace, Asking::of(runner), sent, imported.as_deref());
    let attachments = attachments.into_boxed_slice();

    // Before the refusals, because this is the line's own block closing over
    // what went with it, and a refusal is the next thing to read rather than
    // part of it.
    draw::attached(renderer, &attachments, style)?;

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

/// Imports the image on the operating-system clipboard, and answers with the
/// path an `[Image #N]` marker will stand for.
///
/// Copying a *file* puts its name on the clipboard rather than its pixels, so
/// a clipboard with no image on it is read again as text before the paste is
/// refused — and where the text names an image file, that file is the paste.
/// It goes back as its own path rather than an imported copy, so submission
/// decides about it the way it decides about a path typed by hand.
pub(super) fn clipboard(
    path: &Path,
    id: &crucible_core::SessionId,
    board: &mut arboard::Clipboard,
) -> Result<String, String> {
    let image = match board.get_image() {
        Ok(image) => image,
        Err(problem) => {
            if let Some(path) = board.get_text().ok().and_then(|text| pictured(&text)) {
                return Ok(written(&path));
            }
            return Err(format!(
                "the clipboard does not hold a readable image: {problem}"
            ));
        }
    };
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
    let directory = path.with_file_name("attachments").join(id.as_str());
    let path = import(&directory, "png", hash, &bytes)
        .map_err(|problem| format!("the clipboard image could not be imported: {problem}"))?;

    Ok(written(&path))
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
fn decide(workspace: &Workspace, asking: Asking<'_>, word: &str, imported: Option<&Path>) -> Named {
    let Asking {
        provider,
        model,
        reads,
    } = asking;
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

    // The two halves, asked in order. Asking the provider first is the only
    // thing that lets a refusal say which side of it said no — which is the
    // difference between a sentence somebody can act on and one they cannot.
    if !provider.spells().contains(kind.modality) {
        return Named::Refused(format!(
            "{word} is not attached: crucible's {} requests have no shape for {}. Nothing you \
             type changes that — a later release adds the shape.",
            provider.name(),
            kind.spoken(),
        ));
    }
    let Some(accepts) = reads else {
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

/// The `N` of every `[Image #N]` marker in a prompt, in the order they appear.
///
/// Exactly that shape: an open bracket, the capitalized word, a space, a hash,
/// digits, and a close bracket. Anything looser and prose about images starts
/// sending files.
fn marked(prompt: &str) -> Vec<usize> {
    const OPENS: &str = "[Image #";
    let mut numbers = Vec::new();
    let mut rest = prompt;

    while let Some(at) = rest.find(OPENS) {
        rest = &rest[at + OPENS.len()..];
        if let Some(end) = rest.find(']')
            && !rest[..end].is_empty()
            && rest[..end].bytes().all(|byte| byte.is_ascii_digit())
            && let Ok(number) = rest[..end].parse()
        {
            numbers.push(number);
        }
    }

    numbers
}

/// The image file a clipboard's text names, where it names one.
///
/// Copying a file in a file manager puts a percent-encoded `file://` URI on
/// the clipboard, one per line; copying a path out of a shell puts it bare.
/// Either way what decides is the file itself: it exists, and it is of a kind
/// some model reads — a copied `main.rs` is not a failed image paste.
fn pictured(text: &str) -> Option<PathBuf> {
    let line = text.lines().next()?.trim();
    let path = if let Some(uri) = line.strip_prefix("file://") {
        // `file://host/path` names another machine's file; only an empty
        // authority — `file:///path` — is this one's.
        let mut spelled = decoded(uri.strip_prefix('/').map(|_| uri)?);
        // On Windows the slash that marked the empty authority stands before
        // the drive letter — `file:///C:/…` spells `C:/…`.
        if cfg!(windows) && spelled.as_bytes().get(2) == Some(&b':') {
            spelled.remove(0);
        }
        PathBuf::from(spelled)
    } else if Path::new(line).is_absolute() {
        PathBuf::from(line)
    } else {
        return None;
    };

    (kind(&written(&path)).is_some() && path.is_file()).then_some(path)
}

/// A percent-encoded URI path, back to the characters it spells.
fn decoded(text: &str) -> String {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.bytes();

    while let Some(byte) = rest.next() {
        let escaped = || {
            let high = char::from(rest.clone().next()?).to_digit(16)?;
            let low = char::from(rest.clone().nth(1)?).to_digit(16)?;
            u8::try_from(high * 16 + low).ok()
        };
        match (byte, escaped()) {
            (b'%', Some(spelled)) => {
                bytes.push(spelled);
                rest.nth(1);
            }
            _ => bytes.push(byte),
        }
    }

    String::from_utf8(bytes).unwrap_or_else(|_| text.to_owned())
}

/// How large the named file is, or `None` where it is not a regular file.
fn sized(path: &Path) -> Option<u64> {
    let about = fs::metadata(path).ok()?;
    about.is_file().then_some(about.len())
}

#[cfg(test)]
mod tests;
