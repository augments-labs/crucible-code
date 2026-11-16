//! A unified diff read into a [`Change`].
//!
//! The text is a fence the model wrote, so it is hostile: its bytes and lines
//! are counted before a byte of it is parsed, and its files as they open, so
//! one that names more than a change keeps is refused however its files open.
//! What is read is what `git diff` and `diff -u` write: `diff --git` and
//! `---`/`+++` headers, renames, binary notes, and hunks whose header counts
//! say where each ends. Those counts are what tell a
//! removed line that begins `-- ` from the next file's `--- ` header, so a hunk
//! ends when its counts run out, or at the first line that cannot be in one.
//! Anything else (an `index` line, a mode, prose around the diff) is passed
//! over. A block with no hunk and no binary note is not a diff.

use crate::change::{Change, File, Line, MOST_FILES, Side};

/// The most bytes a diff is read from.
pub const MOST_BYTES: usize = 64 * 1024;

/// Why a block is not drawn as a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NotADiff {
    /// It holds no hunk and no binary note: it is not a diff.
    #[error("not a diff")]
    Unread,
    /// It is longer than a diff is read at: bytes, lines or files.
    #[error("longer than a diff is read at")]
    PastCap,
}

/// What is left of the hunk being read.
struct Hunk {
    old: usize,
    new: usize,
    old_left: usize,
    new_left: usize,
}

/// The files read so far and the one being read.
#[derive(Default)]
struct Reading {
    files: Vec<File>,
    file: Option<File>,
    /// Whether `file` came from a `diff --git` line and has had no hunk yet,
    /// so the `---`/`+++` pair under it names it rather than opening another.
    headed: bool,
    /// The line after the last one the file's previous hunk held.
    next: Option<usize>,
    /// The old path a `---` line named, waiting for its `+++`.
    old: Option<String>,
    /// Whether a file opened past the [`MOST_FILES`] already read.
    past: bool,
}

impl Reading {
    fn open(&mut self, file: File) {
        self.close();
        self.file = Some(file);
        self.next = None;
    }

    fn close(&mut self) {
        if let Some(file) = self.file.take()
            && !file.is_empty()
        {
            if self.files.len() < MOST_FILES {
                self.files.push(file);
            } else {
                self.past = true;
            }
        }
    }

    fn current(&mut self) -> &mut File {
        self.file.get_or_insert_with(|| File::new(None))
    }
}

/// `source` read as a unified diff.
///
/// # Errors
///
/// [`NotADiff::PastCap`] where it is longer than [`MOST_BYTES`] or holds more
/// than [`crate::MOST_LINES`] lines, counted before it is parsed, or where it
/// names more than [`MOST_FILES`] files, whether each opens with `diff --git`
/// or `+++` and whether it holds lines or a binary note;
/// [`NotADiff::Unread`] where it holds no hunk and no binary note.
pub fn read(source: &str) -> Result<Change, NotADiff> {
    if source.len() > MOST_BYTES
        || source.bytes().filter(|&byte| byte == b'\n').count() >= crate::MOST_LINES
        || source.matches("\n+++ ").count() + usize::from(source.starts_with("+++ ")) > MOST_FILES
    {
        return Err(NotADiff::PastCap);
    }

    let mut reading = Reading::default();
    let mut hunk: Option<Hunk> = None;

    for line in source.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);

        if let Some(open) = hunk.as_mut() {
            if body(&mut reading, open, line) {
                if open.old_left == 0 && open.new_left == 0 {
                    reading.next = Some(open.new);
                    hunk = None;
                }
                continue;
            }
            reading.next = Some(open.new);
        }

        hunk = header(&mut reading, line);
    }

    reading.close();
    if reading.past {
        return Err(NotADiff::PastCap);
    }
    if reading.files.is_empty() {
        return Err(NotADiff::Unread);
    }
    Ok(Change::new(reading.files))
}

