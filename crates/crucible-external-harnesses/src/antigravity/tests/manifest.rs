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

/// Each platform's archive is of the reviewed release, and named for it, so
/// what is downloaded is what initializing is then checked against.
#[test]
fn each_published_archive_is_of_the_reviewed_release() {
    for (os, arch, pin) in Manifest::REVIEWED.platforms {
        assert!(
            Manifest::REVIEWED.versions.contains(&pin.version),
            "{os} {arch}"
        );
        assert!(
            pin.url
                .starts_with("https://dl.google.com/agy-extensions/releases/")
                && pin.url.contains(&format!("-{}-", pin.version)),
            "{os} {arch}: {}",
            pin.url
        );
        assert_eq!(pin.sha256.len(), 64, "{os} {arch}");
        let windows = *os == "windows";
        assert_eq!(windowed(pin.executable.name), windows, "{os} {arch}");
        assert_eq!(windowed(pin.companion.name), windows, "{os} {arch}");
    }
}

/// Whether `name` is spelled as a Windows program is.
fn windowed(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}
