//! Turning a path somebody typed into a file the model can look at.
//!
//! A word in a prompt that names a workspace file of an attachable kind is
//! attached without asking, because naming it *is* the choice — the user typed
//! the path, and no tool went looking for it. What that leaves to decide is
//! whether the file can be sent at all, and every answer of no is a sentence
//! the user reads while they can still act on it.

use std::fs;

use crucible_core::{Attachment, CEILING, Provider, Workspace, kind, written};
use crucible_runner::Runner;
use crucible_tui::{Renderer, Row, Terminal, TerminalError, fold};
use sha2::{Digest as _, Sha256};

use crate::cli::attachable;
use crate::cli::draw;
use crate::cli::style::Style;

/// What a prompt turned out to be carrying.
pub(super) struct Attaching {
    /// The files it named that may be sent, in the order it named them.
    pub(super) attachments: Box<[Attachment]>,
    /// What is said about the files it named that may not be.
    pub(super) refusals: Vec<String>,
}

/// Reads the prompt for files, and decides about each one.
pub(super) fn attaching(
    workspace: &Workspace,
    provider: &dyn Provider,
    model: &str,
    prompt: &str,
) -> Attaching {
    let mut attachments = Vec::new();
    let mut refusals = Vec::new();

    for word in prompt.split_whitespace() {
        match decide(workspace, provider, model, word) {
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
    let Attaching {
        attachments,
        refusals,
    } = attaching(workspace, runner.provider(), runner.model(), prompt);

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
fn decide(workspace: &Workspace, provider: &dyn Provider, model: &str, word: &str) -> Named {
    let Some(kind) = kind(word) else {
        return Named::Nothing;
    };
    let (Ok(path), Some(size)) = (workspace.existing(word), sized(workspace, word)) else {
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

    let Ok(bytes) = fs::read(path.as_path()) else {
        return Named::Nothing;
    };
    if !(kind.confirms)(&bytes) {
        return Named::Refused(format!(
            "{word} is not attached: it is named .{0} and its bytes are not a {0}. Rename it to \
             what it is.",
            kind.extension,
        ));
    }

    Named::Attached(Attachment {
        path: written(path.as_path()).into_boxed_str(),
        modality: kind.modality,
        media_type: kind.media_type.into(),
        hash: <[u8; 32]>::from(Sha256::digest(&bytes)),
    })
}

/// How large the named file is, or `None` where it is not a file at all.
///
/// Asked before the bytes are, so a file too large to send is never read. A
/// directory named `pictures.png` answers `None` here and is a word in a
/// sentence, which is what it was.
fn sized(workspace: &Workspace, word: &str) -> Option<u64> {
    let path = workspace.existing(word).ok()?;
    let about = fs::metadata(path.as_path()).ok()?;

    about.is_file().then_some(about.len())
}

#[cfg(test)]
mod tests;
