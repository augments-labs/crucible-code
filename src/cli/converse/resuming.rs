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

use std::io;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crucible_core::{Compacting, Event};
use crucible_runner::Runner;
use crucible_tui::{
    Glyphs, Key, Offered, Panel, Pressed, Prompt, Renderer, Row, Slot, Terminal, Working, pressed,
    waiting,
};

use crate::cli::Fatal;
use crate::cli::draw::{self, when};
use crate::cli::style::Style;

use super::Terms;
use super::picking::{self, Picked};
use super::typing;

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
const RECAP: &str = "Carry on from summary";
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

/// And the key that stops it, named because it is read.
const STOPS: &str = "esc to stop";

/// What the row under it says, which is what is being done and not why.
///
/// Not a word about the window: nothing here says it was full, because at this
/// moment it very often is not — somebody chose this rather than reaching it.
const MAKING: &str = "reading the session back and writing down what matters";

/// How wide the bar under the word is, in columns.
const BAR: usize = 28;

/// The sentence under the title: how much, from when, and what it costs.
///
/// Written here rather than inline so it can be looked at without a terminal —
/// it is the row that has to persuade somebody, and the only one whose wording
/// is worth arguing about.
fn sentence(carrying: u64, runner: &Runner) -> String {
    let started = runner
        .session()
        .id()
        .map(|id| when::ago(id.started(), SystemTime::now()));

    match started {
        Some(started) => format!(
            "{} carried, from a session started {started}. Carrying it whole spends that again on every turn.",
            draw::tokens(carrying)
        ),
        None => format!(
            "{} carried. Carrying it whole spends that again on every turn.",
            draw::tokens(carrying)
        ),
    }
}

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
    runner: &mut Runner,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    let carrying = runner.carrying();

    // Nothing is asked down a pipe: there is nobody to answer, and a question
    // with no reader is a session that carries on whole having pretended to
    // offer a choice.
    if !keys || !worth_asking(carrying, runner.compaction().ask_on_resume) {
        return Ok(());
    }

    let style = terms.style();
    let said = sentence(carrying, runner);
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
            Ok(())
        }
        _ => Ok(()),
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
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let style = terms.style();
    let glyphs = style.glyphs();
    let (events, seen) = channel::<Event>();

    // Cloned out before the scope: `Terms` holds what one command changes in
    // cells, so it is not a thing two threads may share, and the worker needs
    // nothing from it but this.
    let cancel = terms.cancel.clone();

    // Everything the box says at rest, read before the worker borrows the
    // runner — and read from the one place that builds it, so the box drawn
    // here is the box drawn everywhere else. A blank frame while this runs
    // reads as a program that has fallen over rather than one that is busy.
    let says = typing::saying(runner);
    let filled = runner.left().map(|left| 100 - left);

    // Whatever stopped anything earlier is spent. From here the keyboard is
    // read against this, and a flag found raised belongs to it.
    cancel.reset();

    let outcome = thread::scope(|scope| {
        let working = scope.spawn(|| runner.compact(Compacting::Resumed, &events, &cancel));

        let since = Instant::now();
        loop {
            // Looked at every pass, because a request that answers once at the
            // end would otherwise leave the keyboard dead for as long as it
            // takes — which is a program that has stopped, as far as anybody
            // watching can tell.
            if waiting(Duration::ZERO).unwrap_or(false) {
                // Every other key, and a read that failed, are the same
                // answer here: this is not a box being typed into, and the only
                // press it has anything to do about is the one that stops it.
                if let Ok(Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof)) = pressed() {
                    cancel.request();
                }
            }

            match seen.recv_timeout(TICK) {
                // Nothing it reports changes this picture: the reading is off
                // the row while this runs, and what it came to is drawn once at
                // the end. What the beat is for is the mark.
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                // The worker has dropped the sender, so it is finished.
                Err(RecvTimeoutError::Disconnected) => break,
            }

            standing(renderer, since, &says, filled, style)?;
        }

        working
            .join()
            .map_err(|_| Fatal::Worker(io::Error::other("the compacting thread ended badly")))
    })?;

    let columns = renderer.columns();
    match outcome {
        Ok(Some(compacted)) => {
            let rows = draw::compacted_rows(compacted, columns, glyphs);
            renderer.present(&rows, style.palette())?;
        }
        // Nothing to replace, or stopped before it had anything to say. The
        // session is untouched either way, and is carried whole.
        Ok(None) => {}
        Err(problem) => renderer.commit(&format!("! could not make a summary: {problem}"))?,
    }

    Ok(())
}

