//! Emptying the terminal before crucible draws anything.
//!
//! Off unless asked for. Crucible renders inline, so the rows already on screen
//! are somebody's own work and the scrollback above them is theirs to keep —
//! clearing by default would throw away the thing the inline design exists to
//! protect. Somebody who wants a bare screen to start from says so once, in
//! configuration, and this is the whole of what that setting does.
//!
//! It cannot become a frame. [`Renderer`](crate::Renderer) takes a terminal by
//! value, so the only moment a caller holds one to pass here is before the
//! renderer exists — which leaves every rule about what a frame may write,
//! chiefly that it erases downward and never reaches scrollback, exactly as
//! strict as it was.

use crate::terminal::{Terminal, TerminalError};

/// Empty the screen, empty the scrollback, put the cursor home.
///
/// Three sequences because they undo three different things. `2J` clears what
/// is visible and `H` puts the cursor at the top of it, which between them are
/// what most programs mean by clearing — and they leave every row that scrolled
/// off sitting in the buffer, one scroll away. `3J` is what empties that too,
/// and somebody who asked for a clear screen meant the rows above it as well.
pub const CLEARED: &str = "\x1b[2J\x1b[3J\x1b[H";

/// Empties the terminal, if there is one.
///
/// Does nothing when output is redirected. A pipe has no screen to clear, so
/// the sequence would clear nothing there — it would be escape bytes at the top
/// of somebody's file.
///
/// # Errors
///
/// [`TerminalError::Io`] if the sequence cannot be written.
pub fn clear<T: Terminal>(terminal: &mut T) -> Result<(), TerminalError> {
    if !terminal.is_terminal() {
        return Ok(());
    }

    terminal.write(CLEARED)?;
    terminal.flush()
}

#[cfg(test)]
mod tests {
    use crate::terminal::Recording;

    use super::*;

    #[test]
    fn a_clear_empties_the_scrollback_a_frame_may_never_touch() {
        // The one write in this crate that is allowed to reach above the
        // screen, and the reason it is allowed is that it was asked for by
        // name. `3J` is the half of that which a caller would not get from
        // any clear they could write themselves out of habit.
        let mut terminal = Recording::new(80, 24);

        clear(&mut terminal).expect("a recording terminal cannot fail");

        assert_eq!(terminal.written(), "\x1b[2J\x1b[3J\x1b[H");
    }

    #[test]
    fn a_redirected_run_is_not_cleared() {
        let mut terminal = Recording::redirected(80, 24);

        clear(&mut terminal).expect("a recording terminal cannot fail");

        assert_eq!(terminal.written(), "");
        assert_eq!(terminal.flushes(), 0);
    }

    #[test]
    fn the_screen_is_on_the_terminal_before_the_call_returns() {
        // Not queued behind the first frame. Whatever a frame does afterwards
        // starts from a screen that is already empty, so the two never appear
        // in the wrong order.
        let mut terminal = Recording::new(80, 24);

        clear(&mut terminal).expect("a recording terminal cannot fail");

        assert_eq!(terminal.flushes(), 1);
    }
}
