//! What a turn is asked under, over a workspace and nothing else.

use crucible_config::Settings;
use crucible_core::SystemPrompt;

use crucible_tools::Ended;

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
            code: Some(1),
            lines: 96,
        },
        Ended {
            tool: "bash",
            number: 2,
            called: "cargo watch".into(),
            code: Some(0),
            lines: 4,
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
