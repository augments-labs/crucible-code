//! The client holds an agent to its profile and to what it was offered.

use std::path::Path;

use crucible_execution::{
    Cost, Decision, Detail, Event, Failure, Lasting, Outcome, Pending, TurnRequest,
};
use crucible_types::TurnId;
use serde_json::{Value, json};

use crate::acp::{Acp, Allowing, Initialized, Profile};
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

    let [first] = frames.as_slice() else {
        panic!("{frames:?}")
    };
    let initialize: Value = serde_json::from_str(first).unwrap();
    assert_eq!(initialize.pointer("/method"), Some(&json!("initialize")));
    let terminal = initialize.pointer("/params/clientCapabilities/terminal");
    assert_eq!(terminal, Some(&json!(false)));
    let [Act::Send(new)] = opening.as_slice() else {
        panic!("{opening:?}")
    };
    let new: Value = serde_json::from_str(new).unwrap();
    assert_eq!(new.pointer("/method"), Some(&json!("session/new")));
    assert_eq!(new.pointer("/params/cwd"), Some(&json!("/work")));
    let [Act::Session(session), Act::Send(prompt)] = prompted.as_slice() else {
        panic!("{prompted:?}")
    };
    assert_eq!(session, "s");
    let prompt: Value = serde_json::from_str(prompt).unwrap();
    assert_eq!(prompt.pointer("/params/sessionId"), Some(&json!("s")));
    assert_eq!(
        prompt.pointer("/params/prompt/0/text"),
        Some(&json!("hello"))
    );
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
    let answer = acp
        .answer("\"q\"", &Decision::Allow(Lasting::Once))
        .unwrap();

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
        let acts = acp
            .heard(&response(2, &json!({"stopReason": reason})))
            .unwrap();
        assert_eq!(acts, vec![Act::End(outcome)], "{reason}");
    }
    let mut acp = prompting();
    assert!(
        acp.heard(&response(2, &json!({"stopReason": "bored"})))
            .is_err()
    );
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

    assert!(
        matches!(told, Told::Done(Reading::Unreadable(_))),
        "{told:?}"
    );
}

/// A turn stopped while the agent is still being initialized, or while its
/// session is being opened, sends nothing more and ends cancelled.
#[test]
fn a_turn_stopped_before_its_prompt_is_out_sends_nothing_more() {
    let (mut acp, _) = opened();
    let _ = acp.cancel();
    let answered = acp.heard(&initialized(1)).unwrap();
    assert_eq!(answered, vec![Act::End(Outcome::Cancelled)]);

    let (mut acp, _) = opened();
    let _ = acp.heard(&initialized(1)).unwrap();
    let _ = acp.cancel();
    let named = acp.heard(&response(1, &json!({"sessionId": "s"}))).unwrap();
    assert_eq!(
        named,
        vec![Act::Session("s".to_owned()), Act::End(Outcome::Cancelled)]
    );
}

/// A turn stopped while the agent is being initialized leaves it initialized,
/// so the next turn on the same connection opens its session without
/// initializing it again.
#[test]
fn a_turn_stopped_while_initializing_leaves_the_next_to_open_its_session() {
    let (mut acp, _) = opened();
    let _ = acp.cancel();
    let _ = acp.heard(&initialized(1)).unwrap();

    let frames = acp
        .open(&TurnRequest::new(TurnId::FIRST.next(), "again"))
        .unwrap();

    let [frame] = frames.as_slice() else {
        panic!("{frames:?}")
    };
    assert!(frame.contains("\"session/new\""), "{frame}");
}

