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
