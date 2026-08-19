//! The one rule every component keeps, asserted about all of them at once.
//!
//! Nothing drawn is wider than the window it was given, and nothing given a
//! room is taller than that room. Both are the renderer's arithmetic rather
//! than a matter of taste: a row past the last column is one the terminal wraps
//! itself, which leaves the cursor a row below where the next frame expects it,
//! and a live region taller than the window is one whose top has scrolled out
//! of reach of the rewind that has to take it back.
//!
//! Every component tests its own layout beside itself, and this replaces none
//! of that. Those tests say the picture is *right*, which is the longer
//! question; this says only that it fits, and says it about every component in
//! the crate, at every width and at every height one of them decides anything
//! at.
//!
//! So the reason this file exists is the component that is not in it yet. A
//! sweep written beside one component covers the widths its author thought of.
//! This one is a list that reads as incomplete the moment something is missing
//! from it — and `scripts/check.sh` fails while it is, because the one thing a
//! sweep cannot notice is a component it was never given.
//!
//! The fixtures are deliberately hostile. Every string is either long enough to
//! break at any width in the sweep or too long to break at all, because a
//! layout that folds its prose correctly and then puts down one unclipped
//! sentence is the defect this catches, and a fixture of short words never
//! reaches it.

use std::time::Duration;

use crate::asked::{Asked, Choice, Given, Stop, Writing};
use crate::asking::Question;
use crate::color::Slot;
use crate::expanded::{Expanded, Shown};
use crate::glyphs::Glyphs;
use crate::ladder::Ladder;
use crate::menu::{Listed, Menu};
use crate::notice::Notice;
use crate::panel::{Offered, Panel};
use crate::plan::{Plan, State, Task};
use crate::prompt::Prompt;
use crate::row::Row;
use crate::running::{Command, Running};
use crate::welcome::{Recent, Welcome};
use crate::working::Working;

/// Every width worth walking: one column, and on past the widest terminal
/// anybody works in.
///
/// From one rather than from nothing, because a window of no columns is not a
/// window. A terminal that will not say its size is given
/// [`Size::FALLBACK`](crate::Size::FALLBACK) instead of a zero, and the one
/// component that draws into a nought-column window does it on purpose — the
/// prompt mark, so that a record says a prompt was asked — with its own test
/// beside it saying so.
const WIDTHS: std::ops::RangeInclusive<usize> = 1..=200;

/// Every room worth walking, for the components that are given one.
///
/// From no rows at all — which is what a window with something already standing
/// in it has left — through every height a component decides something at, and
/// on past an ordinary terminal.
///
/// A list rather than the range the widths are, because the two are not the
/// same kind of axis. Width is where the wrapping arithmetic lives, and it is
/// wrong at one column and right at the next. Height is where a component picks
/// which rung of its ladder to draw, and those are counted in single rows near
/// the bottom and nowhere near the top.
const ROOMS: [usize; 11] = [0, 1, 2, 3, 4, 5, 6, 8, 12, 24, 40];

/// Both fonts, because a component picks its glyphs by which it was handed and
/// the two are not the same width.
const FONTS: [Glyphs; 2] = [Glyphs::Unicode, Glyphs::Ascii];

/// Prose long enough to break at every width in the sweep, in words short
/// enough that where it breaks is the layout's decision rather than one word's.
const PROSE: &str = "a search that stops partway through a directory nobody meant to open, and \
                     says so on the row it stopped on";

/// A word longer than the narrow end of the sweep: what finds a layout that
/// gives up columns everywhere except on the one thing it cannot cut.
const LONG: &str = "crucible-code/crates/crucible-tui/src/welcome/parts.rs";

/// Sweeps `laid` across every width in both fonts.
#[track_caller]
fn across(what: &str, laid: impl Fn(usize, Glyphs) -> Vec<Row>) {
    for glyphs in FONTS {
        for columns in WIDTHS {
            within(&laid(columns, glyphs), columns, what, glyphs);
        }
    }
}

