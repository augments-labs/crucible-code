//! What a turn is asked under, over a workspace and nothing else.

use crucible_core::{Effort, Workspace};

use crucible_tools::Ended;

use super::under;

/// A workspace at a path that is on no machine, which is all this needs: the
/// root is put into the prompt as text and never opened.
fn somewhere() -> Workspace {
    Workspace::open(std::env::temp_dir()).expect("a workspace")
}

#[test]
fn the_root_every_tool_path_is_relative_to_is_said_rather_than_looked_for() {
    let workspace = somewhere();
    let said = under("claude-opus-5", None, &workspace, &[]);

    assert!(
        said.contains(&workspace.root().display().to_string()),
        "{said}"
    );
}

#[test]
fn what_a_model_cannot_find_out_about_itself_is_said_to_it() {
    // Two facts a model has no way to look at. Its own name it would answer
    // from training, which for a name is a guess that reads like one it knows;
    // the rung is a field on a request it never sees. Both are exactly what
    // somebody asking what they are talking to is asking about.
    let said = under("claude-opus-5", Some(Effort::Xhigh), &somewhere(), &[]);

    assert!(said.contains("claude-opus-5"), "{said}");
    assert!(said.contains("xhigh"), "{said}");
}

#[test]
fn a_rung_nobody_named_is_the_vendors_own_default_rather_than_silence() {
    // The field is left off the request in that state, so something answers it
    // — and a prompt that said nothing would leave the model to invent the
    // answer to a question it is going to be asked either way.
    let said = under("claude-opus-5", None, &somewhere(), &[]);

    assert!(said.contains("claude-opus-5"), "{said}");
    assert!(said.contains("default"), "{said}");
}

#[test]
fn a_session_with_no_model_chosen_is_told_nothing_about_what_it_is() {
    // It cannot take a turn, so there is nothing true to say yet. A sentence
    // naming the empty model would be this program inventing the fact the two
    // above exist to stop being invented.
    let said = under("", None, &somewhere(), &[]);

    assert!(said.contains("You are crucible"), "{said}");
    assert!(!said.contains("asked at"), "{said}");
}

#[test]
fn a_command_that_ended_while_nobody_waited_is_in_the_note_the_next_turn_gets() {
    // The other audience. The reader was told when it happened; the model is told
    // here, because a turn already in flight has nowhere to put a new fact and a
    // server that fell over is something to know before answering rather than
    // after.
    let ended = [
        Ended {
            number: 1,
            called: "Bash(npm run dev)".into(),
            code: Some(1),
            lines: 96,
        },
        Ended {
            number: 2,
            called: "Bash(cargo watch)".into(),
            code: Some(0),
            lines: 4,
        },
    ];

    let said = under("claude-opus-5", None, &somewhere(), &ended);

    assert!(said.contains("#1 Bash(npm run dev)"), "{said}");
    assert!(said.contains("failed with exit status 1"), "{said}");
    assert!(said.contains("96 lines"), "{said}");
    assert!(said.contains("#2 Bash(cargo watch)"), "{said}");
    assert!(said.contains("finished"), "{said}");
}

#[test]
fn a_turn_with_nothing_ended_is_told_nothing_about_it() {
    // Almost every turn. A note about nothing is a sentence to read past on the
    // way to the ones that mean something.
    let said = under("claude-opus-5", None, &somewhere(), &[]);

    assert!(!said.contains("left running"), "{said}");
    assert!(!said.contains("have ended"), "{said}");
}
