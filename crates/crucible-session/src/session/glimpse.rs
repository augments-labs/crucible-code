//! The end of one session's log, as much of it as a preview pane wants.
//!
//! A third read, beside [`super::recent`]'s first line and [`super::replay`]'s
//! everything, for a third reason: somebody is deciding whether to pick a
//! session back up, and what says most about one is how it left off. The read
//! is bounded from the end of the file, so a log of any size costs the same —
//! and a glimpse that could not hold the whole conversation says so, rather
//! than reading as all there was.
//!
//! The glimpse also answers the one question a picker cannot ask any other
//! way without consequences: whether another crucible still holds the log
//! open. Continuing a session cuts its file, so the picker has to know before
//! Enter, not after.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

use crucible_core::{Message, SessionId, ToolOutput, ToolResult, Workspace};

use super::wire;

/// How much of the end of one log may be read.
///
/// Enough conversation to fill any pane many times over, and small enough
/// that the read costs the same for a log of half a line or half a gigabyte.
const TAIL: u64 = 64 * 1024;

/// The end of one session's conversation, and what else a picker needs to
/// know before offering to continue it.
#[derive(Debug)]
pub struct Glimpse {
    /// The last messages of the session, oldest first.
    messages: Vec<Message>,
    /// Whether the log holds conversation this glimpse does not — an earlier
    /// part past the window, or an end that could not be read whole.
    cut: bool,
    /// Whether another crucible holds the session open right now.
    busy: bool,
}

impl Glimpse {
    /// The last messages of the session, oldest first.
    ///
    /// The whole message rather than the prose in it, because a session is its
    /// tool work as much as its answers: what a preview draws is decided by
    /// whatever draws a live turn, and that is handed messages.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Whether the log holds conversation this glimpse does not.
    ///
    /// What is shown under a glimpse with this set has to say it is
    /// incomplete: a bounded read that reads as everything is a lie about the
    /// session it previews.
    #[must_use]
    pub fn cut(&self) -> bool {
        self.cut
    }

    /// Whether another crucible holds the session open right now.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.busy
    }
}

/// The end of the session `id` names, read without touching the log.
///
/// # Errors
///
/// [`super::SessionError::Unknown`] where this workspace has no such session,
/// and [`super::SessionError`] where what is there cannot be opened.
pub fn glimpse(
    directory: &Path,
    workspace: &Workspace,
    id: &SessionId,
) -> Result<Glimpse, super::SessionError> {
    // The same door `/resume` uses, refused the same way: naming a session is
    // a shorter way to reach one, not a way past whose it is.
    let path = directory.join(format!("{}.{}", id.as_str(), super::SUFFIX));

    if !path.is_file() || !super::belongs(&path, workspace)? {
        return Err(super::SessionError::Unknown {
            id: id.as_str().into(),
            at: workspace.root().display().to_string().into(),
        });
    }

    // Taken and let go rather than held: the question is whether anybody else
    // holds it, and a glimpse keeping the claim would be the picker doing the
    // thing it exists to warn about.
    let busy = match super::claimed(&path)? {
        super::Claimed::Busy => true,
        super::Claimed::Taken(held) => {
            drop(held);
            false
        }
        super::Claimed::Lockless => false,
    };

    let failed = |source| super::SessionError::Log {
        at: path.display().to_string().into(),
        source,
    };

    let mut file = File::open(&path).map_err(failed)?;
    let length = file.seek(SeekFrom::End(0)).map_err(failed)?;
    let start = length.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(start)).map_err(failed)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(failed)?;

    // Lossy because the window can open in the middle of a multi-byte
    // character; the piece it lands in is dropped below anyway.
    let text = String::from_utf8_lossy(&bytes);

    let mut cut = start > 0;
    let mut pieces: Vec<&str> = text.split('\n').collect();

    // A last piece with no newline after it is a line a crash tore in half.
    // Left out rather than parsed, and said so: half a line is not something
    // anybody said.
    match pieces.pop() {
        Some("") | None => {}
        Some(_) => cut = true,
    }

    let mut messages = Vec::new();
    for piece in pieces.into_iter().skip(usize::from(start > 0)) {
        // The header and any line a different build wrote parse to nothing;
        // everything a turn is made of survives, because a preview that kept
        // only the prose would show a conversation nobody had.
        match wire::message(piece) {
            Some(Message::User { text, attachments }) => messages.push(Message::User {
                text: cleaned(&text),
                attachments,
            }),
            Some(Message::Agent { text, calls, stop }) => messages.push(Message::Agent {
                text: cleaned(&text),
                calls,
                stop,
            }),
            Some(Message::ToolResults(results)) => {
                messages.push(Message::ToolResults(
                    results.into_iter().map(safely).collect(),
                ));
            }
            None => {}
        }
    }

    Ok(Glimpse {
        messages,
        cut,
        busy,
    })
}

/// `result` with nothing in its text a terminal would act on.
///
/// A result is text a tool took from a file, a process or the network, so it is
/// the least trustworthy thing on the screen and reaches the pane through the
/// same door prose does. What the preview does not draw — the diff a rewrite
/// showed, the files a search attached — is dropped rather than carried,
/// because a glimpse is read and never sent anywhere.
fn safely(result: ToolResult) -> ToolResult {
    let text = cleaned(result.output.text());
    ToolResult {
        id: result.id,
        output: if result.output.is_failed() {
            ToolOutput::failed(text)
        } else {
            ToolOutput::ok(text)
        },
    }
}

/// `text` with nothing in it a terminal would act on.
///
/// The glimpse goes straight to a screen, and a file on disk can claim
/// anything. Line breaks are the one control character that means what the
/// preview means by it.
fn cleaned(text: &str) -> Box<str> {
    text.chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect::<String>()
        .into()
}

#[cfg(test)]
mod tests;
