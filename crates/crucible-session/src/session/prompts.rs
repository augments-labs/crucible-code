//! What was asked in this directory before, to be reached back through.
//!
//! Not a session log, and deliberately not read from one. A log holds a
//! conversation and is opened when one is continued; this holds the lines
//! somebody typed, across every session they typed them in, so an arrow key
//! can put one back in the box. Reading the logs for it would mean opening as
//! many files as there were sessions to find as many prompts as fit on the
//! walk, and doing it before the first frame.
//!
//! One file for the machine, with the directory each prompt was asked in
//! written beside it, and only that directory's offered back. Two checkouts
//! share a bound rather than each keeping their own: what the file may never
//! do is grow with the number of directories somebody works in, and a store
//! per directory is exactly that growth wearing a different name.
//!
//! Every version of the file is written whole, under an operating-system lock
//! and through [`Beside`], so two crucibles finishing a prompt at once produce
//! one file holding both rather than a file holding half of each.
//!
//! What comes back is text a person typed and a disk gave back, which makes it
//! as untrusted as anything else read from one. It is not drawn — it is put
//! into the editor, which already refuses every control character a line is
//! not allowed to carry — so the flattening a title needs has no owner here.

use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use crucible_core::Workspace;
use serde_json::{Value, json};

use super::SessionError;
use super::beside::Beside;
use super::claim;

/// What the file is called. Its suffix deliberately cannot be a log's.
const NAME: &str = "prompt.history";

/// The first line, so a file this build cannot read is refused, not guessed at.
const FORMAT: &str = "crucible-prompt-history-1";

/// How many of a directory's prompts a walk may reach back through.
///
/// The count the box says out loud, so it is a number a person reads rather
/// than a limit they meet: a hundred is further back than anyone arrows on
/// purpose, and short enough that the number stays two digits and three.
pub const PROMPTS: usize = 100;

/// How many prompts the file holds across every directory.
///
/// Above [`PROMPTS`] by enough that the directory somebody is working in keeps its
/// whole window while the ones they are not give theirs up slowly. This is the
/// bound that matters: the file is rewritten whole every time a prompt is
/// finished, so what this really buys is that the write stays the same size in
/// a year as it is today.
const RETAINED: usize = 512;

/// How much of one prompt is retained.
///
/// A prompt past this is a paste, and a paste is not something anybody reaches
/// back for with an arrow — they have the file it came from. Keeping it would
/// put the whole of a pasted log into a file that is read at start-up and
/// rewritten on every prompt after it.
const LONGEST: usize = 1024;

/// How much of the file may be read before it is refused.
///
/// Above what [`RETAINED`] prompts of [`LONGEST`] can come to once JSON has
/// escaped the worst of them and written a path beside each, and far above
/// what the file is in practice. A file past it was not written here.
const BYTES: u64 = 4 * 1024 * 1024;

/// The prompts asked in `workspace`, oldest first, at most [`PROMPTS`] of them.
///
/// Oldest first because that is the order the arrow walks: the newest is the
/// end of it, so the first press back lands on the last thing that was asked
/// and the number the box says falls from there.
///
/// # Errors
///
/// When the file is there and cannot be read, or was written in a format this
/// build does not know. A directory nothing was ever asked in is not an error:
/// it is every session before the first prompt somebody finishes.
pub fn prompts(directory: &Path, workspace: &Workspace) -> Result<Vec<String>, SessionError> {
    let held = read(&named(directory))?;
    let mut mine = held
        .into_iter()
        .filter(|entry| entry.asked_in(workspace))
        .map(|entry| entry.said)
        .collect::<Vec<_>>();

    // From the front, so what a walk keeps is the newest window and not the
    // first hundred somebody ever typed here.
    mine.drain(..mine.len().saturating_sub(PROMPTS));
    Ok(mine)
}

