//! A fixed newest-session index beside the append-only logs.
//!
//! Reading a directory is not bounded by the number of names retained from it.
//! The welcome runs before the first frame, so it reads this one small file and
//! opens only the logs named there. A session start or `--continue` on an older
//! installation builds the index after that frame by scanning the flat log
//! directory once; every later startup performs fixed work.
//!
//! Entries are kept newest first by the start time each name carries, the
//! order [`super::replay::logs`] puts a directory in. Builds before this one
//! migrated by name as text instead, which puts every name the oldest builds
//! wrote above every uuid name and, past [`ENTRIES`] logs, can leave newer
//! sessions out entirely. So the order is restored wherever the index is read,
//! and an index this build did not write last is merged once with a scan of
//! the directory. What says which build wrote it last is the [`ORDERED`] mark:
//! every write here leaves the index's digest in it, under the same lock, so an
//! index an older build wrote or rebuilt since no longer matches its mark and
//! is merged again, while one this build wrote is never scanned twice. The file
//! format is unchanged: a build that knows nothing of the order or the mark
//! reads what this one writes.
//!
//! The window keeps the [`ENTRIES`] newest sessions, with one exception: the
//! session being recorded is always kept. A clock gone back can date a new
//! session before everything a full window holds, and dropping it would leave
//! the session just started out of every listing, with nowhere for its count or
//! title to go; the oldest other entry makes room instead.
//!
//! The index is replaced whole under an operating-system lock. The replacement
//! is synced before its rename and the directory is synced afterwards, so a
//! crash leaves either complete version. A newly minted identifier is indexed
//! before its header is written: a crash in between leaves a candidate readers
//! validate and skip, rather than a complete log discovery can never find.
//!
//! Both files are read as one ordinary file each, through
//! [`super::privacy::opened`]: an index under a link or a pipe is one that
//! cannot be read, and a mark under either vouches for nothing.

use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use crucible_types::SessionId;

use super::SessionError;
use super::beside::Beside;
use super::claim;
use super::privacy::opened;
use super::replay::logs as legacy_logs;

/// What the index file is called. Its suffix deliberately cannot be a log's.
const NAME: &str = "recent.sessions";

/// Beside the index, holding the digest of the index this build last wrote,
/// so the scan that puts an index in order happens once per index rather than
/// once per start.
const ORDERED: &str = "recent.sessions.ordered";

/// More than a digest's hex and a newline. A mark is never read past this.
const MARK_BYTES: u64 = 128;

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

/// Builds the index once for flat logs written before it existed, and merges
/// one an earlier build ordered by name with what is on disk, once.
pub(super) fn ensure(directory: &Path) -> Result<(), SessionError> {
    let path = named(directory);
    let _held = claim::exclusive(&path).map_err(|source| problem(&path, source))?;
    let held = match text(&path)? {
        Some(text) if vouched(directory, &text) => return Ok(()),
        Some(text) => Some(parse(&path, &text)?),
        None => None,
    };

    // The one unbounded read this module makes: once per directory, after the
    // first frame, and through the same name listing `--continue` uses. Every
    // startup after it reads only the fixed index written here. Message counts
    // start at zero — counting would mean reading every log — and are repaired
    // the next time each session is continued, when replay knows the number.
    //
    // What an existing index holds is kept, counts and titles with it, and so
    // is a name it holds with no log yet: that is a session being started
    // this instant, between its entry and its header.
    let mut entries = held.unwrap_or_default();
    let found = legacy_logs(directory)?
        .into_iter()
        .rev()
        .take(ENTRIES)
        .filter_map(|path| SessionId::from_str(path.file_stem()?.to_str()?).ok())
        .collect::<Vec<_>>();
    for id in found {
        if !entries.iter().any(|entry| entry.id == id) {
            entries.push(Entry::new(id));
        }
    }
    newest_first(&mut entries);
    entries.truncate(ENTRIES);
    replace(&path, &entries, true)
}

/// Puts entries newest first by the start time their names carry, as
/// [`super::replay::logs`] orders logs: two that start in the same millisecond
/// go by name.
fn newest_first(entries: &mut [Entry]) {
    entries.sort_unstable_by(|one, other| {
        (other.id.started(), &other.id).cmp(&(one.id.started(), &one.id))
    });
}

