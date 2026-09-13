//! What a turn is asked under, over a workspace and nothing else.

use crucible_config::Settings;
use crucible_core::SystemPrompt;

use crucible_builtins::Ended;

use super::{said, under};

/// What a run with no configuration file anywhere asks under.
fn asked() -> String {
    under(&Settings::default())
}

#[test]
fn stable_instructions_hold_no_session_fact() {
    let said = asked();

    assert_eq!(said, SystemPrompt::default().instructions_text());
    assert!(said.contains("operating inside crucible"), "{said}");
    assert!(!said.contains("# This session"), "{said}");
    assert!(!said.contains("workspace root"), "{said}");
    assert!(!said.contains("Toolset generation"), "{said}");
}

#[test]
fn a_command_that_ended_while_nobody_waited_is_said_in_the_words_the_model_reads_it_in() {
    // The other audience. The reader was told when it happened; the model is
    // told here, because nothing was in flight to hand it to — a turn that was
    // running takes its own notes, and what is left over is what nobody took.
    //
    // The wording and nothing else, because where it goes is not this
    // function's to decide: all three ways of arriving put it in the aside,
    // and the turn drains that into its own transcript.
    let ended = [
        Ended {
            tool: "bash",
            number: 1,
            called: "npm run dev".into(),
            said: "".into(),
            code: Some(1),
            lines: 96,
            printed: Box::from(""),
            unpublished: None,
        },
        Ended {
            tool: "bash",
            number: 2,
            called: "cargo watch".into(),
            said: "".into(),
            code: Some(0),
            lines: 4,
            printed: Box::from(""),
            unpublished: None,
        },
    ];

    let note = said(&ended).expect("a note about two commands");

    assert!(note.contains("#1 Bash(npm run dev)"), "{note}");
    assert!(note.contains("failed with exit status 1"), "{note}");
    assert!(note.contains("96 lines"), "{note}");
    assert!(note.contains("#2 Bash(cargo watch)"), "{note}");
    assert!(note.contains("finished"), "{note}");
}

#[test]
fn what_a_command_printed_travels_with_the_news_that_it_ended() {
    // The gap this note existed with. Told that a command ended and never what
    // it said, a model still needs the answer the command was getting, and the
    // only way left to it is to run something else that asks the same question.
    let ended = [Ended {
        tool: "bash",
        number: 7,
        called: "gh pr checks 622 --watch".into(),
        said: "Wait and check PR checks".into(),
        code: Some(0),
        lines: 3,
        printed: "Rust CI / Linux\tpass\nCodeQL\tpass\n".into(),
        unpublished: None,
    }];

    let note = said(&ended).expect("a note about one command");

    assert!(note.contains("Rust CI / Linux"), "{note}");
    assert!(note.contains("CodeQL"), "{note}");
}

#[test]
fn a_command_that_printed_nothing_adds_no_empty_block_to_the_note() {
    // A silent command is the ordinary case for a watcher that was killed
    // before it said anything, and a heading with nothing under it reads as
    // output that went missing.
    let ended = [Ended {
        tool: "bash",
        number: 8,
        called: "sleep 30".into(),
        said: "".into(),
        code: None,
        lines: 0,
        printed: "".into(),
        unpublished: None,
    }];

    let note = said(&ended).expect("a note about one command");

    assert!(note.contains("#8 Bash(sleep 30)"), "{note}");
    assert!(note.contains("was killed"), "{note}");
    assert!(!note.contains("printed:"), "{note}");
}

#[test]
fn a_command_whose_writes_were_not_published_says_so_and_why() {
    // A command left running that wrote into a root something else changed
    // meanwhile ends with what it wrote discarded. Told only that it finished,
    // the model would go on as though the files it wrote were there.
    let ended = [Ended {
        tool: "bash",
        number: 3,
        called: "npm run codegen".into(),
        said: "".into(),
        code: None,
        lines: 2,
        printed: "wrote 4 files\n".into(),
        unpublished: Some("writable root changed after the command started".into()),
    }];

    let note = said(&ended).expect("a note about one command");

    assert!(note.contains("#3 Bash(npm run codegen)"), "{note}");
    assert!(note.contains("nothing it wrote was published"), "{note}");
    assert!(
        note.contains("writable root changed after the command started"),
        "{note}"
    );
    assert!(!note.contains("was killed"), "{note}");
    assert!(note.contains("wrote 4 files"), "{note}");
}

#[test]
fn a_turn_with_nothing_ended_is_told_nothing_about_it() {
    // Almost every turn. A note about nothing is a sentence to read past on the
    // way to the ones that mean something, and returning `None` is what keeps
    // it out of the aside rather than putting an empty one there.
    assert!(said(&[]).is_none());

    // And the prompt never carried it in the first place, so no wording of it
    // can arrive by that route either.
    let said = asked();

    assert!(!said.contains("left running"), "{said}");
    assert!(!said.contains("have ended"), "{said}");
}

#[test]
fn a_reason_with_a_line_break_reaches_the_model_as_one_line() {
    // The note is a list, and a reason carrying its own newline would read as
    // the next command's line rather than as this one's reason.
    let ended = [Ended {
        tool: "bash",
        number: 4,
        called: "npm run build".into(),
        said: "".into(),
        code: None,
        lines: 1,
        printed: "".into(),
        unpublished: Some(
            "writable root changed after the command started\nterminal delta category: path-set"
                .into(),
        ),
    }];

    let note = said(&ended).expect("a note about one command");

    let line = note
        .lines()
        .find(|line| line.contains("nothing it wrote was published"))
        .expect("the line about the ending");
    assert!(line.contains("terminal delta category"), "{note}");
}
