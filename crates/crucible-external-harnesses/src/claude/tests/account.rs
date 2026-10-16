//! Each account step reads only what the agent's own command established.

use std::path::Path;

use crucible_execution::{Authentication, Text};

use crate::claude::account::quoted_for_powershell;
use crate::claude::{Claude, sign_in};
use crate::{Account, Dialect, OFFERED, Offered, Ran, Reading, Shape, Told};

/// A run that exited with `code` having printed `said`.
fn ran(code: Option<i32>, said: &[&str]) -> Ran {
    Ran {
        code,
        said: said.iter().map(|line| (*line).to_owned()).collect(),
        cut: false,
        muttered: String::new(),
    }
}

#[test]
fn the_steps_run_the_agents_own_commands() {
    let mut agent = Claude::default();
    let arguments = |agent: &mut Claude, step| {
        agent
            .account(step, Shape::default())
            .map(|request| request.arguments)
    };

    assert_eq!(
        arguments(&mut agent, Account::Version),
        Some(vec!["--version".to_owned()])
    );
    assert_eq!(
        arguments(&mut agent, Account::Status),
        Some(vec!["auth".to_owned(), "status".to_owned()])
    );
    assert_eq!(
        arguments(&mut agent, Account::SignOut),
        Some(vec!["auth".to_owned(), "logout".to_owned()])
    );
    assert_eq!(arguments(&mut agent, Account::SignIn), None);
}

/// A step runs in the profile, as a turn does.
#[test]
fn a_step_runs_in_the_profile() {
    let request = Claude::default()
        .account(
            Account::Status,
            Shape {
                profile: Some(Path::new("/profiles/work")),
                ..Shape::default()
            },
        )
        .unwrap();

    assert!(
        request
            .environment
            .contains(&("CLAUDE_CONFIG_DIR".to_owned(), "/profiles/work".to_owned()))
    );
}

