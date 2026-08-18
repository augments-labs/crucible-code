//! What each key does to the list of commands still running.
//!
//! The loop that reads them cannot be driven from here — the keyboard it reads is
//! the process's own — so what is tested is the function of a key, which is where
//! the decision actually lives.

use crucible_core::{
    Ask, Cancel, Mode, Permission, Remember, Rules, Sensitivity, Settled, Tool, ToolArgs, ToolCall,
    ToolId, Unwatched, Verdict,
};
use crucible_tools::{Background, Bash};

use crate::cli::sample::Sample;

use super::*;

/// A registry with `count` commands running in it.
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

        drop(tool.run(approved, &Unwatched));
    }

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
    let (left, _here) = running("two", 2);
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
fn the_key_that_opened_it_closes_it() {
    // What every other `ctrl+` key here does, and what makes it a toggle rather
    // than a door.
    let (left, _here) = running("one", 1);

    for closing in [Pressed::Background, Pressed::Escape] {
        let mut leaving = Leaving::default();
        assert_eq!(leaving.against(closing, &left), Moved::Left);
    }
}

#[test]
fn stopping_the_last_one_takes_the_list_with_it() {
    // Nothing left to stand, and a frame of empty chrome is worse than the row
    // under the box that opened this.
    let (left, _here) = running("one", 1);
    let mut leaving = Leaving::default();

    assert_eq!(
        leaving.against(Pressed::Key(Key::Char('x')), &left),
        Moved::Left
    );
    assert_eq!(left.count(), 0, "the command was not ended");
}

#[test]
fn stopping_one_of_several_keeps_the_list_open() {
    let (left, _here) = running("two", 2);
    let mut leaving = Leaving::default();

    assert_eq!(
        leaving.against(Pressed::Key(Key::Char('x')), &left),
        Moved::Redraw
    );
    assert_eq!(left.count(), 1);
}

#[test]
fn enter_shows_what_one_has_printed_and_the_way_back_is_the_list() {
    let (left, _here) = running("one", 1);
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
    let (left, _here) = running("two", 2);
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
