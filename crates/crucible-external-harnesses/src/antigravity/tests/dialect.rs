//! The agent is started in its profile, and a request to sign in is never
//! mistaken for an answer.

use std::path::Path;

use crucible_execution::{Authentication, TurnRequest};
use crucible_types::{AgentKey, TurnId};
use serde_json::{Value, json};

use crate::acp::Allowing;
use crate::antigravity::Antigravity;
use crate::antigravity::account::LINK_PREFIX;
use crate::catalog::{Admits, Artifact, File, FileKind, Files, Format, Launch, Target};
use crate::release::OwnedPin;
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

/// Every variable the agent is started with, for a turn or a step, says
/// nothing about an account, so its value may be shown where what the agent
/// says is kept.
#[test]
fn every_variable_the_agent_is_started_with_may_be_shown() {
    let shape = || Shape {
        profile: Some(Path::new("/profiles/work")),
        ..Shape::default()
    };
    let mut agent = Antigravity::default();
    let requests = [
        Some(agent.request(shape())),
        agent.account(Account::Status, shape()),
    ];

    for request in requests {
        let request = request.expect("a request");
        let names = request
            .environment
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"GEMINI_HOME"), "{names:?}");
        assert_eq!(request.shown, names);
    }
}

/// The agent printing its sign-in link during a turn means the profile is
/// signed out, and the turn fails saying how to sign in.
#[test]
fn a_sign_in_link_during_a_turn_fails_it() {
    let mut agent = Antigravity::default();
    let _ = agent.request(Shape::default());
    let _ = agent
        .open(&TurnRequest::new(TurnId::FIRST, "hello"))
        .unwrap();

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
    assert!(matches!(
        agent.told(Account::SignIn, &printed),
        Ok(Told::Link(_))
    ));

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

/// Once the agent says a sign-in took, a link it prints while the session
/// after it is being opened reads as signed out, as a status step reads it,
/// and is never shown as a second sign-in.
#[test]
fn a_sign_in_link_after_the_agent_said_it_signed_in_reads_signed_out() {
    let printed = format!("{LINK_PREFIX}{SIGN_IN}");
    let mut agent = Antigravity::allowing(Allowing::Nothing, &[offering("1.3.0", &[])]);
    let _ = agent.account(
        Account::SignIn,
        Shape {
            workspace: Some(Path::new("/work")),
            ..Shape::default()
        },
    );
    assert!(agent.signing_in_with("oauth-personal"));
    let _ = agent.spoken(Account::SignIn).unwrap();
    let asked = agent.told(Account::SignIn, &answered_as("1.3.0")).unwrap();
    assert!(
        matches!(&asked, Told::Send(frame) if frame.contains("\"authenticate\"")),
        "{asked:?}"
    );
    let took = json!({"jsonrpc": "2.0", "id": 1, "result": {}}).to_string();
    let took = agent.told(Account::SignIn, &took).unwrap();
    assert!(
        matches!(&took, Told::Send(frame) if frame.contains("session/new")),
        "{took:?}"
    );

    assert_eq!(
        agent.told(Account::SignIn, &printed).unwrap(),
        Told::Done(Reading::Signed(Authentication::SignedOut))
    );
}

/// A sign-in link during a sign-out or a version check says nothing either
/// step asked, so neither is read from it.
#[test]
fn a_sign_in_link_during_a_sign_out_or_a_version_check_is_unreadable() {
    let printed = format!("{LINK_PREFIX}{SIGN_IN}");
    for step in [Account::SignOut, Account::Version] {
        let mut agent = Antigravity::default();
        let _ = agent.spoken(step).unwrap();
        let told = agent.told(step, &printed);
        assert!(
            matches!(told, Ok(Told::Done(Reading::Unreadable(_)))),
            "{step:?}: {told:?}"
        );
    }
}

/// The release `version` of the agent as the catalog offers it, started with
/// `args`.
fn offering(version: &str, args: &[&str]) -> OwnedPin {
    let file = |path: &str| File {
        path: path.into(),
        kind: FileKind::File {
            size: 1,
            sha256: Box::default(),
            exec: true,
        },
    };
    OwnedPin {
        key: AgentKey::new("antigravity").unwrap(),
        version: version.into(),
        lock_revision: None,
        admits: Admits {
            name: "antigravity-acp".into(),
            version: version.into(),
        },
        target: Target {
            platform: "linux-x86_64".into(),
            cmd: "agent".into(),
            args: args.iter().map(|arg| (*arg).into()).collect(),
            env: Vec::new(),
            hosts: Vec::new(),
            artifacts: vec![Artifact {
                url: "https://example.invalid/agent.zip".into(),
                mirror: None,
                sha256: "ab".repeat(32).into(),
                size: 2,
                format: Format::Zip,
                unpacked: 2,
                decompressed: 2,
                files: Files::Listed(vec![file("agent"), file("harness")]),
                kept: Vec::new(),
                replaced_by: None,
            }],
            launch: Launch::Binary,
        },
    }
}

/// The agent's answer to initializing as release `version`, in a frame.
fn answered_as(version: &str) -> String {
    let result: Value = json!({
        "protocolVersion": 1,
        "agentInfo": {"name": "antigravity-acp", "version": version},
        "agentCapabilities": {
            "loadSession": true,
            "sessionCapabilities": {"resume": {}},
            "auth": {"logout": {}},
        },
        "authMethods": [{"id": "oauth-personal"}],
    });
    json!({"jsonrpc": "2.0", "id": 0, "result": result}).to_string()
}

/// A turn opened on `agent`, which sends its first request.
fn opened(mut agent: Antigravity) -> Antigravity {
    let _ = agent.request(Shape {
        workspace: Some(Path::new("/work")),
        ..Shape::default()
    });
    let _ = agent
        .open(&TurnRequest::new(TurnId::FIRST, "hello"))
        .unwrap();
    agent
}

/// What starts the agent is the target of the release the catalog offers, not
/// a rule about the platform: the arguments it gives are the ones it was
/// reviewed with, and none where nothing is offered.
#[test]
fn the_agent_is_started_with_the_arguments_of_the_release_offered() {
    let started = |offered: &[OwnedPin]| {
        Antigravity::allowing(Allowing::Nothing, offered)
            .request(Shape::default())
            .arguments
    };

    assert_eq!(
        started(&[
            offering("1.4.1", &["--uid=", "--quiet"]),
            offering("1.3.0", &[])
        ]),
        ["--uid=", "--quiet"]
    );
    assert_eq!(started(&[offering("1.4.1", &[])]), Vec::<String>::new());
    assert_eq!(started(&[]), Vec::<String>::new());
}

/// The agent is run as any release the catalog offers, newest or not, and as
/// no other: one that answers as a version it does not offer is sent nothing
/// more, naming the ones it does.
#[test]
fn the_agent_is_admitted_as_any_release_the_catalog_offers_and_no_other() {
    let offers = [offering("1.4.1", &[]), offering("1.3.0", &[])];
    for version in ["1.4.1", "1.3.0"] {
        let mut agent = opened(Antigravity::allowing(Allowing::Nothing, &offers));

        let acts = agent.heard(&answered_as(version)).unwrap();

        assert!(
            matches!(acts.as_slice(), [Act::Send(_)]),
            "{version}: {acts:?}"
        );
    }
    let mut agent = opened(Antigravity::allowing(Allowing::Nothing, &offers));

    let refused = agent.heard(&answered_as("1.2.0")).unwrap();

    let [Act::Fail(failure)] = refused.as_slice() else {
        panic!("{refused:?}")
    };
    let said = failure.reason().as_str();
    assert!(
        said.contains("1.2.0") && said.contains("1.4.1") && said.contains("1.3.0"),
        "{said}"
    );
}

/// With nothing offered, as when the catalog's trust state cannot be used or
/// every release was revoked, no release is admitted.
#[test]
fn nothing_is_admitted_while_nothing_is_offered() {
    let mut agent = opened(Antigravity::allowing(Allowing::Nothing, &[]));

    let refused = agent.heard(&answered_as("1.3.0")).unwrap();

    assert!(matches!(refused.as_slice(), [Act::Fail(_)]), "{refused:?}");
}

/// The program under `programs/antigravity/<version>/`, as an install puts it.
fn kept_at(version: &str) -> std::path::PathBuf {
    Path::new("/home/me/.crucible/programs/antigravity")
        .join(version)
        .join("agy_acp_server.par")
}

/// What the agent is admitted as and started with is the release whose
/// directory its program is in, not the newest one offered: an older install
/// is started with its own arguments, and a version not offered is none.
#[test]
fn the_release_started_is_the_one_its_directory_names() {
    let offers = || vec![offering("1.4.1", &["--new"]), offering("1.3.0", &[])];
    let wire = |program: Option<&Path>| {
        Antigravity::allowing(
            Allowing::Nothing,
            &crate::antigravity::started(offers(), program),
        )
    };

    let older = wire(Some(&kept_at("1.3.0")))
        .request(Shape::default())
        .arguments;
    let newer = wire(Some(&kept_at("1.4.1")))
        .request(Shape::default())
        .arguments;
    let by_hand = wire(Some(Path::new("/usr/bin/antigravity")))
        .request(Shape::default())
        .arguments;

    assert!(older.is_empty(), "{older:?}");
    assert_eq!(newer, ["--new"]);
    assert_eq!(
        by_hand,
        ["--new"],
        "a program kept elsewhere is any offered release"
    );
    let mut other = opened(wire(Some(&kept_at("1.3.0"))));
    let acts = other.heard(&answered_as("1.4.1")).unwrap();
    assert!(matches!(acts.as_slice(), [Act::Fail(_)]), "{acts:?}");
    let mut missing = opened(wire(Some(&kept_at("1.2.0"))));
    let acts = missing.heard(&answered_as("1.2.0")).unwrap();
    assert!(matches!(acts.as_slice(), [Act::Fail(_)]), "{acts:?}");
    let mut own = opened(wire(Some(&kept_at("1.4.1"))));
    let acts = own.heard(&answered_as("1.4.1")).unwrap();
    assert!(matches!(acts.as_slice(), [Act::Send(_)]), "{acts:?}");
}

/// A program kept in the directory of a version the catalog does not offer is
/// told apart from nothing being offered, and named by that version.
#[test]
fn a_program_kept_as_a_version_not_offered_is_named() {
    let offers = || vec![offering("1.4.1", &[]), offering("1.3.0", &[])];

    let refused = crate::antigravity::unoffered(&offers(), Some(&kept_at("1.2.0")));

    assert_eq!(refused.as_deref(), Some("1.2.0"));
    assert_eq!(
        crate::antigravity::unoffered(&offers(), Some(&kept_at("1.3.0"))),
        None
    );
    assert_eq!(
        crate::antigravity::unoffered(&offers(), Some(Path::new("/usr/bin/antigravity"))),
        None
    );
    assert_eq!(
        crate::antigravity::unoffered(&[], Some(&kept_at("1.2.0"))),
        None
    );
}