/// Reads `line` as part of the open hunk; `false` where it cannot be one.
fn body(reading: &mut Reading, hunk: &mut Hunk, line: &str) -> bool {
    let (side, text) = match line.chars().next() {
        Some(' ') => (Side::Kept, line.get(1..).unwrap_or_default()),
        // A blank line is a kept blank line some editor trimmed the space
        // off; one past the counts is the blank after the diff.
        None if hunk.old_left > 0 && hunk.new_left > 0 => (Side::Kept, ""),
        Some('-') if hunk.old_left > 0 => (Side::Removed, line.get(1..).unwrap_or_default()),
        Some('+') if hunk.new_left > 0 => (Side::Added, line.get(1..).unwrap_or_default()),
        Some('\\') => return true,
        _ => return false,
    };
    if side == Side::Kept && (hunk.old_left == 0 || hunk.new_left == 0) {
        return false;
    }

    let number = match side {
        Side::Removed => hunk.old,
        Side::Kept | Side::Added => hunk.new,
    };
    reading.current().push(Line::Text {
        side,
        number,
        text: text.to_owned(),
    });

    if side != Side::Added {
        hunk.old = hunk.old.saturating_add(1);
        hunk.old_left -= 1;
    }
    if side != Side::Removed {
        hunk.new = hunk.new.saturating_add(1);
        hunk.new_left -= 1;
    }
    true
}

/// Reads `line` as a header, answering the hunk it opens where it opens one.
fn header(reading: &mut Reading, line: &str) -> Option<Hunk> {
    if let Some(paths) = line.strip_prefix("diff --git ") {
        reading.open(File::new(git_path(paths)));
        reading.headed = true;
        reading.old = None;
    } else if let Some(old) = line.strip_prefix("--- ") {
        reading.old = Some(path(old).to_owned());
    } else if let Some(new) = line.strip_prefix("+++ ") {
        let old = reading.old.take();
        let new = match path(new) {
            "/dev/null" => old.unwrap_or_default(),
            new => new.to_owned(),
        };
        if reading.headed {
            reading.headed = false;
            if !new.is_empty() {
                reading.current().named(&new);
            }
        } else {
            reading.open(File::new((!new.is_empty()).then_some(new.as_str())));
        }
    } else if let Some(from) = line.strip_prefix("rename from ") {
        reading.current().renamed(from);
    } else if let Some(to) = line.strip_prefix("rename to ") {
        reading.current().named(to);
    } else if line == "GIT binary patch"
        || (line.starts_with("Binary files ") && line.ends_with(" differ"))
    {
        reading.current().binary();
    } else if let Some(hunk) = hunk(line) {
        reading.headed = false;
        let (start, section) = hunk;
        if let Some(next) = reading.next
            && start.new > next
        {
            reading.current().push(Line::Gap {
                lines: start.new - next,
                section,
            });
        }
        return Some(start);
    }
    None
}

/// The path a `diff --git a/P b/P` line names, where its two halves are the
/// same path. A path can hold ` b/` itself, so the line is split at its
/// middle rather than at the first ` b/`; halves that differ name no one
/// path, and a `+++` or `rename to` line later names the file instead.
fn git_path(paths: &str) -> Option<&str> {
    let both = paths.strip_prefix("a/")?;
    let half = both.len().checked_sub(" b/".len())? / 2;
    let (old, new) = (both.get(..half)?, both.get(half..)?.strip_prefix(" b/")?);
    (!new.is_empty() && old == new).then_some(new)
}

/// A `---` or `+++` line's path: the `a/` or `b/` git puts on it gone, and
/// the tab and timestamp `diff -u` puts after it.
fn path(named: &str) -> &str {
    let named = named.split('\t').next().unwrap_or(named).trim_end();
    if named == "/dev/null" {
        return named;
    }
    named
        .strip_prefix("a/")
        .or_else(|| named.strip_prefix("b/"))
        .unwrap_or(named)
}

/// `@@ -a,b +c,d @@ section`, as the hunk it opens and the section it names.
fn hunk(line: &str) -> Option<(Hunk, Option<String>)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, rest) = rest.split_once(" @@")?;
    let (old, old_left) = range(old)?;
    let (new, new_left) = range(new)?;
    if old_left == 0 && new_left == 0 {
        return None;
    }
    let section = rest.trim();
    Some((
        Hunk {
            // A range of no lines names the line before it.
            old: if old_left == 0 {
                old.saturating_add(1)
            } else {
                old
            },
            new: if new_left == 0 {
                new.saturating_add(1)
            } else {
                new
            },
            old_left,
            new_left,
        },
        (!section.is_empty()).then(|| section.to_owned()),
    ))
}

/// `start,count`, or `start` alone for a count of one.
fn range(said: &str) -> Option<(usize, usize)> {
    match said.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((said.parse().ok()?, 1)),
    }
}
