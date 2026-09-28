//! What the run shows the reader first, and what it starts behind that.
//!
//! One value rather than two parameters to the loop, because the second is
//! decided by the first: the caller cannot know when the card is on the screen,
//! because the loop is what puts it there. A check asked for before that is
//! asked for ahead of the frame the reader is waiting for, and leaves a proxy's
//! copy of the request in front of it.
//!
//! Taken by the loop rather than lent to it, so what is armed behind the frame
//! can only be armed once: `/resume` and `/clear` commit the card again later in
//! the same run, and a check asked on each of those is asked as often as the
//! reader changed their mind.

use crucible_tui::{Renderer, Terminal, TerminalError};

use super::draw::opening::Standing;

/// The opening a session starts with, and what the run wants started once it is
/// on the screen.
pub(crate) struct First<'a> {
    /// The card committed into the transcript before anything is asked of it.
    pub(crate) card: &'a Standing,
    /// Started immediately after that card is drawn, or `None` for a run with
    /// nothing to start behind the frame.
    pub(crate) arming: Option<Box<dyn FnOnce()>>,
}

impl First<'_> {
    /// Puts the card in the transcript and then starts what the run wanted
    /// started behind it.
    ///
    /// The order is the whole of this: the frame is on the screen from here, so
    /// a release check asked from here puts its socket, and a proxy's record of
    /// it, after the opening rather than before it. `/resume` and `/clear` reach
    /// the card by the loop's own commit rather than through here, which is what
    /// keeps one run asking at most once.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to, in which
    /// case nothing behind the frame is started.
    pub(crate) fn drawn<T: Terminal>(
        self,
        renderer: &mut Renderer<T>,
    ) -> Result<(), TerminalError> {
        self.card.commit(renderer)?;
        if let Some(arming) = self.arming {
            arming();
        }

        Ok(())
    }
}
