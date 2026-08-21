//! The screen crucible owns while a session runs.
//!
//! Taking the alternate screen is a borrow in the sense [`Raw`](super::raw::Raw)
//! and [`Title`](crate::Title) already use here: the terminal keeps what it was
//! last told, so what enters has to be handed back by a `Drop` rather than by an
//! exit path that remembers. Left behind, the reader's shell comes back to a
//! screen that is not theirs and a scrollback they cannot reach.
//!
//! What it buys is that every cell on it is this process's. There is no
//! scrollback underneath to be careful of and no cursor position to work
//! backwards from: a row is addressed by its number, and a frame writes the rows
//! whose painted text is not what is already there.
//!
//! What it costs is the job the terminal used to do. Scrollback above the
//! viewport is crucible's now, which is why [`crate::record`] is bounded and why
//! a session that ends says where the rest of it went.

use std::fmt;
use std::io::{self, IsTerminal, Write as _};

/// Take the alternate screen, and put the cursor at the top of it.
const ENTER: &str = "\x1b[?1049h\x1b[H";

/// Give it back.
///
/// The cursor is shown first, in case a frame was interrupted between hiding it
/// and putting it back — the sequence that hid it was written to a screen that
/// is about to stop existing, and the terminal would keep the state anyway.
const LEAVE: &str = "\x1b[?25h\x1b[?1049l";

/// What can go wrong taking the screen.
#[derive(Debug, thiserror::Error)]
pub enum ScreenError {
    /// The sequence that takes the screen could not be written.
    #[error("could not take the screen: {0}")]
    Take(#[from] io::Error),
}

/// Holds the alternate screen for as long as this value is alive.
///
/// Dropping it hands the terminal back, including on an early return and on a
/// panic — every panic, because a build that would not unwind one is refused in
/// [`raw`](super::raw). What it does not cover is a process killed outright,
/// where no code of this program's runs at all.
pub struct Screen {
    /// What gives the screen back, called once, by [`Drop`].
    ///
    /// A function pointer for the reason every other guard here holds one: a
    /// test can watch the guard keep its promise without a terminal to keep it
    /// to. Taking the screen for real reaches the controlling terminal, which
    /// under a test harness is the one the tests are being run in.
    leave: fn() -> io::Result<()>,
}

impl Screen {
    /// Takes the alternate screen, and returns what holds it.
    ///
    /// `None` unless the session is a terminal at both ends, which is the
    /// condition every guard here enters under: a sequence written into a file
    /// is bytes in somebody's output rather than state on a terminal.
    ///
    /// # Errors
    ///
    /// [`ScreenError::Take`] if the sequence could not be written.
    pub fn take() -> Result<Option<Self>, ScreenError> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Ok(None);
        }

        write(ENTER)?;

        Ok(Some(Self {
            leave: || write(LEAVE),
        }))
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // Best effort, and deliberately silent, the same as every other guard
        // here: what would report a failure is what is being given up.
        let _ = (self.leave)();
    }
}

/// Written by hand rather than derived: a function pointer's `Debug` is an
/// address, which says nothing about what the guard is holding.
impl fmt::Debug for Screen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Screen").field("held", &true).finish()
    }
}

/// Writes `sequence` to the terminal and flushes it.
///
/// Straight to standard output rather than through [`crate::Terminal`], for the
/// same reason the mode guards do: this is state being borrowed on the way in
/// and handed back on the way out, not a frame. A frame goes through the seam so
/// a test can read it; what makes this one testable is the pointer above.
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
        /// How many times a guard has handed the screen back on this thread.
        static LEFT: Cell<usize> = const { Cell::new(0) };
    }

    /// Stands in for the write: counts, and then refuses.
    ///
    /// Refuses rather than succeeds because the half of the promise a return
    /// value cannot carry is the interesting one. By the time this runs there is
    /// no caller left to tell, so the failure is swallowed — and every test
    /// below still finds the guard finished what it was doing.
    fn leave() -> io::Result<()> {
        LEFT.with(|left| left.set(left.get() + 1));
        Err(io::Error::other("the terminal was already gone"))
    }

    /// A guard over [`leave`], with the counter started at nothing.
    fn held() -> Screen {
        LEFT.with(|left| left.set(0));
        Screen { leave }
    }

    #[test]
    fn dropping_hands_the_screen_back() {
        let screen = held();
        LEFT.with(|left| assert_eq!(left.get(), 0, "left before it was dropped"));

        drop(screen);

        LEFT.with(|left| assert_eq!(left.get(), 1));
    }

    #[test]
    fn a_guard_that_leaves_by_the_question_mark_hands_it_back_too() {
        // The path a normal return does not cover, and the one a hand-written
        // restore forgets: the screen is taken, something fails, and the
        // function ends three frames from here.
        fn fails(_screen: &Screen) -> Result<(), ScreenError> {
            Err(ScreenError::Take(io::Error::other("the provider said no")))
        }

        fn run() -> Result<(), ScreenError> {
            let screen = held();
            fails(&screen)?;
            Ok(())
        }

        assert!(run().is_err());
        LEFT.with(|left| assert_eq!(left.get(), 1));
    }

    #[test]
    fn a_guard_the_stack_unwound_past_hands_it_back_too() {
        // The path that matters most here: a panic on the alternate screen
        // leaves the reader looking at a screen that is not theirs, with the
        // message that would explain it written where they cannot reach it.
        //
        // The panic message on the way past is the harness reporting a panic
        // that was caught on purpose, not a failure.
        let unwound = std::panic::catch_unwind(|| {
            let _screen = held();
            panic!("the reader was left on a screen that was not theirs");
        });

        assert!(unwound.is_err(), "nothing unwound");
        LEFT.with(|left| assert_eq!(left.get(), 1));
    }

    #[test]
    fn a_run_that_is_not_a_terminal_at_both_ends_takes_no_screen() {
        // The test harness captures standard output, so this is the redirected
        // case: a pipe, a file, `crucible | tee`. Holding nothing is the whole
        // assertion — and it is what keeps every other test in this workspace
        // from taking the screen out from under the run they are part of.
        let held = Screen::take().expect("no screen to take");

        assert!(held.is_none(), "a pipe was sent the alternate screen");
    }

    #[test]
    fn the_guard_says_what_it_is_holding_without_saying_where() {
        // It ends up in the `Debug` of everything that holds it, and an address
        // there is noise that changes every run.
        let screen = held();

        assert_eq!(format!("{screen:?}"), "Screen { held: true }");
    }

    #[test]
    fn what_is_given_back_shows_the_cursor_before_the_screen_goes() {
        // Order, not contents: a frame interrupted between hiding the cursor and
        // putting it back leaves the hide in effect, and the shell underneath
        // inherits it. Showing it after the screen has gone would set it on a
        // screen nobody is looking at.
        let shown = LEAVE.find("\x1b[?25h").expect("the cursor is shown");
        let gone = LEAVE.find("\x1b[?1049l").expect("the screen is given back");

        assert!(
            shown < gone,
            "the cursor is put back after the screen: {LEAVE:?}"
        );
    }
}
