//! Where a panic's message goes while a session holds the terminal.
//!
//! The default hook writes a panic's message to standard error as the thread
//! unwinds, before anything joining that thread or task has seen it. While a
//! session holds the terminal in raw mode, that write lands in the middle of a
//! frame: the drawing thread is the screen's only writer, and a line written
//! past it breaks the frame it lands in and is drawn over by the next.
//!
//! So for as long as a session holds the terminal, a panic on any other
//! thread — a task on the application's runtime, most of all — is kept for the
//! drawing thread instead, which says it in the transcript the next time it
//! comes round to the prompt. What it has not said by the time the session
//! lets go is written to standard error then, once the screen is the reader's
//! own again, so a panic is never lost. A panic on the drawing thread itself
//! still goes to the hook in force before: it ends the session, and there is
//! nobody left to keep it for.
//!
//! Only a session holding the terminal takes the hook, and it puts back the
//! hook it found when it lets go, so every other path panics the way that hook
//! has it.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::panic::{self, PanicHookInfo};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, ThreadId};

/// How many panics are kept for the drawing thread at once. Past it, a panic
/// is counted rather than kept: a loop of them says the same thing again.
const KEPT: usize = 8;

/// The most of one panic's message kept, in bytes. A payload is whatever the
/// panicking code formatted, and the transcript line it becomes is one line.
const MESSAGE: usize = 1024;

/// The hook a session found, and puts back when it lets go.
type Found = Arc<dyn Fn(&PanicHookInfo<'_>) + Send + Sync>;

/// What panicked on another thread while a session held the terminal.
///
/// Dropped as the session lets go of the terminal, after the screen is handed
/// back: the hook it found is put back, and whatever the drawing thread has
/// not taken is written to standard error.
pub(crate) struct Panics {
    kept: Arc<Mutex<Kept>>,
    found: Found,
}

impl std::fmt::Debug for Panics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Panics").finish_non_exhaustive()
    }
}

/// The panics waiting for the drawing thread, and how many went past the
/// ceiling.
#[derive(Debug, Default)]
struct Kept {
    said: VecDeque<String>,
    unkept: usize,
}

impl Panics {
    /// Keeps a panic on any thread but this one for this thread to say, until
    /// dropped.
    pub(crate) fn kept() -> Self {
        let kept = Arc::new(Mutex::new(Kept::default()));
        let found: Found = Arc::from(panic::take_hook());
        let drawing = thread::current().id();
        panic::set_hook(Box::new({
            let kept = Arc::clone(&kept);
            let found = Arc::clone(&found);
            move |info| keep(&kept, &found, drawing, info)
        }));
        Self { kept, found }
    }

    /// What was kept since the last time this was asked, oldest first, and how
    /// many panics went past the ceiling without being kept.
    pub(crate) fn take(&self) -> (Vec<String>, usize) {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        let unkept = std::mem::take(&mut kept.unkept);
        (kept.said.drain(..).collect(), unkept)
    }
}

impl Drop for Panics {
    fn drop(&mut self) {
        let found = Arc::clone(&self.found);
        panic::set_hook(Box::new(move |info| found(info)));

        let (said, unkept) = self.take();
        let mut written = String::new();
        for one in said {
            written.push_str("crucible: ");
            written.push_str(&one);
            written.push('\n');
        }
        if unkept > 0 {
            let _ = writeln!(written, "crucible: and {unkept} more panics");
        }
        if !written.is_empty() {
            let _ = io::stderr().write_all(written.as_bytes());
        }
    }
}

/// The hook while a session holds the terminal: `info` kept for the drawing
/// thread, unless it is the drawing thread's own.
fn keep(kept: &Mutex<Kept>, found: &Found, drawing: ThreadId, info: &PanicHookInfo<'_>) {
    let current = thread::current();
    if current.id() == drawing {
        return found(info);
    }
    let mut said = format!("{} panicked", current.name().unwrap_or("a thread"));
    if let Some(at) = info.location() {
        let _ = write!(said, " at {at}");
    }
    said.push_str(": ");
    said.push_str(info.payload_as_str().unwrap_or("with no message"));
    if said.len() > MESSAGE {
        let mut end = MESSAGE;
        while !said.is_char_boundary(end) {
            end -= 1;
        }
        said.truncate(end);
        said.push_str(" [message cut]");
    }

    let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
    if kept.said.len() < KEPT {
        kept.said.push_back(said);
    } else {
        kept.unkept = kept.unkept.saturating_add(1);
    }
}

#[cfg(test)]
mod tests;
