//! Where the pointer was clicked, for a terminal that was asked to say.
//!
//! A terminal does not report the mouse unless it is asked to. Asking is two
//! escape sequences: one that turns button reporting on, and one that asks for
//! the answers in the form that survives a window wider than 223 columns.
//!
//! Reporting is state on the terminal in the same way [`Raw`] mode is, so it is
//! held by a guard and handed back on the way out — including the way out
//! through a `?` or a panic. Nothing may outlive the process: a terminal still
//! reporting clicks after crucible has gone sends escape bytes into whatever
//! runs next.
//!
//! It has a price, and the price is why nothing here is on unless it was asked
//! for. A terminal that is forwarding buttons is not using them itself, and the
//! wheel is a button: `output.mouse` left alone means the wheel goes on
//! scrolling the terminal's own scrollback, which is where this program's
//! transcript lives. Turned on, clicking places the cursor and the wheel stops
//! scrolling — an inline renderer cannot offer both, because the transcript
//! above the prompt is the terminal's rather than crucible's.
//!
//! The binary holds one for exactly as long as a prompt is being read, and not
//! for the session that prompt belongs to. The box under a running turn takes
//! typing but reads no click, so a guard held across a turn would give the
//! wheel up through the longest stretch of a session and buy nothing with it —
//! and a turn is when somebody most wants to scroll back over what has just
//! been written. Two sequences per prompt is what that costs, written where no
//! frame is being assembled.
//!
//! [`Raw`]: super::raw::Raw

use std::fmt;
use std::io::{self, IsTerminal, Write as _};

use super::raw::RawError;

/// Report a button going down and coming up, and report it in the form that
/// carries a column past 223.
const REPORTING: &str = "\x1b[?1000h\x1b[?1006h";

/// The same two, off, innermost first.
const QUIET: &str = "\x1b[?1006l\x1b[?1000l";

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

        write(REPORTING)?;

        Ok(Some(Self {
            quiet: || write(QUIET),
        }))
    }
}

impl Drop for Reporting {
    fn drop(&mut self) {
        // Best effort and deliberately silent, the same as every other guard
        // here: the terminal is being handed back on the way out, and what
        // would report a failure is what is being given up.
        let _ = (self.quiet)();
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

    /// A guard holding nothing but the counter above.
    fn held() -> Reporting {
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
    fn the_sequences_turn_the_same_two_modes_on_and_off() {
        // Off in the reverse order they went on, and neither list longer than
        // the other: a mode left on outlives this process.
        assert!(REPORTING.contains("?1000h") && REPORTING.contains("?1006h"));
        assert!(QUIET.contains("?1000l") && QUIET.contains("?1006l"));
    }

    #[test]
    fn a_guard_is_debug_without_showing_a_function_address() {
        assert_eq!(format!("{:?}", held()), "Reporting { held: true }");
    }
}
