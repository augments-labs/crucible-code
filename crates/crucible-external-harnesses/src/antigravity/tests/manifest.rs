//! Only the reviewed release, on a platform it was published for, is run.

use serde_json::{Value, json};

use crate::acp::Initialized;
use crate::antigravity::Manifest;

/// The reviewed release's answer to initializing, as it gives it.
fn reviewed() -> Value {
    json!({
        "protocolVersion": 1,
        "agentInfo": {"name": "antigravity-acp", "version": "1.3.0"},
        "agentCapabilities": {
            "loadSession": true,
            "sessionCapabilities": {"resume": {}},
            "auth": {"logout": {}},
        },
        "authMethods": [{"id": "oauth-personal"}, {"id": "oauth-business"}],
    })
}

/// Whether the release answering `answer` is admitted on Linux.
fn admitted(answer: &Value) -> Result<(), String> {
    Manifest::REVIEWED
        .admits(&Initialized::read(answer), "linux", "x86_64")
        .map_err(|failure| failure.reason().as_str().to_owned())
}

#[test]
fn the_reviewed_release_is_admitted() {
    assert_eq!(admitted(&reviewed()), Ok(()));
}

/// A version the manifest does not list is refused, naming the one it does.
#[test]
fn a_version_outside_the_manifest_is_refused() {
    let mut newer = reviewed();
    if let Some(version) = newer.pointer_mut("/agentInfo/version") {
        *version = json!("1.4.0");
    }

    let refused = admitted(&newer).unwrap_err();

    assert!(
        refused.contains("1.4.0") && refused.contains("1.3.0"),
        "{refused}"
    );
}

/// A release that stopped offering what a client relies on is refused, as is
/// another program, a protocol version it does not use and a platform it was
/// not published for.
#[test]
fn a_release_that_differs_from_the_reviewed_one_is_refused() {
    for (pointer, value) in [
        ("/agentInfo/name", json!("another-agent")),
        ("/protocolVersion", json!(2)),
        ("/protocolVersion", json!(3)),
        ("/agentCapabilities/loadSession", json!(false)),
        ("/agentCapabilities/sessionCapabilities", json!({})),
        ("/agentCapabilities/auth", json!({})),
        ("/authMethods", json!([{"id": "oauth-business"}])),
    ] {
        let mut changed = reviewed();
        if let Some(field) = changed.pointer_mut(pointer) {
            *field = value;
        }
        assert!(admitted(&changed).is_err(), "{pointer}");
    }
    let elsewhere = Manifest::REVIEWED.admits(&Initialized::read(&reviewed()), "freebsd", "x86_64");
    assert!(elsewhere.is_err());
}
