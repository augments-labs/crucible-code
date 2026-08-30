//! What each key does to the list of commands still running.
//!
//! The loop that reads them cannot be driven from here — the keyboard it reads is
//! the process's own — so what is tested is the function of a key, which is where
//! the decision actually lives.

use crucible_core::{
    Ask, Cancel, DescribeTool, Mode, Permission, Remember, Rules, Sensitivity, Settled, Tool,
    ToolArgs, ToolCall, ToolId, Unwatched, Verdict,
};
use crucible_tools::{Background, Bash};

use crate::cli::sample::Sample;

use super::*;

/// A registry with `count` commands running in it.
///
/// `case` has to be this test's own and nobody else's: the tree it names is
/// removed and remade, so two tests in one process sharing a name delete each
/// other's workspace part way through.
///
/// Real ones, started through the real path: the registry only holds what it was
/// handed, so a list of processes with no processes behind it would be a test
/// about nothing. The verdict comes from the engine in the mode that asks about
/// nothing, which is the only way anything outside it can obtain one.
fn running(case: &str, count: usize) -> (Background, Sample) {
    let here = Sample::new(case);
    let left = Background::new();
    let tool = Bash::new(here.workspace(), Cancel::new()).leaving(left.clone());
    let mut engine = Permission::with(Mode::FullAccess, Rules::default());

    for at in 0..count {
        let call = ToolCall {
            id: ToolId::new(format!("{case}-{at}")),
            name: tool.name().into(),
            args: ToolArgs::new(r#"{"command":"sleep 30","background":true}"#),
        };

        let Settled::Approved(approved) =
            engine.decide(&call, &tool.sensitivity(&call.args), &mut Nobody)
        else {
            panic!("full access asked about a command");
        };

        let output = tool.run(approved, &Unwatched).expect("the command started");
        assert!(
            !output.is_failed(),
            "a command this test needs running was refused: {}",
            output.text()
        );
    }

    // Asserted rather than assumed. What these tests are about is what a key does
    // to a list, and a list one command short would fail them somewhere the reason
    // is not written down.
    assert_eq!(
        left.count(),
        count,
        "the registry did not take every command this test started"
    );

    (left, here)
}

/// Answers nothing, because in this mode nothing is asked.
struct Nobody;

impl Ask for Nobody {
    fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
        (Verdict::Deny, Remember::Never)
    }
}

#[test]
fn the_arrows_walk_the_list_and_stop_at_its_ends() {
    let (left, _here) = running("arrows", 2);
    let mut leaving = Leaving::default();

    assert_eq!(leaving.against(Pressed::Up, &left), Moved::Still);
    assert_eq!(leaving.against(Pressed::Down, &left), Moved::Redraw);
    assert_eq!(leaving.at, 1);
    assert_eq!(
        leaving.against(Pressed::Down, &left),
        Moved::Still,
        "the mark walked off the end of the list"
    );
}

#[test]
fn a_click_on_a_row_marks_it_and_a_second_click_opens_it() {
    // The first click is the arrows' work done at once — it moves the mark to
    // the row pointed at; the second, on the row already marked, is the enter
    // that shows what the command has printed.
    let (left, _here) = running("clicking", 2);
    let mut leaving = Leaving::default();

    // Row HEAD is the first command, HEAD + 1 the second; the rows above them
    // are the heading and its blanks. `Pressed` is not `Copy` — a paste carries
    // a string — so the same click is spelled twice.
    let second = || Pressed::Clicked {
        row: super::HEAD + 1,
        column: 4,
    };
    assert_eq!(leaving.against(second(), &left), Moved::Redraw);
    assert_eq!(leaving.at, 1, "the click marked the second row");
    assert!(leaving.shown.is_none(), "one click only marks");

    assert_eq!(leaving.against(second(), &left), Moved::Redraw);
    assert!(leaving.shown.is_some(), "a second click opened it");
}

#[test]
fn a_click_on_the_chrome_or_past_the_list_is_a_click_on_nothing() {
    let (left, _here) = running("clicking-past", 1);

    // The heading row, and a row below the one command there is.
    for row in [0, super::HEAD - 1, super::HEAD + 1] {
        let mut leaving = Leaving::default();
        assert_eq!(
            leaving.against(Pressed::Clicked { row, column: 2 }, &left),
            Moved::Still,
            "row {row}"
        );
        assert_eq!(leaving.at, 0, "row {row} moved the mark");
        assert!(leaving.shown.is_none(), "row {row} opened something");
    }
}

#[test]
fn the_key_that_opened_it_closes_it() {
    // What every other `ctrl+` key here does, and what makes it a toggle rather
    // than a door.
    let (left, _here) = running("closing", 1);

    for closing in [Pressed::Background, Pressed::Escape] {
        let mut leaving = Leaving::default();
        assert_eq!(leaving.against(closing, &left), Moved::Left);
    }
}

#[test]
fn stopping_the_last_one_takes_the_list_with_it() {
    // Nothing left to stand, and a frame of empty chrome is worse than the row
    // under the box that opened this.
    let (left, _here) = running("stopping-last", 1);
    let mut leaving = Leaving::default();

    assert_eq!(
        leaving.against(Pressed::Key(Key::Char('x')), &left),
        Moved::Left
    );
    assert_eq!(left.count(), 0, "the command was not ended");
}

#[test]
fn stopping_one_of_several_keeps_the_list_open() {
    let (left, _here) = running("stopping-one", 2);
    let mut leaving = Leaving::default();

    assert_eq!(
        leaving.against(Pressed::Key(Key::Char('x')), &left),
        Moved::Redraw
    );
    assert_eq!(left.count(), 1);
}

#[test]
fn enter_shows_what_one_has_printed_and_the_way_back_is_the_list() {
    let (left, _here) = running("showing", 1);
    let mut leaving = Leaving::default();

    assert_eq!(
        leaving.against(Pressed::Key(Key::Enter), &left),
        Moved::Redraw
    );
    assert!(leaving.shown.is_some(), "nothing was opened");

    // And out of it into the list rather than out of both: this was opened from
    // there, and the way back is where the reader came from.
    assert_eq!(leaving.against(Pressed::Escape, &left), Moved::Redraw);
    assert!(leaving.shown.is_none());
}

#[test]
fn a_command_that_ended_while_the_list_was_open_brings_the_mark_back_inside_it() {
    let (left, _here) = running("ended-while-open", 2);
    let mut leaving = Leaving::default();
    leaving.against(Pressed::Down, &left);
    assert_eq!(leaving.at, 1);

    let numbers: Vec<usize> = left.running().iter().map(|one| one.number).collect();
    if let Some(last) = numbers.last() {
        left.stop(*last);
    }

    drop(leaving.rows(&left, 80, 24, Glyphs::Unicode));

    assert_eq!(
        leaving.at, 0,
        "the mark was left pointing at a command that had gone"
    );
}
