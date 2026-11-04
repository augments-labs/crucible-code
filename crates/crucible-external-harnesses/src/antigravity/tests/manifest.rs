//! Only a release the catalog offers, on a platform it was published for, is
//! run.

use serde_json::{Value, json};

use crate::acp::Initialized;
use crate::antigravity::Manifest;
use crate::catalog::{Admits, SNAPSHOT};

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

/// What the catalog says Antigravity must answer with at each of `versions`.
fn listed(versions: &[&str]) -> Vec<Admits> {
    versions
        .iter()
        .map(|version| Admits {
            name: "antigravity-acp".into(),
            version: (*version).into(),
        })
        .collect()
}

/// Whether the release answering `answer` is admitted among `offered`.
fn admitted(answer: &Value, offered: &[Admits]) -> Result<(), String> {
    Manifest::REVIEWED
        .admits(&Initialized::read(answer), offered)
        .map_err(|failure| failure.reason().as_str().to_owned())
}

/// `reviewed`, answering as the release `version`.
fn answering_as(version: &str) -> Value {
    let mut answer = reviewed();
    if let Some(field) = answer.pointer_mut("/agentInfo/version") {
        *field = json!(version);
    }
    answer
}

#[test]
fn the_reviewed_release_is_admitted() {
    assert_eq!(admitted(&reviewed(), &listed(&["1.3.0"])), Ok(()));
}

/// The versions are the catalog's: one it offers is admitted whichever source
/// listed it, and the one the build was reviewed with is no longer special.
#[test]
fn a_version_the_catalog_offers_is_admitted_and_one_it_does_not_is_refused() {
    let offered = listed(&["1.4.1", "1.3.0"]);

    assert_eq!(admitted(&answering_as("1.4.1"), &offered), Ok(()));
    assert_eq!(admitted(&answering_as("1.3.0"), &offered), Ok(()));
    assert!(admitted(&answering_as("1.4.1"), &listed(&["1.3.0"])).is_err());
    assert!(admitted(&answering_as("1.3.0"), &listed(&["1.4.1"])).is_err());
}

/// A version the catalog does not offer is refused, naming the ones it does.
#[test]
fn a_version_outside_the_offered_is_refused() {
    let refused = admitted(&answering_as("1.4.0"), &listed(&["1.3.0"])).unwrap_err();

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
        assert!(
            admitted(&changed, &listed(&["1.3.0"])).is_err(),
            "{pointer}"
        );
    }
    let elsewhere = admitted(&reviewed(), &[]);
    assert!(elsewhere.is_err(), "nothing is offered for the platform");
}

/// The release the build ships the data of is admitted from that data and no
/// other: the snapshot's entry says what the agent must answer with.
#[test]
fn the_snapshots_release_is_one_the_manifest_admits() {
    let [entry] = SNAPSHOT.entries() else {
        panic!("one entry");
    };

    let admitted = admitted(&reviewed(), std::slice::from_ref(&entry.admits));

    assert_eq!(admitted, Ok(()));
}

/// Each platform's archive is of the snapshot's release, and named for it, so
/// what is downloaded is what initializing is then checked against.
#[test]
fn each_published_archive_is_of_the_snapshots_release() {
    let [entry] = SNAPSHOT.entries() else {
        panic!("one entry");
    };
    for target in &entry.targets {
        let [archive] = target.artifacts.as_slice() else {
            panic!("one archive for {}", target.platform);
        };
        assert!(
            archive
                .url
                .starts_with("https://dl.google.com/agy-extensions/releases/")
                && archive.url.contains(&format!("-{}-", entry.version)),
            "{}: {}",
            target.platform,
            archive.url
        );
        assert_eq!(archive.sha256.len(), 64, "{}", target.platform);
        let windows = target.platform.starts_with("windows-");
        assert_eq!(windowed(&target.cmd), windows, "{}", target.platform);
    }
}

/// Whether `name` is spelled as a Windows program is.
fn windowed(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}
