//! The agent is started in its profile, and a request to sign in is never
//! mistaken for an answer.

use std::path::Path;

use crucible_execution::{Authentication, TurnRequest};
use crucible_types::TurnId;
use serde_json::json;

use crate::antigravity::Antigravity;
use crate::antigravity::account::LINK_PREFIX;
use crate::{Account, Act, Dialect, Reading, Shape, Told};

/// A sign-in link as the agent prints it.
const SIGN_IN: &str = "https://accounts.google.com/o/oauth2/v2/auth?state=s";

#[test]
fn the_agent_is_started_with_its_home_in_the_profile() {
    let request = Antigravity::default().request(Shape {
        profile: Some(Path::new("/profiles/work")),
        workspace: Some(Path::new("/work")),
        ..Shape::default()
    });

    let home = request
        .environment
        .iter()
        .find(|(name, _)| name == "GEMINI_HOME")
        .map(|(_, value)| value.as_str());
    assert_eq!(home, Some("/profiles/work"));
    assert_eq!(request.arguments.is_empty(), !cfg!(target_os = "linux"));
}

/// The agent printing its sign-in link during a turn means the profile is
/// signed out, and the turn fails saying how to sign in.
#[test]
fn a_sign_in_link_during_a_turn_fails_it() {
    let mut agent = Antigravity::default();
    let _ = agent.request(Shape::default());
    let _ = agent.open(&TurnRequest::new(TurnId::FIRST, "hello")).unwrap();

    let acts = agent.heard(&format!("{LINK_PREFIX}{SIGN_IN}")).unwrap();

    assert!(
        matches!(acts.as_slice(), [Act::Fail(failure)] if failure.reason().as_str().contains("/login")),
        "{acts:?}"
    );
}

/// The link is shown during a sign-in, and read as signed out during any
/// other step.
#[test]
fn a_sign_in_link_is_shown_only_while_signing_in() {
    let printed = format!("{LINK_PREFIX}{SIGN_IN}");
    let mut agent = Antigravity::default();
    let _ = agent.spoken(Account::SignIn).unwrap();
    assert!(matches!(agent.told(Account::SignIn, &printed), Ok(Told::Link(_))));

    let mut agent = Antigravity::default();
    let _ = agent.spoken(Account::Status).unwrap();
    assert_eq!(
        agent.told(Account::Status, &printed).unwrap(),
        Told::Done(Reading::Signed(Authentication::SignedOut))
    );
    let refused = json!({"jsonrpc": "2.0", "id": 0, "error": {"code": -32000, "message": "x"}});
    assert!(matches!(
        agent.told(Account::Status, &refused.to_string()),
        Ok(Told::Done(Reading::Unreadable(_)))
    ));
}
