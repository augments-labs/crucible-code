//! A fixed newest-session index beside the append-only logs.
//!
//! Reading a directory is not bounded by the number of names retained from it.
//! The welcome runs before the first frame, so it reads this one small file and
//! opens only the logs named there. A session start or `--continue` on an older
//! installation builds the index after that frame by scanning the flat log
//! directory once; every later startup performs fixed work.
//!
//! The index is replaced whole under an operating-system lock. The replacement
//! is synced before its rename and the directory is synced afterwards, so a
//! crash leaves either complete version. A newly minted identifier is indexed
//! before its header is written: a crash in between leaves a candidate readers
//! validate and skip, rather than a complete log discovery can never find.

use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use crucible_core::SessionId;

use super::SessionError;
use super::beside::Beside;
use super::claim;
use super::replay::logs as legacy_logs;

/// What the index file is called. Its suffix deliberately cannot be a log's.
const NAME: &str = "recent.sessions";

/// The first line, so an incompatible index is refused rather than guessed at.
const FORMAT: &str = "crucible-session-index-2";

/// The first format's first line. Still read — a format 2 entry only added a
/// count and a title beside the identifier, and an identifier alone already
/// means "nothing counted, nothing renamed" — but never written again.
const FORMAT_ONE: &str = "crucible-session-index-1";

/// How many global session names discovery can inspect.
pub(super) const ENTRIES: usize = 256;

/// Greater than the header and [`ENTRIES`] maximum-length entries. An entry is
/// an identifier, a count, and a title of at most [`super::recent::TITLE`]
/// characters at up to four bytes each — a little over two kilobytes, so the
/// window's worth stays comfortably under this.
const BYTES: u64 = 640 * 1024;

/// One indexed session: its name and the little the picker shows beside it.
#[derive(Debug, Clone)]
pub(super) struct Entry {
    /// Which session the entry names.
    pub(super) id: SessionId,
    /// How many conversation messages its log holds, maintained as appends
    /// happen.
    pub(super) messages: usize,
    /// The title somebody saved over the first prompt, where they did.
    pub(super) title: Option<Box<str>>,
}

impl Entry {
    /// A newly minted session: nothing said yet, nothing renamed yet.
    fn new(id: SessionId) -> Self {
        Self {
            id,
            messages: 0,
            title: None,
        }
    }
}

/// Builds the index once for flat logs written before it existed.
pub(super) fn ensure(directory: &Path) -> Result<(), SessionError> {
    let path = named(directory);
    let _held = claim::exclusive(&path).map_err(|source| problem(&path, source))?;
    if read(&path)?.is_some() {
        return Ok(());
    }

    // The one unbounded read this module makes: once per directory, after the
    // first frame, and through the same name listing `--continue` uses. Every
    // startup after it reads only the fixed index written here. Message counts
    // start at zero — counting would mean reading every log — and are repaired
    // the next time each session is continued, when replay knows the number.
    let entries = legacy_logs(directory)?
        .into_iter()
        .rev()
        .take(ENTRIES)
        .filter_map(|path| SessionId::from_str(path.file_stem()?.to_str()?).ok())
        .map(Entry::new)
        .collect::<Vec<_>>();
    replace(&path, &entries)
}

/// Puts a newly minted identifier first, retaining a fixed newest window.
///
/// A name the index already holds keeps what it earned: the count and the
/// title move to the front with it, so picking a session back up does not
/// erase what the picker shows for it.
pub(super) fn record(directory: &Path, id: &SessionId) -> Result<(), SessionError> {
    let path = named(directory);
    let _held = claim::exclusive(&path).map_err(|source| problem(&path, source))?;
    let mut entries = read(&path)?.unwrap_or_default();

    let known = entries.iter().position(|held| &held.id == id);
    let entry = match known {
        Some(position) => entries.remove(position),
        None => Entry::new(id.clone()),
    };
    entries.insert(0, entry);
    entries.truncate(ENTRIES);
    replace(&path, &entries)
}

/// Records how many conversation messages the session's log now holds.
///
/// A name the fixed window has already let go of is left gone: the window
/// dropped it deliberately, and a count is not a reason to grow past it.
pub(super) fn tally(directory: &Path, id: &SessionId, messages: usize) -> Result<(), SessionError> {
    amend(directory, id, |entry| entry.messages = messages)
}

