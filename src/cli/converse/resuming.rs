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

use std::time::SystemTime;

use crucible_core::Compacting;
use crucible_runner::Runner;
use crucible_tui::{Offered, Panel, Renderer, Terminal};

use crate::cli::Fatal;
use crate::cli::draw::{self, when};

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
const RECAP: &str = "Carry on from summary";
const WHOLE: &str = "Carry all of it";
const NEVER: &str = "Stop asking";

/// And what each of them means, on the row beneath.
const RECAP_IS: &str = "one request now, and every request after it is smaller";
const WHOLE_IS: &str = "all of it goes back to the model, on every turn from here";
const NEVER_IS: &str = "written down; sessions are carried whole from now on";

/// The key row at the foot.
const KEYS: &str = "enter to choose · esc to carry it whole";

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
) -> Result<Option<Compacting>, Fatal> {
    let carrying = runner.carrying();

    // Nothing is asked down a pipe: there is nobody to answer, and a question
    // with no reader is a session that carries on whole having pretended to
    // offer a choice.
    if !keys
        || runner.transcript().is_empty()
        || !worth_asking(carrying, runner.compaction().ask_on_resume)
    {
        return Ok(None);
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
        Picked::Took(0) => Ok(Some(Compacting::Resumed)),
        Picked::Took(2) => {
            stop(renderer, terms);
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// Writes down that this question is not wanted again.
fn stop<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms) {
    if let Err(problem) = crate::cli::remember::unasked(&terms.choosing) {
        drop(renderer.commit(&format!("! could not write that down: {problem}")));
    }
}

#[cfg(test)]
mod tests {
    use crucible_tui::{Glyphs, Row};

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
    fn no_row_of_it_is_wider_than_the_terminal_it_was_drawn_for() {
        // The failure `responsive-components.md` is about: a row past the last
        // column is one the terminal wraps itself, so a band given one row is
        // written two and the band under it loses the first of its own.
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
