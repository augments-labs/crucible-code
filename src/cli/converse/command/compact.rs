//! `/compact`: making room before the window makes it necessary.
//!
//! The same thing a full window does on its own, asked for deliberately. What
//! it spends is one request, and what it buys is the session carrying on in a
//! fraction of the room it was using.
//!
//! It runs between turns, so nothing here has to worry about a tool being out
//! or an answer being half-read. That is the whole difference from the
//! automatic path: no turn is waiting on it, and none carries on afterwards.
//!
//! Nothing is deleted. What is replaced is what the *model* is sent; the
//! session log keeps every message of it, and that is what `--continue` reads.

use crucible_core::{Cancel, Compacting, Event, Post};
use crucible_runner::Runner;
use crucible_tui::{Renderer, Row, Slot, Terminal, clip};

use crate::cli::Fatal;

use super::Terms;

/// What a session with nothing behind it is told.
///
/// Said rather than done quietly: a command that appears to run and changes
/// nothing is one somebody types again.
const NOTHING: &str = "there is nothing behind this turn worth replacing yet";

/// Where the events of a manual compaction go.
///
/// Nowhere. The rows they exist to move belong to a turn — the word above the
/// box, the bar under it — and no turn is running. What happened is drawn here
/// instead, once, from the value the call hands back.
struct Nowhere;

impl Post for Nowhere {
    fn post(&self, _: Event) {}
}

/// Runs it.
pub(super) fn run<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let columns = renderer.columns();
    let style = terms.style();
    let glyphs = style.glyphs();

    // Nothing draws while this runs. It is one request between turns, on the
    // thread that draws, and the row that would say so belongs to a turn.
    // A compaction that failed is not a session that has to end: the transcript
    // is untouched, and what the user asked for simply did not happen. Said as
    // a row, in the words of the failure, and the prompt comes back.
    let compacted = match runner.compact(Compacting::Asked, &Nowhere, &Cancel::new()) {
        Ok(compacted) => compacted,
        Err(problem) => {
            let said = format!("! could not make room: {problem}");
            let row = Row::new().then(Slot::Quiet, clip(&said, columns));
            renderer.present(&[row], style.palette())?;
            return Ok(());
        }
    };

    let Some(compacted) = compacted else {
        let row = Row::new().then(Slot::Quiet, clip(NOTHING, columns));
        renderer.present(&[row], style.palette())?;
        return Ok(());
    };

    let rows = crate::cli::draw::compacted_rows(compacted, columns, glyphs);
    renderer.present(&rows, style.palette())?;

    Ok(())
}
