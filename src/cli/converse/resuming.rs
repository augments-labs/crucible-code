//! What a session picked up is worth carrying whole.
//!
//! A session that ran for hours is worth what it cost to build, and sending all
//! of it back is what that costs again — every turn of it, on the next request
//! and on every request after. Most of the time what the model needs from it is
//! the notes rather than the transcript, and the difference is the whole of
//! somebody's usage limit.
//!
//! So a large one is put to the reader before it is carried: the age, the size,
//! and three answers. Nothing is decided for them, which is the point — the one
//! case where carrying it whole is right is the one crucible cannot see from
//! here, and it is the reader who knows they are about to ask about something
//! said two hours ago.
//!
//! Asked once, when a session is picked up, and never during a turn. What
//! happens when a window fills mid-turn is not a question — there is nothing
//! for a reader to weigh at that point, and stopping to ask would be stopping
//! the turn they are waiting on.

use std::sync::mpsc::{RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crucible_core::{Compacting, Event};
use crucible_runner::Runner;
use crucible_tui::{Offered, Panel, Prompt, Renderer, Row, Slot, Terminal, Working};

use crate::cli::Fatal;
use crate::cli::draw::{self, when};
use crate::cli::style::Style;

use super::Terms;
use super::picking::{self, Picked};

/// How large a session has to be before it is worth asking about, in tokens.
///
/// A judgement rather than a measurement, and the one number here that is. Far
/// enough in that carrying it whole is a real cost, and not so far that a
/// session somebody has barely started interrupts them to say so. Somebody who
/// disagrees writes their own figure down, and zero is how they say never.
const WORTH_ASKING: u64 = 60_000;

/// What the panel is headed with.
const TITLE: &str = "This session is large";

/// What the three answers say.
const RECAP: &str = "Carry on from notes";
const WHOLE: &str = "Carry all of it";
const NEVER: &str = "Stop asking";

/// And what each of them means, on the row beneath.
const RECAP_IS: &str = "one request now, and every request after it is smaller";
const WHOLE_IS: &str = "all of it goes back to the model, on every turn from here";
const NEVER_IS: &str = "written down; sessions are carried whole from now on";

/// The key row at the foot.
const KEYS: &str = "enter to choose · esc to carry it whole";

/// How long to wait on the worker before drawing the row again.
///
/// The mark turns four times a second, so this is what keeps it turning while
/// nothing at all is arriving — which, for one request that answers once at the
/// end, is the whole of it.
const TICK: Duration = Duration::from_millis(100);

/// The one word the row says while this runs.
const DOING: &str = "compacting";

/// Whether a session this size is worth stopping to ask about.
///
/// `said` is what a layer wrote down, and `None` is nobody having written
/// anything — which takes crucible's own figure. **Zero is not silence**: it is
/// somebody saying never, which is what "stop asking" writes down, and the two
/// have to stay different or that answer would be forgotten on the next launch.
fn worth_asking(carrying: u64, said: Option<u64>) -> bool {
    let worth = said.unwrap_or(WORTH_ASKING);

    worth > 0 && carrying >= worth
}

/// Puts the question, where the session picked up is large enough to be worth
/// one, and acts on the answer.
///
/// Does nothing at all otherwise, which is the ordinary case: a session small
/// enough to carry costs nothing to carry, and a question about it would be a
/// question with one sensible answer.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn asked<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: Runner,
    terms: &Terms,
    keys: bool,
) -> Result<Runner, Fatal> {
    let carrying = runner.carrying();

    // Nothing is asked down a pipe: there is nobody to answer, and a question
    // with no reader is a session that carries on whole having pretended to
    // offer a choice.
    if !keys || !worth_asking(carrying, runner.compaction().ask_on_resume) {
        return Ok(runner);
    }

    let style = terms.style();
    let started = runner
        .session()
        .id()
        .map(|id| when::ago(id.started(), SystemTime::now()));
    let said = match started {
        Some(started) => format!(
            "{} carried, from a session started {started}. Carrying it whole spends that again on every turn.",
            draw::tokens(carrying)
        ),
        None => format!(
            "{} carried. Carrying it whole spends that again on every turn.",
            draw::tokens(carrying)
        ),
    };

    let shown = [
        Offered {
            name: RECAP,
            says: RECAP_IS,
        },
        Offered {
            name: WHOLE,
            says: WHOLE_IS,
        },
        Offered {
            name: NEVER,
            says: NEVER_IS,
        },
    ];

    let panel = Panel {
        title: TITLE,
        said: Some(&said),
        shown: &shown,
        chosen: 0,
        footer: KEYS,
    };

    // Left rather than answered means carried whole, which is the answer that
    // changes nothing: a reader who pressed escape has not asked for a request
    // to be spent on their behalf.
    match picking::pick(renderer, style, panel)? {
        Picked::Took(0) => recap(renderer, runner, terms),
        Picked::Took(2) => {
            stop(renderer, terms);
            Ok(runner)
        }
        _ => Ok(runner),
    }
}

