//! Where the pointer was clicked, for a terminal that was asked to say.
//!
//! A terminal does not report the mouse unless it is asked to. Asking is four
//! escape sequences: one that turns button reporting on, one that adds motion
//! under a held button to it, one that adds motion under no button at all, and
//! one that asks for the answers in the form that survives a window wider than
//! 223 columns.
//!
//! Reporting is state on the terminal in the same way [`Raw`] mode is, so it is
//! held by a guard and handed back on the way out — including the way out
//! through a `?` or a panic. Nothing may outlive the process: a terminal still
//! reporting clicks after crucible has gone sends escape bytes into whatever
//! runs next.
//!
//! It has a price, and the price is a drag that no longer selects: a terminal
//! forwarding buttons is not using them itself. That was once reason enough to
//! leave the pointer alone except where something asked for it, back when the
//! wheel a reader turned was moving a scrollback this process did not own.
//!
//! It is not any more. The transcript is crucible's, and the wheel is the way
//! anybody reaches the part of it that is off screen, so the pointer is held for
//! as long as a session is drawn. The selection comes back the same way the
//! scrolling did — crucible owns the screen, so crucible answers the drag, which
//! is what the motion sequences here ask to hear about. Shift is still the way
//! past a program holding the pointer, and stays the answer for a reader who
//! wants their emulator's own selection instead of this one.
//!
//! Motion under *no* button is asked for as well, and it is the dearest of the
//! four: a terminal doing all-motion sends an event for every cell the pointer
//! crosses. What buys it is the one thing on screen whose picture is a fact
//! about the pointer — a result the transcript cut short rests in the quiet and
//! takes the reader's own foreground while the pointer is over one, so that
//! everything on screen with more behind it says so at the same moment. The
//! price is paid where the events are read rather than here: one that lands on
//! the row the last one did is dropped without a frame, and a frame that
//! changes no row writes nothing.
//!
//! Holders nest, so they are counted. A session holds one for its whole length
//! and something standing inside it may hold another, and the end of the inner
//! one is not the end of the outer — the count is the only thing that knows the
//! difference. It is per thread because writing to the terminal is one thread's
//! job here, and a count two threads could race is a count that turns reporting
//! off under the holder still holding it.
//!
//! [`Raw`]: super::raw::Raw

use std::cell::Cell;
use std::fmt;
use std::io::{self, IsTerminal, Write as _};

use super::raw::RawError;

/// Report a button going down and coming up, report the pointer moving whether
/// or not one is held, and report all of it in the form that carries a column
/// past 223.
///
/// Motion under a held button is what a selection is made of: without it a drag
/// arrives as a press and a release with the whole of the reader's gesture
/// missing from between them.
///
/// Motion under no button is what tells the screen where the pointer is at all,
/// and the module doc above says what that is spent on.
const REPORTING: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h";

/// The same four, off, innermost first.
const QUIET: &str = "\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

thread_local! {
    /// How many holders are alive, so that the inner one of two does not hand
    /// the pointer back under the outer one.
    ///
    /// Zero is the terminal's own mouse, which is what a session starts and
    /// ends with.
    static HELD: Cell<usize> = const { Cell::new(0) };
}

/// Holds the terminal reporting clicks for as long as this value is alive.
pub struct Reporting {
    /// What stops the reporting, called once, by [`Drop`].
    ///
    /// A function pointer for the reason [`Raw`](super::raw::Raw) holds one: a
    /// test can watch the guard keep its promise without a terminal to keep it
    /// to.
    quiet: fn() -> io::Result<()>,
}

impl Reporting {
    /// Asks the terminal to report clicks, and returns what holds it to that.
    ///
    /// `None` unless the session is a terminal at both ends, which is the same
    /// condition raw mode is entered under and for the same reason: a sequence
    /// written into a file is bytes in somebody's output rather than state on a
    /// terminal, and there would be nothing to report clicks about.
    ///
    /// # Errors
    ///
    /// [`RawError::Enter`] if the sequence could not be written.
    pub fn on() -> Result<Option<Self>, RawError> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Ok(None);
        }

        // Counted before the sequence goes out, so a write that fails has a
        // count to put back — which is what keeps the number and the terminal
        // saying the same thing. Only the first holder asks; every one after it
        // is nested inside a terminal that is already reporting.
        let first = HELD.with(|held| {
            held.set(held.get() + 1);
            held.get() == 1
        });
        if first && let Err(trouble) = write(REPORTING) {
            HELD.with(|held| held.set(held.get() - 1));
            return Err(trouble.into());
        }

        Ok(Some(Self {
            quiet: || write(QUIET),
        }))
    }
}