/// The same, across every room as well, checking the height too.
#[track_caller]
fn down(what: &str, laid: impl Fn(usize, usize, Glyphs) -> Vec<Row>) {
    for glyphs in FONTS {
        for columns in WIDTHS {
            for room in ROOMS {
                let rows = laid(columns, room, glyphs);
                within(&rows, columns, what, glyphs);
                assert!(
                    rows.len() <= room,
                    "{what} drew {} rows into a window with room for {room}, at {columns} \
                     columns with {glyphs:?}",
                    rows.len()
                );
            }
        }
    }
}

/// Asserts that nothing in `rows` reaches past column `columns`.
#[track_caller]
fn within(rows: &[Row], columns: usize, what: &str, glyphs: Glyphs) {
    for row in rows {
        assert!(
            row.columns() <= columns,
            "{what} drew {} columns into a window {columns} wide with {glyphs:?}: {:?}",
            row.columns(),
            row.text()
        );
    }
}

#[test]
fn the_welcome_card_fits_the_window_it_opens_on() {
    const SESSIONS: [Recent<'static>; 2] = [
        Recent {
            title: PROSE,
            when: "2h ago",
        },
        Recent {
            title: "rule replacement on windows",
            when: "yesterday",
        },
    ];

    let welcome = Welcome {
        version: "v0.0.8",
        root: LONG,
        sessions: &SESSIONS,
    };
    across("the welcome card", |columns, glyphs| {
        welcome.rows(columns, glyphs)
    });
}

#[test]
fn a_panel_fits_the_window_it_stands_in() {
    const OFFERED: [Offered<'static>; 2] = [
        Offered {
            name: "anthropic",
            says: PROSE,
        },
        Offered {
            name: LONG,
            says: "the one with a name nothing can shorten",
        },
    ];

    let panel = Panel {
        title: PROSE,
        said: Some(PROSE),
        shown: &OFFERED,
        chosen: 1,
        footer: "enter to take it · esc to leave",
    };
    across("a panel", |columns, glyphs| panel.rows(columns, glyphs));
    down("a panel", |columns, room, glyphs| {
        panel.within(columns, room, glyphs)
    });
}

#[test]
fn the_command_list_fits_the_window_it_opens_over() {
    const LISTED: [Listed<'static>; 2] = [
        Listed {
            name: "/resume",
            says: PROSE,
        },
        Listed {
            name: LONG,
            says: "how this is drawn",
        },
    ];

    let menu = Menu {
        shown: &LISTED,
        chosen: Some(1),
    };
    across("the command list", |columns, glyphs| {
        menu.rows(columns, glyphs)
    });
}

#[test]
fn a_notice_fits_the_window_it_is_read_in() {
    let notice = Notice {
        heading: "the session log stopped recording",
        said: PROSE,
        named: Some(LONG),
    };
    across("a notice", |columns, glyphs| notice.rows(columns, glyphs));
}

#[test]
fn a_ladder_fits_the_window_its_rungs_are_chosen_in() {
    const RUNGS: [&str; 5] = ["none", "low", "medium", "high", "the most there is"];

    let ladder = Ladder {
        title: "How much thinking each answer buys",
        rungs: &RUNGS,
        chosen: 2,
        ends: ("answers sooner", "thinks for longer"),
        footer: "←→ to move · enter to take it",
    };
    across("a ladder", |columns, glyphs| ladder.rows(columns, glyphs));
}

#[test]
fn the_prompt_box_fits_the_window_it_is_typed_into() {
    let prompt = Prompt {
        said: PROSE,
        column: 4,
        mode: "ask before edits",
        tone: Slot::Quiet,
        hint: "ctrl+j for a new line",
        model: "claude-opus-5",
        provider: "anthropic",
        effort: Some("high"),
        asking: Some("queued"),
        running: Some(2),
        room: 6,
    };
    across("the prompt box", |columns, glyphs| {
        prompt.rows(columns, glyphs)
    });
    across("a committed prompt", |columns, glyphs| {
        Prompt::committed(PROSE, columns, glyphs, true)
    });
}

