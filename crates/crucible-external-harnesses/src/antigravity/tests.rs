//! What running Antigravity in a profile of its own guarantees, and what
//! running the user's own program does instead.

mod account;
mod dialect;
mod manifest;
mod profile;

use std::path::Path;

use crucible_types::AgentKey;

use crate::acp::Allowing;
use crate::antigravity::{AGENT, Antigravity, Manifest, own_note};
use crate::catalog::SNAPSHOT;
use crate::{Dialect, Shape};

/// The value `name` is started with in `request`, where it is set.
fn variable<'a>(request: &'a crate::Request, name: &str) -> Option<&'a str> {
    request
        .environment
        .iter()
        .find(|(set, _)| set == name)
        .map(|(_, value)| value.as_str())
}

/// The user's own `agy` is started as `agy acp`, keeps its token in a file and
/// is given no home or temporary directory, so it looks for its sign-in in
/// its usual home.
#[test]
fn an_own_agy_is_started_with_file_storage_and_no_gemini_home() {
    let request = Antigravity::own(Path::new("/opt/bin/agy"), Allowing::Nothing, &[])
        .request(Shape::default());

    assert_eq!(request.arguments, ["acp"]);
    assert_eq!(variable(&request, "AGY_ACP_FORCE_FILE_STORAGE"), Some("1"));
    for unset in ["GEMINI_HOME", "TMPDIR", "TEMP", "TMP"] {
        assert_eq!(variable(&request, unset), None, "{unset}");
    }
}

/// A program named as the release's own file is started as the release is,
/// with the newest offered release's arguments, wherever it lies.
#[test]
fn an_own_program_named_as_a_release_is_started_with_the_releases_arguments() {
    let offered = AgentKey::new(AGENT).map_or_else(|_| Vec::new(), |key| SNAPSHOT.releases(&key));
    let released: Vec<String> = offered.first().map_or_else(Vec::new, |newest| {
        newest.target.args.iter().map(ToString::to_string).collect()
    });

    for name in ["agy_acp_server.par", "agy_acp_server.exe", "antigravity"] {
        let program = Path::new("/opt/bin").join(name);

        let request =
            Antigravity::own(&program, Allowing::Nothing, &offered).request(Shape::default());

        assert_eq!(request.arguments, released, "{name}");
    }
}

/// The note shown before the user's own program is chosen says which sign-in
/// it uses: the one it keeps where another Google product may share it, which
/// it does not see under the sandbox.
#[test]
fn the_own_note_says_which_sign_in_holds_with_the_sandbox_on_and_off() {
    let off = own_note(false);
    let on = own_note(true);

    assert!(
        off.contains("~/.gemini") && off.contains("another Google product"),
        "{off}"
    );
    assert!(
        off.contains("store release") && off.contains("profileDir"),
        "{off}"
    );
    assert!(on.contains("~/.gemini") && on.contains("sandbox"), "{on}");
    assert!(on.contains("does not see"), "{on}");
    assert_ne!(off, on);
}

/// What Google's `agy acp` answers initializing with.
fn agy_answer() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": 1,
        "agentInfo": {"name": "Antigravity ACP Server", "version": "1.5.5"},
        "agentCapabilities": {
            "loadSession": true,
            "sessionCapabilities": {"resume": {}},
            "auth": {"logout": {}},
        },
        "authMethods": [{"id": "oauth-personal"}, {"id": "oauth-business"}],
    })
}

/// Whether the user's own program answering `answer` is admitted.
fn own_admitted(answer: &serde_json::Value) -> Result<(), String> {
    Manifest::REVIEWED
        .admits_own(&crate::acp::Initialized::read(answer))
        .map_err(|failure| failure.reason().as_str().to_owned())
}

/// The user's own program is admitted by what it answers: the protocol, the
/// capabilities and the sign-in this client relies on, whatever it calls
/// itself and whatever its version; one lacking any of them is refused.
#[test]
fn an_own_program_is_admitted_by_what_it_answers_and_not_by_its_name() {
    let refused_where = |pointer: &str, value: serde_json::Value| {
        let mut answer = agy_answer();
        if let Some(field) = answer.pointer_mut(pointer) {
            *field = value;
        }
        own_admitted(&answer)
    };

    assert_eq!(own_admitted(&agy_answer()), Ok(()));
    assert_eq!(refused_where("/agentInfo", serde_json::json!({})), Ok(()));
    for (pointer, value) in [
        ("/protocolVersion", serde_json::json!(2)),
        ("/agentCapabilities", serde_json::json!(true)),
        ("/agentCapabilities/loadSession", serde_json::json!(false)),
        (
            "/agentCapabilities/sessionCapabilities",
            serde_json::json!({}),
        ),
        ("/agentCapabilities/auth", serde_json::json!({})),
        (
            "/authMethods",
            serde_json::json!([{"id": "oauth-business"}]),
        ),
    ] {
        assert!(refused_where(pointer, value).is_err(), "{pointer}");
    }
}
