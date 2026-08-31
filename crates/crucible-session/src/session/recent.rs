//! What happened in this directory before, as much of it as a screen holds.
//!
//! A different read from [`super::replay`], for a different reason. That one
//! finds one log and hands back everything in it, because a session is about to
//! be continued. This one finds a few and takes one line from each, because
//! somebody is about to be shown a list — and it runs before the first frame,
//! where twenty milliseconds is the whole budget.
//!
//! Candidate names come from the fixed recent-session index. An older flat log
//! directory is indexed once by session start, after the first frame; this
//! path neither enumerates the directory nor migrates it. Logs opened and
//! bytes read from one log are bounded separately below.

use std::fs::File;
use std::io::{BufRead as _, BufReader, Read as _};
use std::path::Path;
use std::str::FromStr as _;
use std::time::SystemTime;

use crucible_core::{Message, SessionId, Workspace};

use super::index;
use super::wire;

/// How many logs may be opened before the scan gives up.
///
/// The newest logs are the ones most likely to be this directory's, and a
/// welcome screen is a list rather than a search: a machine whose last sixty-odd
/// sessions were all somewhere else gets the heading and nothing under it, which
/// is what it would get from a slower answer too.
const EXAMINED: usize = 64;

/// How much of one log may be read looking for what was first asked.
///
/// The first message is the first line after the header, so this is reached
/// only by a prompt with a pasted file in it. Reading stops there and the
/// session is left out, rather than the startup path being handed a length
/// somebody else chose.
const READ: u64 = 64 * 1024;

/// How much of that message is kept.
///
/// Wider than any terminal, because what fits is the component's question, and
/// far short of what a prompt can be — this is a title, and the rest of it is
/// in the log.
pub(super) const TITLE: usize = 512;

/// One session that was recorded in this directory before.
#[derive(Debug, Clone)]
pub struct Recorded {
    /// Which session, and so also when it started.
    id: SessionId,
    /// The first thing asked of it, on one line.
    asked: Box<str>,
    /// The branch its header said the workspace had checked out, where the
    /// caller that started it could say.
    branch: Option<Box<str>>,
    /// How many conversation messages its log holds, as the index counted
    /// them.
    messages: usize,
    /// The title somebody saved over the first prompt, where they did.
    titled: Option<Box<str>>,
}

impl Recorded {
    /// Which session it was.
    #[must_use]
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// When it started.
    #[must_use]
    pub fn started(&self) -> SystemTime {
        self.id.started()
    }

    /// The first thing asked of it.
    ///
    /// One line, with nothing in it a terminal would act on: it is text a user
    /// typed and a file gave back, so it arrives as untrusted as anything else
    /// read from a disk, and it is flattened where it is read rather than
    /// wherever it is drawn.
    #[must_use]
    pub fn asked(&self) -> &str {
        &self.asked
    }

    /// The branch the session began on, where its header says.
    ///
    /// `None` for a log whose header never learned one — every session a
    /// format 7 build recorded, and any started where nothing could say.
    #[must_use]
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// How many conversation messages the session's log holds.
    ///
    /// Zero for a session indexed before counting existed; the count is
    /// repaired the next time that session is continued.
    #[must_use]
    pub fn messages(&self) -> usize {
        self.messages
    }

    /// What the row is called: the saved title where somebody set one, and the
    /// first prompt otherwise. As flattened as [`Recorded::asked`], by the
    /// same rule.
    #[must_use]
    pub fn title(&self) -> &str {
        self.titled.as_deref().unwrap_or(&self.asked)
    }
}

/// The sessions recorded for `workspace`, newest first, at most `wanted` of
/// them.
///
/// Total, and deliberately so. Every session here is decoration on a screen
/// that has not asked for anything yet: a log this build cannot read, a file
/// that will not open, a directory that is not there — each of those is one
/// fewer row, and none of them is a reason to refuse to start. The paths that
/// have to be loud about a session still are, and `--continue` is the loudest
/// of them.
#[must_use]
pub fn recent(directory: &Path, workspace: &Workspace, wanted: usize) -> Vec<Recorded> {
    // Capped at what could be found rather than at what was asked for, so a
    // caller that wants everything does not name the allocation.
    let mut found = Vec::with_capacity(wanted.min(EXAMINED));
    if wanted == 0 {
        return found;
    }

    // One fixed-size file supplies the names — and the count and title kept
    // beside each — so first-frame work does not grow with the number of logs
    // in the directory.
    let entries = index::entries(directory, EXAMINED).unwrap_or_default();

    for entry in entries {
        let path = directory.join(format!("{}.{}", entry.id.as_str(), super::SUFFIX));
        if let Some(mut session) = read(&path, workspace) {
            session.messages = entry.messages;
            session.titled = entry.title;
            found.push(session);

            if found.len() == wanted {
                break;
            }
        }
    }

    found
}

/// One log, as the session it records, or `None` if it is not one this run can
/// offer.
///
/// The header decides most of it: a log belongs to this directory or it does
/// not, and one written by a build that spelled things differently is left out
/// rather than half-read. What is left is the first message, which is the first
/// thing that was asked.
fn read(path: &Path, workspace: &Workspace) -> Option<Recorded> {
    let id = SessionId::from_str(path.file_stem()?.to_str()?).ok()?;

    let mut log = BufReader::new(File::open(path).ok()?).take(READ);
    let mut line = String::new();

    // A first line the process never finished is a log with nothing whole in
    // it: the header is written before a session can record anything.
    log.read_line(&mut line).ok()?;
    if !line.ends_with('\n') {
        return None;
    }

    let opening = wire::opening(line.trim_end())?;
    if !wire::readable(opening.format) || Path::new(&opening.workspace) != workspace.root() {
        return None;
    }

    loop {
        line.clear();
        if log.read_line(&mut line).ok()? == 0 {
            return None;
        }

        if let Some(Message::User { text, .. }) = wire::message(line.trim_end()) {
            let asked = single(&text);

            // A session whose first prompt was nothing but spaces has no row to
            // draw: the number and the date with a gap between them says less
            // than leaving it out does.
            return (!asked.is_empty()).then_some(Recorded {
                id,
                asked,
                // Flattened like the prompt: a git branch cannot hold a
                // control character, but a file on disk can claim anything.
                branch: opening
                    .branch
                    .as_deref()
                    .map(single)
                    .filter(|branch| !branch.is_empty()),
                messages: 0,
                titled: None,
            });
        }
    }
}

/// One line of what was asked, with nothing in it that could become a row.
///
/// A newline here would be a second row the renderer never counted, and every
/// frame after it would move the cursor to the wrong place. Whitespace of any
/// kind collapses to one space, so a prompt somebody wrote over five lines
/// still reads as a sentence, and leading and trailing runs go entirely.
///
/// The index borrows this for saved titles, so a title and a first prompt are
/// bounded and flattened by the same rule rather than by two that drift.
pub(super) fn single(text: &str) -> Box<str> {
    let mut said = String::new();
    let mut spacing = false;

    for character in text.chars().take(TITLE) {
        if character.is_whitespace() || character.is_control() {
            spacing = !said.is_empty();
            continue;
        }

        if spacing {
            said.push(' ');
            spacing = false;
        }

        said.push(character);
    }

    said.into()
}

#[cfg(test)]
mod tests;
