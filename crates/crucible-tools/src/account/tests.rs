//! What a call is read as having said about itself.

use crucible_core::{Cancel, DescribeTool, ToolArgs, ToolDescriptor, ToolProvenance, Workspace};

use super::of;
use crate::sample::Sample;
use crate::{Bash, Edit, Ledger, Read, Write};

/// The tools whose schemas invite an account, built against a scratch tree.
///
/// Which is the tools whose calls stop at a panel: a command and the two ways
/// to change a file. A read is allowed or refused without being asked about, so
/// prose sent with one would be paid for and never drawn.
fn inviting(workspace: &Workspace) -> Vec<ToolDescriptor> {
    fn descriptor(tool: &impl DescribeTool) -> ToolDescriptor {
        tool.descriptor(ToolProvenance::builtin(tool.name()).unwrap())
            .unwrap()
    }

    vec![
        descriptor(&Bash::new(workspace.clone(), Cancel::new())),
        descriptor(&Write::new(workspace.clone(), Ledger::new())),
        descriptor(&Edit::new(workspace.clone(), Cancel::new())),
    ]
}

#[test]
fn a_call_is_read_as_saying_what_the_model_wrote_in_its_description() {
    let said = of(&ToolArgs::new(
        r#"{"command":"cargo test","description":"run the suite"}"#,
    ));

    assert_eq!(said.description(), "run the suite");
}

#[test]
fn a_call_that_said_nothing_about_itself_is_read_as_having_said_nothing() {
    // Three ways to say nothing, and the panel owes the same answer to all of
    // them: the field left out, the field left blank, and arguments that will
    // not parse at all. The last is the one worth naming — a call that broken
    // is refused by the tool a moment later, and inventing words for it here
    // would describe something that never happened.
    for arguments in [
        r#"{"command":"ls"}"#,
        r#"{"command":"ls","description":""}"#,
        r#"{"command":"ls","description":42}"#,
        "not json at all",
        "",
    ] {
        assert!(
            of(&ToolArgs::new(arguments)).description().is_empty(),
            "{arguments:?}"
        );
    }
}

#[test]
fn a_call_is_read_as_explaining_itself_in_the_paragraphs_it_wrote() {
    let said = of(&ToolArgs::new(
        r#"{"command":"cargo test",
            "description":"run the suite",
            "explanation":["It builds every crate.","It takes about two minutes."]}"#,
    ));

    assert_eq!(said.description(), "run the suite");
    assert_eq!(
        said.explanation().collect::<Vec<_>>(),
        ["It builds every crate.", "It takes about two minutes."]
    );
}

#[test]
fn a_call_that_explained_nothing_is_read_as_having_explained_nothing() {
    // The line and the paragraphs are separate arguments and a call is free to
    // send either without the other, so every way of getting the second wrong
    // has to leave the first standing. The wrong-shape ones are the point: a
    // model that sent one string where a list was asked for still said
    // something worth putting on the panel, and refusing the lot would throw it
    // away over prose nobody validates.
    for arguments in [
        r#"{"command":"ls","description":"list the files"}"#,
        r#"{"command":"ls","description":"list the files","explanation":null}"#,
        r#"{"command":"ls","description":"list the files","explanation":[]}"#,
        r#"{"command":"ls","description":"list the files","explanation":"one paragraph"}"#,
        r#"{"command":"ls","description":"list the files","explanation":["fine",7]}"#,
    ] {
        let said = of(&ToolArgs::new(arguments));

        assert_eq!(said.explanation().len(), 0, "{arguments}");
        assert_eq!(said.description(), "list the files", "{arguments}");
    }
}

#[test]
fn a_tool_that_invites_an_account_is_a_tool_whose_schema_asks_for_one() {
    // The reading above is one name for every tool, so what decides whether a
    // call can account for itself is the schema — and a schema that never
    // mentions the field is a model that never sends it. Checked together with
    // the reading, because the two apart are a field nobody fills and a panel
    // nobody can explain.
    let sample = Sample::new("account-schemas");
    let workspace = sample.workspace();

    for tool in inviting(&workspace) {
        for field in [r#""description": {"#, r#""explanation": {"#] {
            assert!(
                tool.schema().contains(field),
                "{} invites no {field}",
                tool.name()
            );
        }
    }

    // And one that does not, so the assertion above is about this schema rather
    // than about the word `description` appearing anywhere in a schema at all —
    // every one of them opens with the tool's own.
    let quiet = Read::new(workspace, Cancel::new(), Ledger::new());
    assert!(!quiet.schema().contains(r#""description": {"#));
    assert!(!quiet.schema().contains(r#""explanation": {"#));
}
