//! What a turn is asked under, over a workspace and nothing else.

use crucible_config::Settings;
use crucible_core::{Effort, Workspace};

use crucible_tools::Ended;

use super::{Standing, said, under};

/// A workspace at a path that is on no machine, which is all this needs: the
/// root is put into the prompt as text and never opened.
fn somewhere() -> Workspace {
    Workspace::open(std::env::temp_dir()).expect("a workspace")
}

/// What a run with no configuration file anywhere asks under.
fn asked(model: &str, effort: Option<Effort>, workspace: &Workspace) -> String {
    under(Standing {
        settings: &Settings::default(),
        model,
        effort,
        workspace,
        tools: Vec::new(),
    })
}

#[test]
fn the_root_every_tool_path_is_relative_to_is_said_rather_than_looked_for() {
    let workspace = somewhere();
    let said = asked("claude-opus-5", None, &workspace);

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
    let said = asked("claude-opus-5", Some(Effort::Xhigh), &somewhere());

    assert!(said.contains("claude-opus-5"), "{said}");
    assert!(said.contains("xhigh"), "{said}");
}

#[test]
fn a_rung_nobody_named_is_the_vendors_own_default_rather_than_silence() {
    // The field is left off the request in that state, so something answers it
    // — and a prompt that said nothing would leave the model to invent the
    // answer to a question it is going to be asked either way.
    let said = asked("claude-opus-5", None, &somewhere());

    assert!(said.contains("claude-opus-5"), "{said}");
    assert!(said.contains("default"), "{said}");
}

#[test]
fn a_session_with_no_model_chosen_is_told_nothing_about_what_it_is() {
    // It cannot take a turn, so there is nothing true to say yet. A sentence
    // naming the empty model would be this program inventing the fact the two
    // above exist to stop being invented.
    let said = asked("", None, &somewhere());

    assert!(said.contains("operating inside crucible"), "{said}");
    assert!(!said.contains("asked at"), "{said}");
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
    let said = asked("claude-opus-5", None, &somewhere());

    assert!(!said.contains("left running"), "{said}");
    assert!(!said.contains("have ended"), "{said}");
}

#[test]
fn the_tools_this_run_registered_are_named_off_the_registry_that_holds_them() {
    // Names and no sentences. What each does is in the schema that travels
    // with every request anyway, and a second copy here is the one nobody
    // updates when a tool changes.
    let said = under(Standing {
        settings: &Settings::default(),
        model: "claude-opus-5",
        effort: None,
        workspace: &somewhere(),
        tools: ["read", "bash"].map(str::to_owned).to_vec(),
    });

    assert!(said.contains("read and bash"), "{said}");
}
