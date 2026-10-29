//! The client holds an agent to its profile and to what it was offered.

use std::path::Path;

use crucible_execution::{
    Cost, Decision, Detail, Event, Failure, Lasting, Outcome, Pending, SessionFate, TurnRequest,
};
use crucible_types::TurnId;
use serde_json::{Value, json};

use crate::acp::{Acp, Allowing, Initialized, Profile, SignIn};
use crate::catalog::Admits;
use crate::{Account, Act, Dialect, Reading, Shape, Told};

/// Admits every agent.
#[expect(clippy::unnecessary_wraps, reason = "a profile's check has this shape")]
const fn anyone(_: &Initialized, _: &[Admits]) -> Result<(), Failure> {
    Ok(())
}

/// An agent that speaks version 1 and signs in with `login`.
const PROFILE: Profile = Profile {
    program: "agent",
    versions: &[1],
    admits: anyone,
    sign_in: &[SignIn::named("login")],
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

/// A sign-in the agent answers as done is read as a status step reads it, by
/// opening a session on the same start: signed in only where one opens.
#[test]
fn a_sign_in_reads_signed_in_only_once_a_session_opens_after_it() {
    for (answer, signed) in [
        (
            json!({"jsonrpc": "2.0", "id": 2, "error": {"code": -32000, "message": "Authentication required"}}).to_string(),
            crucible_execution::Authentication::SignedOut,
        ),
        (response(2, &json!({"sessionId": "probe"})), crucible_execution::Authentication::SignedIn),
    ] {
        let mut acp = Acp::new(PROFILE);
        let _ = acp.account(Account::SignIn, Shape { workspace: Some(Path::new("/work")), ..Shape::default() });
        let _ = acp.spoken(Account::SignIn).unwrap();
        let asked = acp.told(Account::SignIn, &initialized(1)).unwrap();
        assert!(matches!(&asked, Told::Send(frame) if frame.contains("\"authenticate\"")), "{asked:?}");

        let took = acp.told(Account::SignIn, &response(1, &json!({}))).unwrap();

        assert!(matches!(&took, Told::Send(frame) if frame.contains("session/new")), "{took:?}");
        assert_eq!(
            acp.told(Account::SignIn, &answer).unwrap(),
            Told::Done(Reading::Signed(signed))
        );
    }
}

/// A sign-in asks for the first of the profile's methods the agent offers, in
/// the profile's order whatever the agent's, and for none where the agent
/// offers none of them.
#[test]
fn a_sign_in_asks_for_the_first_of_its_methods_the_agent_offers() {
    const METHODS: Profile = Profile {
        sign_in: &[
            SignIn::named("browser"),
            SignIn::named("login"),
            SignIn::named("key"),
        ],
        ..PROFILE
    };
    let offering = |methods: &[&str]| {
        let methods: Vec<Value> = methods
            .iter()
            .map(|id| json!({"id": id, "name": id}))
            .collect();
        response(
            0,
            &json!({"protocolVersion": 1, "agentCapabilities": {}, "authMethods": methods}),
        )
    };
    for (offered, asked) in [
        (vec!["login"], Some("login")),
        (vec!["key", "login", "browser"], Some("browser")),
        (vec!["other"], None),
    ] {
        let mut acp = Acp::new(METHODS);
        let _ = acp.spoken(Account::SignIn).unwrap();

        let told = acp.told(Account::SignIn, &offering(&offered)).unwrap();

        match (told, asked) {
            (Told::Send(frame), Some(method)) => {
                let sent: Value = serde_json::from_str(&frame).unwrap();
                assert_eq!(sent.pointer("/method"), Some(&json!("authenticate")));
                assert_eq!(
                    sent.pointer("/params/methodId"),
                    Some(&json!(method)),
                    "{offered:?}"
                );
            }
            (Told::Done(Reading::Unreadable(_)), None) => {}
            (told, asked) => panic!("{offered:?}: {told:?}, not {asked:?}"),
        }
    }
}

/// A sign-in the user named a method for asks for that method alone, even one
/// the profile lists later or not at all, and for none where the agent does
/// not offer it.
#[test]
fn a_sign_in_by_a_named_method_asks_for_that_method_alone() {
    const METHODS: Profile = Profile {
        sign_in: &[SignIn::named("browser"), SignIn::named("key")],
        ..PROFILE
    };
    let offering = |methods: &[&str]| {
        let methods: Vec<Value> = methods
            .iter()
            .map(|id| json!({"id": id, "name": id}))
            .collect();
        response(
            0,
            &json!({"protocolVersion": 1, "agentCapabilities": {}, "authMethods": methods}),
        )
    };
    for (named, offered, asked) in [
        ("key", vec!["browser", "key"], Some("key")),
        ("business", vec!["browser", "business"], Some("business")),
        ("key", vec!["browser"], None),
    ] {
        let mut acp = Acp::new(METHODS);
        assert!(Dialect::signing_in_with(&mut acp, named));
        let _ = acp.spoken(Account::SignIn).unwrap();

        let told = acp.told(Account::SignIn, &offering(&offered)).unwrap();

        match (told, asked) {
            (Told::Send(frame), Some(method)) => {
                let sent: Value = serde_json::from_str(&frame).unwrap();
                assert_eq!(sent.pointer("/method"), Some(&json!("authenticate")));
                assert_eq!(
                    sent.pointer("/params/methodId"),
                    Some(&json!(method)),
                    "{named:?} of {offered:?}"
                );
            }
            (Told::Done(Reading::Unreadable(_)), None) => {}
            (told, asked) => panic!("{named:?} of {offered:?}: {told:?}, not {asked:?}"),
        }
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
        (Allowing::Edits, "read", None),
        (Allowing::Edits, "search", None),
        (Allowing::Edits, "other", None),
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

/// Once the turn is told to stop, even full access answers nothing yes by
/// itself: the question is left to be answered no.
#[test]
fn a_permission_asked_after_a_stop_is_never_answered_yes() {
    let mut acp = prompting_allowing(Allowing::Everything);
    let _ = acp.cancel();
    let acts = acp
        .heard(&permission("execute", &[("yes", "allow_once")]))
        .unwrap();

    assert!(matches!(acts.as_slice(), [Act::Ask { .. }]), "{acts:?}");
}

/// What a signed catalog entry reviewed as calling the agent `name` at 1.0.0
/// makes of it, where the entry names `profile_env` as the variable the agent
/// reads its home from.
#[cfg(feature = "proof")]
fn profiled(name: &str, profile_env: Option<&str>) -> crate::catalog::CatalogProfile {
    use std::time::SystemTime;

    use crate::catalog::{Catalog, CatalogProfile, Held, fixture};

    let file = json!({"path": "peer", "size": 1, "sha256": "b".repeat(64), "exec": true});
    let mut entry = json!({
            "key": "peer",
            "registry_id": "fake-peer",
            "version": "1.0.0",
            "kind": "binary",
            "license": "MIT",
            "review": "https://example.com/review/1",
            "admits": {"name": name, "version": "1.0.0"},
            "auth_methods": [{"id": "login", "name": "Log in", "kind": "agent"}],
            "targets": [{
                "platform": "any",
                "cmd": "peer",
                "artifacts": [{
                    "url": "https://example.com/peer.zip",
                    "sha256": "a".repeat(64),
                    "size": 1,
                    "format": "zip",
                    "unpacked": 1,
                    "decompressed": 1,
                    "files": [file],
                }],
            }],
    });
    if let (Some(profile_env), Some(entry)) = (profile_env, entry.as_object_mut()) {
        entry.insert("profile_env".to_owned(), json!(profile_env));
    }
    let catalog = json!({"schema": 1, "serial": 1, "revoked": [], "entries": [entry]});
    let signed = fixture::signed(catalog.to_string().as_bytes());
    let catalog = Catalog::verify(
        &signed.index,
        &signed.signature,
        &signed.keys,
        Held(0),
        SystemTime::now(),
    )
    .unwrap();
    let entry = catalog.entries().first().unwrap();
    CatalogProfile::from_entry(entry, entry.targets.first().unwrap()).unwrap()
}

/// The client of the agent a signed catalog entry reviewed as calling itself
/// `name` at 1.0.0, working in this directory, its first turn opened.
#[cfg(feature = "proof")]
fn cataloged(name: &str) -> Acp {
    let mut acp = Acp::cataloged(profiled(name, None), Allowing::Nothing);
    let _ = acp.request(Shape {
        workspace: Some(Path::new("/work")),
        ..Shape::default()
    });
    let _ = acp.open(&TurnRequest::new(TurnId::FIRST, "hello")).unwrap();
    acp
}

/// A client run from a catalog entry that names the variable its agent reads
/// its home from gives the agent the profile directory the host asked for
/// through that variable, and never the shell's own value of it.
#[cfg(feature = "proof")]
#[test]
fn a_catalog_client_passes_the_profile_directory_through_the_variable_its_entry_names() {
    let mut acp = Acp::cataloged(profiled("fake-peer", Some("HOME")), Allowing::Nothing);
    let profile = Path::new("/profiles/work");

    let request = acp.request(Shape {
        profile: Some(profile),
        workspace: Some(Path::new("/work")),
        ..Shape::default()
    });

    let homes: Vec<&str> = request
        .environment
        .iter()
        .filter(|(name, _)| name == "HOME")
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(homes, ["/profiles/work"], "{:?}", request.shown);
    assert!(!request.shown.contains(&"HOME"), "{:?}", request.shown);
    if let Some(shell) = std::env::var_os("HOME").and_then(|home| home.into_string().ok()) {
        assert!(
            request.environment.iter().all(|(_, value)| *value != shell),
            "the shell's home was passed on"
        );
    }
}

/// A client run from a catalog entry admits the agent the entry names, as the
/// fake peer answers initializing, and sends nothing more to one that calls
/// itself anything else.
#[cfg(feature = "proof")]
#[test]
fn a_catalog_profile_admits_the_agent_its_entry_names_and_no_other() {
    use crate::fake_peer::scripts::acp;

    let mut admitting = cataloged("fake-peer");
    let admitted = admitting
        .heard(&acp::initialized("fake-peer", "1.0.0", "login"))
        .unwrap();
    let mut refusing = cataloged("fake-peer");
    let refused = refusing
        .heard(&acp::initialized("another-agent", "1.0.0", "login"))
        .unwrap();

    let [Act::Send(opened)] = admitted.as_slice() else {
        panic!("{admitted:?}")
    };
    let opened: Value = serde_json::from_str(opened).unwrap();
    assert_eq!(opened.pointer("/method"), Some(&json!("session/new")));
    let [Act::Fail(failure)] = refused.as_slice() else {
        panic!("{refused:?}")
    };
    let said = failure.reason().as_str();
    assert!(
        said.contains("fake-peer") && said.contains("another-agent"),
        "{said}"
    );
}

/// A client run from a catalog entry names its program by the agent's key, as
/// the backend it runs under is described by.
#[cfg(feature = "proof")]
#[test]
fn a_catalog_profile_names_its_program_by_the_agent_s_key() {
    assert_eq!(cataloged("fake-peer").program(), "peer");
}

/// A client run from a catalog entry sends nothing more to the agent its
/// entry names once it answers at a version the entry did not review.
#[cfg(feature = "proof")]
#[test]
fn a_catalog_profile_refuses_its_agent_at_a_version_its_entry_did_not_review() {
    use crate::fake_peer::scripts::acp;

    let refused = cataloged("fake-peer")
        .heard(&acp::initialized("fake-peer", "1.0.1", "login"))
        .unwrap();

    let [Act::Fail(failure)] = refused.as_slice() else {
        panic!("{refused:?}")
    };
    let said = failure.reason().as_str();
    assert!(said.contains("1.0.1") && said.contains("1.0.0"), "{said}");
}

/// Asked to load a session it kept, an agent that has none by that name ends
/// the turn saying the session is gone; refused for want of a sign-in, or
/// refused a new session the same way, the session is not said to be gone.
#[test]
fn only_a_kept_session_the_agent_cannot_find_is_said_to_be_gone() {
    let not_found = json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32002, "message": "Resource not found"}}).to_string();
    let sign_in = json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": "Authentication required"}}).to_string();
    for (resume, method, refused, fate) in [
        (Some("kept"), "session/load", &not_found, SessionFate::Gone),
        (Some("kept"), "session/load", &sign_in, SessionFate::Kept),
        (None, "session/new", &not_found, SessionFate::Kept),
    ] {
        let mut acp = Acp::new(PROFILE);
        let _ = acp.request(Shape {
            workspace: Some(Path::new("/work")),
            resume,
            ..Shape::default()
        });
        let _ = acp.open(&TurnRequest::new(TurnId::FIRST, "hello")).unwrap();
        let opening = acp.heard(&initialized(1)).unwrap();
        let [Act::Send(open)] = opening.as_slice() else {
            panic!("{opening:?}")
        };
        assert!(open.contains(method), "{open}");

        let acts = acp.heard(refused).unwrap();

        let Some(Act::Fail(failure)) = acts.last() else {
            panic!("{acts:?}")
        };
        assert_eq!(failure.fate(), fate, "{method}: {refused}");
    }
}
