//! Asking the input stream to carry what the old encoding had no room for.
//!
//! Two modes, both asked for the same way and held the same way, because both
//! are the same shortfall: the encoding a terminal has used since the seventies
//! throws information away, and a terminal will stop throwing it away when it
//! is asked. One is which modifier was held. The other is whether a block of
//! text was pasted or typed.
//!
//! The encoding a terminal has used since the seventies has no room in it for
//! most modifiers. Enter is one byte, and it is the same byte whether or not
//! Shift was held — so an editor can answer Shift+Enter perfectly and never be
//! given the chance, because what arrived was indistinguishable from Return.
//! That is not a missing feature in the editor. It is a missing field on the
//! wire.
//!
//! There is a newer encoding that has the field, and a terminal offers it when
//! it is asked. Asking is one sequence out; a terminal that does not implement
//! it discards the sequence and nothing changes, which is why this is not
//! guarded by a capability check. There is a way to ask a terminal whether it
//! would — and it is a *query*, so [`ground`] applies and this deliberately
//! does not use it. A question has an answer, an answer arrives in the same
//! queue the prompt is about to read keys from, and the whole benefit here is
//! available without asking one.
//!
//! Only the first level is requested. It is the level that separates modified
//! keys from bare ones, and it leaves the bare ones alone: Return still arrives
//! as Return. The levels above it report keys going *up* as well as down, which
//! would double every keystroke this crate reads.
//!
//! The second mode is bracketed paste, and what it saves is a newline. Pasted
//! text arrives as the bytes it is made of, so every line break in it is the
//! byte Return sends — and a prompt that submits on Return submits on the first
//! line of the paste and takes the rest as the next prompt. Asked to bracket,
//! the terminal marks where the paste starts and ends, and the block arrives as
//! one event whose newlines are structure. What the block costs before it can
//! be capped is the crate underneath's to hold, which `Cargo.toml` says beside
//! the feature that turns it on; what is *kept* is bounded by the editor it
//! lands in.
//!
//! Like every other mode here, both are state on the terminal rather than a
//! write to it, so they outlive the process that set them and are held by
//! guards. The shape is [`Reporting`]'s, for the reasons that module gives.
//!
//! [`ground`]: super::ground
//! [`Reporting`]: super::mouse::Reporting

use std::fmt;
use std::io::{self, IsTerminal, Write as _};

use super::raw::RawError;

/// Push the first level: report a key that was pressed with a modifier in a
/// form that says which modifier it was.
const DISTINCT: &str = "\x1b[>1u";

/// Pop whatever was pushed, back to however the terminal was spelling keys
/// before this process arrived.
const AS_FOUND: &str = "\x1b[<u";

/// Holds the terminal spelling modified keys distinctly for as long as this
/// value is alive.
pub struct Spelling {
    /// What hands the spelling back, called once, by [`Drop`].
    ///
    /// A function pointer for the reason every guard here holds one: a test can
    /// watch it keep its promise without a terminal to keep it to.
    restore: fn() -> io::Result<()>,
}

impl Spelling {
    /// Asks the terminal for the distinct spelling, and returns what holds it
    /// to that.
    ///
    /// `None` unless the session is a terminal at both ends, which is the
    /// condition every guard here is entered under and for the same reason: a
    /// sequence written into a file is bytes in somebody's output rather than
    /// state on a terminal.
    ///
    /// # Errors
    ///
    /// [`RawError::Enter`] if the sequence could not be written.
    pub fn distinct() -> Result<Option<Self>, RawError> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Ok(None);
        }

        write(DISTINCT)?;

        Ok(Some(Self {
            restore: || write(AS_FOUND),
        }))
    }
}

impl Drop for Spelling {
    fn drop(&mut self) {
        // Best effort and deliberately silent, the same as every other guard
        // here: what would report a failure is what is being given up.
        let _ = (self.restore)();
    }
}

/// Written by hand rather than derived: a function pointer's `Debug` is an
/// address, which says nothing about what the guard is holding.
impl fmt::Debug for Spelling {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Spelling").field("held", &true).finish()
    }
}

/// Ask the terminal to mark where a paste begins and ends.
const BRACKETED: &str = "\x1b[?2004h";

/// The same, off.
const UNBRACKETED: &str = "\x1b[?2004l";

/// Holds the terminal bracketing pastes for as long as this value is alive.
pub struct Pasting {
    /// What stops the bracketing, called once, by [`Drop`].
    restore: fn() -> io::Result<()>,
}

impl Pasting {
    /// Asks the terminal to bracket pastes, and returns what holds it to that.
    ///
    /// `None` unless the session is a terminal at both ends, for the reason
    /// [`Spelling::distinct`] gives.
    ///
    /// # Errors
    ///
    /// [`RawError::Enter`] if the sequence could not be written.
    pub fn bracketed() -> Result<Option<Self>, RawError> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Ok(None);
        }

        write(BRACKETED)?;

        Ok(Some(Self {
            restore: || write(UNBRACKETED),
        }))
    }
}

