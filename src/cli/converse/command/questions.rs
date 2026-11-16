//! `/questions`: the dock of questions the model asked without waiting,
//! opened with the keys, closed with every draft kept, or a line saying
//! nothing waits.
//!
//! The same between turns and while one runs: the dock is drawn in the band
//! over the box either way, so opening it stands no panel and stops nothing.

use crucible_tui::{Renderer, Terminal};

use crate::cli::Fatal;
use crate::cli::converse::docking::{Docking, Toggled};

use super::say;

/// What is said where nothing waits.
pub(in crate::cli::converse) const NOTHING: &str = "no questions are waiting";

/// Runs it between turns: the dock opens or closes above the box, and where
/// nothing waits that is said under the line that asked.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on.
pub(super) fn run<T: Terminal>(
    renderer: &mut Renderer<T>,
    docking: &mut Docking,
) -> Result<(), Fatal> {
    match docking.toggled() {
        Toggled::Empty => say(renderer, NOTHING),
        Toggled::Opened | Toggled::Closed => Ok(()),
    }
}
