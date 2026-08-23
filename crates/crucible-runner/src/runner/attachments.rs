//! What a request's attachments come to, for exactly one request.
//!
//! The transcript holds a reference to every file a prompt was given, and holds
//! it for the life of the session. Bytes are the other half of that bargain:
//! they are read here, handed to one request, and dropped when it returns. A
//! session's memory therefore grows with its text and not with its pictures,
//! which is the only reason a picture may be attached at all.
//!
//! A request has a ceiling and a transcript does not, so the two cannot always
//! agree. Where they differ the request loses bytes, never the transcript, and
//! never the turn: the newest attachments that fit are read, and every older
//! one keeps its place in the request carrying a line that says so.

use std::fs;
use std::path::Path;

use sha2::{Digest as _, Sha256};

use crucible_core::{Attached, Attachment, Content, Message, Modality, Transcript};

/// The most raw attachment bytes one request may carry.
///
/// Not a vendor's limit — this one binds first. The peak a request reaches is
/// about 2.33 times this figure, because the bytes and the serialized body are
/// alive together and the body holds them base64-encoded; 6 MB therefore costs
/// about 14 MB, which is what the 35 MB budget in `performance-budgets.md` has
/// room for once a long text session's own crest is paid for.
///
/// It is deliberately below what the headroom allows. A budget spent to its
/// last megabyte is not a budget.
pub const CEILING: usize = 6 * 1024 * 1024;

/// One request's worth of attachments, owned until the request returns.
pub(crate) struct Resolved(Vec<Held>);

/// One attachment, and whatever it came to.
struct Held {
    message: usize,
    index: usize,
    media_type: Box<str>,
    modality: Modality,
    carried: Carried,
}

/// The owned form of [`Content`].
enum Carried {
    Bytes(Vec<u8>),
    Instead(String),
}

/// Reads what this request can carry, newest first.
///
/// Newest first because the ceiling has to fall on something, and the picture a
/// user is talking about now is the one they are talking about now. By bytes
/// rather than by count: two screenshots and twenty icons are the same number
/// of files and not the same problem.
pub(crate) fn resolve(transcript: &Transcript) -> Resolved {
    let mut held = Vec::new();
    let mut spent = 0;

    for (message, said) in transcript.messages().iter().enumerate().rev() {
        let Message::User { attachments, .. } = said else {
            continue;
        };
        for (index, attachment) in attachments.iter().enumerate().rev() {
            held.push(Held {
                message,
                index,
                media_type: attachment.media_type.clone(),
                modality: attachment.modality,
                carried: read(attachment, &mut spent),
            });
        }
    }

    // Walked backwards to decide, handed over forwards to be written: a body
    // module puts content where the prompt put it, and that order is the
    // transcript's.
    held.reverse();
    Resolved(held)
}

/// The file, or the line that takes its place.
fn read(attachment: &Attachment, spent: &mut usize) -> Carried {
    let Ok(bytes) = fs::read(Path::new(attachment.path.as_ref())) else {
        return instead(attachment, "because it could not be read");
    };
    if <[u8; 32]>::from(Sha256::digest(&bytes)) != attachment.hash {
        return instead(attachment, "because it changed after it was attached");
    }
    if bytes.len() > CEILING.saturating_sub(*spent) {
        return instead(attachment, "to keep the request within its size limit");
    }
    *spent = spent.saturating_add(bytes.len());
    Carried::Bytes(bytes)
}

/// One line, in place of one file.
///
/// The next move is a call the model can already make, so the line ends in it
/// rather than in an apology — and it is offered even where the file has just
/// been found missing, because what was seen here a moment ago is not what a
/// `read` will see, and the tool that owns the file answers that better than a
/// guess would.
fn instead(attachment: &Attachment, why: &str) -> Carried {
    Carried::Instead(format!(
        "{} is not attached to this request, {why}: read it again if you need it.",
        attachment.path
    ))
}

impl Resolved {
    /// The attachments this request went out without, in transcript order.
    ///
    /// Read back out of the transcript rather than off the line that stood in
    /// for them: that line is written for the model, and what a reader is owed
    /// is the file it was written about.
    pub(crate) fn aged(&self, transcript: &Transcript) -> Box<[Attachment]> {
        self.0
            .iter()
            .filter(|held| matches!(held.carried, Carried::Instead(_)))
            .filter_map(|held| {
                let Some(Message::User { attachments, .. }) =
                    transcript.messages().get(held.message)
                else {
                    return None;
                };
                attachments.get(held.index).cloned()
            })
            .collect()
    }

    /// What the request borrows, in transcript order.
    pub(crate) fn attached(&self) -> Vec<Attached<'_>> {
        self.0
            .iter()
            .map(|held| Attached {
                message: held.message,
                index: held.index,
                media_type: &held.media_type,
                modality: held.modality,
                content: match &held.carried {
                    Carried::Bytes(bytes) => Content::Bytes(bytes),
                    Carried::Instead(line) => Content::Instead(line),
                },
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