/// Saves `title` over the session's first prompt in every later listing.
///
/// The title is flattened and bounded here, where it is written, so nothing
/// multi-line or unbounded ever reaches the file; one that flattens to nothing
/// clears the override instead of saving an empty one. A name the fixed window
/// no longer holds is left as it is, the same way [`tally`] leaves it.
pub(super) fn retitle(directory: &Path, id: &SessionId, title: &str) -> Result<(), SessionError> {
    let title = super::recent::single(title);
    amend(directory, id, |entry| {
        entry.title = (!title.is_empty()).then(|| title.clone());
    })
}

/// Rewrites one entry under the lock, leaving an absent name untouched.
fn amend(
    directory: &Path,
    id: &SessionId,
    change: impl Fn(&mut Entry),
) -> Result<(), SessionError> {
    let path = named(directory);
    let _held = claim::exclusive(&path).map_err(|source| problem(&path, source))?;
    let mut entries = read(&path)?.unwrap_or_default();

    let Some(entry) = entries.iter_mut().find(|held| &held.id == id) else {
        return Ok(());
    };
    change(entry);
    replace(&path, &entries)
}

/// The newest indexed entries, newest first and at most `maximum`.
pub(super) fn entries(directory: &Path, maximum: usize) -> Result<Vec<Entry>, SessionError> {
    let path = named(directory);
    let mut entries = read(&path)?.unwrap_or_default();

    entries.truncate(maximum);
    Ok(entries)
}

/// Reads a complete index, `None` where migration has not made one yet.
fn read(path: &Path) -> Result<Option<Vec<Entry>>, SessionError> {
    let opened = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
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
            io::Error::new(
                io::ErrorKind::InvalidData,
                "session index is larger than its bound",
            ),
        ));
    }

    let mut lines = text.lines();
    let entries = match lines.next() {
        Some(FORMAT) => lines.map(entry).collect::<Option<Vec<_>>>(),
        // A format 1 line is the identifier alone, which already says
        // "nothing counted, nothing renamed".
        Some(FORMAT_ONE) => lines
            .map(|line| SessionId::from_str(line).ok().map(Entry::new))
            .collect(),
        _ => {
            return Err(problem(
                path,
                io::Error::new(io::ErrorKind::InvalidData, "unknown session index format"),
            ));
        }
    };
    let entries = entries.ok_or_else(|| {
        problem(
            path,
            io::Error::new(io::ErrorKind::InvalidData, "invalid session index entry"),
        )
    })?;
    if entries.len() > ENTRIES {
        return Err(problem(
            path,
            io::Error::new(
                io::ErrorKind::InvalidData,
                "session index has too many entries",
            ),
        ));
    }

    Ok(Some(entries))
}

/// One format 2 line as the entry it records, or `None` if it is not one.
///
/// The title is flattened again where it is read: the file is as untrusted as
/// any other on disk, and a control character that survived into it must not
/// survive out of it.
fn entry(line: &str) -> Option<Entry> {
    let mut fields = line.splitn(3, '\t');
    let id = SessionId::from_str(fields.next()?).ok()?;
    let messages = fields.next()?.parse().ok()?;
    let title = fields
        .next()
        .map(super::recent::single)
        .filter(|title| !title.is_empty());

    Some(Entry {
        id,
        messages,
        title,
    })
}

/// Replaces the index with one whole, durable version.
fn replace(path: &Path, entries: &[Entry]) -> Result<(), SessionError> {
    let directory = path.parent().ok_or_else(|| {
        problem(
            path,
            io::Error::other("session index has no parent directory"),
        )
    })?;
    let mut beside = Beside::new(directory, "recent").map_err(|source| problem(path, source))?;

    {
        let file = beside.file().map_err(|source| problem(path, source))?;
        writeln!(file, "{FORMAT}").map_err(|source| problem(path, source))?;
        for entry in entries {
            writeln!(
                file,
                "{}\t{}\t{}",
                entry.id.as_str(),
                entry.messages,
                entry.title.as_deref().unwrap_or_default(),
            )
            .map_err(|source| problem(path, source))?;
        }
        file.sync_all().map_err(|source| problem(path, source))?;
    }
    beside.over(path).map_err(|source| problem(path, source))
}

fn named(directory: &Path) -> PathBuf {
    directory.join(NAME)
}