/// Replaces what is behind with notes on it, and says what that came to.
///
/// The request runs on a worker and this thread draws, which is the same shape
/// a turn has and for the same reason: one provider request can take ten
/// seconds, and a screen that says nothing for ten seconds is a screen somebody
/// reads as a hang.
///
/// Nothing offers to stop it. The row names no key because no key is read here
/// — there is no turn to interrupt and no editor standing — and a row naming a
/// key that does nothing is worse than a row naming none.
fn recap<T: Terminal>(
    renderer: &mut Renderer<T>,
    mut runner: Runner,
    terms: &Terms,
) -> Result<Runner, Fatal> {
    let style = terms.style();
    let glyphs = style.glyphs();
    let (events, seen) = channel::<Event>();

    let cancel = terms.cancel.clone();
    let working = thread::Builder::new()
        .name("compacting".to_owned())
        .spawn(move || {
            let outcome = runner.compact(Compacting::Resumed, &events, &cancel);
            (runner, outcome)
        })
        .map_err(Fatal::Worker)?;

    let since = Instant::now();
    let mut left = None;
    loop {
        match seen.recv_timeout(TICK) {
            // The reading is taken off the row while this runs, the same as it
            // is mid-turn: the number it would show is the one being replaced.
            Ok(Event::Carried { left: said }) => left = said,
            // Everything else it reports, and the beat with nothing on it: both
            // reach the same redraw below, which is what turns the mark while
            // one request answers once at the end.
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            // The worker has dropped the sender, so it is finished.
            Err(RecvTimeoutError::Disconnected) => break,
        }

        standing(renderer, since, left, style)?;
    }

    let (runner, outcome) = working
        .join()
        .map_err(|_| Fatal::Worker(std::io::Error::other("the compacting thread ended badly")))?;

    let columns = renderer.columns();
    match outcome {
        Ok(Some(compacted)) => {
            let rows = draw::compacted_rows(compacted, columns, glyphs);
            renderer.present(&rows, style.palette())?;
        }
        Ok(None) => {}
        // The session is untouched, so there is nothing to undo and nothing to
        // warn about beyond what happened. It is carried whole instead.
        Err(problem) => renderer.commit(&format!("! could not make notes: {problem}"))?,
    }

    Ok(runner)
}

/// The row above the box, and the box, while room is being made.
fn standing<T: Terminal>(
    renderer: &mut Renderer<T>,
    since: Instant,
    left: Option<u8>,
    style: Style,
) -> Result<(), Fatal> {
    let columns = renderer.columns();
    let glyphs = style.glyphs();

    let mut rows = vec![
        Row::new(),
        Working {
            doing: DOING,
            running: since.elapsed(),
            spent: None,
            stops: None,
            left,
        }
        .row(columns, glyphs),
        Row::new(),
    ];
    rows.extend(
        Prompt {
            said: "",
            column: 0,
            mode: "",
            tone: Slot::Quiet,
            hint: "",
            model: "",
            provider: "",
            effort: None,
            asking: None,
            running: None,
            room: 1,
        }
        .rows(columns, glyphs),
    );

    renderer.under(&rows, None, style.palette())?;
    Ok(())
}

/// Writes down that this question is not wanted again.
fn stop<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms) {
    if let Err(problem) = crate::cli::remember::unasked(&terms.choosing) {
        drop(renderer.commit(&format!("! could not write that down: {problem}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_small_enough_to_carry_is_carried_without_a_question() {
        assert!(!worth_asking(1_000, None));
        assert!(worth_asking(WORTH_ASKING, None));
    }

    #[test]
    fn somebody_who_said_never_is_never_asked_again() {
        // Zero and silence are different answers, and the difference is the
        // whole of what "stop asking" buys: written down as nothing at all it
        // would be indistinguishable from never having been asked, and the
        // question would come back on the next launch.
        assert!(!worth_asking(10_000_000, Some(0)));
        assert!(worth_asking(10_000_000, None));
    }

    #[test]
    fn a_figure_somebody_wrote_down_is_the_one_that_decides() {
        assert!(worth_asking(9_000, Some(9_000)));
        assert!(!worth_asking(8_999, Some(9_000)));
    }
}
