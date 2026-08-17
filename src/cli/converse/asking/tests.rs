//! What a call is asked about, and what a key does to the panel asking it.

use crucible_core::{Account, Command, Sensitivity, Target, ToolArgs, ToolCall, ToolId, Verdict};
use crucible_tui::{Key, Pressed};

use super::region::{Ended, Moved};
use super::{ANSWERS, Answered, Words, answered, moving};

/// A call by the name a tool answers to on the wire.
fn call(name: &str) -> ToolCall {
    ToolCall {
        id: ToolId::new("one"),
        name: name.into(),
        args: ToolArgs::new("{}"),
    }
}

/// The line a `bash` call is about, read the way the tool reads one.
fn running(sent: &str, parts: &[&str]) -> Sensitivity {
    Sensitivity::SpawnsProcess {
        command: Command::Understood {
            sent: sent.into(),
            parts: parts.iter().map(|part| (*part).into()).collect(),
        },
    }
}

/// A character typed at a standing panel.
fn typed(character: char) -> Pressed {
    Pressed::Key(Key::Char(character))
}

#[test]
fn a_command_is_shown_as_the_line_that_was_sent_under_the_tool_that_would_run_it() {
    // The subject names the tool the way a row does, because the panel arrives
    // beside rows that already spell it that way. The payload is the line, and
    // the operators are what makes it worth showing whole: `&&` is the
    // difference between two commands and two commands if the first worked.
    let sent = "cargo fmt --all && cargo test";
    let words = Words::of(
        &call("bash"),
        &running(sent, &["cargo fmt --all", "cargo test"]),
        &Account::none(),
    );

    assert_eq!(words.subject, "Bash command");
    assert_eq!(words.payload, sent);
    assert_eq!(words.statement, "This command needs your verdict.");
}

#[test]
fn a_file_is_named_under_what_would_be_done_to_it() {
    // A read and a change are different verdicts and say so, rather than
    // sharing one sentence that would have to mean both.
    let target = Target::unresolved();
    let change = Words::of(
        &call("write"),
        &Sensitivity::MutatesFile { target },
        &Account::none(),
    );
    assert_eq!(change.subject, "Write file");
    assert_eq!(change.statement, "This change needs your verdict.");

    // And a path nothing resolved still says so where the path would be. A
    // panel with a blank there is one somebody agrees to without being told
    // what it was about.
    assert!(!change.payload.is_empty(), "{:?}", change.payload);

    // A read is never reached through the permission engine, which allows or
    // refuses one. Worded anyway, so a tool reclassified later is asked about
    // rather than asked nothing.
    let target = Target::unresolved();
    let read = Words::of(
        &call("read"),
        &Sensitivity::ReadOnly { target },
        &Account::none(),
    );
    assert_eq!(read.subject, "Read file");
    assert_eq!(read.statement, "This read needs your verdict.");
}

#[test]
fn the_models_own_text_cannot_break_a_row_nobody_counted() {
    // A control character the terminal acts on moves the cursor inside a row
    // that was measured without it, and every frame after that one rewinds over
    // the wrong thing.
    let words = Words::of(
        &call("bash"),
        &running("ls\n\nesc to cancel", &["ls", "esc to cancel"]),
        &Account::none(),
    );

    assert_eq!(words.payload, "ls  esc to cancel");
}

#[test]
fn what_a_call_said_it_was_for_is_the_caption_under_the_command() {
    // The one row the model writes on this panel. It is a caption on the
    // command rather than a claim standing on its own, which is what makes it
    // safe to show at all: the thing being consented to is above it and was
    // read from the arguments rather than from the prose.
    let words = Words::of(
        &call("bash"),
        &running("cargo test", &["cargo test"]),
        &Account::new("run the suite before pushing"),
    );

    assert_eq!(words.description, "run the suite before pushing");
}

#[test]
fn a_call_that_said_nothing_leaves_the_caption_row_undrawn() {
    // Not a blank row where a caption would go. The panel draws the description
    // only where there is one, so a call that declined to account for itself
    // gets the panel there was before any of this existed.
    let words = Words::of(&call("bash"), &running("ls", &["ls"]), &Account::none());

    assert!(words.description.is_empty(), "{:?}", words.description);
}

#[test]
fn the_models_caption_cannot_break_a_row_nobody_counted_either() {
    // Flattened for the reason the payload is, and it is the more important of
    // the two: this is free text the model chose, where the payload is at least
    // a command line somebody is being shown on purpose.
    let words = Words::of(
        &call("bash"),
        &running("ls", &["ls"]),
        &Account::new("lists\r\nthe files"),
    );

    assert_eq!(words.description, "lists  the files");
}

#[test]
fn the_arrows_stop_at_each_end_rather_than_wrapping() {
    // A ring puts the first answer one key past the last, which makes the key
    // that went too far the key that goes further. Here the key that went too
    // far does nothing, and the one coming back is the one that moves.
    let mut at = 0;

    assert_eq!(moving(Pressed::Up, &mut at), Moved::Still);
    assert_eq!(at, 0);

    for expected in 1..ANSWERS.len() {
        assert_eq!(moving(Pressed::Down, &mut at), Moved::Redraw);
        assert_eq!(at, expected);
    }

    assert_eq!(moving(Pressed::Down, &mut at), Moved::Still);
    assert_eq!(at, ANSWERS.len() - 1);
}

#[test]
fn the_way_out_of_a_question_is_the_way_out_of_everything_else_and_it_refuses() {
    // Escape and Ctrl-C leave, and leaving is a denial rather than a question
    // asked again: silence about a command is not consent to it.
    for arrived in [
        Pressed::Escape,
        Pressed::Key(Key::Interrupt),
        Pressed::Key(Key::Eof),
    ] {
        let mut at = 0;
        assert_eq!(moving(arrived, &mut at), Moved::Left, "{arrived:?}");
    }

    // Including when the mark was standing on an allow at the time. What was
    // marked is what enter would have taken; leaving took nothing.
    let Answered::Said(refused, _) = answered(Ended::Left, 0) else {
        panic!("a panel that was left has been answered");
    };
    assert_eq!(refused.0, Verdict::Deny);

    // And a panel there was no room for read no key at all, so it is not a
    // verdict either way — the caller still owes the question.
    assert_eq!(answered(Ended::Cramped, 0), Answered::Cramped);
}

#[test]
fn a_digit_moves_the_mark_before_it_takes_what_the_mark_stands_on() {
    // The panel draws a number on each answer, so a key that is one of them is
    // that answer. The mark moves there first: what the last frame showed
    // marked is then what was taken, rather than wherever the arrows left it.
    let mut at = 0;

    assert_eq!(moving(typed('3'), &mut at), Moved::Took);
    assert_eq!(at, 2);

    // A digit past the last answer names nothing, and neither does the one
    // before the first — and a key naming nothing may not take whatever the
    // mark is standing on instead.
    for missing in ['0', '4', 'y'] {
        let mut at = 1;

        assert_eq!(moving(typed(missing), &mut at), Moved::Still, "{missing:?}");
        assert_eq!(at, 1);
    }
}