impl Drop for Pasting {
    fn drop(&mut self) {
        // Best effort and deliberately silent, the same as every other guard
        // here. A terminal left bracketing sends `[200~` into whatever runs
        // next, which a shell reads as text somebody typed.
        let _ = (self.restore)();
    }
}

/// Written by hand rather than derived, for the reason above.
impl fmt::Debug for Pasting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pasting").field("held", &true).finish()
    }
}

/// One sequence, straight out and flushed.
///
/// Not through the renderer, for the reason the guard next door gives: this is
/// terminal state rather than a frame, and it is written when no frame is being
/// assembled.
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
        /// How many times a guard has handed the spelling back on this thread.
        static RESTORED: Cell<usize> = const { Cell::new(0) };
    }

    /// Stands in for the write: counts, and then refuses.
    ///
    /// Refuses because the half of the promise a return value cannot carry is
    /// the interesting one — by the time this runs there is no caller left to
    /// tell, and every test below still finds the guard finished.
    fn counted() -> io::Result<()> {
        RESTORED.with(|count| count.set(count.get() + 1));
        Err(io::Error::other("the terminal went away"))
    }

    /// A guard holding nothing but the counter above.
    fn held() -> Spelling {
        RESTORED.with(|count| count.set(0));
        Spelling { restore: counted }
    }

    /// A function that fails, for dropping a guard on the way out of.
    fn early(_held: Spelling) -> Result<(), RawError> {
        Err(RawError::Enter(io::Error::other("went wrong")))
    }

    #[test]
    fn dropping_hands_the_spelling_back() {
        let spelling = held();
        RESTORED.with(|count| assert_eq!(count.get(), 0, "restored before it was dropped"));

        drop(spelling);

        assert_eq!(RESTORED.with(Cell::get), 1);
    }

    #[test]
    fn a_guard_dropped_by_an_early_return_still_hands_it_back() {
        let _ = early(held());

        assert_eq!(RESTORED.with(Cell::get), 1);
    }

    #[test]
    fn a_guard_the_stack_unwound_past_still_hands_it_back() {
        // A terminal left spelling keys the new way after crucible has gone
        // sends `CSI u` sequences into a shell that reads them as text.
        let unwound = std::panic::catch_unwind(|| {
            let _spelling = held();
            panic!("the reader was handed a terminal that had gone");
        });

        assert!(unwound.is_err(), "nothing unwound");
        assert_eq!(RESTORED.with(Cell::get), 1);
    }

    #[test]
    fn the_sequences_push_one_level_and_pop_whatever_was_pushed() {
        // The level matters: the ones above it report keys going up as well as
        // down, and this crate reads every key exactly once.
        assert_eq!(DISTINCT, "\x1b[>1u");
        assert_eq!(AS_FOUND, "\x1b[<u");
    }

    #[test]
    fn a_run_that_is_not_a_terminal_at_both_ends_asks_for_nothing() {
        // The test harness captures standard output, so this is the redirected
        // case — and it is what keeps every other test in this workspace from
        // writing escape bytes into the harness's capture.
        let held = Spelling::distinct().expect("nothing to ask");

        assert!(held.is_none(), "a pipe was asked to respell its keys");
    }

    #[test]
    fn the_guard_says_what_it_is_holding_without_saying_where() {
        assert_eq!(format!("{:?}", held()), "Spelling { held: true }");
    }

    /// A paste guard over the same counter.
    fn bracketing() -> Pasting {
        RESTORED.with(|count| count.set(0));
        Pasting { restore: counted }
    }

    #[test]
    fn dropping_stops_the_bracketing() {
        drop(bracketing());

        assert_eq!(RESTORED.with(Cell::get), 1);
    }

    #[test]
    fn a_paste_guard_the_stack_unwound_past_still_stops_it() {
        let unwound = std::panic::catch_unwind(|| {
            let _pasting = bracketing();
            panic!("the reader was handed a terminal that had gone");
        });

        assert!(unwound.is_err(), "nothing unwound");
        assert_eq!(RESTORED.with(Cell::get), 1);
    }

    #[test]
    fn the_paste_sequences_turn_the_same_mode_on_and_off() {
        assert_eq!(BRACKETED, "\x1b[?2004h");
        assert_eq!(UNBRACKETED, "\x1b[?2004l");
    }

    #[test]
    fn a_run_that_is_not_a_terminal_at_both_ends_asks_for_no_bracketing() {
        let held = Pasting::bracketed().expect("nothing to ask");

        assert!(held.is_none(), "a pipe was asked to bracket its pastes");
    }

    #[test]
    fn the_paste_guard_says_what_it_is_holding_without_saying_where() {
        assert_eq!(format!("{:?}", bracketing()), "Pasting { held: true }");
    }
}
