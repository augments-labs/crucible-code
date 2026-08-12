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
/// Going on is what the newline is for. A line reaches the file as its bytes
/// and then the newline that ends it, so a write that fails can stop between
/// the two and leave a line with nothing after it — and the next line, appended
/// straight onto that, makes one line that is neither message in the middle of
/// the log. Starting the line after a failure with a newline of its own is what
/// keeps the damage to the line that took it. Nothing was recorded where an
/// empty line lands, which is why the replay reads past one.
pub(super) fn write<W: io::Write>(mut sink: W, lines: &Receiver<Box<str>>, trouble: &Trouble) {
    let mut torn = false;

    for line in lines {
        let ended = if torn { "\n" } else { "" };

        let Err(problem) = writeln!(sink, "{ended}{line}") else {
            torn = false;
            continue;
        };

        torn = true;

        if let Ok(mut held) = trouble.lock() {
            held.get_or_insert_with(|| problem.to_string().into());
        }
    }
}

/// Opens a log for appending, making it if it is not there.
///
/// Reachable by this account and no other — see [`super::privacy`], which is
/// where what that means on each platform is written down. A log holds what was
/// typed, what the model said, the contents of the files that were read and
/// everything a command printed.
pub(super) fn open(path: &Path) -> Result<File, SessionError> {
    super::privacy::log(path).map_err(|source| SessionError::Log {
        at: path.display().to_string().into(),
        source,
    })
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
