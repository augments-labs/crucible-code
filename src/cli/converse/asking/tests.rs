//! What a call is asked about, and what a key does to the panel asking it.

use crucible_core::{Account, Command, Sensitivity, Target, ToolArgs, ToolCall, ToolId, Verdict};
use crucible_tui::{Key, Pressed};

use super::region::{Ended, Moved};
use super::{ANSWERS, Answered, CANCEL, EXPLAIN, HIDE, Standing, Words, answered, footer, moving};

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

/// A panel about a call that explained itself, with the prose open on a window
/// that `end` rows of it are below.
///
/// The end is the layout's answer and is written by the frame that worked it
/// out, so a test that wants to press a key at a cut explanation says so here
/// rather than drawing one.
fn reading(end: usize) -> Standing {
    Standing {
        open: true,
        end,
        ..Standing::new(true)
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
    // that was measured without it, so what the row is charged for and what it
    // takes on screen stop being the same number.
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
fn the_paragraphs_are_the_models_own_words_and_the_panel_says_whose_they_are() {
    // Every other row on this panel is crucible's, written out of what a tool
    // read from the arguments. These are not, and the one row that says so is
    // what keeps a page of the model's prose from reading as the program's
    // account of what it is about to do.
    let words = Words::of(
        &call("bash"),
        &running("cargo test", &["cargo test"]),
        &Account::explained(
            "run the suite",
            ["Runs every test in the workspace.", "Nothing is written."],
        ),
    );

    assert_eq!(words.attribution, "bash's own account of this call:");
    assert_eq!(
        words.explanation,
        ["Runs every test in the workspace.", "Nothing is written."]
    );
}

#[test]
fn the_models_paragraphs_cannot_break_a_row_nobody_counted_either() {
    // The same reason the payload and the caption are flattened, over more text
    // than either: a control character here moves the cursor inside a row that
    // was measured without it, so the panel takes rows it never asked for.
    let words = Words::of(
        &call("bash"),
        &running("ls", &["ls"]),
        &Account::explained("list the files", ["It\r\nreads", "and\twrites nothing"]),
    );

    assert_eq!(words.explanation, ["It  reads", "and writes nothing"]);
}

#[test]
fn the_arrows_stop_at_each_end_rather_than_wrapping() {
    // A ring puts the first answer one key past the last, which makes the key
    // that went too far the key that goes further. Here the key that went too
    // far does nothing, and the one coming back is the one that moves.
    let mut standing = Standing::new(false);

    assert_eq!(moving(Pressed::Up, &mut standing), Moved::Still);
    assert_eq!(standing.marked, 0);

    for expected in 1..ANSWERS.len() {
        assert_eq!(moving(Pressed::Down, &mut standing), Moved::Redraw);
        assert_eq!(standing.marked, expected);
    }

    assert_eq!(moving(Pressed::Down, &mut standing), Moved::Still);
    assert_eq!(standing.marked, ANSWERS.len() - 1);
}

#[test]
fn the_key_that_opens_the_paragraphs_is_the_key_that_closes_them() {
    // One key doing both is what makes it worth naming in the footer: a reader
    // who opened a page of prose over the answers needs the way back, and the
    // way back is the key they already pressed.
    let mut standing = Standing::new(true);

    assert_eq!(moving(Pressed::Explain, &mut standing), Moved::Redraw);
    assert!(standing.open);

    assert_eq!(moving(Pressed::Explain, &mut standing), Moved::Redraw);
    assert!(!standing.open);
}

#[test]
fn a_call_that_explained_nothing_is_a_panel_that_key_does_nothing_at() {
    // A key named where it does nothing is worse than a key nobody was offered,
    // so the footer does not name it — and the key itself is a frame nobody is
    // owed rather than an empty window over an explanation that never arrived.
    let mut standing = Standing::new(false);

    assert_eq!(moving(Pressed::Explain, &mut standing), Moved::Still);
    assert!(!standing.open);
    assert_eq!(footer(&standing), CANCEL);
}

#[test]
fn the_footer_names_the_key_by_what_it_would_do_next() {
    // Three rows for three states, because the one thing this row is for is
    // telling somebody what pressing it now does. *Explain* on prose already
    // open would send them looking for a second page.
    let mut standing = Standing::new(true);
    assert_eq!(footer(&standing), EXPLAIN);

    standing.open = true;
    assert_eq!(footer(&standing), HIDE);
}

#[test]
fn the_arrows_read_the_prose_while_it_is_open_and_was_cut() {
    // The one pair of keys does whichever job the picture is asking about.
    // Prose that ran past the window is asking to be read, so the arrows move
    // the window and the mark stays where it was — the answers are still there
    // under a number, which is the key somebody reading is not pressing.
    let mut standing = reading(2);

    assert_eq!(moving(Pressed::Up, &mut standing), Moved::Still);
    assert_eq!(standing.from, 0);

    for expected in 1..=2 {
        assert_eq!(moving(Pressed::Down, &mut standing), Moved::Redraw);
        assert_eq!(standing.from, expected);
    }

    // And it stops where the layout said the prose stops. Past that every press
    // of the key that went too far is a frame drawing the picture that is
    // already on screen.
    assert_eq!(moving(Pressed::Down, &mut standing), Moved::Still);
    assert_eq!(standing.from, 2);
    assert_eq!(standing.marked, 0);
}

#[test]
fn the_arrows_go_back_to_the_answers_where_the_prose_fitted() {
    // Open is not the question — cut is. Prose with nothing below the window has
    // nowhere to scroll to, and arrows that did nothing there would be two keys
    // taken away from the thing this panel exists to ask.
    let mut standing = reading(0);

    assert_eq!(moving(Pressed::Down, &mut standing), Moved::Redraw);
    assert_eq!(standing.marked, 1);
    assert_eq!(standing.from, 0);
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
        let mut standing = Standing::new(false);
        assert_eq!(
            moving(arrived.clone(), &mut standing),
            Moved::Left,
            "{arrived:?}"
        );
    }

    // Including when the mark was standing on an allow at the time. What was
    // marked is what enter would have taken; leaving took nothing.
    let Answered::Said(refused) = answered(Ended::Left, 0) else {
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
    let mut standing = Standing::new(false);

    assert_eq!(moving(typed('3'), &mut standing), Moved::Took);
    assert_eq!(standing.marked, 2);

    // A digit past the last answer names nothing, and neither does the one
    // before the first — and a key naming nothing may not take whatever the
    // mark is standing on instead.
    for missing in ['0', '4', 'y'] {
        let mut standing = Standing {
            marked: 1,
            ..Standing::new(false)
        };

        assert_eq!(
            moving(typed(missing), &mut standing),
            Moved::Still,
            "{missing:?}"
        );
        assert_eq!(standing.marked, 1);
    }
}

#[test]
fn the_wheel_at_a_question_is_the_transcripts_and_takes_no_answer() {
    // Two things at once. Three answers are reached with an arrow, so the wheel
    // has nothing to walk here — and `Still` is what hands it to the transcript
    // the panel is standing over, which is what a reader deciding about a call
    // is reading back through. What it may never be is an answer: a notch that
    // took whatever the mark stood on would allow a call by accident.
    for back in [true, false] {
        let mut standing = Standing {
            marked: 1,
            ..Standing::new(false)
        };

        assert_eq!(
            moving(Pressed::Scrolled { back }, &mut standing),
            Moved::Still,
            "{back}"
        );
        assert_eq!(standing.marked, 1);
    }
}

#[test]
fn a_command_that_will_be_left_running_says_so_where_it_is_agreed_to() {
    // Allowing it is allowing a process to go on after the answer that started it
    // has been given. A panel that said only what the command was would be asking
    // about the wrong thing.
    let call = ToolCall {
        id: ToolId::new("a"),
        name: "bash".into(),
        args: ToolArgs::new(r#"{"command":"npm run dev","background":true}"#),
    };

    let words = Words::of(
        &call,
        &running("npm run dev", &["npm", "run", "dev"]),
        &Account::none(),
    );

    assert!(
        words.statement.contains("left running"),
        "{:?}",
        words.statement
    );
}

#[test]
fn a_command_that_will_not_outlive_its_turn_says_nothing_about_it() {
    let call = ToolCall {
        id: ToolId::new("a"),
        name: "bash".into(),
        args: ToolArgs::new(r#"{"command":"ls"}"#),
    };

    let words = Words::of(&call, &running("ls", &["ls"]), &Account::none());

    assert!(
        !words.statement.contains("left running"),
        "{:?}",
        words.statement
    );
}
