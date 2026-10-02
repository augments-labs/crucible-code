//! What happened in this directory before, as much of it as a screen holds.
//!
//! A different read from [`mod@super::replay`], for a different reason. That
//! one finds one log and hands back everything in it, because a session is
//! about to be continued. This one finds a few and takes one line from each,
//! because somebody is about to be shown a list — and the welcome screen runs
//! it before the first frame, where twenty milliseconds is the whole budget.
//!
//! Candidate names come from the fixed recent-session index. An older flat log
//! directory is indexed once by session start, after the first frame; this
//! path neither enumerates the directory nor migrates it. How many logs are
//! opened is the caller's [`Reach`], and bytes read from one log are bounded
//! below. Which directories a session may have been recorded in to be listed
//! is the caller's [`Roots`].

use std::fs::File;
use std::io::{BufRead as _, BufReader, Read as _};
use std::path::Path;
use std::str::FromStr as _;
use std::time::SystemTime;

use crucible_types::{Message, SessionId};

use super::index;
use super::wire;

/// How many logs the first frame may open before the scan gives up.
///
/// The newest logs are the ones most likely to be this directory's, and a
/// welcome screen is a list rather than a search: a machine whose last sixty-odd
/// sessions were all somewhere else gets the heading and nothing under it, which
/// is what it would get from a slower answer too.
const EXAMINED: usize = 64;

/// How far back a listing looks, chosen by whoever asked for it.
///
/// Two answers rather than a number, because the two callers want different
/// things for different reasons, and a bare count beside the count of rows
/// wanted is one argument swap away from a first frame that opens every log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// The newest [`EXAMINED`] logs: the welcome screen, before the first
    /// frame, where the cost of each log opened is paid by every launch.
    FirstFrame,
    /// Every session the index names. For a listing somebody asked for after
    /// the first frame, which can spend the opening of a few hundred headers.
    Indexed,
}

impl Reach {
    /// How many logs this reach may open.
    const fn logs(self) -> usize {
        match self {
            Self::FirstFrame => EXAMINED,
            Self::Indexed => index::ENTRIES,
        }
    }
}

/// Which directories a session may have been recorded in to be listed.
#[derive(Debug, Clone, Copy)]
pub enum Roots<'a> {
    /// Only these, each matched whole: `/w/crucible` is not `/w/crucible-code`.
    These(&'a [&'a Path]),
    /// Any directory at all. A caller that narrows in memory afterwards asks
    /// for this, so one scan answers every narrowing it offers.
    Any,
}

impl Roots<'_> {
    /// Whether a log recorded in `workspace` is one of these.
    fn admit(self, workspace: &Path) -> bool {
        match self {
            Self::These(roots) => roots.contains(&workspace),
            Self::Any => true,
        }
    }
}

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

/// One session that was recorded before.
#[derive(Debug, Clone)]
pub struct Recorded {
    /// Which session, and so also when it started.
    id: SessionId,
    /// The directory it was recorded in, exactly as its header spells it.
    workspace: Box<Path>,
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

    /// The directory it was recorded in, as its header spells it.
    ///
    /// A path from a file, so it is as untrusted as the prompt beside it: it
    /// is compared whole rather than drawn, and whoever draws it flattens it.
    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace
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

/// The sessions recorded in one of `roots`, newest first, at most `wanted` of
/// them, found among the logs `reach` lets the scan open.
///
/// Total, and deliberately so. Every session here is decoration on a screen
/// that has not asked for anything yet: a log this build cannot read, a file
/// that will not open, a directory that is not there — each of those is one
/// fewer row, and none of them is a reason to refuse to start. The paths that
/// have to be loud about a session still are, and `--continue` is the loudest
/// of them.
#[must_use]
pub fn recent(directory: &Path, roots: Roots<'_>, reach: Reach, wanted: usize) -> Vec<Recorded> {
    // Capped at what could be found rather than at what was asked for, so a
    // caller that wants everything does not name the allocation.
    let mut found = Vec::with_capacity(wanted.min(reach.logs()));
    if wanted == 0 {
        return found;
    }

    // One fixed-size file supplies the names — and the count and title kept
    // beside each — so first-frame work does not grow with the number of logs
    // in the directory.
    let entries = index::entries(directory, reach.logs()).unwrap_or_default();

    for entry in entries {
        let path = directory.join(format!("{}.{}", entry.id.as_str(), super::SUFFIX));
        if let Some(mut session) = read(&path, roots) {
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
/// The header decides most of it: a log belongs to one of `roots` or it does
/// not, and one written by a build that spelled things differently is left out
/// rather than half-read. What is left is the first message, which is the first
/// thing that was asked — and a log with none, crucible opened and left without
/// a word typed, is no session to offer under any roots.
fn read(path: &Path, roots: Roots<'_>) -> Option<Recorded> {
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
    if !wire::readable(opening.format) || !roots.admit(Path::new(&opening.workspace)) {
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
            return (!asked.is_empty()).then(|| Recorded {
                id,
                workspace: Path::new(&opening.workspace).into(),
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
