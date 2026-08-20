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

/// And what it says once somebody has asked it to stop.
///
/// On screen from the press rather than from the moment the request notices:
/// what a reader needs to know is that the key landed, and the provider may be
/// a second or two behind that.
const STOPPING: &str = "stopping";

/// And the key that stops it, named because it is read.
const STOPS: &str = "esc to stop";

/// What the row under it says.
///
/// Not a word about the window: nothing here says it was full, because at this
/// moment it very often is not — somebody chose this rather than reaching it.
const MAKING: &str = "writing down what matters";

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
/// The keyboard is read on the same pass, so the key the row names is a key
/// that does something: a request answering once at the end would otherwise
/// leave it dead for as long as it takes, which is a program that has stopped
/// as far as anybody watching can tell.
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

    // Whatever stopped anything earlier is spent. From here the keyboard is
    // read against this, and a flag found raised belongs to it.
    cancel.reset();

    let outcome = thread::scope(|scope| {
        // `move`, and it matters: captured by reference the sender outlives the
        // worker, the channel never closes, and the loop below waits for a
        // disconnect that cannot come — a finished compaction with a screen
        // that never moves on and a keyboard that answers nothing.
        let stopping = cancel.clone();
        let working = scope.spawn(move || runner.compact(Compacting::Resumed, &events, &stopping));

        let since = Instant::now();
        let mut part = 0;
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
                // How much of the notes has been written. The rest of what it
                // reports changes nothing here — what it came to is drawn once,
                // at the end.
                Ok(Event::Compacting { part: said, .. }) => part = said,
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                // The worker has dropped the sender, so it is finished.
                Err(RecvTimeoutError::Disconnected) => break,
            }

            // And asked directly as well, rather than trusting the channel to
            // say so. A sender left alive anywhere else closes nothing, and the
            // loop would wait for a disconnect that cannot come — a finished
            // request with a screen that never moves on. The worker itself is
            // the fact; the channel is a way of hearing about it.
            if working.is_finished() {
                break;
            }

            standing(
                renderer,
                since,
                &says,
                Making {
                    part,
                    stopping: cancel.requested(),
                },
                style,
            )?;
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
/// Drawn the way the box is drawn *between turns*, because that is where this
/// is: `live` owns the region and settles whatever came before it. The footing
/// call beside it is for a turn that is streaming into a tail, and using it
/// here paints under a region that is not there.
///
/// The box says what it says at rest rather than standing blank — it cannot be
/// typed into while this runs, and a box gone empty as well reads as a program
/// that has fallen over.
fn standing<T: Terminal>(
    renderer: &mut Renderer<T>,
    since: Instant,
    says: &typing::Says,
    making: Making,
    style: Style,
) -> Result<(), Fatal> {
    let columns = renderer.columns();
    let glyphs = style.glyphs();

    let prompt = Prompt {
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
        room: Prompt::room(renderer.rows()),
    };

    let mut boxed = prompt.rows(columns, glyphs);
    let mut caret = prompt.caret(columns);

    let mut rows = vec![
        Row::new(),
        Working {
            doing: if making.stopping { STOPPING } else { DOING },
            running: since.elapsed(),
            spent: None,
            // Nothing left to offer once it has been asked: a row still naming
            // the key is a row saying the press did not land.
            stops: (!making.stopping).then_some(STOPS),
            // Taken off while this runs: the number it would show is the one
            // being replaced.
            left: None,
        }
        .row(columns, glyphs),
    ];

    if let Some(row) = bar(making.part, columns, glyphs) {
        rows.push(row);
    }
    rows.push(Row::new());

    // The caret belongs to the box, which is however many rows below the top of
    // the region it now is.
    caret.row += rows.len();
    rows.append(&mut boxed);

    renderer.live(&rows, caret, style.palette())?;
    Ok(())
}

/// How far the notes have got, and whether somebody has asked it to stop.
///
/// Two facts about one row, carried together rather than as two arguments: what
/// the row says is one picture, and a call taking every part of it separately
/// is a call nobody can read.
#[derive(Debug, Clone, Copy)]
struct Making {
    /// How much of the notes has been written, as a percentage.
    part: u8,
    /// Whether the key has been pressed.
    stopping: bool,
}

/// The bar under the word, or nothing where there is no room for one.
///
/// It fills as the notes are written, which is the one thing here that is
/// actually known: the answer is arriving and this is how much of the room it
/// was given has been used. It is not a clock, and does not claim to be.
fn bar(part: u8, columns: usize, glyphs: Glyphs) -> Option<Row> {
    let gutter = Working::gutter(glyphs);
    if columns < gutter + BAR + 8 {
        return None;
    }

    let full = usize::from(part) * BAR / 100;

    // Filled against hollow, and `Plain` against `Quiet`: the shape carries it
    // and the colour only reinforces, so it survives a terminal drawing none.
    Some(
        Row::new()
            .then(Slot::Quiet, " ".repeat(gutter))
            .then(Slot::Plain, glyphs.filled().repeat(full))
            .then(Slot::Quiet, glyphs.hollow().repeat(BAR - full))
            .then(Slot::Quiet, format!("  {part}%  {MAKING}")),
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
        let bar = bar(31, 80, Glyphs::Unicode).expect("a bar").text();
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
    fn the_bar_fills_with_the_notes_and_is_absent_where_there_is_no_room() {
        let full = bar(100, 80, Glyphs::Ascii).expect("a bar").text();
        let empty = bar(0, 80, Glyphs::Ascii).expect("a bar").text();

        assert!(full.contains(&"*".repeat(BAR)), "{full}");
        assert!(full.contains("100%"), "{full}");
        assert!(empty.contains(&"-".repeat(BAR)), "{empty}");

        // Nothing where there is no room for one, rather than a bar cut in
        // half, which is a proportion that is not the proportion.
        assert!(bar(50, 12, Glyphs::Ascii).is_none());
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
