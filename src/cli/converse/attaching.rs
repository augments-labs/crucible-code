//! Turning a path somebody typed into a file the model can look at.
//!
//! A word in a prompt that names a workspace file of an attachable kind is
//! attached without asking, because naming it *is* the choice — the user typed
//! the path, and no tool went looking for it. What that leaves to decide is
//! whether the file can be sent at all, and every answer of no is a sentence
//! the user reads while they can still act on it.

use std::fs;

use crucible_core::{Attachment, Modality, Provider, Workspace, written};
use crucible_runner::{Runner, attachments::CEILING};
use crucible_tui::{Renderer, Row, Terminal, TerminalError, fold};
use sha2::{Digest as _, Sha256};

use crate::cli::attachable;

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
) -> Result<Box<[Attachment]>, TerminalError> {
    let Attaching {
        attachments,
        refusals,
    } = attaching(workspace, runner.provider(), runner.model(), prompt);

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
    let Some(kind) = KINDS.iter().find(|kind| kind.names(word)) else {
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

/// One kind of file that may be attached, under the name it goes by.
struct Kind {
    /// The extension, without its dot, as a prompt would spell it.
    extension: &'static str,
    /// What the model would be asked to do with it.
    modality: Modality,
    /// What the provider labels the bytes with.
    media_type: &'static str,
    /// Whether the bytes are what the extension claims.
    confirms: fn(&[u8]) -> bool,
}

impl Kind {
    /// Whether a word in a prompt is a path spelled with this extension.
    ///
    /// Case-insensitive on the extension alone: a camera writes `IMG_0001.JPG`
    /// and a person types what the camera wrote.
    fn names(&self, word: &str) -> bool {
        word.rsplit_once('.')
            .is_some_and(|(_, tail)| tail.eq_ignore_ascii_case(self.extension))
    }

    /// The kind as it appears mid-sentence, with the article English wants.
    fn spoken(&self) -> String {
        let article = match self.modality {
            Modality::Image | Modality::Audio => "an",
            Modality::Text | Modality::Pdf | Modality::Video => "a",
        };

        format!("{article} {}", self.modality.as_str())
    }
}

/// Every kind crucible will attach: the picture formats all three vendors
/// document accepting, and the one document format any of them reads.
///
/// A closed list rather than a guess from the extension, because the cost of
/// being wrong is a refused request the user paid for. Anything not here is
/// text, and the `read` tool already opens it.
const KINDS: &[Kind] = &[
    Kind {
        extension: "png",
        modality: Modality::Image,
        media_type: "image/png",
        confirms: png,
    },
    Kind {
        extension: "jpg",
        modality: Modality::Image,
        media_type: "image/jpeg",
        confirms: jpeg,
    },
    Kind {
        extension: "jpeg",
        modality: Modality::Image,
        media_type: "image/jpeg",
        confirms: jpeg,
    },
    Kind {
        extension: "gif",
        modality: Modality::Image,
        media_type: "image/gif",
        confirms: gif,
    },
    Kind {
        extension: "webp",
        modality: Modality::Image,
        media_type: "image/webp",
        confirms: webp,
    },
    Kind {
        extension: "pdf",
        modality: Modality::Pdf,
        media_type: "application/pdf",
        confirms: pdf,
    },
];

/// The eight bytes a PNG starts with, of which the last four catch a file a
/// transfer has rewritten the line endings of.
fn png(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
}

/// Every JPEG starts with a start-of-image marker and the next marker's
/// introducer. What follows differs by encoder, so three bytes is the whole of
/// what is common to all of them.
fn jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8, 0xff])
}

/// The two GIF versions, both still written by something.
fn gif(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")
}

/// A WebP is a RIFF container, and the four bytes saying which kind sit after
/// the length rather than beside the tag.
fn webp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(&b"WEBP"[..])
}

/// The header a PDF opens with, version and all.
fn pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

#[cfg(test)]
mod tests;
