//! The terminal tab title.
//!
//! A terminal shows whatever the program last set with an OSC sequence, and it
//! keeps showing it after the program exits. Setting a title is therefore a
//! borrow of the terminal, not a write to it, so [`Title`] hands it back when
//! it is dropped.

use std::fmt;
use std::io::{self, IsTerminal, Write};

/// The title crucible sets while it runs: the mark, then the name.
///
/// `▽` is the crucible silhouette in one character. It reads as this program
/// rather than as a generic shell, and being a geometric-shapes glyph rather
/// than an emoji it stays single-width in a monospace font and renders in a
/// bare TTY.
pub const TITLE: &str = "▽ crucible";

/// Sets the icon name and the window title together — a terminal tab shows
/// that pair, so both have to move. Terminated with BEL rather than ST because
/// every terminal accepts BEL and some accept nothing else.
const SET: &str = "\x1b]0;▽ crucible\x07";

/// Empties both, which returns the tab to whatever the terminal names it by
/// default. There is no sequence that asks for the previous title back.
const CLEAR: &str = "\x1b]0;\x07";

/// What can go wrong while setting the terminal title.
#[derive(Debug, thiserror::Error)]
pub enum TitleError {
    /// The terminal could not be written to.
    #[error("could not set the terminal title")]
    Write(#[from] io::Error),
}

/// Holds the terminal's title for as long as this value is alive.
///
/// Dropping it restores the terminal, including on an early return, so no exit
/// path has to remember to.
pub struct Title<W: Write> {
    out: W,
}

impl Title<io::Stdout> {
    /// Sets the terminal title to [`TITLE`], and returns what holds it.
    ///
    /// `None` when standard output is not a terminal. A pipe or a file has no
    /// tab to name, so the sequence would not be a title there — it would be
    /// bytes in the middle of whatever read the output, twice over, because the
    /// guard writes again on its way out. This is the only way to set the
    /// title, so a redirected run cannot be given one by mistake.
    ///
    /// # Errors
    ///
    /// Returns [`TitleError::Write`] if the sequence cannot be written to the
    /// terminal.
    pub fn set() -> Result<Option<Self>, TitleError> {
        if !io::stdout().is_terminal() {
            return Ok(None);
        }

        Ok(Some(Self::onto(io::stdout())?))
    }
}

impl<W: Write> Title<W> {
    /// The same, onto a writer a test can read back.
    fn onto(out: W) -> Result<Self, TitleError> {
        let mut held = Self { out };
        held.out.write_all(SET.as_bytes())?;
        held.out.flush()?;
        Ok(held)
    }
}

impl<W: Write> Drop for Title<W> {
    fn drop(&mut self) {
        // Best effort: the terminal is being handed back on the way out, and a
        // write that fails here has nowhere left to be reported.
        let _ = self.out.write_all(CLEAR.as_bytes());
        let _ = self.out.flush();
    }
}

/// Written by hand rather than derived: the generic writer is an implementation
/// detail and rarely implements `Debug` itself.
impl<W: Write> fmt::Debug for Title<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Title").field("title", &TITLE).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{CLEAR, SET, TITLE, Title};

    #[test]
    fn set_writes_the_mark_and_the_name_to_the_terminal() {
        let mut term = Vec::new();
        let held = Title::onto(&mut term).unwrap();
        drop(held);

        assert!(
            term.starts_with(SET.as_bytes()),
            "expected the title sequence, wrote {:?}",
            String::from_utf8_lossy(&term),
        );
    }

    #[test]
    fn dropping_restores_the_terminal() {
        let mut term = Vec::new();
        let held = Title::onto(&mut term).unwrap();
        drop(held);

        assert!(
            term.ends_with(CLEAR.as_bytes()),
            "expected the title to be cleared, wrote {:?}",
            String::from_utf8_lossy(&term),
        );
    }

    #[test]
    fn a_run_whose_output_is_not_a_terminal_sets_no_title() {
        // The test harness captures standard output, so this is the redirected
        // case: a pipe, a file, `crucible | tee`. Nothing there displays a tab
        // name, so the sequence is not a title but seventeen bytes in the
        // middle of somebody's data — followed by five more when the guard is
        // dropped.
        //
        // Holding nothing is the whole assertion. There is no title to hand
        // back either, which is what `None` says.
        let held = Title::set().expect("no title to be set");

        assert!(held.is_none(), "a pipe was sent a title sequence");
    }

    /// The sequence spells its title out because `concat!` takes literals only.
    /// This is the check that keeps the two from drifting apart.
    #[test]
    fn the_sequence_carries_the_title_verbatim() {
        assert_eq!(SET, format!("\x1b]0;{TITLE}\x07"));
    }
}
