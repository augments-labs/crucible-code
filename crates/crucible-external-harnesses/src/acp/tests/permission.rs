//! An answer is one of the options the agent offered for that question.

use crucible_execution::{Decision, Lasting, Text};

use crate::acp::permission::Permissions;

/// The agent's own option ids, one of each kind it offered.
fn offered() -> Vec<(Box<str>, Box<str>)> {
    vec![
        ("go-1".into(), "allow_once".into()),
        ("stop-1".into(), "reject_once".into()),
    ]
}

/// An allow is sent as the agent's own id for its allow-once option.
#[test]
fn an_allow_is_sent_as_the_option_id_the_agent_offered() {
    let mut permissions = Permissions::default();
    permissions.offered("7", offered());

    let frame = permissions
        .answer("7", &Decision::Allow(Lasting::Once))
        .unwrap();

    assert_eq!(
        frame,
        r#"{"id":7,"jsonrpc":"2.0","result":{"outcome":{"optionId":"go-1","outcome":"selected"}}}"#
    );
}

/// A decision no offered option carries, a question never asked and an answer
/// in words are each refused rather than sent under an id the agent did not
/// give.
#[test]
fn a_reply_with_an_option_the_agent_did_not_offer_is_refused() {
    let mut permissions = Permissions::default();
    permissions.offered("7", offered());

    let always = permissions.answer("7", &Decision::Allow(Lasting::Session));
    let unasked = permissions.answer("8", &Decision::Allow(Lasting::Once));
    let worded = permissions.answer("7", &Decision::Answer(Text::new("yes")));

    assert!(always.is_err(), "{always:?}");
    assert!(unasked.is_err(), "{unasked:?}");
    assert!(worded.is_err(), "{worded:?}");
}