fn problem(path: &Path, source: io::Error) -> SessionError {
    SessionError::Index {
        at: path.display().to_string().into(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::str::FromStr as _;

    use crate::sample::Sample;

    use super::*;

    fn id(nth: usize) -> SessionId {
        SessionId::from_str(&format!(
            "{:013}-{:06x}",
            1_700_000_000_000_u64 + u64::try_from(nth).unwrap_or(u64::MAX),
            nth,
        ))
        .expect("a session identifier")
    }

    #[test]
    fn recording_keeps_exactly_the_fixed_newest_window() {
        let sample = Sample::new("session-index-window");
        let path = named(&sample.logs());
        let initial: Vec<Entry> = (0..ENTRIES).map(|nth| Entry::new(id(nth))).rev().collect();
        replace(&path, &initial).expect("the initial index");

        let newest = id(ENTRIES);
        record(&sample.logs(), &newest).expect("the next session");
        let indexed = read(&path).expect("the index").expect("an index");

        assert_eq!(indexed.len(), ENTRIES);
        assert_eq!(indexed.first().map(|entry| &entry.id), Some(&newest));
        assert_eq!(indexed.last().map(|entry| &entry.id), Some(&id(1)));
    }

    #[test]
    fn a_count_and_a_title_survive_the_index() {
        let sample = Sample::new("session-index-metadata");
        let noted = id(1);
        record(&sample.logs(), &noted).expect("the session recorded");

        tally(&sample.logs(), &noted, 5).expect("the count kept");
        retitle(&sample.logs(), &noted, "fix the parser").expect("the title kept");
        let read = entries(&sample.logs(), ENTRIES).expect("the index");
        let [entry] = read.as_slice() else {
            panic!("one session went in")
        };

        assert_eq!(entry.id, noted);
        assert_eq!(entry.messages, 5);
        assert_eq!(entry.title.as_deref(), Some("fix the parser"));
    }

    #[test]
    fn recording_a_known_name_again_keeps_what_it_earned() {
        let sample = Sample::new("session-index-reopen");
        let kept = id(1);
        record(&sample.logs(), &kept).expect("the session recorded");
        tally(&sample.logs(), &kept, 9).expect("the count kept");
        retitle(&sample.logs(), &kept, "still mine").expect("the title kept");
        record(&sample.logs(), &id(2)).expect("a newer session");

        record(&sample.logs(), &kept).expect("the session picked back up");
        let read = entries(&sample.logs(), ENTRIES).expect("the index");
        let front = read.first().expect("the reopened session leads");

        assert_eq!(front.id, kept);
        assert_eq!(front.messages, 9);
        assert_eq!(front.title.as_deref(), Some("still mine"));
    }

    #[test]
    fn a_title_is_flattened_and_bounded_where_it_is_written() {
        let sample = Sample::new("session-index-title-bound");
        let noted = id(1);
        record(&sample.logs(), &noted).expect("the session recorded");

        let sprawling = format!("two\nlines\tand {}", "x".repeat(1024));
        retitle(&sample.logs(), &noted, &sprawling).expect("the title kept");
        let read = entries(&sample.logs(), ENTRIES).expect("the index");
        let title = read
            .first()
            .and_then(|entry| entry.title.as_deref())
            .expect("a title was saved");

        assert!(title.starts_with("two lines and x"), "{title:?}");
        assert!(!title.contains(['\n', '\t']), "{title:?}");
        assert!(
            title.chars().count() <= super::super::recent::TITLE,
            "{title:?}"
        );
    }

    #[test]
    fn a_format_one_index_still_reads_and_its_entries_start_bare() {
        let sample = Sample::new("session-index-format-one");
        let path = named(&sample.logs());
        // Frozen bytes: exactly what a format 1 build left behind.
        fs::write(
            &path,
            format!(
                "crucible-session-index-1\n{}\n{}\n",
                id(2).as_str(),
                id(1).as_str()
            ),
        )
        .expect("a writable temporary directory");

        let read = entries(&sample.logs(), ENTRIES).expect("the old index still reads");

        assert_eq!(read.len(), 2);
        assert_eq!(read.first().map(|entry| &entry.id), Some(&id(2)));
        assert!(
            read.iter()
                .all(|entry| entry.messages == 0 && entry.title.is_none())
        );
    }

    #[test]
    fn a_corrupt_index_is_refused_by_name() {
        let sample = Sample::new("session-index-corrupt");
        let path = named(&sample.logs());
        fs::write(&path, "not the index\n").expect("a writable temporary directory");

        let problem = entries(&sample.logs(), 4).expect_err("a corrupt index to fail");

        assert!(matches!(problem, SessionError::Index { .. }), "{problem:?}");
        assert!(problem.to_string().contains("recent.sessions"), "{problem}");
    }
}
