//! Keys as they are pressed, rather than lines as they are finished.
//!
//! A terminal in its usual mode collects a line, lets the user edit it with
//! whatever the driver implements, and hands it over at Enter. That is a fine
//! deal for a program that reads lines, and the wrong one for a program that
//! draws a prompt: nothing can be shown about a keystroke that has not arrived
//! yet, and the driver's editing draws over rows this process is responsible
//! for.
//!
//! Raw mode takes the line discipline off, and with it the echo, the erase and
//! kill processing, and the signals the terminal sends on Ctrl-C and Ctrl-Z.
//! Everything that used to happen for free is this program's job afterwards —
//! which is the point, since drawing it is the job.
//!
//! Mode is state on the terminal, not a write to it, so it outlives the process
//! that set it. A shell left in raw mode shows nothing as the user types and
//! takes no interrupt: the terminal looks broken, and the program that broke it
//! has already exited. [`Raw`] is therefore a guard, in the shape
//! [`crate::Title`] already uses here — entering is the only way to get one, and
//! dropping it is the only way to leave.

use std::fmt;
use std::io::{self, IsTerminal};

// Every guard here — this one, `Reporting`, `Title` — hands the terminal back
// through `Drop`, and the only thing that runs a `Drop` on the way out of a
// panic is unwinding. Built to abort, none of them runs: the shell that gets the
// terminal back echoes nothing, reports clicks and wears somebody else's tab
// name, and `reset` is the remedy.
//
// So this is a build that must not exist rather than one that is merely worse,
// and the check is here because no test can make it. A test binary unwinds
// whatever the release profile says, which is exactly how a guard can be proved
// to keep its promise under `cargo test` and break it in the shipped artifact.
#[cfg(panic = "abort")]
compile_error!(
    "the terminal guards hand the terminal back on unwind, so this crate cannot be built with \
     panic = \"abort\""
);

/// What can go wrong entering raw mode.
#[derive(Debug, thiserror::Error)]
pub enum RawError {
    /// The terminal would not change mode.
    #[error("could not read keys as they are pressed: {0}")]
    Enter(#[from] io::Error),
}

/// Holds the terminal in raw mode for as long as this value is alive.
///
/// Dropping it hands the terminal back, including on an early return, so no
/// exit path has to remember to.
///
/// What that covers is every path this process decides: a return, a `?`, a
/// panic — every panic, because a build that would not unwind one is refused
/// above. Ctrl-C is inside it rather than outside — raw mode is what stops the
/// terminal turning that key into a signal, so it arrives as a byte for the
/// reader to act on and the guard is dropped on the way out like any other.
/// What it does not cover is a process killed outright, where no
/// code of this program's runs at all; `reset` is the terminal's own answer to
/// that, and no program can do better from inside.
pub struct Raw {
    /// What hands the terminal back, called once, by [`Drop`].
    ///
    /// A function pointer rather than a call written into the drop, so a test
    /// can watch the guard keep its promise without a terminal to keep it to.
    /// Entering raw mode for real reaches the controlling terminal, which under
    /// a test harness is the one the tests are being run in.
    leave: fn() -> io::Result<()>,
}

impl Raw {
    /// Puts the terminal into raw mode, and returns what holds it there.
    ///
    /// `None` unless the session is a terminal at both ends. Keys have to come
    /// from one for there to be a mode to change, and frames have to reach one
    /// for the change to be worth making: a run whose output is a file would
    /// take the echo away from somebody who can no longer see what they type,
    /// having redirected the only thing that would have drawn it. This is the
    /// only way to enter, so neither can be done to a pipe by mistake.
    ///
    /// # Errors
    ///
    /// [`RawError::Enter`] if the terminal will not change mode.
    pub fn enter() -> Result<Option<Self>, RawError> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Ok(None);
        }

        crossterm::terminal::enable_raw_mode()?;

        Ok(Some(Self {
            leave: crossterm::terminal::disable_raw_mode,
        }))
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        // Best effort, and deliberately silent: the terminal is being handed
        // back on the way out, and an error here has nowhere left to be
        // reported — the thing that would report it is what is being given up.
        let _ = (self.leave)();
    }
}

/// Written by hand rather than derived: a function pointer's `Debug` is an
/// address, which says nothing about what the guard is holding.
impl fmt::Debug for Raw {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Raw").field("held", &true).finish()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    thread_local! {
        /// How many times a guard has handed the terminal back on this thread.
        static LEFT: Cell<usize> = const { Cell::new(0) };
    }

    /// Stands in for the syscall: counts, and then refuses.
    ///
    /// Refuses rather than succeeds because the half of the promise a return
    /// value cannot carry is the interesting one. By the time this runs there
    /// is no caller left to tell, so the failure is swallowed — and every test
    /// below still finds the guard finished what it was doing.
    fn leave() -> io::Result<()> {
        LEFT.with(|left| left.set(left.get() + 1));
        Err(io::Error::other("the terminal was already gone"))
    }

    /// A guard over [`leave`], with the counter started at nothing.
    fn held() -> Raw {
        LEFT.with(|left| left.set(0));
        Raw { leave }
    }

    #[test]
    fn dropping_hands_the_terminal_back() {
        let raw = held();
        LEFT.with(|left| assert_eq!(left.get(), 0, "left before it was dropped"));

        drop(raw);

        LEFT.with(|left| assert_eq!(left.get(), 1));
    }

    #[test]
    fn a_guard_that_leaves_by_the_question_mark_hands_it_back_too() {
        // The path a normal return does not cover, and the one a hand-written
        // restore forgets: the terminal is raw, something fails, and the
        // function ends three frames from here.
        fn fails(_raw: &Raw) -> Result<(), RawError> {
            Err(RawError::Enter(io::Error::other("the provider said no")))
        }

        fn run() -> Result<(), RawError> {
            let raw = held();
            fails(&raw)?;
            Ok(())
        }

        assert!(run().is_err());
        LEFT.with(|left| assert_eq!(left.get(), 1));
    }

    #[test]
    fn a_guard_the_stack_unwound_past_hands_it_back_too() {
        // The path the module doc claims and nothing here used to reach: the
        // terminal is raw, something panics, and the frames in between are
        // discarded. It proves the `Drop`, and it cannot prove the strategy —
        // this binary unwinds because a test binary always does. What says the
        // shipped one unwinds is the `compile_error!` above, which fails the
        // build rather than a test.
        //
        // The panic message on the way past is the harness reporting a panic
        // that was caught on purpose, not a failure.
        let unwound = std::panic::catch_unwind(|| {
            let _raw = held();
            panic!("the reader was handed a terminal that had gone");
        });

        assert!(unwound.is_err(), "nothing unwound");
        LEFT.with(|left| assert_eq!(left.get(), 1));
    }

    #[test]
    fn a_run_that_is_not_a_terminal_at_both_ends_changes_no_mode() {
        // The test harness captures standard output, so this is the redirected
        // case: a pipe, a file, `crucible | tee`. Holding nothing is the whole
        // assertion — and it is what keeps every other test in this workspace
        // from putting the terminal they are being run in into raw mode.
        let held = Raw::enter().expect("no mode to change");

        assert!(held.is_none(), "a pipe was put into raw mode");
    }

    #[test]
    fn the_guard_says_what_it_is_holding_without_saying_where() {
        // It ends up in the `Debug` of everything that holds it, and an address
        // there is noise that changes every run.
        let raw = held();

        assert_eq!(format!("{raw:?}"), "Raw { held: true }");
    }
}
