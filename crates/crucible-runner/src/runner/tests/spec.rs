//! What an agent definition holds, and what a run built from one reports.

use super::*;

/// A definition with everything said about it that can be said.
fn described() -> AgentSpec {
    AgentSpec {
        id: AgentId::new("coding"),
        name: "Coding".into(),
        description: "Edits this repository and runs its checks.".into(),
        instructions: Some("You are an expert in coding.".into()),
        model: Model {
            name: "claude-test".into(),
            max_tokens: 1024,
            window: None,
            accepts: Some(READS),
            effort: None,
        },
    }
}

#[test]
fn a_definition_given_only_an_id_answers_to_it_and_claims_nothing_else() {
    let spec = AgentSpec::new(AgentId::new("coding"), described().model);

    assert_eq!(spec.id.as_str(), "coding");
    assert_eq!(
        &*spec.name, "coding",
        "an unnamed agent lost the word it is selected under"
    );
    assert_eq!(
        &*spec.description, "",
        "a description nobody wrote was invented"
    );
    assert!(
        spec.instructions.is_none(),
        "instructions nobody wrote were invented"
    );
}

#[test]
fn a_session_told_nothing_at_all_is_not_told_the_empty_string() {
    // `None` is nobody said, and the empty string is a request that carries a
    // system field holding nothing — two different requests, per the rule on
    // the field itself. `telling` is the one place that state can be written,
    // and the same reading `crucible-config` already applies to a prompt key
    // written empty applies here: nothing said.
    //
    // Reachable only through this method — the wiring's prompt always names
    // the workspace root, so it is never empty — which is why the read below
    // is the one an outside caller branching on `is_none()` would get wrong.
    let mut scripted = Scripted::new(Script::new(vec![]), Tools::new(), Verdict::Allow);
    scripted.runner.telling("mind the workspace");
    assert_eq!(scripted.runner.instructions(), Some("mind the workspace"));

    scripted.runner.telling("");

    assert_eq!(
        scripted.runner.instructions(),
        None,
        "a session told nothing reports having been told the empty string"
    );
}
