//! What a call is asked about, and what a key does to the panel asking it.

use crucible_core::{Command, Sensitivity, Target, ToolArgs, ToolCall, ToolId, Verdict};
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
    let change = Words::of(&call("write"), &Sensitivity::MutatesFile { target });
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
    let read = Words::of(&call("read"), &Sensitivity::ReadOnly { target });
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
    );

    assert_eq!(words.payload, "ls  esc to cancel");
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
