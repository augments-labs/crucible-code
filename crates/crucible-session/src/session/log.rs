//! The file a session is written to: opening it, appending to it, and cutting
//! it back.
//!
//! Everything here touches the handle. The shape of what goes through it is
//! [`super::wire`]'s, who may read it is [`super::privacy`]'s, and what a
//! session *is* stays one level up — this is the part that would otherwise
//! spread out across all three.

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use super::SessionError;

/// Where the first write that failed is left for the main thread to find.
pub(super) type Trouble = Arc<Mutex<Option<Box<str>>>>;

/// Appends every line that arrives until the session is dropped.
///
/// A failure is recorded once and the loop goes on, because the senders are
/// not waiting for an answer: stopping here would fill the queue and block the
/// turn instead of losing a log nobody can write anyway.
///
/// Going on is why every write counts its bytes: what a failure leaves in the
/// file decides what may follow it, and there are three answers. A write that
/// left nothing leaves the file exactly as it was, and the next line starts
/// clean. A line whose bytes all landed and whose newline did not is ended
/// with that newline before the next line starts, which completes the record
/// it cut short. And a line torn in the middle is the one thing no byte can
/// mend — a newline would make a line that is not a message in the middle of
/// the log, and the replay refuses everything from there on — so from a
/// fragment onward nothing more is written: the file ends at the fragment,
/// which the replay reads as a log torn at the tail, whole up to its last
/// line.
pub(super) fn write<W: io::Write>(mut sink: W, lines: &Receiver<Box<str>>, trouble: &Trouble) {
    // A line that landed whole and is still owed the newline that ends it.
    let mut torn = false;
    // A fragment landed mid-line, and the file must end where it ends.
    let mut dead = false;

    for line in lines {
        if dead {
            continue;
        }

        if torn {
            if let (_, Some(problem)) = append(&mut sink, b"\n") {
                record(trouble, &problem);
                continue;
            }
            torn = false;
        }

        match append(&mut sink, line.as_bytes()) {
            (_, None) => {
                if let (_, Some(problem)) = append(&mut sink, b"\n") {
                    torn = true;
                    record(trouble, &problem);
                }
            }
            (0, Some(problem)) => record(trouble, &problem),
            (_, Some(problem)) => {
                dead = true;
                record(trouble, &problem);
            }
        }
    }
}

/// Writes all of `bytes`, saying how many landed beside any failure.
///
/// [`io::Write::write_all`] with the count kept, because the count is the
/// whole point: an error alone cannot say whether the file is untouched, torn
/// between a line and its newline, or torn in the middle of one, and those are
/// three different recoveries. A failed call is guaranteed to have written
/// nothing, so the count is exact.
fn append<W: io::Write>(sink: &mut W, bytes: &[u8]) -> (usize, Option<io::Error>) {
    let mut written = 0;

    while let Some(rest) = bytes.get(written..).filter(|rest| !rest.is_empty()) {
        match sink.write(rest) {
            Ok(0) => return (written, Some(io::ErrorKind::WriteZero.into())),
            Ok(landed) => written += landed,
            Err(problem) if problem.kind() == io::ErrorKind::Interrupted => {}
            Err(problem) => return (written, Some(problem)),
        }
    }

    (written, None)
}

/// Keeps the first failure for the main thread to find; later ones tell it
/// nothing it can act on.
fn record(trouble: &Trouble, problem: &io::Error) {
    if let Ok(mut held) = trouble.lock() {
        held.get_or_insert_with(|| problem.to_string().into());
    }
}

/// Opens a log that is already there for appending, making it if it is not.
///
/// Reachable by this account and no other — see [`super::privacy`], which is
/// where what that means on each platform is written down. A log holds what was
/// typed, what the model said, the contents of the files that were read and
/// everything a command printed.
///
/// What reaches this is a session being continued, which found its log before
/// it got here. A session starting takes [`make`] instead: it is the call that
/// must not open a log somebody else is writing.
pub(super) fn open(path: &Path) -> Result<File, SessionError> {
    super::privacy::log(path).map_err(|source| SessionError::Log {
        at: path.display().to_string().into(),
        source,
    })
}

/// Makes the log for a session starting now, or says one is already there.
///
/// `None` is not a failure: it is the filesystem answering that this name
/// belongs to somebody else, which is the answer [`super::taking`] asked for.
/// Every other way the call can fail is one, and is reported against the log
/// the same as any other.
///
/// The creation is exclusive — see the platform module — which is what settles
/// the name between two crucibles that minted it in the same millisecond.
/// [`open`] is the other half of the pair and does the opposite on purpose: a
/// session being continued has a log and must find it.
pub(super) fn make(path: &Path) -> Result<Option<File>, SessionError> {
    match super::privacy::fresh(path) {
        Ok(file) => Ok(Some(file)),
        Err(problem) if problem.kind() == io::ErrorKind::AlreadyExists => Ok(None),
        Err(source) => Err(SessionError::Log {
            at: path.display().to_string().into(),
            source,
        }),
    }
}

/// Cuts a log back to `bytes`, through a handle opened for that and nothing
/// else.
///
/// Its own handle because of what appending is. On Windows a handle opened for
/// append is granted the right to add to a file and not the right to change
/// what is already in it — the two are separate rights, and shortening a file
/// needs the second one. So the log is shortened through a handle that may
/// write it, which is closed again before the one that may only append is
/// opened.
pub(super) fn shorten(path: &Path, bytes: u64) -> Result<(), SessionError> {
    let trouble = |source| SessionError::Log {
        at: path.display().to_string().into(),
        source,
    };

    File::options()
        .write(true)
        .open(path)
        .map_err(trouble)?
        .set_len(bytes)
        .map_err(trouble)
}