/// Adds a newly minted identifier where its start time puts it, retaining a
/// fixed newest window.
///
/// A name the index already holds keeps what it earned: the count and the
/// title stay with it, so recording a session again does not erase what the
/// picker shows for it.
pub(super) fn record(directory: &Path, id: &SessionId) -> Result<(), SessionError> {
    let path = named(directory);
    let _held = claim::exclusive(&path).map_err(|source| problem(&path, source))?;
    let (mut entries, vouched) = held(&path)?;

    let known = entries.iter().position(|held| &held.id == id);
    let entry = match known {
        Some(position) => entries.remove(position),
        None => Entry::new(id.clone()),
    };
    entries.insert(0, entry);
    newest_first(&mut entries);

    // Past the window, the oldest entry that is not this one goes: the session
    // being recorded is the one somebody just started.
    while entries.len() > ENTRIES {
        let Some(oldest) = entries.iter().rposition(|held| &held.id != id) else {
            break;
        };
        entries.remove(oldest);
    }
    replace(&path, &entries, vouched)
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
    let (mut entries, vouched) = held(&path)?;

    let Some(entry) = entries.iter_mut().find(|held| &held.id == id) else {
        return Ok(());
    };
    change(entry);
    replace(&path, &entries, vouched)
}

/// The newest indexed entries, newest first and at most `maximum`.
pub(super) fn entries(directory: &Path, maximum: usize) -> Result<Vec<Entry>, SessionError> {
    Ok(written(directory, maximum)?.unwrap_or_default())
}

/// The newest indexed entries, newest first and at most `maximum`, or `None`
/// where no index has been written yet. Read without the lock and without
/// writing, as [`entries`] is.
pub(super) fn written(
    directory: &Path,
    maximum: usize,
) -> Result<Option<Vec<Entry>>, SessionError> {
    let mut entries = read(&named(directory))?;
    if let Some(entries) = entries.as_mut() {
        entries.truncate(maximum);
    }
    Ok(entries)
}

/// What the index holds, and whether the mark vouches for it, so a change
/// written over it vouches for the result only where the mark already did: an
/// index no scan has put in order yet stays one the next start scans.
fn held(path: &Path) -> Result<(Vec<Entry>, bool), SessionError> {
    let Some(text) = text(path)? else {
        return Ok((Vec::new(), false));
    };
    let vouched = path
        .parent()
        .is_some_and(|directory| vouched(directory, &text));
    Ok((parse(path, &text)?, vouched))
}

/// Reads a complete index, newest first, `None` where migration has not made
/// one yet.
///
/// Put in start-time order here, whatever order the file is in, so an index an
/// earlier build kept by name is listed as one timeline and rewritten as one
/// the next time anything changes it.
fn read(path: &Path) -> Result<Option<Vec<Entry>>, SessionError> {
    text(path)?.map(|text| parse(path, &text)).transpose()
}