/// An agent refused at initializing, whether or not its turn was stopped, is
/// initialized again by the next turn and never sent a session.
#[test]
fn an_agent_refused_at_initializing_is_initialized_again_by_the_next_turn() {
    for stopped in [false, true] {
        let (mut acp, _) = opened();
        if stopped {
            let _ = acp.cancel();
        }
        let _ = acp.heard(&initialized(2)).unwrap();

        let frames = acp
            .open(&TurnRequest::new(TurnId::FIRST.next(), "again"))
            .unwrap();

        let [frame] = frames.as_slice() else {
            panic!("{frames:?}")
        };
        assert!(
            frame.contains("\"initialize\""),
            "stopped {stopped}: {frame}"
        );
    }
}

/// A permission asked of no open session, or of another one, is refused
/// rather than asked of the user.
#[test]
fn a_permission_asked_outside_the_open_session_is_refused() {
    let ask = |session: &str| {
        json!({"jsonrpc": "2.0", "id": "q", "method": "session/request_permission", "params": {
            "sessionId": session,
            "toolCall": {"toolCallId": "t", "title": "ls", "kind": "execute", "rawInput": {"command": "ls"}},
            "options": [{"optionId": "yes", "name": "Yes", "kind": "allow_once"}],
        }})
        .to_string()
    };
    let mut acp = prompting();
    let elsewhere = acp.heard(&ask("other"));
    let (mut early, _) = opened();
    let unopened = early.heard(&ask("s"));

    assert!(elsewhere.is_err(), "{elsewhere:?}");
    assert!(unopened.is_err(), "{unopened:?}");
}

/// A permission is asked as what the agent would run or fetch, as its input
/// says, and never as the title it gave the call.
#[test]
fn a_permission_is_asked_as_its_input_never_its_title() {
    for (kind, input, said) in [
        ("execute", Some(json!({"cmd": "rm -rf /"})), "rm -rf /"),
        (
            "fetch",
            Some(json!({"address": "https://elsewhere.example"})),
            "https://elsewhere.example",
        ),
        ("execute", None, "The agent did not say what it would run."),
        (
            "execute",
            Some(json!(null)),
            "The agent did not say what it would run.",
        ),
        ("fetch", None, "The agent did not say what it would fetch."),
    ] {
        let mut call = json!({"toolCallId": "t", "title": "list the files", "kind": kind});
        if let (Some(input), Some(fields)) = (input, call.as_object_mut()) {
            fields.insert("rawInput".to_owned(), input);
        }
        let mut acp = prompting();
        let asked = acp
            .heard(
                &json!({"jsonrpc": "2.0", "id": "q", "method": "session/request_permission", "params": {
                    "sessionId": "s",
                    "toolCall": call,
                    "options": [{"optionId": "yes", "name": "Yes", "kind": "allow_once"}],
                }})
                .to_string(),
            )
            .unwrap();

        let [
            Act::Ask {
                pending: Pending::Permission { detail, .. },
                ..
            },
        ] = asked.as_slice()
        else {
            panic!("{asked:?}")
        };
        let sent = match detail {
            Detail::Command { sent, .. } | Detail::Network { sent } => sent.as_str(),
            other => panic!("{other:?}"),
        };
        assert!(sent.contains(said), "{kind}: {sent}");
        assert!(!sent.contains("list the files"), "{kind}: {sent}");
    }
}

/// Each turn of one run reports what it added to the session's cost.
#[test]
fn each_turn_of_a_run_reports_what_it_added_to_the_cost() {
    let usage = |amount: f64| {
        json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s", "update":
            {"sessionUpdate": "usage_update", "used": 1, "size": 2, "cost": {"amount": amount, "currency": "USD"}}}})
        .to_string()
    };
    let spent = |acts: &[Act]| {
        acts.iter().find_map(|act| match act {
            Act::Say(Event::Usage(observed)) => observed.value().spent().map(Cost::millionths),
            _ => None,
        })
    };
    let mut acp = prompting();
    let _ = acp.heard(&usage(0.25)).unwrap();
    let first = acp
        .heard(&response(2, &json!({"stopReason": "end_turn"})))
        .unwrap();
    let again = acp
        .open(&TurnRequest::new(TurnId::FIRST.next(), "again"))
        .unwrap();
    let _ = acp.heard(&usage(0.75)).unwrap();
    let second = acp
        .heard(&response(3, &json!({"stopReason": "end_turn"})))
        .unwrap();

    assert_eq!(again.len(), 1, "{again:?}");
    assert_eq!(spent(&first), Some(250_000));
    assert_eq!(spent(&second), Some(500_000));
}