impl Drop for Reporting {
    fn drop(&mut self) {
        // Only the last one hands it back. The others are nested inside a
        // holder that is still holding it, and a sequence written for one of
        // those would take the pointer away from whoever asked first.
        let last = HELD.with(|held| {
            let left = held.get().saturating_sub(1);
            held.set(left);
            left == 0
        });
        if last {
            // Best effort and deliberately silent, the same as every other
            // guard here: the terminal is being handed back on the way out, and
            // what would report a failure is what is being given up.
            let _ = (self.quiet)();
        }
    }
}

/// Written by hand rather than derived: a function pointer's `Debug` is an
/// address, which says nothing about what the guard is holding.
impl fmt::Debug for Reporting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reporting").field("held", &true).finish()
    }
}

/// One sequence, straight out and flushed.
///
/// Not through the renderer: this is terminal state rather than a frame, and it
/// is written when no frame is being assembled. The bytes cost no column and
/// nothing will ever be drawn back over them.
fn write(sequence: &str) -> io::Result<()> {
    let mut out = io::stdout();
    out.write_all(sequence.as_bytes())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    thread_local! {
        /// How many times a guard has handed the terminal back on this thread.
        static QUIETED: Cell<usize> = const { Cell::new(0) };
    }

    fn counted() -> io::Result<()> {
        QUIETED.with(|count| count.set(count.get() + 1));

        // Not `Ok(())`: the failure a guard has to survive is the one it cannot
        // report, so the fake answers the way the real one does when the
        // terminal has already gone.
        Err(io::Error::other("the terminal went away"))
    }

    /// A guard holding nothing but the counter above, counted the way a real
    /// one is so that dropping it means what dropping a real one means.
    fn held() -> Reporting {
        HELD.with(|held| held.set(held.get() + 1));
        Reporting { quiet: counted }
    }

    /// A function that fails, for dropping a guard on the way out of.
    fn early(_held: Reporting) -> Result<(), RawError> {
        Err(RawError::Enter(io::Error::other("went wrong")))
    }

    #[test]
    fn a_guard_stops_the_reporting_when_it_is_dropped() {
        QUIETED.with(|count| count.set(0));

        drop(held());

        assert_eq!(QUIETED.with(Cell::get), 1);
    }

    #[test]
    fn a_guard_dropped_by_an_early_return_still_stops_it() {
        // The path that matters: a `?` on the way out of the session leaves the
        // terminal reporting clicks into whatever runs next.
        QUIETED.with(|count| count.set(0));

        let _ = early(held());

        assert_eq!(QUIETED.with(Cell::get), 1);
    }

    #[test]
    fn a_guard_the_stack_unwound_past_still_stops_it() {
        // The other path the module doc claims. A terminal still forwarding
        // buttons after crucible has gone sends escape bytes into whatever runs
        // next, so the panic has to leave through the same door the `?` does.
        //
        // What makes that true of the shipped binary rather than only of this
        // one is the `compile_error!` in `raw.rs`: a test binary unwinds
        // whatever the release profile says.
        QUIETED.with(|count| count.set(0));

        let unwound = std::panic::catch_unwind(|| {
            let _pointer = held();
            panic!("the frame was drawn against a terminal that had gone");
        });

        assert!(unwound.is_err(), "nothing unwound");
        assert_eq!(QUIETED.with(Cell::get), 1);
    }

    #[test]
    fn the_sequences_turn_the_same_modes_on_and_off() {
        // Off in the reverse order they went on, and neither list longer than
        // the other: a mode left on outlives this process.
        for mode in ["?1000", "?1002", "?1003", "?1006"] {
            assert!(
                REPORTING.contains(&format!("{mode}h")),
                "{mode} never went on"
            );
            assert!(QUIET.contains(&format!("{mode}l")), "{mode} was left on");
        }
    }

    #[test]
    fn a_second_holder_does_not_hand_the_pointer_back_under_the_first() {
        // The nesting the module doc claims: something stands while the session
        // that took the pointer is still running, and the end of the one is not
        // the end of the other. Without the count the reader opens one list,
        // closes it, and finds the wheel dead for the rest of the session.
        QUIETED.with(|count| count.set(0));

        let outer = held();
        drop(held());
        assert_eq!(QUIETED.with(Cell::get), 0, "the inner holder gave it back");

        drop(outer);
        assert_eq!(QUIETED.with(Cell::get), 1, "the outer holder kept it");
    }

    #[test]
    fn a_guard_is_debug_without_showing_a_function_address() {
        assert_eq!(format!("{:?}", held()), "Reporting { held: true }");
    }
}