/// The index file's text, bounded, `None` where there is none yet.
///
/// A link or a pipe under the index's name is an index that cannot be read,
/// refused without following the one or waiting on the other.
fn text(path: &Path) -> Result<Option<String>, SessionError> {
    let opened = match opened(path) {
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

    Ok(Some(text))
}

/// An index's text as its entries, newest first.
fn parse(path: &Path, text: &str) -> Result<Vec<Entry>, SessionError> {
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
    let mut entries = entries.ok_or_else(|| {
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

    newest_first(&mut entries);
    Ok(entries)
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

/// Replaces the index with one whole, durable version, and where `vouch` says
/// so, leaves its digest in the mark.
fn replace(path: &Path, entries: &[Entry], vouch: bool) -> Result<(), SessionError> {
    let directory = path.parent().ok_or_else(|| {
        problem(
            path,
            io::Error::other("session index has no parent directory"),
        )
    })?;

    let mut text = format!("{FORMAT}\n");
    for entry in entries {
        text.push_str(entry.id.as_str());
        text.push('\t');
        text.push_str(&entry.messages.to_string());
        text.push('\t');
        text.push_str(entry.title.as_deref().unwrap_or_default());
        text.push('\n');
    }

    let mut beside = Beside::new(directory, "recent").map_err(|source| problem(path, source))?;
    {
        let file = beside.file().map_err(|source| problem(path, source))?;
        file.write_all(text.as_bytes())
            .map_err(|source| problem(path, source))?;
        file.sync_all().map_err(|source| problem(path, source))?;
    }
    beside.over(path).map_err(|source| problem(path, source))?;

    // Only once the index it vouches for is in place. A mark that could not be
    // left costs one more scan at the next start, never a session.
    if vouch {
        let _ = leave_mark(directory, &text);
    }
    Ok(())
}

/// What the mark holds for an index whose text is `text`.
fn digest(text: &str) -> String {
    use sha2::{Digest as _, Sha256};
    use std::fmt::Write as _;

    let mut hex = String::with_capacity(65);
    for byte in Sha256::digest(text.as_bytes()) {
        let _ = write!(hex, "{byte:02x}");
    }
    hex.push('\n');
    hex
}

/// Whether the mark says this build wrote `text` last.
///
/// A mark that is a link or a pipe vouches for nothing, as one that will not
/// open does: the next start scans once more rather than taking the word of a
/// file outside the directory or waiting on a writer.
fn vouched(directory: &Path, text: &str) -> bool {
    let mut held = String::new();
    opened(&directory.join(ORDERED))
        .and_then(|mark| mark.take(MARK_BYTES).read_to_string(&mut held))
        .is_ok_and(|_| held == digest(text))
}

/// Leaves the digest of the index just written in the mark.
fn leave_mark(directory: &Path, text: &str) -> io::Result<()> {
    let mut mark = super::privacy::mark(&directory.join(ORDERED))?;
    mark.set_len(0)?;
    mark.write_all(digest(text).as_bytes())
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
        replace(&path, &initial, false).expect("the initial index");

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

        record(&sample.logs(), &kept).expect("the session recorded again");
        let read = entries(&sample.logs(), ENTRIES).expect("the index");
        let order: Vec<&SessionId> = read.iter().map(|entry| &entry.id).collect();
        let again = read.get(1).expect("the earlier session");

        // Placed by when it started, as every listing is, not by when it was
        // last recorded.
        assert_eq!(order, [&id(2), &kept]);
        assert_eq!(again.messages, 9);
        assert_eq!(again.title.as_deref(), Some("still mine"));
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

    /// A session named the way builds since 0.25 name one, `minutes` after
    /// 2025-01-01.
    fn uuid(minutes: u64) -> SessionId {
        let millis = 1_735_689_600_000_u64 + minutes * 60_000;
        let hex = format!("{millis:012x}");
        let (high, low) = hex.split_at(8);
        SessionId::from_str(&format!("{high}-{low}-7000-8000-000000000000"))
            .expect("a uuid session identifier")
    }

    /// An empty file under each name, which is all a scan of names looks at.
    fn on_disk(directory: &Path, ids: &[SessionId]) {
        for id in ids {
            fs::write(
                directory.join(format!("{}.{}", id.as_str(), super::super::SUFFIX)),
                "",
            )
            .expect("a writable temporary directory");
        }
    }

    /// An index exactly as builds before this one migrated a directory that
    /// held both shapes of name: by name as text, newest last, so every legacy
    /// name above every uuid one.
    fn by_name(path: &Path, ids: &[SessionId]) {
        let mut ordered = ids.to_vec();
        ordered.sort_unstable();
        let mut text = format!("{FORMAT}\n");
        for id in ordered.iter().rev() {
            text.push_str(id.as_str());
            text.push_str("\t0\t\n");
        }
        fs::write(path, text).expect("a writable temporary directory");
    }

    #[test]
    fn an_index_kept_in_name_order_is_read_in_start_order() {
        let sample = Sample::new("session-index-name-order");
        // 2023-11-14, named the way older builds named it; then 2025.
        let old = id(1);
        let new = uuid(0);
        by_name(&named(&sample.logs()), &[old.clone(), new.clone()]);

        let read = entries(&sample.logs(), ENTRIES).expect("the index");
        let order: Vec<&SessionId> = read.iter().map(|entry| &entry.id).collect();

        assert_eq!(order, [&new, &old]);
    }

    #[test]
    fn sessions_an_index_kept_in_name_order_left_out_are_indexed_again() {
        // The old migration took the window by name, so a directory with a
        // full window of legacy logs kept none of its newer uuid ones.
        let sample = Sample::new("session-index-name-order-window");
        let legacy: Vec<SessionId> = (0..ENTRIES).map(id).collect();
        let newer = [uuid(0), uuid(1)];
        on_disk(&sample.logs(), &legacy);
        on_disk(&sample.logs(), &newer);
        let path = named(&sample.logs());
        by_name(&path, &legacy);
        let titled = id(ENTRIES - 1);
        tally(&sample.logs(), &titled, 3).expect("the count kept");
        retitle(&sample.logs(), &titled, "kept across the repair").expect("the title kept");

        ensure(&sample.logs()).expect("the index repaired");
        let read = entries(&sample.logs(), ENTRIES).expect("the index");
        let order: Vec<&SessionId> = read.iter().map(|entry| &entry.id).collect();

        assert_eq!(read.len(), ENTRIES);
        assert_eq!(
            order.get(..3),
            Some([&newer[1], &newer[0], &titled].as_slice())
        );
        assert_eq!(order.last(), Some(&&id(2)), "the oldest two make room");
        let kept = read.get(2).expect("the newest legacy session");
        assert_eq!(kept.messages, 3);
        assert_eq!(kept.title.as_deref(), Some("kept across the repair"));
    }

    #[test]
    fn an_index_is_repaired_once_and_then_read_without_a_scan() {
        let sample = Sample::new("session-index-repaired-once");
        let path = named(&sample.logs());
        on_disk(&sample.logs(), &[id(1)]);
        by_name(&path, &[id(1)]);
        ensure(&sample.logs()).expect("the index repaired");

        // What this build writes afterwards keeps the mark with the index:
        // a session started, and the same session ending, counted and named.
        record(&sample.logs(), &id(2)).expect("a session recorded");
        tally(&sample.logs(), &id(2), 1).expect("a session counted");
        retitle(&sample.logs(), &id(2), "named").expect("a session named");

        // A log the index was never told about: only a scan could find it, and
        // the startup after a repair does fixed work.
        on_disk(&sample.logs(), &[uuid(0)]);
        ensure(&sample.logs()).expect("the index already repaired");
        let read = entries(&sample.logs(), ENTRIES).expect("the index");

        assert_eq!(read.len(), 2);
    }

    #[test]
    fn a_session_recorded_older_than_a_full_window_is_still_kept() {
        // A clock gone back can date a new session before everything a full
        // window holds. It is the session just started, so it stays, and the
        // oldest other entry makes room for it.
        let sample = Sample::new("session-index-clock-back");
        let path = named(&sample.logs());
        let initial: Vec<Entry> = (0..ENTRIES).map(|nth| Entry::new(id(nth))).collect();
        replace(&path, &initial, false).expect("the initial index");
        let behind = SessionId::from_str("1600000000000-abcdef").expect("a session identifier");

        record(&sample.logs(), &behind).expect("the session recorded");
        let read = entries(&sample.logs(), ENTRIES).expect("the index");

        assert_eq!(read.len(), ENTRIES);
        assert!(read.iter().any(|entry| entry.id == behind), "dropped");
        assert!(
            !read.iter().any(|entry| entry.id == id(0)),
            "kept the oldest"
        );
        tally(&sample.logs(), &behind, 4).expect("the count kept");
        let counted = entries(&sample.logs(), ENTRIES).expect("the index");
        assert!(
            counted
                .iter()
                .any(|entry| entry.id == behind && entry.messages == 4)
        );
    }

    #[test]
    fn an_index_an_earlier_build_rebuilt_by_name_is_repaired_again() {
        // A rollback to a build that knows nothing of the order, and an index
        // deleted while it ran: that build rebuilds the index by name, under a
        // mark this one left for a different index.
        let sample = Sample::new("session-index-rebuilt-by-name");
        let legacy: Vec<SessionId> = (0..ENTRIES).map(id).collect();
        let newer = [uuid(0), uuid(1)];
        on_disk(&sample.logs(), &legacy);
        on_disk(&sample.logs(), &newer);
        let path = named(&sample.logs());
        ensure(&sample.logs()).expect("the index built");

        fs::remove_file(&path).expect("the index deleted");
        by_name(&path, &legacy);
        ensure(&sample.logs()).expect("the index repaired");
        let read = entries(&sample.logs(), ENTRIES).expect("the index");
        let order: Vec<&SessionId> = read.iter().map(|entry| &entry.id).collect();

        assert_eq!(order.get(..2), Some([&newer[1], &newer[0]].as_slice()));
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
