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
//!
//! A transcript outlives a model, too. What may be attached is settled per
//! request rather than when a file was named, because the model being asked can
//! change between one request and the next and a file's kind cannot. A file the
//! model does not read stands down the same way, and for the same reason: the
//! alternative is bytes labelled with a kind the request has no word for, which
//! is a wrong answer rather than a refused one.

use std::fs;
use std::path::Path;

use sha2::{Digest as _, Sha256};

use crucible_core::{Attached, Attachment, Content, Modalities, Modality, Transcript};

/// The most raw attachment bytes one request may carry.
///
/// Not a vendor's limit — this one binds first. What a request peaks at is
/// measured rather than derived: `scripts/bench.sh mem` runs a session at this
/// ceiling every time it runs, and reads about three times this figure on top
/// of what the session was already holding. The bytes, their base64 form and
/// the serialized body are alive at once, and the last two each hold the
/// encoding whole.
///
/// The figure this replaced was half as much again, from a reading of the code
/// that counted one of those copies and not the other. At that size the same
/// measurement swung between three times and four from run to run, which is the
/// other half of why this one is lower — a reading that moves by seven
/// megabytes needs somewhere to move to.
///
/// The worst a session is otherwise holding is its record full, at about 14 MB.
/// This is deliberately below what the rest of the 35 MB in
/// `performance-budgets.md` allows. A budget spent to its last megabyte is not
/// a budget.
pub const CEILING: usize = 4 * 1024 * 1024;

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
    Unread(String),
}

/// Reads what this request can carry, newest first.
///
/// Newest first because the ceiling has to fall on something, and the picture a
/// user is talking about now is the one they are talking about now. By bytes
/// rather than by count: two screenshots and twenty icons are the same number
/// of files and not the same problem.
pub(crate) fn resolve(transcript: &Transcript, carries: Modalities) -> Resolved {
    let mut held = Vec::new();
    let mut spent = 0;

    for (message, said) in transcript.messages().iter().enumerate().rev() {
        let attachments = said.attachments();
        for (index, attachment) in attachments.iter().enumerate().rev() {
            held.push(Held {
                message,
                index,
                media_type: attachment.media_type.clone(),
                modality: attachment.modality,
                carried: read(attachment, &mut spent, carries),
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
fn read(attachment: &Attachment, spent: &mut usize, carries: Modalities) -> Carried {
    // Asked before the file is opened, because a kind that cannot go does not
    // become one by being on disk, and this is the one reason that does not
    // depend on what is found there.
    if !carries.contains(attachment.modality) {
        return unread(attachment);
    }
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

/// One line, in place of one file whose kind this request cannot carry.
///
/// It ends in no next move, unlike [`instead`]. Reading the file again produces
/// the same bytes of the same kind, and which model is being asked is not the
/// model's to choose — an invitation here would be one to a call that cannot
/// work.
fn unread(attachment: &Attachment) -> Carried {
    Carried::Unread(format!(
        "{} is not attached to this request: it is {}, which the model being asked does not read.",
        attachment.path,
        attachment.modality.as_str()
    ))
}

impl Resolved {
    /// The attachments this request went out without, in transcript order.
    ///
    /// Read back out of the transcript rather than off the line that stood in
    /// for them: that line is written for the model, and what a reader is owed
    /// is the file it was written about.
    pub(crate) fn aged(&self, transcript: &Transcript) -> Box<[Attachment]> {
        self.named(transcript, |held| {
            matches!(held.carried, Carried::Instead(_))
        })
    }

    /// The attachments this request could never have carried, in transcript
    /// order.
    ///
    /// Apart from [`Resolved::aged`] because the two differ in the one part a
    /// reader can act on: an aged file goes out if it is asked for again, and
    /// this one does not go out until the model does.
    pub(crate) fn unread(&self, transcript: &Transcript) -> Box<[Attachment]> {
        self.named(transcript, |held| {
            matches!(held.carried, Carried::Unread(_))
        })
    }

    /// The files behind whichever held entries a rule picks out.
    fn named(&self, transcript: &Transcript, which: impl Fn(&Held) -> bool) -> Box<[Attachment]> {
        self.0
            .iter()
            .filter(|held| which(held))
            .filter_map(|held| {
                let said = transcript.messages().get(held.message)?;
                said.attachments().get(held.index).map(|one| (*one).clone())
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
                    Carried::Instead(line) | Carried::Unread(line) => Content::Instead(line),
                },
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