/// The status is read only where what it printed and how it exited agree.
#[test]
fn the_status_is_read_only_where_its_words_and_its_exit_agree() {
    let agent = Claude::default();
    let read = |code, said: &[&str]| agent.read(Account::Status, &ran(code, said));

    assert_eq!(
        read(
            Some(0),
            &[r#"{"loggedIn": true, "authMethod": "claude.ai"}"#]
        ),
        Reading::Signed(Authentication::SignedIn)
    );
    assert_eq!(
        read(Some(1), &["{", r#"  "loggedIn": false"#, "}"]),
        Reading::Signed(Authentication::SignedOut)
    );
    for (code, said) in [
        (Some(1), r#"{"loggedIn": true}"#),
        (Some(0), r#"{"loggedIn": false}"#),
        (Some(0), "Logged in"),
        (Some(0), r#"{"loggedIn": "yes"}"#),
        (None, r#"{"loggedIn": true}"#),
    ] {
        assert!(
            matches!(read(code, &[said]), Reading::Unreadable(_)),
            "{code:?} {said}"
        );
    }
}

/// A run whose output was cut is read as nothing.
#[test]
fn a_cut_run_is_unreadable() {
    let mut cut = ran(Some(0), &[r#"{"loggedIn": true}"#]);
    cut.cut = true;

    assert!(matches!(
        Claude::default().read(Account::Status, &cut),
        Reading::Unreadable(_)
    ));
}

#[test]
fn the_version_is_read_against_the_manifest() {
    let agent = Claude::default();

    assert!(matches!(
        agent.read(Account::Version, &ran(Some(0), &["2.1.296 (Claude Code)"])),
        Reading::Version(version) if version.as_str() == "2.1.296"
    ));
    assert!(matches!(
        agent.read(Account::Version, &ran(Some(0), &["1.0.0 (Claude Code)"])),
        Reading::Unreadable(_)
    ));
    assert!(matches!(
        agent.read(Account::Version, &ran(Some(1), &["2.1.296 (Claude Code)"])),
        Reading::Unreadable(_)
    ));
}

#[test]
fn a_sign_out_is_read_from_its_exit() {
    let agent = Claude::default();

    assert_eq!(
        agent.read(Account::SignOut, &ran(Some(0), &[])),
        Reading::Signed(Authentication::SignedOut)
    );
    assert!(matches!(
        agent.read(Account::SignOut, &ran(Some(1), &[])),
        Reading::Unreadable(_)
    ));
}

/// The command shown for signing in names the profile, quoted so a path
/// with a quote in it is still one word.
#[test]
fn the_sign_in_command_names_the_profile() {
    assert_eq!(sign_in(None).as_str(), "claude auth login");
    if cfg!(windows) {
        assert_eq!(
            sign_in(Some(Path::new(r"C:\p\o'k"))).as_str(),
            r"$env:CLAUDE_CONFIG_DIR='C:\p\o''k'; claude auth login"
        );
    } else {
        assert_eq!(
            sign_in(Some(Path::new("/p/o'k"))).as_str(),
            r"CLAUDE_CONFIG_DIR='/p/o'\''k' claude auth login"
        );
    }
}

/// Every character PowerShell reads as a single quote is doubled, so a
/// profile path cannot end the quoted string early.
#[test]
fn a_powershell_path_doubles_every_single_quote() {
    assert_eq!(
        quoted_for_powershell("C:\\p\\o'k\u{2018}a\u{2019}b\u{201A}c\u{201B}d"),
        "'C:\\p\\o''k\u{2018}\u{2018}a\u{2019}\u{2019}b\u{201A}\u{201A}c\u{201B}\u{201B}d'"
    );
}

/// A status that could not be read says how the agent exited and the first
/// thing it said, its standard error before its output.
#[test]
fn an_unread_status_says_how_the_agent_exited_and_what_it_said() {
    let agent = Claude::default();
    let reason = |ran: &Ran| match agent.read(Account::Status, ran) {
        Reading::Unreadable(failure) => failure.reason().as_str().to_owned(),
        other => panic!("read as {other:?}"),
    };
    let mut muttered = ran(Some(2), &["ignored"]);
    muttered.muttered = "\n status unavailable \nretry later".to_owned();

    assert_eq!(
        reason(&muttered),
        "The agent exited with status 2: status unavailable"
    );
    assert_eq!(
        reason(&ran(Some(3), &["", "  not configured  "])),
        "The agent exited with status 3: not configured"
    );
    assert_eq!(
        reason(&ran(Some(4), &[])),
        "The agent exited with status 4."
    );
    assert_eq!(
        reason(&ran(Some(0), &["Logged in"])),
        "The agent's sign-in status could not be read."
    );
    assert_eq!(
        reason(&ran(None, &["stopped"])),
        "The agent's sign-in status could not be read."
    );
}

/// What the agent answers `initialize` with, cut to what the models step
/// reads: its model rows, beside fields the step leaves alone.
fn initialized(id: &str, models: &serde_json::Value) -> String {
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": id,
            "response": {"commands": [], "models": models, "pid": 7},
        },
    })
    .to_string()
}

/// The `initialize` request the models step opens with, and its id.
fn opened(agent: &mut Claude) -> (serde_json::Value, String) {
    let frames = agent
        .spoken(Account::Models)
        .expect("the models step is spoken");
    let [frame] = frames.as_slice() else {
        panic!("one frame: {frames:?}");
    };
    let frame: serde_json::Value = serde_json::from_str(frame).expect("a JSON frame");
    let id = frame
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .expect("an id")
        .to_owned();
    (frame, id)
}

/// The models step starts the agent on its stream wire, with no turn and no
/// model of its own, and asks it to initialize.
#[test]
fn the_models_step_initializes_the_agent_on_its_stream() {
    let mut agent = Claude::default();
    let request = agent
        .account(
            Account::Models,
            Shape {
                model: Some("opus"),
                ..Shape::default()
            },
        )
        .expect("a models step");
    let (frame, _) = opened(&mut agent);

    assert_eq!(
        request.arguments,
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ]
    );
    assert_eq!(
        frame.get("type").and_then(serde_json::Value::as_str),
        Some("control_request")
    );
    assert_eq!(
        frame
            .pointer("/request/subtype")
            .and_then(serde_json::Value::as_str),
        Some("initialize")
    );
}

/// Each row the agent offers is read as the value it is launched with and
/// the name it shows, its default as the agent's own choice.
#[test]
fn the_models_step_reads_each_row_the_agent_offers() {
    let mut agent = Claude::default();
    let (_, id) = opened(&mut agent);
    let models = serde_json::json!([
        {"value": "default", "displayName": "Default (recommended)", "resolvedModel": "claude-opus-5-5"},
        {"value": "opus", "displayName": "Opus", "resolvedModel": "claude-opus-5-5"},
        {"value": "haiku", "displayName": "Haiku"},
    ]);

    let told = agent
        .told(Account::Models, &initialized(&id, &models))
        .expect("a frame of this wire");

    assert_eq!(
        told,
        Told::Done(Reading::Models(vec![
            Offered {
                model: None,
                shown: Text::new("Default (recommended)"),
            },
            Offered {
                model: Some(Text::new("opus")),
                shown: Text::new("Opus"),
            },
            Offered {
                model: Some(Text::new("haiku")),
                shown: Text::new("Haiku"),
            },
        ]))
    );
}

/// What the agent says before its answer is waited past; an answer to
/// another request is not this one.
#[test]
fn the_models_step_waits_past_what_is_not_its_answer() {
    let mut agent = Claude::default();
    let (_, id) = opened(&mut agent);
    let rows = serde_json::json!([{"value": "opus", "displayName": "Opus"}]);

    for frame in [
        serde_json::json!({"type": "system", "subtype": "init"}).to_string(),
        initialized("someone-else", &rows),
    ] {
        assert_eq!(
            agent.told(Account::Models, &frame).expect("a frame"),
            Told::Nothing,
            "{frame}"
        );
    }
    assert!(matches!(
        agent.told(Account::Models, &initialized(&id, &rows)),
        Ok(Told::Done(Reading::Models(_)))
    ));
}

/// An answer that lists no models, a row with no value or name, or a list
/// longer than is kept, is unread rather than read in part.
#[test]
fn a_models_answer_that_cannot_be_read_whole_is_unread() {
    let many: Vec<_> = (0..=OFFERED)
        .map(|row| serde_json::json!({"value": format!("m{row}"), "displayName": "M"}))
        .collect();
    for models in [
        serde_json::Value::Null,
        serde_json::json!([{"displayName": "Opus"}]),
        serde_json::json!([{"value": "opus"}]),
        serde_json::json!([{"value": 7, "displayName": "Opus"}]),
        serde_json::Value::from(many),
    ] {
        let mut agent = Claude::default();
        let (_, id) = opened(&mut agent);
        assert!(
            matches!(
                agent.told(Account::Models, &initialized(&id, &models)),
                Ok(Told::Done(Reading::Unreadable(_)))
            ),
            "{models}"
        );
    }
}

/// A refused `initialize` is unread, saying the agent refused it.
#[test]
fn a_refused_initialize_is_unread() {
    let mut agent = Claude::default();
    let (_, id) = opened(&mut agent);
    let refused = serde_json::json!({
        "type": "control_response",
        "response": {"subtype": "error", "request_id": id, "error": "no"},
    })
    .to_string();

    assert!(matches!(
        agent.told(Account::Models, &refused),
        Ok(Told::Done(Reading::Unreadable(_)))
    ));
}