/// Puts `said` last, keeping this directory to [`PROMPTS`] and the file itself
/// to a fixed window across every directory.
///
/// A prompt too long to retain is not retained at all, rather than cut to fit.
/// Half a prompt handed back by an arrow is a line somebody sends without
/// noticing what is missing from it, which is worse than an arrow that finds
/// nothing.
///
/// # Errors
///
/// When the file cannot be claimed, read or replaced. The caller decides what
/// that costs: a history that will not take a line is not a reason to refuse
/// the prompt itself.
pub fn remember(directory: &Path, workspace: &Workspace, said: &str) -> Result<(), SessionError> {
    if said.len() > LONGEST {
        return Ok(());
    }

    let path = named(directory);
    let _held = claim::exclusive(&path).map_err(|source| problem(&path, source))?;
    let mut entries = read(&path)?;

    entries.push(Entry {
        root: workspace.root().display().to_string(),
        said: said.to_owned(),
    });

    // This directory's window, applied where the prompt is written and not
    // only where one is read back. A prompt past it is one no arrow can reach
    // again, and a file holding lines nobody can reach is a file carrying the
    // cost of a history it is not offering: the hundredth prompt somebody
    // sends here is what the first one is spent on.
    let mut room = PROMPTS;
    let mut kept = Vec::with_capacity(entries.len());
    for entry in entries.into_iter().rev() {
        if entry.asked_in(workspace) {
            if room == 0 {
                continue;
            }
            room -= 1;
        }
        kept.push(entry);
    }
    kept.reverse();

    // And the file's own, which is what stops it growing with the number of
    // directories somebody works in. It bites only on the ones they are not
    // working in, because the window above has already trimmed this one.
    kept.drain(..kept.len().saturating_sub(RETAINED));
    replace(&path, &kept)
}

/// One prompt, and where it was asked.
#[derive(Debug)]
struct Entry {
    /// The workspace root, as the directory that recorded it wrote it down.
    root: String,
    /// What was typed, whole, breaks and all.
    said: String,
}

impl Entry {
    /// Whether this is one of `workspace`'s.
    ///
    /// Compared as paths rather than as text, so a root a different build
    /// spelled with a trailing separator is still the same directory. Both
    /// sides are already the canonical form [`Workspace`] settled on, so this
    /// asks nothing of the filesystem.
    fn asked_in(&self, workspace: &Workspace) -> bool {
        Path::new(&self.root) == workspace.root()
    }

    /// The line the file holds it as.
    fn line(&self) -> String {
        json!({ "root": self.root, "said": self.said }).to_string()
    }

    /// One back, or `None` where the line is not one.
    ///
    /// Total, because the only thing a line the file cannot hold should cost
    /// is that prompt. A disk that filled mid-rename leaves one torn line and
    /// every whole prompt above it is still somebody's history.
    fn of(line: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(line).ok()?;
        let root = value.get("root")?.as_str()?;
        let said = value.get("said")?.as_str()?;

        (said.len() <= LONGEST).then(|| Self {
            root: root.to_owned(),
            said: said.to_owned(),
        })
    }
}

/// Every whole prompt the file holds, oldest first, empty where there is none.
fn read(path: &Path) -> Result<Vec<Entry>, SessionError> {
    let opened = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(problem(path, source)),
    };

    let mut text = String::new();
    opened
        .take(BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|source| problem(path, source))?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > BYTES {
        return Err(problem(
            path,
            io::Error::new(io::ErrorKind::InvalidData, "prompt history is too large"),
        ));
    }

    let mut lines = text.lines();
    if lines.next() != Some(FORMAT) {
        return Err(problem(
            path,
            io::Error::new(io::ErrorKind::InvalidData, "unknown prompt history format"),
        ));
    }

    // Bounded by the same number the writing is, and from the front, so a
    // file somebody grew by hand gives up its oldest prompts rather than the
    // newest — which are the ones a walk was going to reach.
    let mut entries = lines.filter_map(Entry::of).collect::<Vec<_>>();
    entries.drain(..entries.len().saturating_sub(RETAINED));
    Ok(entries)
}

/// Replaces the history with one whole, durable version.
fn replace(path: &Path, entries: &[Entry]) -> Result<(), SessionError> {
    let directory = path.parent().ok_or_else(|| {
        problem(
            path,
            io::Error::other("the prompt history has no parent directory"),
        )
    })?;
    let mut beside = Beside::new(directory, "prompts").map_err(|source| problem(path, source))?;

    {
        let file = beside.file().map_err(|source| problem(path, source))?;
        writeln!(file, "{FORMAT}").map_err(|source| problem(path, source))?;
        for entry in entries {
            writeln!(file, "{}", entry.line()).map_err(|source| problem(path, source))?;
        }
        file.sync_all().map_err(|source| problem(path, source))?;
    }
    beside.over(path).map_err(|source| problem(path, source))
}

fn named(directory: &Path) -> PathBuf {
    directory.join(NAME)
}

fn problem(path: &Path, source: io::Error) -> SessionError {
    SessionError::History {
        at: path.display().to_string().into(),
        source,
    }
}

#[cfg(test)]
mod tests;
