//! The file a session is written to: opening it, appending to it, and cutting
//! it back.
//!
//! Everything here touches the handle. The shape of what goes through it is
//! [`super::wire`]'s, who may read it is [`super::privacy`]'s, and what a
//! session *is* stays one level up — this is the part that would otherwise
//! spread out across all three.

use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};

use crucible_types::ToolId;
use tokio::sync::{Notify, oneshot};

use super::SessionError;
use super::places::Place;

/// Where the first write that failed is left for the main thread to find.
pub(super) type Trouble = Arc<Mutex<Option<Box<str>>>>;

/// One ordered request to the thread that owns the append handle.
pub(super) enum Request {
    /// Append one complete JSON line.
    Line(Box<str>),
    /// Append one complete JSON line, flush, and then say so: by the time the
    /// sender hears back, the line is with the operating system, or the
    /// failure that stopped it is the session's trouble.
    Acknowledged(Box<str>, oneshot::Sender<()>),
    /// Flush every earlier line before acknowledging the caller.
    Barrier(SyncSender<()>),
    /// Append one message line that holds the results of `calls`, keep where
    /// it went, and acknowledge `taken` where there is one to tell.
    Results {
        /// The message's line. A message of results is written with no guard
        /// before it, so where the line begins is where its record does.
        line: Box<str>,
        /// The calls whose results the message holds.
        calls: Box<[ToolId]>,
        /// Told once the line is taken, where the sender waits.
        taken: Option<oneshot::Sender<()>>,
    },
}

/// The most places kept for the session to hand over, oldest let go first.
///
/// Eight passes of the most calls one pass may make. The screen takes them as
/// each result arrives, so this bounds a screen that falls that far behind,
/// and a session nothing draws, rather than a number an ordinary run reaches.
pub(super) const PLACED: usize = 8 * 128;

/// Where the results the writer appended went, waiting to be taken.
pub(super) type Placed = Arc<Mutex<VecDeque<Place>>>;

/// Appends every line that arrives until the session is dropped, telling
/// `room` each time one is taken off the queue.
///
/// However this ends — the session dropped, or the sink coming apart and
/// unwinding through here — the queue is closed before `room` is told once
/// more, so a write still waiting for room wakes to a queue that is gone and
/// answers as unacknowledged rather than waiting for a line nobody will take.
///
/// A failure is recorded once and the loop goes on, because the senders that
/// do not wait are not waiting for an answer: stopping here would fill the
/// queue and block them instead of losing a log nobody can write anyway. A
/// sender that does wait hears back after the failure is recorded.
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
///
/// Counting is also what places a result. `start` is how long the file was
/// when this began, and from there the line of each message of results lands
/// at a count this knows, which is kept in `placed` until it is taken. Where
/// the length could not be read there is no count to trust, and nothing is
/// placed.
pub(super) fn write<W: io::Write>(
    mut sink: W,
    lines: Receiver<Request>,
    trouble: &Trouble,
    room: Arc<Notify>,
    (start, placed): (Option<u64>, Placed),
) {
    let queue = Closing {
        lines: Some(lines),
        room,
    };
    let Some(lines) = queue.lines.as_ref() else {
        return;
    };
    let placing = start;
    let mut tail = Tail {
        written: start.unwrap_or(0),
        ..Tail::default()
    };

    for request in lines {
        queue.room.notify_waiters();
        match request {
            Request::Line(line) => {
                tail.append(&mut sink, &line, trouble);
            }
            Request::Acknowledged(line, taken) => {
                tail.append(&mut sink, &line, trouble);
                flushed(&mut sink, trouble);
                // A sender that stopped waiting has nobody to tell.
                let _ = taken.send(());
            }
            Request::Barrier(done) => {
                flushed(&mut sink, trouble);
                let _ = done.send(());
            }
            Request::Results { line, calls, taken } => {
                let begins = tail.append(&mut sink, &line, trouble);
                // Only where the file's length was known when the writer
                // started: a count begun anywhere else would name places the
                // log does not bear out.
                if let Some(begins) = begins.filter(|_| placing.is_some()) {
                    let mut held = placed.lock().unwrap_or_else(PoisonError::into_inner);
                    for call in calls {
                        held.push_back(Place::new(call, begins));
                    }
                    while held.len() > PLACED {
                        held.pop_front();
                    }
                }
                if let Some(taken) = taken {
                    flushed(&mut sink, trouble);
                    let _ = taken.send(());
                }
            }
        }
    }
}

/// The writer's end of the queue, which closes it and wakes every write
/// waiting for room as it goes, on whatever path the writer leaves by.
struct Closing {
    lines: Option<Receiver<Request>>,
    room: Arc<Notify>,
}

impl Drop for Closing {
    fn drop(&mut self) {
        // Closed first, so a waiter the notice wakes finds it closed.
        drop(self.lines.take());
        self.room.notify_waiters();
    }
}

/// What the failures so far have left at the end of the file.
#[derive(Default)]
struct Tail {
    /// A line that landed whole and is still owed the newline that ends it.
    torn: bool,
    /// A fragment landed mid-line, and the file must end where it ends.
    dead: bool,
    /// How long the file is, counting every byte that landed.
    written: u64,
}

impl Tail {
    /// Appends `line` and the newline that ends it, as far as what earlier
    /// failures left allows, and says where `line` began where all of it and
    /// its newline landed.
    fn append<W: io::Write>(&mut self, sink: &mut W, line: &str, trouble: &Trouble) -> Option<u64> {
        if self.dead {
            return None;
        }

        if self.torn {
            if let Some(problem) = self.counted(append(sink, b"\n")) {
                record(trouble, &problem);
                return None;
            }
            self.torn = false;
        }

        let begins = self.written;
        match append(sink, line.as_bytes()) {
            (landed, None) => {
                self.count(landed);
                if let Some(problem) = self.counted(append(sink, b"\n")) {
                    self.torn = true;
                    record(trouble, &problem);
                    return None;
                }
                Some(begins)
            }
            (0, Some(problem)) => {
                record(trouble, &problem);
                None
            }
            (landed, Some(problem)) => {
                self.count(landed);
                self.dead = true;
                record(trouble, &problem);
                None
            }
        }
    }

    /// Adds what landed to the length of the file.
    fn count(&mut self, landed: usize) {
        self.written = self
            .written
            .saturating_add(u64::try_from(landed).unwrap_or(u64::MAX));
    }

    /// Counts what one write landed and hands back what stopped it.
    fn counted(&mut self, (landed, problem): (usize, Option<io::Error>)) -> Option<io::Error> {
        self.count(landed);
        problem
    }
}

/// Flushes `sink`, keeping a failure as trouble.
fn flushed<W: io::Write>(sink: &mut W, trouble: &Trouble) {
    if let Err(problem) = sink.flush() {
        record(trouble, &problem);
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
pub(super) fn record(trouble: &Trouble, problem: &io::Error) {
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
