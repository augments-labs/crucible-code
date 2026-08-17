//! What a call is read as having said about itself.

use crucible_core::{Cancel, Tool, ToolArgs, Workspace};

use super::of;
use crate::sample::Sample;
use crate::{Bash, Ledger, Read};

/// The tools whose schemas invite an account, built against a scratch tree.
fn inviting(workspace: &Workspace) -> Vec<Box<dyn Tool>> {
    vec![Box::new(Bash::new(workspace.clone(), Cancel::new()))]
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
fn a_tool_that_invites_an_account_is_a_tool_whose_schema_asks_for_one() {
    // The reading above is one name for every tool, so what decides whether a
    // call can account for itself is the schema — and a schema that never
    // mentions the field is a model that never sends it. Checked together with
    // the reading, because the two apart are a field nobody fills and a panel
    // nobody can explain.
    let sample = Sample::new("account-schemas");
    let workspace = sample.workspace();

    for tool in inviting(&workspace) {
        assert!(
            tool.schema().contains(r#""description": {"#),
            "{} invites no account",
            tool.name()
        );
    }

    // And one that does not, so the assertion above is about this schema rather
    // than about the word `description` appearing anywhere in a schema at all —
    // every one of them opens with the tool's own.
    let quiet = Read::new(workspace, Cancel::new(), Ledger::new());
    assert!(!quiet.schema().contains(r#""description": {"#));
}