/// The row above the box, the bar under it, and the box, while room is made.
///
/// The same three rows a running turn puts there, which is what `METERED` is:
/// the blank, the word, and one row under it. No blank between the word and the
/// bar — it is a second line of the same thing rather than a second thing.
fn standing<T: Terminal>(
    renderer: &mut Renderer<T>,
    since: Instant,
    says: &typing::Says,
    filled: Option<u8>,
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
            stops: Some(STOPS),
            // Taken off while this runs: the number it would show is the one
            // being replaced.
            left: None,
        }
        .row(columns, glyphs),
    ];

    if let Some(row) = bar(filled, columns, glyphs) {
        rows.push(row);
    }

    rows.push(Row::new());
    rows.extend(
        Prompt {
            said: "",
            column: 0,
            mode: says.mode.as_ref(),
            tone: says.tone,
            hint: "",
            model: says.model.as_str(),
            provider: says.provider,
            effort: says.effort,
            asking: None,
            running: None,
            room: 1,
        }
        .rows(columns, glyphs),
    );

    renderer.under(&rows, None, style.palette())?;
    Ok(())
}

/// The bar under the word, or nothing where no window is known.
///
/// It shows how full the window was when this started, and holds still: what is
/// being replaced is exactly that, and a bar moving while the session was
/// rewritten would be measuring two different things a second apart.
fn bar(filled: Option<u8>, columns: usize, glyphs: Glyphs) -> Option<Row> {
    let filled = filled?;
    let gutter = Working::gutter(glyphs);
    if columns < gutter + BAR + 2 {
        return None;
    }

    let full = usize::from(filled) * BAR / 100;

    // Filled against hollow, and `Plain` against `Quiet`: the shape carries it
    // and the colour only reinforces, so it survives a terminal drawing none.
    Some(
        Row::new()
            .then(Slot::Quiet, " ".repeat(gutter))
            .then(Slot::Plain, glyphs.filled().repeat(full))
            .then(Slot::Quiet, glyphs.hollow().repeat(BAR - full))
            .then(Slot::Quiet, format!("  {MAKING}")),
    )
}

/// Writes down that this question is not wanted again.
fn stop<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms) {
    if let Err(problem) = crate::cli::remember::unasked(&terms.choosing) {
        drop(renderer.commit(&format!("! could not write that down: {problem}")));
    }
}

#[cfg(test)]
mod tests {
    use crucible_tui::Glyphs;

    use super::*;

    /// The panel as it stands, drawn for a terminal `columns` wide.
    fn drawn(columns: usize) -> String {
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
        let said = "340k carried, from a session started 3 hours ago. \
                    Carrying it whole spends that again on every turn.";

        Panel {
            title: TITLE,
            said: Some(said),
            shown: &shown,
            chosen: 0,
            footer: KEYS,
        }
        .within(columns, 24, Glyphs::Unicode)
        .iter()
        .map(Row::text)
        .collect::<Vec<_>>()
        .join("\n")
    }

    #[test]
    fn the_panel_says_the_cost_the_three_answers_and_the_keys() {
        let panel = drawn(80);
        println!("\n{panel}");

        for wanted in [TITLE, RECAP, WHOLE, NEVER, KEYS, "340k carried"] {
            assert!(panel.contains(wanted), "missing {wanted:?} in:\n{panel}");
        }
    }

    #[test]
    fn the_screen_while_room_is_made_says_what_is_happening_and_offers_the_key() {
        // What the owner has to be able to see: a word that is moving, a bar
        // saying how full it was, the key that stops it, and a box that still
        // says what it says at rest rather than a blank frame.
        let bar = bar(Some(96), 80, Glyphs::Unicode).expect("a bar").text();
        let row = Working {
            doing: DOING,
            running: std::time::Duration::from_secs(8),
            spent: None,
            stops: Some(STOPS),
            left: None,
        }
        .row(80, Glyphs::Unicode)
        .text();

        println!("\n{row}\n{bar}");

        assert!(row.contains(DOING), "{row}");
        assert!(row.contains(STOPS), "{row}");
        assert!(
            !row.contains('%'),
            "the reading is drawn while it is replaced"
        );
        assert!(bar.contains(MAKING), "{bar}");
    }

    #[test]
    fn the_bar_is_full_where_the_window_is_and_absent_where_none_is_known() {
        let full = bar(Some(100), 80, Glyphs::Ascii).expect("a bar").text();
        let empty = bar(Some(0), 80, Glyphs::Ascii).expect("a bar").text();

        assert!(full.contains(&"*".repeat(BAR)), "{full}");
        assert!(empty.contains(&"-".repeat(BAR)), "{empty}");

        // Nothing at all where no window is known — the same rule the reading
        // keeps, and not a bar drawn empty, which would read as a window with
        // nothing in it.
        assert!(bar(None, 80, Glyphs::Ascii).is_none());

        // And nothing where there is no room for one, rather than a bar cut in
        // half, which is a proportion that is not the proportion.
        assert!(bar(Some(50), 12, Glyphs::Ascii).is_none());
    }

    #[test]
    fn no_row_of_it_is_wider_than_the_terminal_it_was_drawn_for() {
        // The failure `responsive-components.md` is about: a row past the last
        // column is one the terminal wraps itself, and every frame after it
        // rewinds over the wrong number of rows.
        for columns in [40, 60, 80, 120] {
            for row in drawn(columns).lines() {
                assert!(crucible_tui::columns(row) <= columns, "{columns}: {row:?}");
            }
        }
    }

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
