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

use super::beside::Beside;
use super::claim;
use super::replay::logs as legacy_logs;
use super::{SUFFIX, SessionError};

/// What the index file is called. Its suffix deliberately cannot be a log's.
const NAME: &str = "recent.sessions";

/// The first line, so an incompatible index is refused rather than guessed at.
const FORMAT: &str = "crucible-session-index-1";

/// How many global session names discovery can inspect.
pub(super) const ENTRIES: usize = 256;

/// Greater than the header and [`ENTRIES`] maximum-length identifiers.
const BYTES: u64 = 32 * 1024;

/// Builds the index once for flat logs written before it existed.
pub(super) fn ensure(directory: &Path) -> Result<(), SessionError> {
    let path = named(directory);
    let _held = claim::exclusive(&path).map_err(|source| problem(&path, source))?;
    if read(&path)?.is_some() {
        return Ok(());
    }

    // The one unbounded read this module makes: once per directory, after the
    // first frame, and through the same name listing `--continue` uses. Every
    // startup after it reads only the fixed index written here.
    let ids = legacy_logs(directory)?
        .into_iter()
        .rev()
        .take(ENTRIES)
        .filter_map(|path| SessionId::from_str(path.file_stem()?.to_str()?).ok())
        .collect::<Vec<_>>();
    replace(&path, &ids)
}

/// Puts a newly minted identifier first, retaining a fixed newest window.
pub(super) fn record(directory: &Path, id: &SessionId) -> Result<(), SessionError> {
    let path = named(directory);
    let _held = claim::exclusive(&path).map_err(|source| problem(&path, source))?;
    let mut ids = read(&path)?.unwrap_or_default();

    ids.retain(|held| held != id);
    ids.insert(0, id.clone());
    ids.truncate(ENTRIES);
    replace(&path, &ids)
}

/// The newest indexed log paths, newest first and at most `maximum`.
pub(super) fn logs(directory: &Path, maximum: usize) -> Result<Vec<PathBuf>, SessionError> {
    let path = named(directory);
    let ids = read(&path)?.unwrap_or_default();

    Ok(ids
        .into_iter()
        .take(maximum)
        .map(|id| directory.join(format!("{}.{SUFFIX}", id.as_str())))
        .collect())
}

/// Reads a complete index, `None` where migration has not made one yet.
fn read(path: &Path) -> Result<Option<Vec<SessionId>>, SessionError> {
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
            io::Error::new(io::ErrorKind::InvalidData, "session index exceeds 32 KiB"),
        ));
    }

    let mut lines = text.lines();
    if lines.next() != Some(FORMAT) {
        return Err(problem(
            path,
            io::Error::new(io::ErrorKind::InvalidData, "unknown session index format"),
        ));
    }

    let ids = lines
        .map(SessionId::from_str)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| {
            problem(
                path,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid session index identifier",
                ),
            )
        })?;
    if ids.len() > ENTRIES {
        return Err(problem(
            path,
            io::Error::new(
                io::ErrorKind::InvalidData,
                "session index has too many entries",
            ),
        ));
    }

    Ok(Some(ids))
}

/// Replaces the index with one whole, durable version.
fn replace(path: &Path, ids: &[SessionId]) -> Result<(), SessionError> {
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
        for id in ids {
            writeln!(file, "{}", id.as_str()).map_err(|source| problem(path, source))?;
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
        let initial: Vec<SessionId> = (0..ENTRIES).map(id).rev().collect();
        replace(&path, &initial).expect("the initial index");

        let newest = id(ENTRIES);
        record(&sample.logs(), &newest).expect("the next session");
        let indexed = read(&path).expect("the index").expect("an index");

        assert_eq!(indexed.len(), ENTRIES);
        assert_eq!(indexed.first(), Some(&newest));
        assert_eq!(indexed.last(), Some(&id(1)));
    }

    #[test]
    fn a_corrupt_index_is_refused_by_name() {
        let sample = Sample::new("session-index-corrupt");
        let path = named(&sample.logs());
        fs::write(&path, "not the index\n").expect("a writable temporary directory");

        let problem = logs(&sample.logs(), 4).expect_err("a corrupt index to fail");

        assert!(matches!(problem, SessionError::Index { .. }), "{problem:?}");
        assert!(problem.to_string().contains("recent.sessions"), "{problem}");
    }
}
