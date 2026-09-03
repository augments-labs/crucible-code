//! What a confined process said beside the conversation, kept and bounded.
//!
//! Standard error is not part of the protocol, and that is exactly why it needs
//! an owner. The sandbox gives every command a pipe for it whether anybody is
//! reading or not, and a pipe nobody reads fills — at which point the extension
//! blocks in a write crucible is not waiting on, having said nothing wrong. A
//! host that leaves standard error alone is a host that hangs on an extension
//! for being talkative.
//!
//! So it is drained, on a thread, and thrown away except for the beginning. The
//! beginning rather than the end because the question this answers is why an
//! extension stopped, and the first thing that went wrong says that; what
//! follows is usually the same thing again with the process falling over on top
//! of it. How much was dropped is kept too, because a bound nobody is told
//! about reads as an extension that went quiet.

use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crucible_core::{SandboxOutput, SandboxRead};

/// How much of one extension's complaint is kept.
///
/// Enough for a stack trace or a loader's list of what it could not find, and
/// far short of anything worth streaming. This is a diagnostic, not an output
/// channel: an extension with something to say to crucible has a protocol for
/// saying it.
const KEPT: usize = 8 * 1024;

/// How long the drain sleeps between asking a quiet stream again.
const PAUSE: Duration = Duration::from_millis(5);

/// How much of one read is taken at a time.
const CHUNK: usize = 4 * 1024;

/// What was said and how much was not.
#[derive(Debug, Default)]
struct Kept {
    /// The beginning of it, up to [`KEPT`] bytes.
    beginning: Vec<u8>,
    /// How many bytes went past that and were dropped.
    dropped: usize,
}

/// A confined process's standard error, drained and bounded.
pub struct Muttered {
    /// What has been kept so far.
    kept: Arc<Mutex<Kept>>,
    /// Whether the drain should stop at its next look.
    done: Arc<AtomicBool>,
}

impl Muttered {
    /// Starts draining `output` and keeps the beginning of what it says.
    ///
    /// The thread lives until the stream ends, until it fails, or until this
    /// value is dropped, whichever comes first.
    #[must_use]
    pub fn draining<O: SandboxOutput + 'static>(output: O) -> Self {
        Self::with_pause(output, PAUSE)
    }

    /// The same, with the pause between polls chosen rather than inherited.
    pub(crate) fn with_pause<O: SandboxOutput + 'static>(mut output: O, pause: Duration) -> Self {
        let kept = Arc::new(Mutex::new(Kept::default()));
        let done = Arc::new(AtomicBool::new(false));
        let writing = Arc::clone(&kept);
        let stopping = Arc::clone(&done);
        thread::spawn(move || {
            let mut buffer = [0_u8; CHUNK];
            while !stopping.load(Ordering::Relaxed) {
                match output.read_ready(&mut buffer) {
                    // Nothing yet, and nothing to wait for on this stream in
                    // particular: it is drained so that it cannot fill, not
                    // because anybody is expecting a word on it.
                    Ok(SandboxRead::Bytes(0) | SandboxRead::Pending) => thread::sleep(pause),
                    Ok(SandboxRead::Bytes(count)) => keep(&writing, buffer.get(..count)),
                    // Bytes the sandbox itself dropped are dropped bytes here
                    // too, and the count is the whole of what they mean.
                    Ok(SandboxRead::Limited {
                        retained,
                        discarded,
                    }) => {
                        keep(&writing, buffer.get(..retained));
                        if let Ok(mut held) = writing.lock() {
                            held.dropped = held.dropped.saturating_add(discarded);
                        }
                    }
                    // An ending or a broken stream is the same instruction:
                    // there is nothing further to read and nobody to tell.
                    Ok(SandboxRead::End) | Err(_) => break,
                }
            }
        });
        Self { kept, done }
    }

    /// What the extension said, as text, saying so where it was cut short.
    ///
    /// Lossy, because standard error is whatever the program wrote and a
    /// diagnostic that cannot be shown because of one bad byte is worse than a
    /// replacement character.
    #[must_use]
    pub fn text(&self) -> String {
        let Ok(held) = self.kept.lock() else {
            return String::new();
        };
        let mut said = String::from_utf8_lossy(&held.beginning).into_owned();
        if held.dropped > 0 {
            // Writing into a string it owns cannot fail. The result is here
            // only because the same trait covers writers that can.
            let _footnote = write!(
                said,
                "\n[{} further bytes were dropped: crucible keeps the first \
                 {KEPT} bytes an extension writes to standard error]",
                held.dropped
            );
        }
        said
    }
}

impl fmt::Debug for Muttered {
    /// By size, because the content is a program's diagnostics and belongs
    /// wherever the caller decided to put it rather than in every other log
    /// line that happens to include this value.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kept, dropped) = self
            .kept
            .lock()
            .map_or((0, 0), |held| (held.beginning.len(), held.dropped));
        f.debug_struct("Muttered")
            .field("kept", &kept)
            .field("dropped", &dropped)
            .finish_non_exhaustive()
    }
}

impl Drop for Muttered {
    /// Tells the drain to stop, without waiting for it to notice.
    ///
    /// Joining would mean waiting out one pause on every extension that ends,
    /// and the thread holds nothing anybody else is about to want: the stream
    /// closes when it lets go of it, which is the only thing left to happen.
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
    }
}

/// Keeps as much of `said` as the bound still allows, counting the rest.
fn keep(kept: &Mutex<Kept>, said: Option<&[u8]>) {
    let (Some(said), Ok(mut held)) = (said, kept.lock()) else {
        return;
    };
    let room = KEPT.saturating_sub(held.beginning.len());
    let taken = room.min(said.len());
    if let Some(bytes) = said.get(..taken) {
        held.beginning.extend_from_slice(bytes);
    }
    held.dropped = held
        .dropped
        .saturating_add(said.len().saturating_sub(taken));
}

#[cfg(test)]
mod tests;