#[test]
fn the_working_row_fits_the_window_the_turn_runs_in() {
    let working = Working {
        doing: PROSE,
        running: Duration::from_secs(93),
        spent: Some(12_345),
        stops: Some("esc to stop"),
        left: Some(7),
    };
    across("the working row", |columns, glyphs| {
        vec![working.row(columns, glyphs)]
    });
}

#[test]
fn a_plan_fits_the_window_it_stands_under() {
    const TASKS: [Task<'static>; 3] = [
        Task {
            said: PROSE,
            state: State::Done,
        },
        Task {
            said: LONG,
            state: State::Doing,
        },
        Task {
            said: "and one nobody has started",
            state: State::Open,
        },
    ];

    for expanded in [true, false] {
        let plan = Plan {
            tasks: &TASKS,
            expanded,
        };
        down("a plan", |columns, room, glyphs| {
            plan.rows(columns, room, glyphs)
        });
    }
}

#[test]
fn the_list_of_what_is_still_running_fits_the_window_it_opens_in() {
    const COMMANDS: [Command<'static>; 2] = [
        Command {
            number: 1,
            called: LONG,
            running: Duration::from_secs(9),
            lines: 240,
            bytes: 18_000,
        },
        Command {
            number: 2,
            called: "cargo test --workspace",
            running: Duration::from_secs(97),
            lines: 4,
            bytes: 96,
        },
    ];

    let running = Running {
        shown: &COMMANDS,
        at: 1,
    };
    down(
        "the list of what is still running",
        |columns, room, glyphs| running.rows(columns, room, glyphs),
    );
}

#[test]
fn a_view_of_what_was_cut_fits_the_window_it_is_read_in() {
    const SHOWN: [Shown<'static>; 1] = [Shown {
        called: LONG,
        text: PROSE,
    }];

    let expanded = Expanded {
        shown: &SHOWN,
        from: 0,
    };
    down("a view of what was cut", |columns, room, glyphs| {
        expanded.within(columns, room, glyphs)
    });
}

#[test]
fn a_permission_question_fits_the_window_it_is_answered_in() {
    const PAYLOAD: [&str; 2] = [PROSE, LONG];
    const EXPLANATION: [&str; 1] = [PROSE];
    const ANSWERS: [&str; 3] = ["yes", "no", LONG];

    let question = Question {
        subject: LONG,
        payload: &PAYLOAD,
        description: PROSE,
        attribution: "the model's own words about it",
        explanation: &EXPLANATION,
        from: 0,
        more: "↑↓ to see more",
        statement: "crucible wants to change a file",
        question: "Allow it?",
        answers: &ANSWERS,
        marked: 1,
        footer: "enter to take it · esc to leave",
    };
    down("a permission question", |columns, room, glyphs| {
        question.within(columns, room, glyphs)
    });
}

#[test]
fn an_ask_fits_the_window_its_questions_are_put_in() {
    const STOPS: [Stop<'static>; 2] = [
        Stop {
            name: "Language",
            done: true,
            asks: true,
        },
        Stop {
            name: PROSE,
            done: false,
            asks: true,
        },
    ];
    const ANSWERS: [Choice<'static>; 2] = [
        Choice {
            answer: "Rust",
            says: PROSE,
            chosen: Some(true),
            shows: &[LONG, PROSE],
        },
        Choice {
            answer: LONG,
            says: "the one with a name nothing can shorten",
            // The empty specimen, which is drawn from a sentence of this
            // crate's own rather than from anything the caller handed in.
            chosen: Some(false),
            shows: &[],
        },
    ];
    const GIVEN: [Given<'static>; 1] = [Given {
        question: "Language",
        answer: "Rust",
    }];

    let asked = Asked {
        subject: LONG,
        stops: &STOPS,
        at: 1,
        statement: PROSE,
        given: &GIVEN,
        question: PROSE,
        answers: &ANSWERS,
        marked: 1,
        note: PROSE,
        writing: Some(Writing {
            text: PROSE,
            column: 3,
            placeholder: "something else",
        }),
        at_note: true,
        leaves: "esc to leave",
        footer: "enter to take it",
    };
    down("an ask", |columns, room, glyphs| {
        asked.within(columns, room, glyphs).0
    });
}
