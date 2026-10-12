//! The client holds an agent to its profile and to what it was offered.

use std::path::Path;

use crucible_execution::{Decision, Failure, Lasting, Outcome, TurnRequest};
use crucible_types::TurnId;
use serde_json::{Value, json};

use crate::acp::{Acp, Initialized, Profile};
use crate::{Account, Act, Dialect, Reading, Shape, Told};

/// Admits every agent.
#[expect(clippy::unnecessary_wraps, reason = "a profile's check has this shape")]
const fn anyone(_: &Initialized) -> Result<(), Failure> {
    Ok(())
}

/// An agent that speaks version 1 and signs in with `login`.
const PROFILE: Profile = Profile {
    program: "agent",
    versions: &[1],
    admits: anyone,
    sign_in: Some("login"),
};

/// A response to request `id` carrying `result`.
fn response(id: u64, result: &Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

/// An initialized answer in protocol version `version`.
fn initialized(version: u64) -> String {
    response(
        0,
        &json!({
            "protocolVersion": version,
            "agentInfo": {"name": "agent", "version": "1.0.0"},
            "agentCapabilities": {"loadSession": true, "auth": {"logout": {}}},
            "authMethods": [{"id": "login", "name": "Log in"}],
        }),
    )
}

/// A client of [`PROFILE`] working in this directory, its first turn opened.
fn opened() -> (Acp, Vec<String>) {
    let mut acp = Acp::new(PROFILE);
    let _ = acp.request(Shape {
        workspace: Some(Path::new("/work")),
        ..Shape::default()
    });
    let frames = acp.open(&TurnRequest::new(TurnId::FIRST, "hello")).unwrap();
    (acp, frames)
}

/// A client whose session `s` is open and whose first prompt is out.
fn prompting() -> Acp {
    let (mut acp, _) = opened();
    let _ = acp.heard(&initialized(1)).unwrap();
    let _ = acp.heard(&response(1, &json!({"sessionId": "s"}))).unwrap();
    acp
}

/// The first turn initializes, opens a session in the workspace, says its
/// name, then sends the prompt to it.
#[test]
fn the_first_turn_opens_a_session_then_sends_the_prompt() {
    let (mut acp, frames) = opened();
    let opening = acp.heard(&initialized(1)).unwrap();
    let prompted = acp.heard(&response(1, &json!({"sessionId": "s"}))).unwrap();

    let initialize: Value = serde_json::from_str(&frames[0]).unwrap();
    assert_eq!(initialize["method"], "initialize");
    assert_eq!(initialize["params"]["clientCapabilities"]["terminal"], false);
    let Act::Send(new) = &opening[0] else { panic!("{opening:?}") };
    let new: Value = serde_json::from_str(new).unwrap();
    assert_eq!(new["method"], "session/new");
    assert_eq!(new["params"]["cwd"], "/work");
    assert_eq!(prompted[0], Act::Session("s".to_owned()));
    let Act::Send(prompt) = &prompted[1] else { panic!("{prompted:?}") };
    let prompt: Value = serde_json::from_str(prompt).unwrap();
    assert_eq!(prompt["params"]["sessionId"], "s");
    assert_eq!(prompt["params"]["prompt"][0]["text"], "hello");
}

/// An agent answering in a protocol version its profile does not list is
/// sent nothing more: the turn fails.
#[test]
fn a_protocol_version_outside_the_profile_is_refused() {
    let (mut acp, _) = opened();

    let acts = acp.heard(&initialized(2)).unwrap();

    assert!(matches!(acts.as_slice(), [Act::Fail(_)]), "{acts:?}");
}

/// A request for the file system or a terminal, which this client never
/// offered, ends the turn rather than going unanswered.
#[test]
fn a_request_for_authority_never_offered_ends_the_turn() {
    let mut acp = prompting();

    let read = acp.heard(
        &json!({"jsonrpc": "2.0", "id": 9, "method": "fs/read_text_file", "params": {"sessionId": "s", "path": "/etc/passwd"}})
            .to_string(),
    );

    assert!(read.is_err(), "{read:?}");
}

/// A question still open when the turn is stopped is answered cancelled,
/// whatever was decided.
#[test]
fn a_question_open_when_the_turn_stops_is_answered_cancelled() {
    let mut acp = prompting();
    let asked = acp
        .heard(
            &json!({"jsonrpc": "2.0", "id": "q", "method": "session/request_permission", "params": {
                "sessionId": "s",
                "toolCall": {"toolCallId": "t", "title": "rm -rf build", "kind": "execute", "rawInput": {"command": "rm -rf build"}},
                "options": [{"optionId": "yes", "name": "Yes", "kind": "allow_once"}],
            }})
            .to_string(),
        )
        .unwrap();
    let stop = acp.cancel();
    let answer = acp.answer("\"q\"", &Decision::Allow(Lasting::Once)).unwrap();

    assert!(matches!(asked.as_slice(), [Act::Ask { .. }]), "{asked:?}");
    assert!(stop.is_some_and(|stop| stop.contains("session/cancel")));
    assert_eq!(
        answer,
        r#"{"id":"q","jsonrpc":"2.0","result":{"outcome":{"outcome":"cancelled"}}}"#
    );
}

/// Each stop reason ends the turn as the protocol means it, and one it does
/// not name ends it incomplete.
#[test]
fn each_stop_reason_ends_the_turn_as_it_means() {
    for (reason, outcome) in [
        ("end_turn", Outcome::Answered),
        ("cancelled", Outcome::Cancelled),
        ("max_tokens", Outcome::CutShort),
        ("max_turn_requests", Outcome::CutShort),
        ("refusal", Outcome::CutShort),
    ] {
        let mut acp = prompting();
        let acts = acp.heard(&response(2, &json!({"stopReason": reason}))).unwrap();
        assert_eq!(acts, vec![Act::End(outcome)], "{reason}");
    }
    let mut acp = prompting();
    assert!(acp.heard(&response(2, &json!({"stopReason": "bored"}))).is_err());
}

/// A status check reads a refusal to open a session for want of sign-in as
/// signed out, and a session opened as signed in.
#[test]
fn a_status_check_reads_sign_in_from_opening_a_session() {
    for (answer, signed) in [
        (
            json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": "Authentication required"}}).to_string(),
            crucible_execution::Authentication::SignedOut,
        ),
        (response(1, &json!({"sessionId": "probe"})), crucible_execution::Authentication::SignedIn),
    ] {
        let mut acp = Acp::new(PROFILE);
        let _ = acp.account(Account::Status, Shape { workspace: Some(Path::new("/work")), ..Shape::default() });
        let _ = acp.spoken(Account::Status).unwrap();
        let Told::Send(new) = acp.told(Account::Status, &initialized(1)).unwrap() else { panic!() };
        assert!(new.contains("session/new"));
        assert_eq!(
            acp.told(Account::Status, &answer).unwrap(),
            Told::Done(Reading::Signed(signed))
        );
    }
}

/// An agent that offers no sign-out is never reported signed out.
#[test]
fn a_sign_out_the_agent_does_not_offer_is_unverified() {
    let mut acp = Acp::new(PROFILE);
    let _ = acp.spoken(Account::SignOut).unwrap();
    let without = response(0, &json!({"protocolVersion": 1, "agentCapabilities": {}}));

    let told = acp.told(Account::SignOut, &without).unwrap();

    assert!(matches!(told, Told::Done(Reading::Unreadable(_))), "{told:?}");
}