/// A permission request of the open session for a call of `kind`, offering
/// `options`, each an id and its kind.
fn permission(kind: &str, options: &[(&str, &str)]) -> String {
    let options: Vec<Value> = options
        .iter()
        .map(|(id, kind)| json!({"optionId": id, "name": id, "kind": kind}))
        .collect();
    json!({"jsonrpc": "2.0", "id": "q", "method": "session/request_permission", "params": {
        "sessionId": "s",
        "toolCall": {"toolCallId": "t", "title": "t", "kind": kind, "rawInput": {"command": "ls"}},
        "options": options,
    }})
    .to_string()
}

/// A client allowing `allowing`, whose session `s` is open and whose first
/// prompt is out.
fn prompting_allowing(allowing: Allowing) -> Acp {
    let mut acp = Acp::allowing(PROFILE, allowing);
    let _ = acp.request(Shape {
        workspace: Some(Path::new("/work")),
        ..Shape::default()
    });
    let _ = acp.open(&TurnRequest::new(TurnId::FIRST, "hello")).unwrap();
    let _ = acp.heard(&initialized(1)).unwrap();
    let _ = acp.heard(&response(1, &json!({"sessionId": "s"}))).unwrap();
    acp
}

/// What a client allowing `allowing` does with a permission for a call of
/// `kind`, offering `options`: the option id it answered with by itself, or
/// `None` where it asked.
fn answered_by_itself(allowing: Allowing, kind: &str, options: &[(&str, &str)]) -> Option<String> {
    let mut acp = prompting_allowing(allowing);
    let acts = acp.heard(&permission(kind, options)).unwrap();
    match acts.as_slice() {
        [Act::Ask { .. }] => None,
        [Act::Send(frame)] => {
            let sent: Value = serde_json::from_str(frame).unwrap();
            assert_eq!(sent.pointer("/id"), Some(&json!("q")), "{frame}");
            sent.pointer("/result/outcome/optionId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }
        other => panic!("{other:?}"),
    }
}

/// A client answers yes by itself, with the one-time allow the agent offered,
/// to what its mode allows, and asks the person everything else.
#[test]
fn a_permission_the_mode_allows_is_answered_with_the_allow_once_offered() {
    let offered = [
        ("no", "reject_once"),
        ("yes", "allow_once"),
        ("always", "allow_always"),
    ];
    for (allowing, kind, answered) in [
        (Allowing::Nothing, "edit", None),
        (Allowing::Nothing, "execute", None),
        (Allowing::Edits, "edit", Some("yes")),
        (Allowing::Edits, "delete", Some("yes")),
        (Allowing::Edits, "move", Some("yes")),
        (Allowing::Edits, "execute", None),
        (Allowing::Edits, "fetch", None),
        (Allowing::Everything, "execute", Some("yes")),
        (Allowing::Everything, "fetch", Some("yes")),
        (Allowing::Everything, "edit", Some("yes")),
    ] {
        assert_eq!(
            answered_by_itself(allowing, kind, &offered).as_deref(),
            answered,
            "{allowing:?} {kind}"
        );
    }
}

/// Where the agent offers no one-time allow, even full access asks the
/// person rather than grant a lasting one.
#[test]
fn a_permission_with_no_allow_once_offered_is_asked_in_full_access() {
    let offered = [("no", "reject_once"), ("always", "allow_always")];
    assert_eq!(
        answered_by_itself(Allowing::Everything, "execute", &offered),
        None
    );
}
