//! A catalog is believed only signed, bounded, current and in a known schema,
//! and is parsed into every field an entry carries. What a user is offered
//! from it is decided by the trust state and the snapshot, whatever the
//! catalogs a machine has seen since.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Barrier;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crucible_sandbox::SandboxDomainPattern;
use crucible_types::AgentKey;
use serde_json::{Value, json};

use super::fixture::{Signed, signed};
use super::trust::{
    DIRECTORY, FILE, LOCK, REVOKED_CAP, STATE_BYTES, State, decode, here, merge, platform,
};
use super::{
    Admits, AssetRef, AuthMethod, Catalog, CatalogError, CatalogProfile, ContentUse, Disposition,
    Entry, File, FileKind, Files, Format, GRACE, Held, INDEX_BYTES, KEYS, Keys, Kind, Launch, Lock,
    Method, Revoked, Route, SCHEMA_SHOWN, SNAPSHOT, Script, Trust, TrustError, Unprofiled,
    Unusable, VERSION_BYTES, Variable, built_list, file_list, result_list,
};
use crate::acp::SignIn;
use crate::antigravity::AGENT;
use crate::release::MEMBERS;
use crate::tests::Owned;

const DAY: Duration = Duration::from_hours(24);

/// When the catalogs below with a date of their own were signed.
const SIGNED_AT: u64 = 1_790_000_000;

/// When they expire: thirty days later.
const EXPIRES: u64 = SIGNED_AT + 30 * 24 * 60 * 60;

/// A SHA-256 spelled with `digit` alone.
fn hash(digit: char) -> String {
    digit.to_string().repeat(64)
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// A binary agent: one zip, a mirror, every sign-in route.
fn binary() -> Value {
    let auth = json!([
        {"id": "browser", "name": "Sign in", "description": "In your browser", "kind": "agent"},
        {"id": "key", "name": "API key", "kind": "env_var", "var": "GOOSE_API_KEY"},
        {
            "id": "login", "name": "Log in", "kind": "terminal",
            "command": "goose", "args": ["login"], "env": {"GOOSE_LOGIN": "1"},
            "route": "terminal"
        }
    ]);
    let files = json!([
        {"path": "goose", "size": 2900, "sha256": hash('b'), "exec": true},
        {"path": "lib", "kind": "dir"},
        {"path": "lib/libgoose.so", "kind": "link", "target": "../goose"},
        {"path": "LICENSE", "size": 100, "sha256": hash('c')}
    ]);
    let zip = json!({
        "url": "https://github.com/example/goose/releases/download/v1.4.1/goose.zip",
        "mirror": "https://github.com/augments-labs/crucible-agent-catalog/releases/download/catalog/goose.zip",
        "sha256": hash('a'),
        "size": 1000,
        "format": "zip",
        "unpacked": 3000,
        "decompressed": 4000,
        "files": files,
        "kept": ["goose", "LICENSE"]
    });
    let target = json!({
        "platform": "linux-x86_64",
        "cmd": "goose",
        "args": ["acp"],
        "env": {"GOOSE_MODE": "acp"},
        "hosts": ["github.com", "objects.githubusercontent.com"],
        "artifacts": [zip]
    });
    json!({
        "key": "goose",
        "registry_id": "goose-acp",
        "version": "1.4.1",
        "kind": "binary",
        "license": "Apache-2.0",
        "terms": "https://example.com/terms",
        "caution": "Read the terms first.",
        "review": "https://github.com/augments-labs/crucible-agent-catalog/pull/12",
        "content_use": {
            "text": "Prompts are not used for training.",
            "source": "https://example.com/privacy",
            "date": "2026-10-21",
            "origins": ["https://api.example.com"]
        },
        "admits": {"name": "goose", "version": "1.4.1"},
        "reach": ["api.example.com"],
        "sign_in_hosts": ["accounts.example.com"],
        "updater_off": {"name": "GOOSE_NO_UPDATE", "value": "1"},
        "profile_env": "GOOSE_HOME",
        "state_paths": [".config/goose"],
        "auth_methods": auth,
        "targets": [target]
    })
}

/// A `uvx` agent: an interpreter with its file list kept as an asset, and a
/// wheel built at review in place of the vendor's.
fn uvx() -> Value {
    json!({
        "key": "fast-agent",
        "registry_id": "fast-agent-acp",
        "version": "0.10.1",
        "kind": "uvx",
        "license": "Apache-2.0",
        "review": "https://github.com/augments-labs/crucible-agent-catalog/pull/13",
        "admits": {"name": "fast-agent", "version": "0.10.1"},
        "targets": [{
            "platform": "darwin-aarch64",
            "cmd": "python/bin/python3",
            "interpreter_args": ["-I"],
            "entry_point": "fast_agent.acp:main",
            "artifacts": [
                {
                    "url": "https://github.com/astral-sh/python-build-standalone/releases/download/x/cpython.tar.gz",
                    "sha256": hash('d'),
                    "size": 30_000_000,
                    "format": "tar.gz",
                    "unpacked": 90_000_000,
                    "decompressed": 95_000_000,
                    "files": {"sha256": hash('e'), "size": 400_000}
                },
                {
                    "url": "https://files.pythonhosted.org/packages/fast_agent-0.10.1.tar.gz",
                    "sha256": hash('f'),
                    "size": 2000,
                    "format": "wheel",
                    "unpacked": 5000,
                    "decompressed": 5000,
                    "files": [{"path": "site/fast_agent/acp.py", "size": 5000, "sha256": hash('1')}],
                    "replaced_by": {"sha256": hash('2'), "size": 2100}
                }
            ]
        }]
    })
}

/// An npm agent: a pinned Node keeping two members, a lock asset and its
/// install scripts.
fn npm() -> Value {
    json!({
        "key": "gemini",
        "registry_id": "gemini",
        "version": "0.9.0",
        "lock_revision": 2,
        "kind": "npm",
        "license": "Apache-2.0",
        "review": "https://github.com/augments-labs/crucible-agent-catalog/pull/14",
        "admits": {"name": "gemini-cli", "version": "0.9.0"},
        "targets": [{
            "platform": "windows-x86_64",
            "cmd": "node.exe",
            "node_args": ["--require", "preload.cjs"],
            "bin": "node_modules/@google/gemini-cli/dist/index.js",
            "args": ["--experimental-acp"],
            "lock": {"sha256": hash('3'), "size": 300_000, "unpack_rules": 1},
            "scripts": [
                {"package": "node_modules/a", "event": "postinstall", "disposition": "inert"},
                {"package": "node_modules/b", "event": "install", "disposition": "replaced"},
                {"package": "node_modules/c", "event": "preinstall", "disposition": "lost"},
                {
                    "package": "node_modules/d", "event": "install", "disposition": "built",
                    "asset": {"sha256": hash('5'), "size": 2_000},
                    "files": {"sha256": hash('6'), "size": 100},
                    "unpacked": 7_000, "decompressed": 8_000
                },
                {
                    "package": "node_modules/e", "event": "postinstall", "disposition": "local",
                    "command": "node install.js", "hosts": ["*.example.com"],
                    "result": {"sha256": hash('4'), "size": 900}
                }
            ],
            "artifacts": [{
                "url": "https://nodejs.org/dist/v24.1.0/node-v24.1.0-win-x64.zip",
                "sha256": hash('5'),
                "size": 30_000_000,
                "format": "zip",
                "unpacked": 80_000_000,
                "decompressed": 100_000_000,
                "files": [
                    {"path": "node.exe", "size": 79_000_000, "sha256": hash('6'), "exec": true},
                    {"path": "LICENSE", "size": 1_000_000, "sha256": hash('7')}
                ],
                "kept": ["node-v24.1.0-win-x64/node.exe", "node-v24.1.0-win-x64/LICENSE"]
            }]
        }]
    })
}

/// A whole catalog of the three, with no date of its own, so the fixture
/// signs it as of now.
fn catalog() -> Value {
    json!({
        "schema": 1,
        "serial": 7,
        "entries": [binary(), uvx(), npm()],
        "revoked": [
            {"key": "goose", "version": "1.3.0", "reason": "It wrote outside its directory."},
            {"key": "gemini", "version": "0.9.0", "lock_revision": 1, "reason": "A package was replaced."}
        ]
    })
}

/// `catalog` dated as signed at [`SIGNED_AT`] and expiring at [`EXPIRES`].
fn dated(mut catalog: Value) -> Value {
    set(&mut catalog, "/signed_at", json!(SIGNED_AT));
    set(&mut catalog, "/expires", json!(EXPIRES));
    catalog
}

/// `catalog` with what `pointer` names set to `to`, added to its object where
/// it is not there yet.
// A test that changes a field its catalog does not have has nothing to report.
#[allow(clippy::panic)]
fn set(catalog: &mut Value, pointer: &str, to: Value) {
    if let Some(slot) = catalog.pointer_mut(pointer) {
        *slot = to;
        return;
    }
    let (parent, key) = pointer.rsplit_once('/').unwrap_or_default();
    let Some(object) = catalog.pointer_mut(parent).and_then(Value::as_object_mut) else {
        panic!("{parent} is an object of the catalog");
    };
    object.insert(key.to_owned(), to);
}

/// The catalog with what `pointer` names set to `to`.
fn with(pointer: &str, to: Value) -> Value {
    let mut catalog = catalog();
    set(&mut catalog, pointer, to);
    catalog
}

/// The catalog without the field `pointer` names.
// A test that removes a field its catalog does not have has nothing to report.
#[allow(clippy::panic)]
fn without(pointer: &str) -> Value {
    let mut catalog = catalog();
    let (parent, key) = pointer.rsplit_once('/').unwrap_or_default();
    let removed = catalog
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .and_then(|object| object.remove(key));
    assert!(removed.is_some(), "{pointer} is in the catalog");
    catalog
}

/// Each case, a field and the pointer set to a value, that is not refused
/// naming that field, and saying `why` where one is given.
fn misnamed(cases: Vec<(&str, String, Value)>, why: Option<&str>) -> Vec<String> {
    cases
        .into_iter()
        .filter_map(|(field, pointer, to)| {
            let refused = fetched(&with(&pointer, to));
            let named = matches!(
                &refused,
                Err(CatalogError::Field { field: named, why: said })
                    if **named == *field && why.is_none_or(|why| *said == why)
            );
            (!named).then(|| format!("{field}: {:?}", refused.map(|_| "believed")))
        })
        .collect()
}

fn sign(catalog: &Value) -> Signed {
    signed(&serde_json::to_vec(catalog).unwrap_or_default())
}

/// `catalog`, signed, as a fetched catalog is believed: nothing held, now.
///
/// For a catalog with no date of its own, which the fixture signs as of now. A
/// catalog dated by [`dated`] or [`listing`] is believed as of an instant
/// inside its dates, with [`fetched_at`], whatever day the test runs.
fn fetched(catalog: &Value) -> Result<Catalog, CatalogError> {
    fetched_at(catalog, SystemTime::now())
}

/// `catalog`, signed, as a fetched catalog is believed as of `now`: nothing
/// held.
fn fetched_at(catalog: &Value, now: SystemTime) -> Result<Catalog, CatalogError> {
    let signed = sign(catalog);
    Catalog::verify(&signed.index, &signed.signature, &signed.keys, Held(0), now)
}

/// The whole catalog, believed.
// A test whose good catalog is refused has nothing else to report.
#[allow(clippy::panic)]
fn believed() -> Catalog {
    let catalog = fetched(&catalog());
    let Ok(catalog) = catalog else {
        panic!("a good catalog is believed: {catalog:?}");
    };
    assert_eq!(catalog.serial(), 7);
    assert!(catalog.expires() > catalog.signed_at());
    assert_eq!(catalog.entries().len(), 3);
    catalog
}

#[test]
fn a_signed_catalog_parses_a_binary_entry_into_every_field() {
    let catalog = believed();
    let Some(goose) = catalog.entries().first() else {
        panic!("an entry");
    };

    assert_eq!(goose.key.as_str(), "goose");
    assert_eq!(&*goose.registry_id, "goose-acp");
    assert_eq!(&*goose.version, "1.4.1");
    assert_eq!(goose.kind, Kind::Binary);
    assert_eq!(&*goose.license, "Apache-2.0");
    assert_eq!(goose.terms.as_deref(), Some("https://example.com/terms"));
    assert_eq!(goose.caution.as_deref(), Some("Read the terms first."));
    assert_eq!(
        goose.content_use,
        Some(ContentUse {
            text: "Prompts are not used for training.".into(),
            source: "https://example.com/privacy".into(),
            date: "2026-10-21".into(),
            origins: vec!["https://api.example.com".into()],
        })
    );
    assert_eq!(
        goose.admits,
        Admits {
            name: "goose".into(),
            version: "1.4.1".into()
        }
    );
    assert_eq!(goose.reach, ["api.example.com".into()]);
    assert_eq!(goose.sign_in_hosts, ["accounts.example.com".into()]);
    assert_eq!(
        goose.updater_off,
        Some(Variable {
            name: "GOOSE_NO_UPDATE".into(),
            value: "1".into()
        })
    );
    assert_eq!(goose.profile_env.as_deref(), Some("GOOSE_HOME"));
    assert_eq!(goose.state_paths, [".config/goose".into()]);
    assert_eq!(
        goose.auth_methods,
        [
            AuthMethod {
                id: "browser".into(),
                name: "Sign in".into(),
                description: Some("In your browser".into()),
                method: Method::Agent,
            },
            AuthMethod {
                id: "key".into(),
                name: "API key".into(),
                description: None,
                method: Method::EnvVar {
                    var: "GOOSE_API_KEY".into()
                },
            },
            AuthMethod {
                id: "login".into(),
                name: "Log in".into(),
                description: None,
                method: Method::Terminal {
                    command: "goose".into(),
                    args: vec!["login".into()],
                    env: vec![Variable {
                        name: "GOOSE_LOGIN".into(),
                        value: "1".into()
                    }],
                    route: Route::Terminal,
                },
            },
        ]
    );
    let [target] = goose.targets.as_slice() else {
        panic!("one target");
    };
    assert_eq!(&*target.platform, "linux-x86_64");
    assert_eq!(&*target.cmd, "goose");
    assert_eq!(target.args, ["acp".into()]);
    assert_eq!(
        target.env,
        [Variable {
            name: "GOOSE_MODE".into(),
            value: "acp".into()
        }]
    );
    assert_eq!(
        target.hosts,
        ["github.com".into(), "objects.githubusercontent.com".into()]
    );
    assert_eq!(target.launch, Launch::Binary);
    let [zip] = target.artifacts.as_slice() else {
        panic!("one artifact");
    };
    assert!(zip.url.starts_with("https://github.com/example/"));
    assert!(
        zip.mirror
            .as_deref()
            .is_some_and(|mirror| mirror.contains("crucible-agent-catalog"))
    );
    assert_eq!(&*zip.sha256, hash('a'));
    assert_eq!(
        (zip.size, zip.format, zip.unpacked, zip.decompressed),
        (1000, Format::Zip, 3000, 4000)
    );
    assert_eq!(zip.kept, ["goose".into(), "LICENSE".into()]);
    assert_eq!(zip.replaced_by, None);
    let Files::Listed(files) = &zip.files else {
        panic!("a listed file list");
    };
    let kinds: Vec<(&str, &FileKind)> =
        files.iter().map(|file| (&*file.path, &file.kind)).collect();
    assert_eq!(
        kinds,
        [
            (
                "goose",
                &FileKind::File {
                    size: 2900,
                    sha256: hash('b').into(),
                    exec: true
                }
            ),
            ("lib", &FileKind::Dir),
            (
                "lib/libgoose.so",
                &FileKind::Link {
                    target: "../goose".into()
                }
            ),
            (
                "LICENSE",
                &FileKind::File {
                    size: 100,
                    sha256: hash('c').into(),
                    exec: false
                }
            ),
        ]
    );
}

#[test]
fn a_signed_catalog_parses_its_interpreter_and_node_entries() {
    let catalog = believed();
    let [_, fast, gemini] = catalog.entries() else {
        panic!("three entries");
    };
    assert_eq!(fast.kind, Kind::Uvx);
    assert_eq!(fast.terms, None);
    assert_eq!(fast.content_use, None);
    assert!(fast.reach.is_empty() && fast.auth_methods.is_empty());
    let [target] = fast.targets.as_slice() else {
        panic!("one target");
    };
    assert_eq!(
        target.launch,
        Launch::Uvx {
            interpreter_args: vec!["-I".into()],
            entry_point: "fast_agent.acp:main".into(),
        }
    );
    let [python, wheel] = target.artifacts.as_slice() else {
        panic!("two artifacts");
    };
    assert_eq!(python.format, Format::TarGz);
    assert_eq!(
        python.files,
        Files::Asset(AssetRef {
            sha256: hash('e').into(),
            size: 400_000
        })
    );
    assert_eq!(wheel.format, Format::Wheel);
    assert_eq!(
        wheel.replaced_by,
        Some(AssetRef {
            sha256: hash('2').into(),
            size: 2100
        })
    );

    assert_eq!(gemini.kind, Kind::Npm);
    assert_eq!(gemini.lock_revision, Some(2));
    let [target] = gemini.targets.as_slice() else {
        panic!("one target");
    };
    assert_eq!(&*target.cmd, "node.exe");
    assert_eq!(
        target.artifacts.first().map(|node| node.kept.len()),
        Some(2)
    );
    let script = |package: &str, event: &str, disposition| Script {
        package: package.into(),
        event: event.into(),
        disposition,
    };
    assert_eq!(
        target.launch,
        Launch::Npm {
            node_args: vec!["--require".into(), "preload.cjs".into()],
            bin: "node_modules/@google/gemini-cli/dist/index.js".into(),
            lock: Lock {
                asset: AssetRef {
                    sha256: hash('3').into(),
                    size: 300_000
                },
                unpack_rules: 1,
            },
            scripts: vec![
                script("node_modules/a", "postinstall", Disposition::Inert),
                script("node_modules/b", "install", Disposition::Replaced),
                script("node_modules/c", "preinstall", Disposition::Lost),
                script(
                    "node_modules/d",
                    "install",
                    Disposition::Built {
                        asset: AssetRef {
                            sha256: hash('5').into(),
                            size: 2_000,
                        },
                        files: AssetRef {
                            sha256: hash('6').into(),
                            size: 100,
                        },
                        unpacked: 7_000,
                        decompressed: 8_000,
                    },
                ),
                script(
                    "node_modules/e",
                    "postinstall",
                    Disposition::Local {
                        command: "node install.js".into(),
                        hosts: vec!["*.example.com".into()],
                        result: AssetRef {
                            sha256: hash('4').into(),
                            size: 900
                        },
                    }
                ),
            ],
        }
    );
}

#[test]
fn a_signed_catalog_parses_its_revocations() {
    let catalog = believed();
    let revoked: Vec<(&str, &str, Option<u64>, &str)> = catalog
        .revoked()
        .iter()
        .map(|revoked| {
            (
                revoked.key.as_str(),
                &*revoked.version,
                revoked.lock_revision,
                &*revoked.reason,
            )
        })
        .collect();
    assert_eq!(
        revoked,
        [
            ("goose", "1.3.0", None, "It wrote outside its directory."),
            ("gemini", "0.9.0", Some(1), "A package was replaced."),
        ]
    );
}

#[test]
fn a_catalog_is_believed_for_seven_days_past_its_expiry_and_no_longer() {
    let signed = sign(&dated(catalog()));
    let believed =
        |now| Catalog::verify(&signed.index, &signed.signature, &signed.keys, Held(0), now);

    let six = believed(at(EXPIRES) + 6 * DAY);
    let eight = believed(at(EXPIRES) + 8 * DAY);

    assert!(six.is_ok(), "{six:?}");
    assert_eq!(six.ok().map(|catalog| catalog.expires()), Some(at(EXPIRES)));
    assert!(matches!(eight, Err(CatalogError::Expired)), "{eight:?}");
    assert!(
        believed(at(EXPIRES) + GRACE).is_ok(),
        "the last second of grace"
    );
}

#[test]
fn the_compiled_snapshot_is_believed_long_past_its_expiry() {
    let mut old = dated(catalog());
    set(&mut old, "/signed_at", json!(1_000_000_000));
    set(
        &mut old,
        "/expires",
        json!(1_000_000_000 + 30 * 24 * 60 * 60),
    );
    let signed = sign(&old);

    let snapshot = Catalog::compiled(&signed.index, &signed.signature, &signed.keys);
    let fetched = Catalog::verify(
        &signed.index,
        &signed.signature,
        &signed.keys,
        Held(0),
        SystemTime::now(),
    );

    assert_eq!(snapshot.ok().map(|catalog| catalog.serial()), Some(7));
    assert!(matches!(fetched, Err(CatalogError::Expired)), "{fetched:?}");
}

#[test]
fn a_catalog_whose_signature_does_not_verify_is_refused() {
    let signed = sign(&catalog());
    let other = sign(&json!({"schema": 1, "serial": 8, "entries": [], "revoked": []}));
    let mut tampered = signed.index.clone();
    if let Some(byte) = tampered.iter_mut().find(|byte| **byte == b'7') {
        *byte = b'9';
    }

    for (index, signature) in [
        (&tampered, &signed.signature),
        (&signed.index, &other.signature),
        (&signed.index, &b"not a signature".to_vec()),
    ] {
        let refused = Catalog::verify(index, signature, &signed.keys, Held(0), SystemTime::now());
        assert!(
            matches!(refused, Err(CatalogError::Signature)),
            "{refused:?}"
        );
        let refused = Catalog::compiled(index, signature, &signed.keys);
        assert!(
            matches!(refused, Err(CatalogError::Signature)),
            "{refused:?}"
        );
    }
}

#[test]
fn a_catalog_older_than_the_one_held_is_refused() {
    let signed = sign(&catalog());
    let believed = |held| {
        Catalog::verify(
            &signed.index,
            &signed.signature,
            &signed.keys,
            held,
            SystemTime::now(),
        )
    };

    let older = believed(Held(8));

    assert!(
        matches!(
            older,
            Err(CatalogError::Serial {
                offered: 7,
                held: 8
            })
        ),
        "{older:?}"
    );
    assert!(believed(Held(7)).is_ok(), "the serial already held");
}

#[test]
fn a_catalog_larger_than_the_cap_is_refused_before_it_is_read() {
    // Dated already, so the fixture signs the padded bytes as they are.
    let mut text = serde_json::to_vec(&dated(catalog())).unwrap_or_default();
    text.resize(INDEX_BYTES + 1, b' ');
    let signed = signed(&text);
    assert_eq!(signed.index.len(), INDEX_BYTES + 1, "signed as given");

    let fetched = Catalog::verify(
        &signed.index,
        &signed.signature,
        &signed.keys,
        Held(0),
        SystemTime::now(),
    );
    let snapshot = Catalog::compiled(&signed.index, &signed.signature, &signed.keys);

    let size = INDEX_BYTES + 1;
    assert!(
        matches!(fetched, Err(CatalogError::Size(n)) if n == size),
        "{fetched:?}"
    );
    assert!(
        matches!(snapshot, Err(CatalogError::Size(n)) if n == size),
        "{snapshot:?}"
    );
}

#[test]
fn a_catalog_in_an_unknown_schema_is_refused() {
    for later in [
        with("/schema", json!(2)),
        with("/schema", json!(0)),
        with("/schema", json!("1")),
        without("/schema"),
    ] {
        let signed = sign(&later);

        let fetched = Catalog::verify(
            &signed.index,
            &signed.signature,
            &signed.keys,
            Held(0),
            SystemTime::now(),
        );
        let snapshot = Catalog::compiled(&signed.index, &signed.signature, &signed.keys);

        assert!(
            matches!(fetched, Err(CatalogError::Schema(_))),
            "{fetched:?}"
        );
        assert!(
            matches!(snapshot, Err(CatalogError::Schema(_))),
            "{snapshot:?}"
        );
    }
}

#[test]
fn a_loader_variable_is_refused_wherever_an_entry_sets_a_variable() {
    let loaders = [
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "DYLD_INSERT_LIBRARIES",
        "NODE_OPTIONS",
        "NODE_PATH",
        "PYTHONPATH",
        "PATH",
        "Path",
    ];
    for name in loaders {
        for (place, pointer, to) in [
            (
                "entries[0].targets[0].env",
                format!("/entries/0/targets/0/env/{name}"),
                json!("x"),
            ),
            (
                "entries[0].profile_env",
                "/entries/0/profile_env".to_owned(),
                json!(name),
            ),
            (
                "entries[0].auth_methods[2].env",
                format!("/entries/0/auth_methods/2/env/{name}"),
                json!("x"),
            ),
            (
                "entries[0].auth_methods[1].var",
                "/entries/0/auth_methods/1/var".to_owned(),
                json!(name),
            ),
            (
                "entries[0].updater_off.name",
                "/entries/0/updater_off/name".to_owned(),
                json!(name),
            ),
        ] {
            let refused = fetched(&with(&pointer, to));
            assert!(
                matches!(
                    &refused,
                    Err(CatalogError::Loader { field, name: named })
                        if &**field == place && &**named == name
                ),
                "{name} in {place}: {refused:?}"
            );
        }
    }
    for near in ["LDFLAGS", "MY_PATH", "PATHEXT", "NODE_ENV", "PYTHONHOME_X"] {
        let kept = fetched(&with(
            &format!("/entries/0/targets/0/env/{near}"),
            json!("x"),
        ));
        assert!(kept.is_ok(), "{near}: {kept:?}");
    }
}

#[test]
fn the_compiled_keys_refuse_every_signature() {
    let signed = sign(&catalog());

    let fetched = Catalog::verify(
        &signed.index,
        &signed.signature,
        &KEYS,
        Held(0),
        SystemTime::now(),
    );
    let snapshot = Catalog::compiled(&signed.index, &signed.signature, &KEYS);

    assert!(
        matches!(fetched, Err(CatalogError::Signature)),
        "{fetched:?}"
    );
    assert!(
        matches!(snapshot, Err(CatalogError::Signature)),
        "{snapshot:?}"
    );
}

#[test]
fn a_field_that_is_not_what_it_must_be_is_refused_by_its_name() {
    let file = json!({"path": "f", "size": 1, "sha256": hash('0')});
    let goose = "/entries/0/targets/0/artifacts/0";
    let cases = [
        (
            "entries[0].key",
            "/entries/0/key".to_owned(),
            json!("Goose"),
        ),
        (
            "entries[0].kind",
            "/entries/0/kind".to_owned(),
            json!("cargo"),
        ),
        (
            "entries[0].version",
            "/entries/0/version".to_owned(),
            Value::Null,
        ),
        (
            "entries[0].targets[0].artifacts[0].url",
            format!("{goose}/url"),
            json!("http://example.com/goose.zip"),
        ),
        (
            "entries[0].targets[0].artifacts[0].mirror",
            format!("{goose}/mirror"),
            json!("ftp://example.com/goose.zip"),
        ),
        (
            "entries[0].targets[0].artifacts[0].sha256",
            format!("{goose}/sha256"),
            json!(hash('A')),
        ),
        (
            "entries[0].targets[0].artifacts[0].format",
            format!("{goose}/format"),
            json!("tar.xz"),
        ),
        (
            "entries[0].targets[0].artifacts[0].files",
            format!("{goose}/files"),
            json!(vec![file; 257]),
        ),
        (
            "entries[0].targets[0].artifacts[0].files[0].path",
            format!("{goose}/files/0/path"),
            json!("../goose"),
        ),
        (
            "entries[0].targets[0].cmd",
            "/entries/0/targets/0/cmd".to_owned(),
            json!("/bin/sh"),
        ),
        (
            "entries[0].state_paths[0]",
            "/entries/0/state_paths/0".to_owned(),
            json!("C:\\Users\\goose"),
        ),
        (
            "entries[0].targets[0].artifacts",
            "/entries/0/targets/0/artifacts".to_owned(),
            json!([]),
        ),
        (
            "entries[2].targets[0].lock.unpack_rules",
            "/entries/2/targets/0/lock/unpack_rules".to_owned(),
            json!(2),
        ),
        (
            "entries[1].targets[0].artifacts[0].files.size",
            "/entries/1/targets/0/artifacts/0/files/size".to_owned(),
            json!((4 << 20) + 1),
        ),
    ];
    let failures = misnamed(cases.into(), None);

    assert!(failures.is_empty(), "{failures:#?}");
}

/// A local script is run by the command it gives, so one that gives none, or
/// one a terminal could not show as it is, or one longer than the intake lets
/// a catalog text be, is refused by that field's name.
#[test]
fn a_local_script_without_a_command_it_can_show_is_refused() {
    let command = "entries[2].targets[0].scripts[4].command";
    let at = "/entries/2/targets/0/scripts/4/command";
    let missing = fetched(&without(at));
    assert!(
        matches!(
            &missing,
            Err(CatalogError::Field { field, why: "is missing" }) if **field == *command
        ),
        "a local script without a command: {:?}",
        missing.map(|_| "believed")
    );
    let cases = vec![
        (command, at.to_owned(), json!("node install.js\u{1b}[2J")),
        (command, at.to_owned(), json!("node\ninstall.js")),
        (command, at.to_owned(), json!("x".repeat(4097))),
    ];
    let failures = misnamed(cases, None);
    assert!(failures.is_empty(), "{failures:#?}");
}

/// A built script is installed from the asset it names and read against the
/// list it names, so one naming neither is not an entry the catalog holds; the
/// fixture's own, with both, is.
#[test]
fn a_built_script_without_its_asset_or_its_file_list_is_refused() {
    let built = "/entries/2/targets/0/scripts/3";
    let mut named = catalog();
    let script = named.pointer_mut(built).expect("the built script");
    script["asset"] = json!({"sha256": hash('6'), "size": 9_000});
    script["files"] = json!({"sha256": hash('7'), "size": 700});
    script["unpacked"] = json!(90_000);
    script["decompressed"] = json!(100_000);
    let read = fetched(&named);
    assert!(read.is_ok(), "{:?}", read.map(|_| "believed"));
    for field in ["asset", "files", "unpacked", "decompressed"] {
        let mut bare = named.clone();
        bare.pointer_mut(built)
            .and_then(Value::as_object_mut)
            .expect("the built script")
            .remove(field);
        let missing = fetched(&bare);
        let whole = format!("entries[2].targets[0].scripts[3].{field}");
        assert!(
            matches!(
                &missing,
                Err(CatalogError::Field { field, why: "is missing" }) if **field == *whole
            ),
            "a built script without {field}: {:?}",
            missing.map(|_| "believed")
        );
    }
}

/// What a local script may reach is held to the intake's rule where it is
/// read, so a signed entry cannot widen it to everything or to an address.
#[test]
fn a_local_script_reaching_anything_but_a_host_by_name_is_refused() {
    let host = "entries[2].targets[0].scripts[4].hosts[0]";
    let at = "/entries/2/targets/0/scripts/4/hosts/0";
    let cases = [
        "*",
        "*.",
        "*.com",
        "*.*.example.com",
        "localhost",
        "127.0.0.1",
        "169.254.169.254",
        "::1",
        "[::1]",
        "example.com:80",
        "https://example.com",
        "example.com/",
        "example.com.",
        "EXAMPLE.com",
        "-a.example.com",
        "a-.example.com",
        "a..example.com",
    ]
    .into_iter()
    .map(|pattern| (host, at.to_owned(), json!(pattern)))
    .chain([
        (
            host,
            at.to_owned(),
            json!(format!("{}.com", "a".repeat(64))),
        ),
        (
            host,
            at.to_owned(),
            json!(format!("{}com", "a.".repeat(127))),
        ),
    ])
    .collect::<Vec<_>>();
    let failures = misnamed(cases, None);
    assert!(failures.is_empty(), "{failures:#?}");
    let many = vec![json!("a.example.com"); 65];
    let failures = misnamed(
        vec![(
            "entries[2].targets[0].scripts[4].hosts",
            "/entries/2/targets/0/scripts/4/hosts".to_owned(),
            json!(many),
        )],
        None,
    );
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn every_path_address_digest_and_word_is_checked_where_it_is_read() {
    let goose = "/entries/0/targets/0/artifacts/0";
    let size = format!("{goose}/size");
    let above = serde_json::from_str("18446744073709551616").unwrap_or(Value::Null);
    let cases = vec![
        (
            "entries[0].targets[0].artifacts[0].kept[0]",
            format!("{goose}/kept/0"),
            json!("../x"),
        ),
        (
            "entries[2].targets[0].bin",
            "/entries/2/targets/0/bin".into(),
            json!("../x"),
        ),
        (
            "entries[2].targets[0].scripts[0].package",
            "/entries/2/targets/0/scripts/0/package".into(),
            json!("../x"),
        ),
        (
            "entries[0].auth_methods[2].command",
            "/entries/0/auth_methods/2/command".into(),
            json!("../x"),
        ),
        (
            "entries[0].targets[0].artifacts[0].files[2].target",
            format!("{goose}/files/2/target"),
            json!("/etc/passwd"),
        ),
        (
            "entries[0].terms",
            "/entries/0/terms".into(),
            json!("http://example.com/terms"),
        ),
        (
            "entries[0].review",
            "/entries/0/review".into(),
            json!("http://example.com/pull/1"),
        ),
        (
            "entries[0].content_use.source",
            "/entries/0/content_use/source".into(),
            json!("http://example.com/privacy"),
        ),
        (
            "entries[1].targets[0].entry_point",
            "/entries/1/targets/0/entry_point".into(),
            json!("fast_agent.acp"),
        ),
        (
            "entries[0].auth_methods[2].route",
            "/entries/0/auth_methods/2/route".into(),
            json!("pty"),
        ),
        (
            "entries[2].targets[0].scripts[0].disposition",
            "/entries/2/targets/0/scripts/0/disposition".into(),
            json!("ran"),
        ),
        (
            "entries[0].auth_methods[0].kind",
            "/entries/0/auth_methods/0/kind".into(),
            json!("oauth"),
        ),
        (
            "entries[0].targets[0].artifacts[0].size",
            size.clone(),
            json!(-1),
        ),
        (
            "entries[0].targets[0].artifacts[0].size",
            size.clone(),
            json!(1.5),
        ),
        ("entries[0].targets[0].artifacts[0].size", size, above),
        (
            "entries[0].targets[0].artifacts[0].files[0].sha256",
            format!("{goose}/files/0/sha256"),
            json!(hash('A')),
        ),
    ];

    let failures = misnamed(cases, None);

    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn a_version_that_cannot_name_one_directory_is_refused() {
    let long = "1".repeat(VERSION_BYTES + 1);
    let mut cases = Vec::new();
    for version in [
        "..",
        ".",
        "1.0/x",
        "1.0\\x",
        "1:0",
        "1.0\0",
        "1.0\u{7}",
        "1.0\n",
        long.as_str(),
    ] {
        cases.push((
            "entries[0].version",
            "/entries/0/version".to_owned(),
            json!(version),
        ));
        cases.push((
            "revoked[0].version",
            "/revoked/0/version".to_owned(),
            json!(version),
        ));
    }

    let failures = misnamed(cases, Some("is not a version that can name one directory"));

    assert!(failures.is_empty(), "{failures:#?}");
    for version in [
        "1".repeat(VERSION_BYTES),
        "1.4.1+2".to_owned(),
        "2.0.0-rc.1".to_owned(),
    ] {
        let kept = fetched(&with("/entries/0/version", json!(version)));
        assert!(kept.is_ok(), "{version}: {kept:?}");
    }
}

#[test]
fn an_artifact_without_its_file_list_is_refused() {
    let files = "/entries/0/targets/0/artifacts/0/files";
    for catalog in [without(files), with(files, Value::Null)] {
        let refused = fetched(&catalog);

        assert!(
            matches!(
                &refused,
                Err(CatalogError::Field { field, why: "is missing, so nothing would be pinned" })
                    if &**field == "entries[0].targets[0].artifacts[0].files"
            ),
            "{:?}",
            refused.map(|_| "believed")
        );
    }
}

/// A local script's result is the listing review's runs left of its package's
/// directory, `{"files": [...]}`, regular files alone, each read as a file of
/// a file list is; a bare list, or a member of any other kind, is refused.
// A test whose good catalog lists no goose has nothing else to report.
#[allow(clippy::panic)]
#[test]
fn a_built_scripts_list_reads_files_and_directories_and_refuses_a_link() {
    let catalog = believed();
    let Some(goose) = catalog.entries().first() else {
        panic!("an entry");
    };
    let dir = json!({"path": "build", "kind": "dir"});
    let file = json!({"exec": true, "path": "build/x.node", "sha256": hash('9'), "size": 3});
    let listing = serde_json::to_vec(&json!([dir, file])).unwrap_or_default();

    let read = built_list(goose, &listing);

    assert_eq!(
        read.ok(),
        Some(vec![
            File {
                path: "build".into(),
                kind: FileKind::Dir,
            },
            File {
                path: "build/x.node".into(),
                kind: FileKind::File {
                    size: 3,
                    sha256: hash('9').into(),
                    exec: true
                },
            },
        ])
    );
    let linked = serde_json::to_vec(&json!([
        dir,
        {"path": "build/y.node", "kind": "link", "target": "x.node"}
    ]))
    .unwrap_or_default();
    assert!(
        matches!(
            built_list(goose, &linked),
            Err(CatalogError::Field {
                why: "is not a file or a directory",
                ..
            })
        ),
        "a build is files and directories alone"
    );
}

#[test]
fn a_script_result_reads_the_files_its_listing_gives_and_files_alone() {
    let catalog = believed();
    let Some(goose) = catalog.entries().first() else {
        panic!("an entry");
    };
    let member = json!({"exec": true, "path": "build/x.node", "sha256": hash('9'), "size": 3});
    let listing = serde_json::to_vec(&json!({"files": [member]})).unwrap_or_default();

    let read = result_list(goose, &listing);

    assert_eq!(
        read.ok(),
        Some(vec![File {
            path: "build/x.node".into(),
            kind: FileKind::File {
                size: 3,
                sha256: hash('9').into(),
                exec: true
            },
        }])
    );
    let bare = serde_json::to_vec(&json!([member])).unwrap_or_default();
    assert!(
        result_list(goose, &bare).is_err(),
        "a bare list is no result"
    );
    let dir = serde_json::to_vec(&json!({"files": [{"path": "build", "kind": "dir"}]}))
        .unwrap_or_default();
    assert!(
        matches!(
            result_list(goose, &dir),
            Err(CatalogError::Field {
                why: "is not a file",
                ..
            })
        ),
        "a result lists files alone"
    );
}

/// goose as the whole catalog believes it, the files its one artifact lists
/// inline, and that list as the catalog spells it.
// A test whose good catalog lists no goose has nothing else to report.
#[allow(clippy::panic)]
fn goose_listed() -> (Entry, Vec<File>, Value) {
    let catalog = believed();
    let Some(goose) = catalog.entries().first() else {
        panic!("an entry");
    };
    let Some(Files::Listed(files)) = goose
        .targets
        .first()
        .and_then(|target| target.artifacts.first())
        .map(|artifact| artifact.files.clone())
    else {
        panic!("goose lists its files inline");
    };
    let spelled = binary()
        .pointer("/targets/0/artifacts/0/files")
        .cloned()
        .unwrap_or_default();
    (goose.clone(), files, spelled)
}

#[test]
fn a_file_list_asset_reads_to_the_files_the_inline_list_of_them_gives() {
    let (goose, inline, spelled) = goose_listed();
    // As the intake writes it, keys sorted and nothing between them, and laid
    // out, since whitespace is not the format.
    let compact = serde_json::to_vec(&spelled).unwrap_or_default();
    let laid_out = serde_json::to_vec_pretty(&spelled).unwrap_or_default();

    for bytes in [compact, laid_out] {
        let read = file_list(&goose, &bytes);

        assert_eq!(read.as_ref().ok(), Some(&inline), "{read:?}");
    }
}

#[test]
fn a_file_list_asset_is_refused_naming_its_entry_and_the_first_member_at_fault() {
    let (goose, _, spelled) = goose_listed();
    let first = spelled.get(0).cloned().unwrap_or_default();
    let after = |member: Value| serde_json::to_vec(&json!([first, member])).unwrap_or_default();
    let cases: Vec<(&str, Vec<u8>, &str, &str)> = vec![
        (
            "a member whose size is not a number",
            after(json!({"path": "lib/x", "size": "large", "sha256": hash('d')})),
            "goose 1.4.1 files[1].size",
            "is not a whole number",
        ),
        (
            "a member that is not an object",
            after(json!("goose")),
            "goose 1.4.1 files[1]",
            "is not an object",
        ),
        (
            "a member whose path leaves its directory",
            after(json!({"path": "../goose", "kind": "dir"})),
            "goose 1.4.1 files[1].path",
            "is not a relative path that stays inside its directory",
        ),
        (
            "a path listed twice",
            after(json!({"path": "goose", "kind": "dir"})),
            "goose 1.4.1 files[1].path",
            "is listed twice",
        ),
        (
            "a link to an absolute path",
            after(json!({"path": "lib/libgoose.so", "kind": "link", "target": "/etc/passwd"})),
            "goose 1.4.1 files[1].target",
            "is not a relative link",
        ),
        (
            "a link through a backslash",
            after(json!({"path": "lib/libgoose.so", "kind": "link", "target": "..\\goose"})),
            "goose 1.4.1 files[1].target",
            "is not a relative link",
        ),
        (
            "an object in place of the list",
            serde_json::to_vec(&first).unwrap_or_default(),
            "goose 1.4.1 files",
            "is not a JSON list of files",
        ),
        (
            "bytes that are not JSON",
            b"[{\"path\":".to_vec(),
            "goose 1.4.1 files",
            "is not a JSON list of files",
        ),
    ];

    let failures: Vec<String> = cases
        .into_iter()
        .filter_map(|(case, bytes, named, said)| {
            let refused = file_list(&goose, &bytes);
            let right = matches!(
                &refused,
                Err(CatalogError::Field { field, why }) if **field == *named && *why == said
            );
            (!right).then(|| format!("{case}: {refused:?}"))
        })
        .collect();

    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn a_file_list_asset_holds_as_many_files_as_an_unpack_writes_and_no_more() {
    let (goose, _, _) = goose_listed();
    let list = |members: usize| {
        let files: Vec<Value> = (0..members)
            .map(|index| json!({"path": format!("d{index}"), "kind": "dir"}))
            .collect();
        serde_json::to_vec(&files).unwrap_or_default()
    };

    let whole = file_list(&goose, &list(MEMBERS));
    let over = file_list(&goose, &list(MEMBERS + 1));

    assert_eq!(whole.as_ref().map(Vec::len).ok(), Some(MEMBERS));
    let past = format!("goose 1.4.1 files[{MEMBERS}]");
    assert!(
        matches!(
            &over,
            Err(CatalogError::Field { field, why: "is past the most files one list may hold" })
                if **field == *past
        ),
        "{:?}",
        over.map(|files| files.len())
    );
}

#[test]
fn a_refusal_names_the_value_at_fault_and_says_truly_what_is_wrong() {
    let empty = vec![
        (
            "entries[0].caution",
            "/entries/0/caution".to_owned(),
            json!(""),
        ),
        (
            "entries[0].profile_env",
            "/entries/0/profile_env".to_owned(),
            json!(""),
        ),
        (
            "entries[0].reach[0]",
            "/entries/0/reach/0".to_owned(),
            json!(""),
        ),
    ];
    let value = vec![(
        "entries[0].targets[0].env.GOOSE_MODE",
        "/entries/0/targets/0/env/GOOSE_MODE".to_owned(),
        json!(1),
    )];

    let mut failures = misnamed(empty, Some("is empty"));
    failures.extend(misnamed(value, Some("is not text")));
    let root = sign(&json!(["not", "a", "catalog"]));
    let root = Catalog::compiled(&root.index, &root.signature, &root.keys);
    if !matches!(&root, Err(CatalogError::Field { field, .. }) if &**field == "the catalog") {
        failures.push(format!("the catalog: {:?}", root.map(|_| "believed")));
    }

    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn an_unknown_schema_is_repeated_only_in_part() {
    for schema in [
        json!("x".repeat(100_000)),
        json!({"version": "y".repeat(100_000)}),
    ] {
        let signed = sign(&with("/schema", schema));

        let refused = Catalog::compiled(&signed.index, &signed.signature, &signed.keys);

        assert!(
            matches!(&refused, Err(CatalogError::Schema(shown)) if shown.chars().count() <= SCHEMA_SHOWN),
            "{:?}",
            refused.map_err(|error| error.to_string().len())
        );
    }
}

/// A day after the catalogs above were signed, when the tests below take them
/// in.
fn taken() -> SystemTime {
    at(SIGNED_AT) + DAY
}

/// The agent `name`.
// A test naming a key that is not one has nothing to report.
#[allow(clippy::panic)]
fn agent(name: &str) -> AgentKey {
    let Ok(key) = AgentKey::new(name) else {
        panic!("{name} is an agent key");
    };
    key
}

/// `version` of the agent `name`, published for the platform the tests run on.
fn release(name: &str, version: &str, lock_revision: Option<u64>) -> Value {
    let mut entry = binary();
    set(&mut entry, "/key", json!(name));
    set(&mut entry, "/version", json!(version));
    set(&mut entry, "/admits/version", json!(version));
    set(&mut entry, "/targets/0/platform", json!(here()));
    if let Some(revision) = lock_revision {
        set(&mut entry, "/lock_revision", json!(revision));
    }
    entry
}

/// The withdrawal of `version` of `name`, of one lock revision where one is
/// given.
fn withdrawn(name: &str, version: &str, lock_revision: Option<u64>) -> Value {
    let mut revoked = json!({"key": name, "version": version, "reason": "It was withdrawn."});
    if let Some(revision) = lock_revision {
        set(&mut revoked, "/lock_revision", json!(revision));
    }
    revoked
}

/// A catalog at `serial` that lists `entries` and withdraws `revoked`, dated
/// as the ones above are.
fn listing(
    serial: u64,
    entries: impl IntoIterator<Item = Value>,
    revoked: impl IntoIterator<Item = Value>,
) -> Value {
    let entries: Vec<Value> = entries.into_iter().collect();
    let revoked: Vec<Value> = revoked.into_iter().collect();
    dated(json!({"schema": 1, "serial": serial, "entries": entries, "revoked": revoked}))
}

/// The keys the tests' catalogs verify under.
fn keys() -> Keys {
    sign(&listing(1, Vec::new(), Vec::new())).keys
}

/// The one key `keys` holds.
fn the_key(keys: &Keys) -> &'static str {
    keys.0.first().copied().unwrap_or_default()
}

/// The keys of a later build: the same signer among others, so the same
/// catalogs verify, but not the set an earlier state was taken in under.
fn replacement_keys() -> Keys {
    let own = keys();
    let replacement: &'static [&'static str] = Box::leak(Box::new(["another key", the_key(&own)]));
    Keys::new(replacement)
}

/// The state file in `owned`.
fn state_file(owned: &Owned) -> PathBuf {
    owned.0.join(DIRECTORY).join(FILE)
}

/// The trust state kept in `owned`, read as of `now`.
fn trust(owned: &Owned, now: SystemTime) -> Trust {
    Trust::load(&owned.0, now)
}

/// `catalog`, signed, taken into `trust` as of `now`.
fn take_in(trust: &mut Trust, catalog: &Value, now: SystemTime) -> Result<(), TrustError> {
    let signed = sign(catalog);
    trust.take_in(&signed.index, &signed.signature, &signed.keys, now)
}

/// `catalog` taken into `trust`, which must believe it.
// A test whose good catalog is refused has nothing else to report.
#[allow(clippy::panic)]
fn believe(trust: &mut Trust, catalog: &Value, now: SystemTime) {
    if let Err(refused) = take_in(trust, catalog, now) {
        panic!("a good catalog is taken in: {refused}");
    }
}

/// The versions of the agent `name` that `trust` offers, newest source first,
/// each with its lock revision where it has one.
fn offered(trust: &Trust, name: &str) -> Vec<String> {
    trust
        .offered(&agent(name))
        .iter()
        .map(|pin| match pin.lock_revision {
            Some(revision) => format!("{}+{revision}", pin.version),
            None => pin.version.to_string(),
        })
        .collect()
}

/// The release 0.48 shipped for one platform: the archive it was published
/// in, and the two files that archive holds, each as a name and a size.
struct Shipped {
    platform: &'static str,
    url: &'static str,
    sha256: &'static str,
    archive: u64,
    executable: (&'static str, u64),
    companion: (&'static str, u64),
}

/// The version 0.48 shipped.
const SHIPPED_VERSION: &str = "1.3.0";

/// What 0.48 shipped, platform by platform, as its reviewed manifest
/// recorded it: the oracle the snapshot built into the binary is held to.
const SHIPPED: [Shipped; 6] = [
    Shipped {
        platform: "darwin-aarch64",
        url: "https://dl.google.com/agy-extensions/releases/macos/agy-acp-server-1.3.0-darwin-arm64.zip",
        sha256: "7cd97045f7b4fe81175a107cdf16f9c51484e3c78a5162cae415338bb6aa5b88",
        archive: 111_456_962,
        executable: ("agy_acp_server.par", 278_535_456),
        companion: ("localharness_external", 118_611_392),
    },
    Shipped {
        platform: "darwin-x86_64",
        url: "https://dl.google.com/agy-extensions/releases/macos/agy-acp-server-1.3.0-darwin-x86_64.zip",
        sha256: "bb23956b89984bf5d354af2c3725e6c57f0cc1b7228e77a0e91c9c2bc1d47646",
        archive: 117_245_544,
        executable: ("agy_acp_server.par", 282_840_688),
        companion: ("localharness_external", 124_175_392),
    },
    Shipped {
        platform: "linux-x86_64",
        url: "https://dl.google.com/agy-extensions/releases/linux/agy-acp-server-1.3.0-linux-x86_64.zip",
        sha256: "9fb60956af0a9d76220a4db91ca9ac88e2a2372ad68f985ab5fceace6b825b96",
        archive: 333_727_150,
        executable: ("agy_acp_server.par", 926_533_965),
        companion: ("localharness_external", 130_388_040),
    },
    Shipped {
        platform: "linux-aarch64",
        url: "https://dl.google.com/agy-extensions/releases/linux/agy-acp-server-1.3.0-linux-arm64.zip",
        sha256: "500b0bc0fb858e88f4df404d4cedf80bf9298c178291e39e383d6c50b111cbdf",
        archive: 321_690_363,
        executable: ("agy_acp_server.par", 930_848_992),
        companion: ("localharness_external", 123_224_968),
    },
    Shipped {
        platform: "windows-x86_64",
        url: "https://dl.google.com/agy-extensions/releases/windows/agy-acp-server-1.3.0-windows-x86_64.zip",
        sha256: "65215e0688681fa3116e048a9eab27ef53af1bbd6f3da3f1c52bd4911d8b17f9",
        archive: 124_509_787,
        executable: ("agy_acp_server.exe", 81_437_336),
        companion: ("localharness_external.exe", 145_548_952),
    },
    Shipped {
        platform: "windows-aarch64",
        url: "https://dl.google.com/agy-extensions/releases/windows/agy-acp-server-1.3.0-windows-arm64.zip",
        sha256: "4a0f469720e9beb9438a979f543fdbfad5022ebe0992c052c590bd78b3144ca3",
        archive: 124_654_803,
        executable: ("agy_acp_server.exe", 85_893_472),
        companion: ("localharness_external.exe", 135_640_216),
    },
];

/// What 0.48 shipped for the platform this build runs on, where it shipped one.
fn shipped_here() -> Option<&'static Shipped> {
    SHIPPED.iter().find(|shipped| shipped.platform == here())
}

/// `listed`, then what the snapshot offers of Antigravity on this platform.
fn then_the_snapshot(listed: &[&str]) -> Vec<String> {
    listed
        .iter()
        .copied()
        .chain(shipped_here().map(|_| SHIPPED_VERSION))
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_platform_is_spelled_as_the_catalog_spells_it() {
    for (os, arch, spelled) in [
        ("macos", "aarch64", "darwin-aarch64"),
        ("macos", "x86_64", "darwin-x86_64"),
        ("linux", "x86_64", "linux-x86_64"),
        ("linux", "aarch64", "linux-aarch64"),
        ("windows", "x86_64", "windows-x86_64"),
        ("windows", "aarch64", "windows-aarch64"),
    ] {
        assert_eq!(platform(os, arch), spelled);
    }
}

#[test]
fn the_interim_snapshot_holds_what_0_48_shipped_field_for_field() {
    let [entry] = SNAPSHOT.entries() else {
        panic!("one entry");
    };

    assert!(SNAPSHOT.serial() >= 1, "a serial below it can be refused");
    assert!(SNAPSHOT.revoked().is_empty());
    assert_eq!(entry.key.as_str(), AGENT);
    assert_eq!(&*entry.version, SHIPPED_VERSION);
    assert_eq!(entry.lock_revision, None);
    assert_eq!(entry.kind, Kind::Binary);
    assert_eq!(&*entry.admits.name, "antigravity-acp");
    assert_eq!(&*entry.admits.version, SHIPPED_VERSION);
    assert_eq!(entry.targets.len(), SHIPPED.len());
    for shipped in &SHIPPED {
        let spelled = shipped.platform;
        let Some(target) = entry.targets.iter().find(|t| *t.platform == *spelled) else {
            panic!("a target for {spelled}");
        };
        assert_eq!(&*target.cmd, shipped.executable.0, "{spelled}");
        // 0.48 starts the agent on Linux with an empty user id, and with no
        // arguments of its own elsewhere.
        let arguments: Vec<&str> = target.args.iter().map(|argument| &**argument).collect();
        let started_with: &[&str] = if spelled.starts_with("linux-") {
            &["--uid="]
        } else {
            &[]
        };
        assert_eq!(arguments, started_with, "{spelled}");
        assert_eq!(target.launch, Launch::Binary, "{spelled}");
        assert_eq!(target.hosts, ["dl.google.com".into()], "{spelled}");
        let [archive] = target.artifacts.as_slice() else {
            panic!("one archive for {spelled}");
        };
        assert_eq!(&*archive.url, shipped.url, "{spelled}");
        assert_eq!(&*archive.sha256, shipped.sha256, "{spelled}");
        assert_eq!(archive.size, shipped.archive, "{spelled}");
        assert_eq!(archive.format, Format::Zip, "{spelled}");
        assert_eq!(archive.mirror, None, "{spelled}");
        assert_eq!(archive.replaced_by, None, "{spelled}");
        assert_eq!(
            archive.unpacked,
            shipped.executable.1 + shipped.companion.1,
            "{spelled}"
        );
        let Files::Listed(files) = &archive.files else {
            panic!("a listed file list for {spelled}");
        };
        let listed: Vec<(&str, Option<(u64, bool)>)> = files
            .iter()
            .map(|file| match file.kind {
                FileKind::File { size, exec, .. } => (&*file.path, Some((size, exec))),
                FileKind::Dir | FileKind::Link { .. } => (&*file.path, None),
            })
            .collect();
        assert_eq!(
            listed,
            [
                (shipped.executable.0, Some((shipped.executable.1, true))),
                (shipped.companion.0, Some((shipped.companion.1, true))),
            ],
            "{spelled}"
        );
    }
}

/// The file a front end tells the user to repair or remove is the one the
/// state is read from, whether or not it is there.
#[test]
fn the_state_file_is_named_by_the_trust_that_reads_it() {
    let owned = Owned::new("trust-file");

    let trust = Trust::load(&owned.0, taken());

    assert_eq!(trust.file(), state_file(&owned));
    assert!(!trust.file().exists());
}

/// A release offered says what the agent must answer with, as its entry does,
/// and a catalog lists only what it lists for this platform.
#[test]
fn an_offered_release_carries_what_its_entry_says_the_agent_answers_with() {
    let owned = Owned::new("trust-admits");
    let mut held = trust(&owned, taken());
    let catalog = listing(5, vec![release("antigravity", "1.4.1", None)], Vec::new());
    believe(&mut held, &catalog, taken());

    let pins = held.offered(&agent("antigravity"));

    let admitted: Vec<(&str, &str)> = pins
        .iter()
        .map(|pin| (&*pin.admits.name, &*pin.admits.version))
        .collect();
    let versions: Vec<&str> = pins.iter().map(|pin| &*pin.version).collect();
    assert_eq!(admitted.len(), versions.len());
    for ((_, answers), listed) in admitted.iter().zip(&versions) {
        assert_eq!(answers, listed);
    }
    assert_eq!(
        admitted.first().map(|(name, _)| *name),
        Some("goose"),
        "the entry's own name, not the name of an agent this build knows"
    );
    assert_eq!(
        SNAPSHOT
            .releases(&agent("antigravity"))
            .iter()
            .map(|pin| pin.version.to_string())
            .collect::<Vec<_>>(),
        then_the_snapshot(&[])
    );
    assert!(SNAPSHOT.releases(&agent("goose")).is_empty());
}

#[test]
fn nothing_taken_in_offers_the_snapshots_release_and_touches_no_disk() {
    let owned = Owned::new("trust-fresh");

    let trust = Trust::load(&owned.0, taken());

    let pins = trust.offered(&agent("antigravity"));
    let reviewed = shipped_here();
    assert_eq!(pins.len(), usize::from(reviewed.is_some()));
    if let (Some(shipped), Some(offered)) = (reviewed, pins.first()) {
        assert_eq!(offered.key.as_str(), "antigravity");
        assert_eq!(&*offered.version, SHIPPED_VERSION);
        assert_eq!(offered.lock_revision, None);
        assert_eq!(&*offered.target.cmd, shipped.executable.0);
        let [archive] = offered.target.artifacts.as_slice() else {
            panic!("one archive");
        };
        assert_eq!(
            (&*archive.url, &*archive.sha256, archive.size),
            (shipped.url, shipped.sha256, shipped.archive)
        );
    }
    assert!(offered(&trust, "goose").is_empty());
    assert_eq!(trust.unusable(), None);
    assert!(!owned.0.join(DIRECTORY).exists());
}

#[test]
fn a_taken_in_catalog_adds_its_releases_to_the_snapshots_and_keeps_them() {
    let owned = Owned::new("trust-adds");
    let mut held = trust(&owned, taken());
    let catalog = listing(
        5,
        vec![
            release("antigravity", "1.4.1", None),
            release("goose", "1.4.1", None),
        ],
        Vec::new(),
    );

    believe(&mut held, &catalog, taken());

    assert_eq!(offered(&held, "antigravity"), then_the_snapshot(&["1.4.1"]));
    assert_eq!(offered(&held, "goose"), ["1.4.1"]);
    let restarted = trust(&owned, taken());
    assert_eq!(
        offered(&restarted, "antigravity"),
        then_the_snapshot(&["1.4.1"])
    );
    assert_eq!(offered(&restarted, "goose"), ["1.4.1"]);
}

#[test]
fn an_older_serial_after_a_newer_one_is_refused() {
    let owned = Owned::new("trust-serial");
    let mut held = trust(&owned, taken());
    believe(&mut held, &listing(9, Vec::new(), Vec::new()), taken());

    let refused = take_in(&mut held, &listing(8, Vec::new(), Vec::new()), taken());

    assert!(
        matches!(
            refused,
            Err(TrustError::Catalog(CatalogError::Serial {
                offered: 8,
                held: 9
            }))
        ),
        "{refused:?}"
    );
    // The serial is kept, not remembered: a restart refuses it too, and the
    // serial held is still believed.
    let mut restarted = trust(&owned, taken());
    assert!(matches!(
        take_in(&mut restarted, &listing(8, Vec::new(), Vec::new()), taken()),
        Err(TrustError::Catalog(CatalogError::Serial {
            offered: 8,
            held: 9
        }))
    ));
    believe(&mut restarted, &listing(9, Vec::new(), Vec::new()), taken());
}

#[test]
fn a_revocation_survives_the_next_catalog_that_omits_it() {
    let owned = Owned::new("trust-revoked");
    let mut held = trust(&owned, taken());
    let both = || {
        vec![
            release("goose", "1.4.1", None),
            release("goose", "1.5.0", None),
        ]
    };

    believe(
        &mut held,
        &listing(5, both(), vec![withdrawn("goose", "1.4.1", None)]),
        taken(),
    );
    assert_eq!(offered(&held, "goose"), ["1.5.0"]);
    believe(&mut held, &listing(6, both(), Vec::new()), taken());

    assert_eq!(offered(&held, "goose"), ["1.5.0"]);
    assert_eq!(offered(&trust(&owned, taken()), "goose"), ["1.5.0"]);
}

#[test]
fn a_revocation_withdraws_what_the_snapshot_lists_too() {
    let owned = Owned::new("trust-revoked-snapshot");
    let mut held = trust(&owned, taken());
    assert_eq!(offered(&held, "antigravity"), then_the_snapshot(&[]));

    believe(
        &mut held,
        &listing(5, Vec::new(), vec![withdrawn("antigravity", "1.3.0", None)]),
        taken(),
    );

    assert!(offered(&held, "antigravity").is_empty());
    assert!(offered(&trust(&owned, taken()), "antigravity").is_empty());
}

#[test]
fn a_revocation_names_a_lock_revision_only_when_it_sets_one() {
    let owned = Owned::new("trust-revision");
    let mut held = trust(&owned, taken());
    let revisions = || {
        vec![
            release("gemini", "0.9.0", Some(1)),
            release("gemini", "0.9.0", Some(2)),
            release("gemini", "0.8.0", None),
        ]
    };
    believe(&mut held, &listing(5, revisions(), Vec::new()), taken());
    assert_eq!(offered(&held, "gemini"), ["0.9.0+1", "0.9.0+2", "0.8.0"]);

    believe(
        &mut held,
        &listing(6, revisions(), vec![withdrawn("gemini", "0.9.0", Some(1))]),
        taken(),
    );
    assert_eq!(offered(&held, "gemini"), ["0.9.0+2", "0.8.0"]);

    believe(
        &mut held,
        &listing(7, revisions(), vec![withdrawn("gemini", "0.9.0", None)]),
        taken(),
    );
    assert_eq!(offered(&held, "gemini"), ["0.8.0"]);
    // A revocation of a revision does not reach a release with none.
    believe(
        &mut held,
        &listing(8, revisions(), vec![withdrawn("gemini", "0.8.0", Some(3))]),
        taken(),
    );
    assert_eq!(offered(&trust(&owned, taken()), "gemini"), ["0.8.0"]);
}

#[test]
fn the_4097th_revocation_refuses_the_catalog() {
    let owned = Owned::new("trust-cap");
    let mut held = trust(&owned, taken());
    let withdrawals = |count: usize| -> Vec<Value> {
        (0..count)
            .map(|n| withdrawn("goose", &format!("v{n}"), None))
            .collect()
    };
    believe(
        &mut held,
        &listing(5, Vec::new(), withdrawals(REVOKED_CAP)),
        taken(),
    );
    // The same revocations again are not new ones.
    believe(
        &mut held,
        &listing(6, Vec::new(), withdrawals(REVOKED_CAP)),
        taken(),
    );

    let mut over = withdrawals(REVOKED_CAP);
    over.push(withdrawn("goose", "one-more", None));
    let refused = take_in(
        &mut held,
        &listing(7, vec![release("goose", "fine", None)], over),
        taken(),
    );

    assert!(
        matches!(refused, Err(TrustError::Revocations)),
        "{refused:?}"
    );
    // Nothing of the refused catalog was kept: not its serial, not its entry.
    assert!(offered(&held, "goose").is_empty());
    let mut restarted = trust(&owned, taken());
    assert!(offered(&restarted, "goose").is_empty());
    believe(&mut restarted, &listing(6, Vec::new(), Vec::new()), taken());
    // Nor does it take a catalog that brings the 4097th to a state with none.
    let alone = Owned::new("trust-cap-alone");
    let mut fresh = trust(&alone, taken());
    let mut all = withdrawals(REVOKED_CAP);
    all.push(withdrawn("goose", "one-more", None));
    assert!(matches!(
        take_in(&mut fresh, &listing(5, Vec::new(), all), taken()),
        Err(TrustError::Revocations)
    ));
}

#[test]
fn deleting_the_state_falls_back_to_the_snapshot_floor() {
    let owned = Owned::new("trust-deleted");
    let mut held = trust(&owned, taken());
    let goose = || vec![release("goose", "1.4.1", None)];
    believe(
        &mut held,
        &listing(9, goose(), vec![withdrawn("goose", "1.4.1", None)]),
        taken(),
    );
    assert!(offered(&held, "goose").is_empty());

    fs::remove_dir_all(owned.0.join(DIRECTORY)).unwrap_or_default();

    // The floor is the snapshot's serial, so a catalog below the one that was
    // held is believed again, and the revocation that was lost returns with
    // the catalog that repeats it, not before.
    let mut rebuilt = trust(&owned, taken());
    let floor = SNAPSHOT.serial();
    believe(
        &mut rebuilt,
        &listing(floor + 1, goose(), Vec::new()),
        taken(),
    );
    assert_eq!(offered(&rebuilt, "goose"), ["1.4.1"]);
    // And no lower: the floor is not nothing.
    let alone = Owned::new("trust-floor");
    let mut fresh = trust(&alone, taken());
    let refused = take_in(
        &mut fresh,
        &listing(floor - 1, Vec::new(), Vec::new()),
        taken(),
    );
    assert!(
        matches!(
            refused,
            Err(TrustError::Catalog(CatalogError::Serial { offered, held }))
                if offered == floor - 1 && held == floor
        ),
        "{refused:?}"
    );
}

#[test]
fn a_held_catalog_eight_days_past_its_expiry_offers_only_the_snapshots_releases() {
    let owned = Owned::new("trust-expired");
    let mut held = trust(&owned, taken());
    believe(
        &mut held,
        &listing(5, vec![release("antigravity", "1.4.1", None)], Vec::new()),
        at(EXPIRES) - DAY,
    );

    for (days_past, listed) in [(0, true), (6, true), (7, true), (8, false), (400, false)] {
        let later = trust(&owned, at(EXPIRES) + DAY * days_past);
        let wanted = if listed {
            then_the_snapshot(&["1.4.1"])
        } else {
            then_the_snapshot(&[])
        };
        assert_eq!(offered(&later, "antigravity"), wanted, "{days_past} days");
    }
    // The next fetch, signed to expire later, brings the catalog's releases
    // back; what was held before it was not forgotten, only not believed.
    let mut later = trust(&owned, at(EXPIRES) + DAY * 8);
    let mut renewed = listing(6, vec![release("antigravity", "1.4.2", None)], Vec::new());
    set(&mut renewed, "/expires", json!(EXPIRES + 30 * 24 * 60 * 60));
    believe(&mut later, &renewed, at(EXPIRES) + DAY * 8);
    assert_eq!(
        offered(&later, "antigravity"),
        then_the_snapshot(&["1.4.2"])
    );
}

#[test]
fn a_revocation_outlives_the_catalog_that_gave_it() {
    let owned = Owned::new("trust-revoked-expired");
    let mut held = trust(&owned, taken());
    let nothing = Owned::new("trust-revoked-expired-none");
    let late = at(EXPIRES) + DAY * 400;
    assert_eq!(
        offered(&trust(&nothing, late), "antigravity"),
        then_the_snapshot(&[])
    );
    believe(
        &mut held,
        &listing(5, Vec::new(), vec![withdrawn("antigravity", "1.3.0", None)]),
        at(EXPIRES) - DAY,
    );

    let later = trust(&owned, late);

    assert!(offered(&later, "antigravity").is_empty());
}

#[test]
fn a_catalog_that_does_not_verify_leaves_the_state_as_it_was() {
    let owned = Owned::new("trust-refused");
    let mut held = trust(&owned, taken());
    let good = listing(5, vec![release("goose", "1.4.1", None)], Vec::new());
    believe(&mut held, &good, taken());
    let before = fs::read(owned.0.join(DIRECTORY).join(FILE)).unwrap_or_default();
    assert!(!before.is_empty());

    let mut tampered = sign(&listing(
        6,
        vec![release("goose", "9.9.9", None)],
        Vec::new(),
    ));
    tampered.index.push(b' ');
    let bad_signature = held.take_in(
        &tampered.index,
        &tampered.signature,
        &tampered.keys,
        taken(),
    );
    let past_grace = take_in(
        &mut held,
        &listing(7, vec![release("goose", "9.9.9", None)], Vec::new()),
        at(EXPIRES) + DAY * 8,
    );

    assert!(
        matches!(
            bad_signature,
            Err(TrustError::Catalog(CatalogError::Signature))
        ),
        "{bad_signature:?}"
    );
    assert!(
        matches!(past_grace, Err(TrustError::Catalog(CatalogError::Expired))),
        "{past_grace:?}"
    );
    assert_eq!(offered(&held, "goose"), ["1.4.1"]);
    let after = fs::read(owned.0.join(DIRECTORY).join(FILE)).unwrap_or_default();
    assert_eq!(before, after);
    // And none is written where nothing was ever believed.
    let nothing = Owned::new("trust-nothing");
    let mut empty = trust(&nothing, taken());
    let refused = empty.take_in(
        &tampered.index,
        &tampered.signature,
        &tampered.keys,
        taken(),
    );
    assert!(refused.is_err());
    assert!(!nothing.0.join(DIRECTORY).exists());
}

#[test]
fn a_catalog_listing_one_release_twice_is_refused_by_its_own_error() {
    let owned = Owned::new("trust-repeated");
    let mut held = trust(&owned, taken());
    let twice = listing(
        5,
        vec![
            release("goose", "1.4.1", None),
            release("goose", "1.4.1", None),
        ],
        Vec::new(),
    );

    let refused = take_in(&mut held, &twice, taken());

    assert!(
        matches!(refused, Err(TrustError::Repeated { .. })),
        "{refused:?}"
    );
    assert!(offered(&held, "goose").is_empty());
    // A version listed under two lock revisions is two releases, not one twice.
    let revisions = listing(
        5,
        vec![
            release("goose", "1.4.1", Some(1)),
            release("goose", "1.4.1", Some(2)),
            release("goose", "1.4.1", None),
        ],
        Vec::new(),
    );
    believe(&mut held, &revisions, taken());
    assert_eq!(offered(&held, "goose"), ["1.4.1+1", "1.4.1+2", "1.4.1"]);
}

#[test]
fn a_release_the_snapshot_and_the_catalog_both_list_is_offered_once_from_the_catalog() {
    let owned = Owned::new("trust-both");
    let mut held = trust(&owned, taken());
    believe(
        &mut held,
        &listing(5, vec![release("antigravity", "1.3.0", None)], Vec::new()),
        taken(),
    );

    let pins = held.offered(&agent("antigravity"));

    let [pin] = pins.as_slice() else {
        panic!("one pin, not {}", pins.len());
    };
    assert_eq!(&*pin.version, "1.3.0");
    let url = pin.target.artifacts.first().map(|archive| &*archive.url);
    assert!(url.is_some_and(|url| url.starts_with("https://github.com/example/")));
}

#[test]
fn the_most_the_state_can_hold_is_kept_whole_within_its_ceiling() {
    let owned = Owned::new("trust-worst");
    let mut held = trust(&owned, taken());
    // The longest key and version, with quotes the file has to escape, and the
    // largest revision.
    let quoted = |n: usize| format!("{n:04}{}", "\"".repeat(VERSION_BYTES - 4));
    let longest = "a".repeat(32);
    let withdrawals: Vec<Value> = (0..REVOKED_CAP)
        .map(|n| {
            json!({
                "key": longest, "version": quoted(n),
                "lock_revision": u64::MAX, "reason": "x"
            })
        })
        .collect();
    believe(&mut held, &listing(5, Vec::new(), withdrawals), taken());
    // A catalog of the largest size, whose every byte the file has to escape.
    let mut padded = release("goose", "1.4.1", None);
    set(
        &mut padded,
        "/caution",
        json!("\"".repeat(INDEX_BYTES / 2 - 4096)),
    );
    let big = listing(6, vec![padded], Vec::new());
    assert!(serde_json::to_vec(&big).map_or(0, |bytes| bytes.len()) <= INDEX_BYTES);
    believe(&mut held, &big, taken());

    let length = fs::metadata(owned.0.join(DIRECTORY).join(FILE)).map_or(0, |meta| meta.len());

    assert!(length > 2_000_000, "{length} is the worst case");
    assert!(
        length <= u64::try_from(STATE_BYTES).unwrap_or(u64::MAX),
        "{length} bytes"
    );
    let mut restarted = trust(&owned, taken());
    assert_eq!(offered(&restarted, "goose"), ["1.4.1"]);
    let refused = take_in(
        &mut restarted,
        &listing(7, Vec::new(), vec![withdrawn("goose", "the 4097th", None)]),
        taken(),
    );
    assert!(
        matches!(refused, Err(TrustError::Revocations)),
        "{refused:?}"
    );
}

#[test]
fn keys_that_replace_the_old_ones_are_not_held_to_the_serial_the_old_ones_reached() {
    let owned = Owned::new("trust-replaced");
    let mut old = trust(&owned, taken());
    believe(
        &mut old,
        &listing(
            u64::MAX,
            vec![
                release("antigravity", "1.4.1", None),
                release("goose", "1.4.1", None),
            ],
            vec![withdrawn("goose", "1.4.1", None)],
        ),
        taken(),
    );
    // A launch reads the state with no key to check it under: what the old
    // keys made held is offered, and what they revoked is not.
    let mut reloaded = trust(&owned, taken());
    assert_eq!(
        offered(&reloaded, "antigravity"),
        then_the_snapshot(&["1.4.1"])
    );
    assert!(offered(&reloaded, "goose").is_empty());
    // A later build's keys: the same signer among others, so the same catalog
    // verifies, but it is not the set the state was taken in under.
    let replacement = replacement_keys();
    let signed = sign(&listing(
        3,
        vec![release("goose", "1.4.1", None)],
        Vec::new(),
    ));

    let taken_in = reloaded.take_in(&signed.index, &signed.signature, &replacement, taken());

    assert!(taken_in.is_ok(), "{taken_in:?}");
    // What the old keys made held is not kept, so not the release they
    // listed; what they revoked stays revoked, so the replacement's catalog
    // listing it does not offer it.
    assert_eq!(offered(&reloaded, "antigravity"), then_the_snapshot(&[]));
    assert!(offered(&reloaded, "goose").is_empty());
    assert!(offered(&trust(&owned, taken()), "goose").is_empty());
    // And the replacement keys are held to what they reached.
    let again = sign(&listing(2, Vec::new(), Vec::new()));
    let refused = reloaded.take_in(&again.index, &again.signature, &replacement, taken());
    assert!(
        matches!(
            refused,
            Err(TrustError::Catalog(CatalogError::Serial {
                offered: 2,
                held: 3
            }))
        ),
        "{refused:?}"
    );
}

#[test]
fn two_processes_that_loaded_before_either_took_in_lose_nothing() {
    let owned = Owned::new("trust-two");
    let mut first = trust(&owned, taken());
    let mut second = trust(&owned, taken());
    believe(
        &mut first,
        &listing(9, Vec::new(), vec![withdrawn("goose", "1.4.1", None)]),
        taken(),
    );

    // The second still holds what it loaded. A catalog older than the file's
    // serial is refused against the file, not against its memory.
    let stale = take_in(
        &mut second,
        &listing(5, vec![release("goose", "1.4.1", None)], Vec::new()),
        taken(),
    );

    assert!(
        matches!(
            stale,
            Err(TrustError::Catalog(CatalogError::Serial {
                offered: 5,
                held: 9
            }))
        ),
        "{stale:?}"
    );
    let mut restarted = trust(&owned, taken());
    assert!(offered(&restarted, "goose").is_empty());
    assert!(matches!(
        take_in(&mut restarted, &listing(6, Vec::new(), Vec::new()), taken()),
        Err(TrustError::Catalog(CatalogError::Serial {
            offered: 6,
            held: 9
        }))
    ));
    // One at or above the file's serial is merged with the file: the
    // revocation the first process took in is still there.
    let both = vec![
        release("goose", "1.4.1", None),
        release("goose", "1.5.0", None),
    ];
    believe(&mut second, &listing(10, both, Vec::new()), taken());
    assert_eq!(offered(&second, "goose"), ["1.5.0"]);
    assert_eq!(offered(&trust(&owned, taken()), "goose"), ["1.5.0"]);
}

#[test]
fn processes_taking_in_at_once_keep_every_revocation() {
    let owned = Owned::new("trust-threads");
    let processes = 8_usize;
    let releases = || -> Vec<Value> {
        (0..processes)
            .map(|n| release("goose", &format!("v{n}"), None))
            .collect()
    };
    let catalogs: Vec<Signed> = (0..processes)
        .map(|n| {
            sign(&listing(
                5,
                releases(),
                vec![withdrawn("goose", &format!("v{n}"), None)],
            ))
        })
        .collect();
    let loaded = Barrier::new(processes);

    // Every one loads before any takes in, so each holds a view that none of
    // the others' catalogs are in.
    let results: Vec<Result<(), TrustError>> = std::thread::scope(|scope| {
        let threads: Vec<_> = catalogs
            .iter()
            .map(|signed| {
                let (owned, loaded) = (&owned, &loaded);
                scope.spawn(move || {
                    let mut held = trust(owned, taken());
                    loaded.wait();
                    held.take_in(&signed.index, &signed.signature, &signed.keys, taken())
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap_or(Err(TrustError::Contended)))
            .collect()
    });

    assert!(results.iter().all(Result::is_ok), "{results:?}");
    assert!(offered(&trust(&owned, taken()), "goose").is_empty());
}

#[test]
fn a_state_that_is_there_and_cannot_be_used_is_refused_and_not_overwritten() {
    let owned = Owned::new("trust-unusable");
    let mut held = trust(&owned, taken());
    believe(
        &mut held,
        &listing(
            5,
            vec![release("goose", "1.4.1", None)],
            vec![withdrawn("antigravity", "1.3.0", None)],
        ),
        taken(),
    );
    let file = state_file(&owned);
    let good = fs::read(&file).unwrap_or_default();
    let other_format = String::from_utf8_lossy(&good).replacen(r#""format":1"#, r#""format":2"#, 1);
    assert_ne!(other_format.as_bytes(), good, "the format is written");

    for (what, garbage, why) in [
        (
            "another format",
            other_format.into_bytes(),
            Unusable::Format(2),
        ),
        ("not json", b"not json".to_vec(), Unusable::Corrupt),
        ("a list", b"[]".to_vec(), Unusable::Corrupt),
        ("no format", b"{}".to_vec(), Unusable::Corrupt),
        (
            "cut short",
            good.get(..good.len() / 2).unwrap_or_default().to_vec(),
            Unusable::Corrupt,
        ),
        ("too large", vec![b' '; STATE_BYTES + 1], Unusable::Oversize),
    ] {
        fs::write(&file, &garbage).unwrap_or_default();

        let mut read = trust(&owned, taken());

        assert_eq!(read.unusable(), Some(why), "{what}");
        // Its revocations cannot be known, so nothing is offered, not even the
        // snapshot's release.
        assert!(offered(&read, "goose").is_empty(), "{what}");
        assert!(offered(&read, "antigravity").is_empty(), "{what}");
        let refused = take_in(
            &mut read,
            &listing(6, vec![release("goose", "1.4.1", None)], Vec::new()),
            taken(),
        );
        assert!(
            matches!(&refused, Err(TrustError::Unusable(said)) if *said == why),
            "{what}: {refused:?}"
        );
        assert_eq!(fs::read(&file).unwrap_or_default(), garbage, "{what}");
    }
    // Only taking the file away makes it a state of nothing believed.
    fs::remove_file(&file).unwrap_or_default();
    let mut repaired = trust(&owned, taken());
    assert_eq!(repaired.unusable(), None);
    assert_eq!(offered(&repaired, "antigravity"), then_the_snapshot(&[]));
    believe(
        &mut repaired,
        &listing(2, vec![release("goose", "1.4.1", None)], Vec::new()),
        taken(),
    );
    assert_eq!(offered(&repaired, "goose"), ["1.4.1"]);
}

#[test]
fn a_state_another_name_reaches_is_unusable_until_the_name_is_gone() {
    let owned = Owned::new("trust-linked");
    let mut held = trust(&owned, taken());
    believe(
        &mut held,
        &listing(5, Vec::new(), vec![withdrawn("antigravity", "1.3.0", None)]),
        taken(),
    );
    let backup = owned.0.join("backup");
    fs::hard_link(state_file(&owned), &backup).unwrap_or_default();

    let mut linked = trust(&owned, taken());

    assert!(
        matches!(linked.unusable(), Some(Unusable::Unreadable(_))),
        "{:?}",
        linked.unusable()
    );
    assert!(offered(&linked, "antigravity").is_empty());
    let refused = take_in(&mut linked, &listing(6, Vec::new(), Vec::new()), taken());
    assert!(
        matches!(refused, Err(TrustError::Unusable(Unusable::Unreadable(_)))),
        "{refused:?}"
    );
    fs::remove_file(&backup).unwrap_or_default();
    let unlinked = trust(&owned, taken());
    assert_eq!(unlinked.unusable(), None);
    // Its revocation was there all along.
    assert!(offered(&unlinked, "antigravity").is_empty());
}

#[cfg(unix)]
#[test]
fn a_state_its_owner_cannot_read_is_unusable_and_not_deleted() {
    use std::os::unix::fs::PermissionsExt as _;
    let owned = Owned::new("trust-mode");
    let mut held = trust(&owned, taken());
    believe(
        &mut held,
        &listing(5, Vec::new(), vec![withdrawn("antigravity", "1.3.0", None)]),
        taken(),
    );
    let file = state_file(&owned);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap_or_default();
    // A process that can read a file whatever its mode, as root can, has no
    // unreadable state to refuse.
    if fs::File::open(&file).is_ok() {
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap_or_default();
        return;
    }

    let mut unreadable = trust(&owned, taken());

    assert!(
        matches!(unreadable.unusable(), Some(Unusable::Unreadable(_))),
        "{:?}",
        unreadable.unusable()
    );
    assert!(offered(&unreadable, "antigravity").is_empty());
    assert!(
        take_in(
            &mut unreadable,
            &listing(6, Vec::new(), Vec::new()),
            taken()
        )
        .is_err()
    );
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap_or_default();
    assert!(offered(&trust(&owned, taken()), "antigravity").is_empty());
}

#[test]
fn a_catalog_that_cannot_be_kept_still_narrows_what_this_process_offers() {
    let owned = Owned::new("trust-unkept");
    let mut held = trust(&owned, taken());
    let goose = || vec![release("goose", "1.4.1", None)];
    believe(&mut held, &listing(2, goose(), Vec::new()), taken());
    assert_eq!(offered(&held, "goose"), ["1.4.1"]);
    // The lock cannot be taken: its name is a directory.
    let lock = owned.0.join(DIRECTORY).join(LOCK);
    fs::remove_file(&lock).unwrap_or_default();
    fs::create_dir(&lock).unwrap_or_default();
    let revoking = listing(3, goose(), vec![withdrawn("goose", "1.4.1", None)]);

    let refused = take_in(&mut held, &revoking, taken());

    assert!(matches!(refused, Err(TrustError::Io(_))), "{refused:?}");
    // Revocations only narrow what is offered, so this process applies them
    // though they could not be kept.
    assert!(offered(&held, "goose").is_empty());
    // What was kept is as it was.
    assert_eq!(offered(&trust(&owned, taken()), "goose"), ["1.4.1"]);
    // And the catalog is believed once it can be kept.
    fs::remove_dir(&lock).unwrap_or_default();
    believe(&mut held, &revoking, taken());
    assert!(offered(&trust(&owned, taken()), "goose").is_empty());
}

#[test]
fn a_trust_state_another_process_holds_is_waited_for_a_while_and_then_refused() {
    let owned = Owned::new("trust-held");
    let wait = Duration::from_millis(200);
    let mut held = trust(&owned, taken()).waiting(wait);
    let goose = || vec![release("goose", "1.4.1", None)];
    believe(&mut held, &listing(2, goose(), Vec::new()), taken());
    let lock = crucible_privacy::lock(&owned.0.join(DIRECTORY).join(LOCK));
    assert!(
        lock.as_ref()
            .is_ok_and(|file| crucible_privacy::try_lock_identity(file).unwrap_or(false)),
        "the lock is taken"
    );
    let revoking = sign(&listing(
        3,
        goose(),
        vec![withdrawn("goose", "1.4.1", None)],
    ));

    let started = Instant::now();
    let refused = held.take_in(
        &revoking.index,
        &revoking.signature,
        &revoking.keys,
        taken(),
    );

    assert!(matches!(refused, Err(TrustError::Contended)), "{refused:?}");
    assert!(started.elapsed() >= wait, "{:?}", started.elapsed());
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(offered(&held, "goose").is_empty());
    // Let go, and the same catalog is kept.
    drop(lock);
    held.take_in(
        &revoking.index,
        &revoking.signature,
        &revoking.keys,
        taken(),
    )
    .unwrap_or_default();
    assert!(offered(&trust(&owned, taken()), "goose").is_empty());
}

#[test]
fn a_key_set_change_keeps_the_revocations_and_lowers_only_the_floor() {
    let owned = Owned::new("trust-reset");
    let mut old = trust(&owned, taken());
    believe(
        &mut old,
        &listing(
            9,
            vec![release("goose", "1.4.1", None)],
            vec![withdrawn("goose", "1.4.1", None)],
        ),
        taken(),
    );
    let replacement = replacement_keys();
    // A catalog from before the revocation, still signed by a key the new set
    // keeps.
    let stale = sign(&listing(
        2,
        vec![release("goose", "1.4.1", None)],
        Vec::new(),
    ));
    let mut launch = trust(&owned, taken());

    let taken_in = launch.take_in(&stale.index, &stale.signature, &replacement, taken());

    assert!(taken_in.is_ok(), "{taken_in:?}");
    assert!(offered(&launch, "goose").is_empty());
    assert!(offered(&trust(&owned, taken()), "goose").is_empty());
}

#[test]
fn a_key_set_change_drops_the_revocations_only_when_keeping_them_would_pass_the_cap() {
    let owned = Owned::new("trust-reset-cap");
    let mut old = trust(&owned, taken());
    let many: Vec<Value> = (0..REVOKED_CAP)
        .map(|n| withdrawn("goose", &format!("v{n}"), None))
        .collect();
    believe(&mut old, &listing(9, Vec::new(), many), taken());
    let replacement = replacement_keys();
    let fresh = sign(&listing(
        2,
        vec![
            release("goose", "v0", None),
            release("goose", "extra", None),
        ],
        vec![withdrawn("goose", "extra", None)],
    ));
    let mut launch = trust(&owned, taken());

    let taken_in = launch.take_in(&fresh.index, &fresh.signature, &replacement, taken());

    assert!(taken_in.is_ok(), "{taken_in:?}");
    // The old keys' revocations were more than fit beside the new one, so
    // they are gone and the new one stands.
    assert_eq!(offered(&launch, "goose"), ["v0"]);
    assert_eq!(offered(&trust(&owned, taken()), "goose"), ["v0"]);
}

/// A snapshot that withdraws `version` of `name`, as a built-in one could.
fn snapshot_withdrawing(name: &str, version: &str) -> Catalog {
    Catalog {
        serial: 1,
        signed_at: UNIX_EPOCH,
        expires: UNIX_EPOCH,
        entries: Vec::new(),
        revoked: vec![Revoked {
            key: agent(name),
            version: version.into(),
            lock_revision: None,
            reason: "It was withdrawn.".into(),
        }],
    }
}

#[test]
fn the_snapshots_own_revocations_are_in_every_base_state_and_count() {
    let snapshot = snapshot_withdrawing("antigravity", "1.3.0");
    let [reviewed] = SNAPSHOT.entries() else {
        panic!("one entry");
    };
    // A machine with nothing believed.
    assert!(State::rebuilt(&snapshot).withdraws(reviewed));
    assert!(!State::rebuilt(&SNAPSHOT).withdraws(reviewed));
    // A state written before the snapshot withdrew it.
    let owned = Owned::new("trust-snapshot-revoked");
    let mut held = trust(&owned, taken());
    believe(&mut held, &listing(5, Vec::new(), Vec::new()), taken());
    let kept = fs::read(state_file(&owned)).unwrap_or_default();
    assert!(matches!(decode(&kept, &snapshot), Ok(state) if state.withdraws(reviewed)));
    assert!(matches!(decode(&kept, &SNAPSHOT), Ok(state) if !state.withdraws(reviewed)));
    // And they take their place under the cap: a snapshot that withdraws as
    // many as are kept leaves no room for one more.
    let full = Catalog {
        revoked: (0..REVOKED_CAP)
            .map(|n| Revoked {
                key: agent("goose"),
                version: format!("v{n}").into(),
                lock_revision: None,
                reason: "It was withdrawn.".into(),
            })
            .collect(),
        ..snapshot
    };
    let one_more = fetched_at(
        &listing(5, Vec::new(), vec![withdrawn("goose", "one-more", None)]),
        taken(),
    );
    let Ok(one_more) = one_more else {
        panic!("a good catalog is believed");
    };
    let merged = merge(State::rebuilt(&full), &one_more, "keys", &full);
    assert!(matches!(merged, Err(TrustError::Revocations)));
}

#[test]
fn leftover_drafts_of_a_crashed_writer_are_removed_and_do_not_block_the_next() {
    let owned = Owned::new("trust-drafts");
    let mut held = trust(&owned, taken());
    believe(
        &mut held,
        &listing(2, vec![release("goose", "1.4.1", None)], Vec::new()),
        taken(),
    );
    // Every name this process could draw, left by a process that had the same
    // id and died before it renamed.
    let directory = owned.0.join(DIRECTORY);
    for n in 0..5_000_u32 {
        fs::write(
            directory.join(format!(".trust.{}.{n}.writing", std::process::id())),
            b"x",
        )
        .unwrap_or_default();
    }

    let kept = take_in(
        &mut held,
        &listing(3, Vec::new(), vec![withdrawn("goose", "1.4.1", None)]),
        taken(),
    );

    assert!(kept.is_ok(), "{kept:?}");
    let left: Vec<String> = fs::read_dir(&directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".writing"))
        .collect();
    assert!(left.is_empty(), "{} drafts left", left.len());
    assert!(offered(&trust(&owned, taken()), "goose").is_empty());
}

#[test]
fn a_release_published_only_for_another_platform_is_not_offered_here() {
    let owned = Owned::new("trust-platform");
    let mut held = trust(&owned, taken());
    let mut elsewhere = release("goose", "1.4.1", None);
    set(
        &mut elsewhere,
        "/targets/0/platform",
        json!("plan9-riscv64"),
    );
    // One listing a target for another platform ahead of this one's.
    let mut both = release("gemini", "0.9.0", None);
    let mut other = both.pointer("/targets/0").cloned().unwrap_or_default();
    set(&mut other, "/platform", json!("plan9-riscv64"));
    let mut targets = vec![other];
    targets.extend(
        both.pointer("/targets")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    );
    set(&mut both, "/targets", json!(targets));

    believe(
        &mut held,
        &listing(2, vec![elsewhere, both], Vec::new()),
        taken(),
    );

    assert!(offered(&held, "goose").is_empty());
    let pins = held.offered(&agent("gemini"));
    let [pin] = pins.as_slice() else {
        panic!("one pin, not {}", pins.len());
    };
    assert_eq!(*pin.target.platform, *here());
}

/// The profile `catalog`'s first entry gives the agent installed from its
/// first target.
// A test whose entry gives no profile has nothing else to report.
#[allow(clippy::panic)]
fn profiled(catalog: &Value) -> Result<CatalogProfile, Unprofiled> {
    let Ok(catalog) = fetched(catalog) else {
        panic!("the catalog is believed");
    };
    let Some(entry) = catalog.entries().first() else {
        panic!("an entry");
    };
    let Some(target) = entry.targets.first() else {
        panic!("a target");
    };
    CatalogProfile::from_entry(entry, target)
}

/// An entry's profile signs its agent in only by the methods the agent
/// carries out itself, starts it with its target's arguments and variables,
/// each of those replacing the shell's value of the same name, and passes it
/// nothing else of the shell but what every agent is passed, which alone is
/// shown.
#[test]
fn an_entry_gives_its_agent_its_own_sign_in_arguments_variables_and_hosts() {
    let catalog = with(
        "/entries/0/targets/0/env",
        json!({"GOOSE_MODE": "acp", "LANG": "C.UTF-8"}),
    );
    let profile = profiled(&catalog).unwrap_or_else(|why| panic!("{why}"));
    let ambient = [
        ("PATH", "/usr/bin"),
        ("LANG", "en_GB.UTF-8"),
        ("GOOGLE_API_KEY", "ambient-sentinel-value"),
    ]
    .map(|(name, value)| (name.into(), value.into()));

    let request = profile.request(ambient, None);

    let methods: Vec<&str> = profile.sign_in().iter().map(SignIn::id).collect();
    assert_eq!(methods, ["browser"]);
    let hosts: Vec<&str> = profile
        .reach()
        .iter()
        .map(SandboxDomainPattern::as_str)
        .collect();
    assert_eq!(hosts, ["api.example.com"]);
    assert_eq!(request.arguments, ["acp"]);
    let mut environment = request.environment.clone();
    environment.sort();
    let expected = [
        ("GOOSE_MODE", "acp"),
        ("LANG", "C.UTF-8"),
        ("PATH", "/usr/bin"),
    ]
    .map(|(name, value)| (name.to_owned(), value.to_owned()));
    assert_eq!(environment, expected);
    assert_eq!(request.shown, ["PATH"]);
}

/// An entry's profile says which variable its agent reads its home from and
/// where under the user's home it keeps its sign-in, as its entry lists them.
#[test]
fn an_entry_says_where_its_agent_keeps_its_home_and_its_sign_in() {
    let profile = profiled(&catalog()).unwrap_or_else(|why| panic!("{why}"));

    assert_eq!(profile.profile_env(), Some("GOOSE_HOME"));
    assert_eq!(profile.state_paths(), [Box::<str>::from(".config/goose")]);

    let bare = profiled(&without("/entries/0/profile_env")).unwrap_or_else(|why| panic!("{why}"));
    assert_eq!(bare.profile_env(), None);
}

/// A profile directory is given to the agent as the value of the variable its
/// entry names, exactly as written and not marked as shown, replacing the
/// target's own value of it; without one, that variable is not set from the
/// shell either, so no stray value of the user's reaches the agent as though
/// it were a profile.
#[test]
fn a_profile_directory_is_applied_through_the_variable_the_entry_names() {
    let catalog = with("/entries/0/targets/0/env", json!({"GOOSE_HOME": "/target"}));
    let profile = profiled(&catalog).unwrap_or_else(|why| panic!("{why}"));
    let ambient = || {
        [("PATH", "/usr/bin"), ("GOOSE_HOME", "/shell")]
            .map(|(name, value)| (name.into(), value.into()))
    };

    let given = profile.request(ambient(), Some(Path::new("/profiles/work")));
    let held: Vec<&str> = given
        .environment
        .iter()
        .filter(|(name, _)| name == "GOOSE_HOME")
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(held, ["/profiles/work"]);
    assert!(!given.shown.contains(&"GOOSE_HOME"));

    let without = profile.request(ambient(), None);
    let held: Vec<&str> = without
        .environment
        .iter()
        .filter(|(name, _)| name == "GOOSE_HOME")
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(held, ["/target"]);
}

/// A variable an agent reads its home from that the shell also passes, as
/// `HOME` is, is the profile directory where there is one, and is not passed
/// from the shell where there is none.
#[test]
fn the_shells_value_of_the_profile_variable_is_never_passed() {
    let profile = profiled(&with("/entries/0/profile_env", json!("HOME")))
        .unwrap_or_else(|why| panic!("{why}"));
    let ambient = || {
        [("PATH", "/usr/bin"), ("HOME", "/home/user")]
            .map(|(name, value)| (name.into(), value.into()))
    };

    let given = profile.request(ambient(), Some(Path::new("/profiles/work")));
    let mut names: Vec<(&str, &str)> = given
        .environment
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            ("GOOSE_MODE", "acp"),
            ("HOME", "/profiles/work"),
            ("PATH", "/usr/bin")
        ]
    );

    let alone = profile.request(ambient(), None);
    let mut names: Vec<&str> = alone
        .environment
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["GOOSE_MODE", "PATH"]);
}

/// A host an entry's agent reaches is granted only as the sandbox's parser
/// reads it, and one that names every host, or every host under a single
/// label, is refused by where it is listed.
#[test]
fn a_host_that_names_every_host_is_never_granted() {
    for (reach, refused) in [
        (json!(["*"]), Some("reach[0] names every host")),
        (
            json!(["api.example.com", "*.com"]),
            Some("reach[1] names every host"),
        ),
        (
            json!(["https://api.example.com"]),
            Some("reach[0] is not a host a sandbox can grant"),
        ),
        (json!(["*.example.com"]), None),
    ] {
        let profile = profiled(&with("/entries/0/reach", reach.clone()));

        match (profile, refused) {
            (Ok(profile), None) => {
                let hosts: Vec<&str> = profile
                    .reach()
                    .iter()
                    .map(SandboxDomainPattern::as_str)
                    .collect();
                assert_eq!(json!(hosts), reach);
            }
            (Err(why), Some(said)) => assert!(why.to_string().starts_with(said), "{reach}: {why}"),
            (profile, refused) => panic!("{reach}: {profile:?}, not {refused:?}"),
        }
    }
}

/// A host an entry's agent reaches is named, never numbered: an address in
/// any spelling, bracketed, with a port, mapped into IPv6, under a wildcard
/// or as a C resolver reads digits, is refused by where it is listed, public
/// or not, while a name is still granted.
#[test]
fn an_address_is_never_granted_as_a_host_an_agent_reaches() {
    for address in [
        "169.254.169.254",
        "127.0.0.1",
        "10.0.0.5",
        "8.8.8.8",
        "::1",
        "fd00:ec2::254",
        "::ffff:169.254.169.254",
        "[::1]",
        "[fd00:ec2::254]:443",
        "127.0.0.1:443",
        "127.0.0.1.",
        "*.10.0.0.5",
        "127.1",
        "2130706433",
        "0x7f.1",
        "0X7F000001",
    ] {
        let refused = profiled(&with(
            "/entries/0/reach",
            json!(["api.example.com", address]),
        ));

        match refused {
            Err(why) => assert!(
                why.to_string()
                    .starts_with("reach[1] is an address, and an agent's hosts are named"),
                "{address}: {why}"
            ),
            Ok(profile) => panic!("{address} was granted: {profile:?}"),
        }
    }
    for named in [
        "api.example.com",
        "localhost",
        "a1.example.com",
        "*.example.com",
    ] {
        let profile = profiled(&with("/entries/0/reach", json!([named])))
            .unwrap_or_else(|why| panic!("{named}: {why}"));
        let hosts: Vec<&str> = profile
            .reach()
            .iter()
            .map(SandboxDomainPattern::as_str)
            .collect();
        assert_eq!(hosts, [named]);
    }
}

/// An entry whose agent's name or version is as long as an answer to
/// initializing is cut to gives no profile, since a longer answer cut to the
/// same bytes would be taken for it; one a byte shorter than any cut answer
/// can be is compared whole.
#[test]
fn an_admitted_name_or_version_an_answer_could_be_cut_to_is_refused() {
    let whole = "n".repeat(252);
    let cut = "n".repeat(253);
    for (pointer, field) in [
        ("/entries/0/admits/name", "admits.name"),
        ("/entries/0/admits/version", "admits.version"),
    ] {
        let refused = profiled(&with(pointer, json!(cut)));
        let admitted = profiled(&with(pointer, json!(whole)));

        match refused {
            Err(why) => assert!(why.to_string().starts_with(field), "{field}: {why}"),
            Ok(profile) => panic!("{field} of 253 bytes gave {profile:?}"),
        }
        assert!(admitted.is_ok(), "{field}: {admitted:?}");
    }
}

/// An entry the intake describes, before any catalog signs it, is read by the
/// reader that believes a signed one: its fields come out the same, and a
/// field the reader refuses is refused here, naming the field.
#[test]
fn an_entry_is_read_unsigned_by_the_reader_that_believes_a_signed_one() {
    let mut described = binary();
    *described
        .pointer_mut("/targets/0/platform")
        .expect("a platform") = json!("plan9-mips");
    let entry = Entry::unsigned(described.to_string().as_bytes()).expect("an entry");

    assert_eq!(entry.key.as_str(), "goose");
    assert_eq!(&*entry.admits.name, "goose");
    assert_eq!(entry.auth_methods.len(), 3);
    assert_eq!(
        entry.here().map(|target| &*target.cmd),
        None,
        "its one target is for another platform"
    );

    *described
        .pointer_mut("/targets/0/platform")
        .expect("a platform") = json!(here());
    let entry = Entry::unsigned(described.to_string().as_bytes()).expect("an entry");
    assert_eq!(entry.here().map(|target| &*target.cmd), Some("goose"));

    *described.pointer_mut("/targets/0/env").expect("an env") = json!({"LD_PRELOAD": "x"});
    let refused = Entry::unsigned(described.to_string().as_bytes()).expect_err("a loader");
    assert!(refused.to_string().contains("env"), "{refused}");
}

/// The catalog the intake renders from merged review data is signed here as
/// the publishing job signs it, and the reader believes it: at the moment it
/// was signed it is current, its serial is the one after the catalog before it,
/// and it lives for thirty days. The clock is the catalog's own, so the test
/// does not age.
#[test]
fn a_catalog_the_intake_renders_verifies_under_the_key_that_signed_it() {
    let rendered: &[u8] =
        include_bytes!("../../../../tests/fixtures/agent-catalog/render/catalog.json");
    let document: Value = serde_json::from_slice(rendered).expect("rendered JSON");
    let signed_at = document
        .get("signed_at")
        .and_then(Value::as_u64)
        .expect("a signing time");

    let signed = signed(rendered);
    assert_eq!(
        signed.index, rendered,
        "dated by the intake, so the signer adds nothing"
    );
    let catalog = Catalog::verify(
        &signed.index,
        &signed.signature,
        &signed.keys,
        Held(7),
        at(signed_at),
    )
    .expect("a rendered catalog verifies");

    assert_eq!(catalog.serial(), 8);
    assert_eq!(catalog.signed_at(), at(signed_at));
    assert_eq!(catalog.expires(), at(signed_at) + 30 * DAY);
    let keys: Vec<&str> = catalog
        .entries()
        .iter()
        .map(|entry| entry.key.as_str())
        .collect();
    assert_eq!(keys, ["big", "fake", "nodeagent", "pyagent"]);
    let [.., node, python] = catalog.entries() else {
        panic!("an npm entry and an interpreter entry");
    };
    assert_eq!(node.kind, Kind::Npm);
    assert_eq!(node.lock_revision, Some(2));
    let [target] = node.targets.as_slice() else {
        panic!("one npm target");
    };
    assert_eq!(
        target.launch,
        Launch::Npm {
            node_args: Vec::new(),
            bin: "node_modules/node-agent/bin/agent.js".into(),
            lock: Lock {
                asset: AssetRef {
                    sha256: "9".repeat(64).into(),
                    size: 512,
                },
                unpack_rules: 1,
            },
            scripts: vec![Script {
                package: "node_modules/node-agent".into(),
                event: "postinstall".into(),
                disposition: Disposition::Inert,
            }],
        },
        "the rendered npm target starts as the intake reviewed it"
    );
    assert_eq!(python.kind, Kind::Uvx);
    let [target] = python.targets.as_slice() else {
        panic!("one target");
    };
    assert_eq!(
        target.launch,
        Launch::Uvx {
            interpreter_args: vec!["-I".into()],
            entry_point: "pyagent.acp:main".into(),
        }
    );
    let revoked: Vec<(&str, &str)> = catalog
        .revoked()
        .iter()
        .map(|revoked| (revoked.key.as_str(), &*revoked.version))
        .collect();
    assert_eq!(revoked, [("antigravity", "1.2.0"), ("fake", "0.9.0")]);
    assert!(
        Catalog::verify(
            &signed.index,
            &signed.signature,
            &signed.keys,
            Held(9),
            at(signed_at)
        )
        .is_err(),
        "a catalog older than the one held is not taken"
    );
}

#[test]
fn an_offer_carries_its_entry_and_the_serial_of_the_catalog_that_listed_it() {
    let owned = Owned::new("catalog-offers");
    let mut trust = trust(&owned, taken());
    believe(
        &mut trust,
        &listing(
            SNAPSHOT.serial() + 4,
            [
                release("goose", "1.4.1", None),
                release("goose", "1.4.0", None),
                release("kiro", "2.0.0", None),
            ],
            [withdrawn("goose", "1.4.0", None)],
        ),
        taken(),
    );

    let goose = trust.offers(Some(&agent("goose")));
    let every = trust.offers(None);

    assert_eq!(goose.len(), 1, "{goose:?}");
    let offer = goose.first().expect("an offer");
    assert_eq!(&*offer.pin.version, "1.4.1");
    assert_eq!(offer.entry.key, agent("goose"));
    assert_eq!(&*offer.entry.license, "Apache-2.0");
    assert_eq!(
        offer.entry.terms.as_deref(),
        Some("https://example.com/terms")
    );
    assert_eq!(offer.serial, SNAPSHOT.serial() + 4);
    let keys: Vec<&str> = every
        .iter()
        .map(|offer| offer.entry.key.as_str())
        .filter(|key| *key != "antigravity")
        .collect();
    assert_eq!(keys, ["goose", "kiro"]);
    assert!(
        every
            .iter()
            .filter(|offer| offer.entry.key.as_str() == "antigravity")
            .all(|offer| offer.serial == SNAPSHOT.serial())
    );
    assert!(trust.revoked(&agent("goose"), "1.4.0", None));
    assert!(trust.revoked(&agent("goose"), "1.4.0", Some(3)));
    assert!(!trust.revoked(&agent("goose"), "1.4.1", None));
}

#[test]
fn a_trust_state_that_cannot_be_used_offers_nothing_and_counts_every_release_revoked() {
    let owned = Owned::new("catalog-offers-unusable");
    fs::create_dir_all(owned.0.join(DIRECTORY)).expect("the directory");
    fs::write(state_file(&owned), b"not a trust state").expect("the file");
    let trust = trust(&owned, taken());

    assert!(trust.offers(None).is_empty());
    assert!(trust.revoked(&agent("goose"), "1.4.1", None));
}

// The official ACP Registry: what it offers, what it refuses, and the entry
// the adapter builds from it.

use super::registry::{RegistryIndex, Unoffered};

/// One agent of a registry index, published at `platform` with `target`.
fn registry_agent(id: &str, platform: &str, target: &Value) -> Value {
    json!({
        "id": id,
        "name": id,
        "version": "1.0.0",
        "license": "Apache-2.0",
        "distribution": {"binary": {platform: target}},
    })
}

/// A binary target of the registry at `archive`, with `sha256` where given.
fn registry_target(archive: &str, sha256: Option<&str>) -> Value {
    match sha256 {
        Some(sha256) => json!({
            "archive": archive,
            "cmd": "./bin\\agent",
            "sha256": sha256,
            "args": ["acp"],
            "env": {"AGENT_HOME": "/tmp/agent"},
        }),
        None => json!({"archive": archive, "cmd": "./agent", "args": ["acp"]}),
    }
}

/// A registry index for the platform this build runs on: a hashed `.tar.gz`,
/// an unhashed target, a hashed `.tar.bz2`, four hashed targets on four hosts
/// that are not GitHub, an agent with no binary at all, one with only another
/// platform, and one whose archive this crucible does not unpack.
fn registry(platform: &str) -> Value {
    let hashed = hash('a');
    let host = |name: &str| format!("https://{name}/agent-1.0.0.tar.gz");
    json!({
        "version": "1.0.0",
        "agents": [
            registry_agent("hashed", platform, &registry_target(&host("downloads.example.com"), Some(&hashed))),
            registry_agent("unhashed", platform, &registry_target(&host("unhashed.example.com"), None)),
            registry_agent("bzipped", platform, &registry_target("https://downloads.example.com/agent-1.0.0.tar.bz2", Some(&hashed))),
            registry_agent("one", platform, &registry_target(&host("one.example.com"), Some(&hashed))),
            registry_agent("two", platform, &registry_target(&host("two.example.com"), Some(&hashed))),
            registry_agent("three", platform, &registry_target(&host("three.example.com"), Some(&hashed))),
            registry_agent("four", platform, &registry_target(&host("four.example.com"), Some(&hashed))),
            json!({
                "id": "npx", "name": "npx", "version": "1.0.0", "license": "MIT",
                "distribution": {"npx": {"package": "npx@1.0.0"}},
            }),
            json!({
                "id": "elsewhere", "name": "elsewhere", "version": "1.0.0", "license": "MIT",
                "distribution": {"binary": {"plan9-x86_64": {
                    "archive": "https://downloads.example.com/agent.zip",
                    "cmd": "./agent.exe",
                    "sha256": hash('b'),
                }}},
            }),
            registry_agent("zipped", platform, &registry_target("https://downloads.example.com/agent.rar", Some(&hashed))),
        ],
    })
}

#[allow(clippy::expect_used)] // a fixture that cannot be built is the test's own failure
fn registry_of(platform: &str) -> RegistryIndex {
    RegistryIndex::parse(
        &serde_json::to_vec(&registry(platform)).expect("a registry is JSON"),
        at(0),
    )
    .expect("a registry index")
}

#[test]
fn a_registry_offers_only_a_hashed_target_with_an_archive_crucible_unpacks() {
    let platform = super::trust::here();
    let index = registry_of(&platform);
    let offered: Vec<&str> = index
        .offers()
        .iter()
        .map(|offer| offer.entry().id.as_str())
        .collect();

    assert_eq!(
        offered,
        ["hashed", "bzipped", "one", "two", "three", "four"],
        "the hashed targets this build unpacks, and no other"
    );
    assert_eq!(index.offers().len(), 6, "the per-platform count");
    for id in ["unhashed", "npx", "elsewhere", "zipped"] {
        assert!(index.offer(&agent(id)).is_err(), "{id} is offered");
    }
    let why = index
        .offer(&agent("unhashed"))
        .expect_err("no hash")
        .to_string();
    assert!(why.contains("SHA-256"), "{why}");
    assert_eq!(
        index.offer(&agent("npx")).expect_err("no binary"),
        Unoffered::Platform {
            id: agent("npx"),
            platform: platform.as_str().into(),
        }
    );
    assert_eq!(
        index
            .offer(&agent("elsewhere"))
            .expect_err("another platform"),
        Unoffered::Platform {
            id: agent("elsewhere"),
            platform: platform.as_str().into(),
        }
    );
    assert!(matches!(
        index
            .offer(&agent("zipped"))
            .expect_err("an archive to unpack"),
        Unoffered::Archive { .. }
    ));
    assert!(matches!(
        index.offer(&agent("missing")).expect_err("no such agent"),
        Unoffered::Unknown { .. }
    ));
}

/// Each entry the index lists for this build's platform, by id, with why it is
/// not offered where it is not.
fn listed_here(index: &RegistryIndex) -> Vec<(&str, Option<Unoffered>)> {
    index
        .listed()
        .into_iter()
        .map(|(entry, offer)| (entry.id.as_str(), offer.err()))
        .collect()
}

/// An entry whose target here carries no SHA-256 is listed, with the reason
/// that names the hash, and is still never offered.
#[test]
fn an_unhashed_target_is_listed_with_its_reason_and_never_offered() {
    let platform = super::trust::here();
    let index = registry_of(&platform);

    let listed = listed_here(&index);

    assert!(
        listed.contains(&(
            "unhashed",
            Some(Unoffered::Unhashed {
                id: agent("unhashed"),
                platform: platform.as_str().into(),
            })
        )),
        "{listed:?}"
    );
    assert!(
        index
            .offers()
            .iter()
            .all(|offer| offer.entry().id.as_str() != "unhashed")
    );
    assert!(index.offer(&agent("unhashed")).is_err());
}

/// An entry whose archive here is one crucible does not unpack is listed,
/// with the reason that names the archive, and is still never offered.
#[test]
fn an_unread_archive_is_listed_with_its_reason_and_never_offered() {
    let platform = super::trust::here();
    let index = registry_of(&platform);

    let listed = listed_here(&index);

    assert!(
        listed.contains(&(
            "zipped",
            Some(Unoffered::Archive {
                id: agent("zipped"),
                platform: platform.as_str().into(),
                archive: "https://downloads.example.com/agent.rar".into(),
            })
        )),
        "{listed:?}"
    );
    assert!(
        index
            .offers()
            .iter()
            .all(|offer| offer.entry().id.as_str() != "zipped")
    );
    assert!(index.offer(&agent("zipped")).is_err());
}

/// The listing holds every entry with a target here, offered or not, and
/// every entry published here only as a package, with why it is not offered,
/// in registry order; none with only another platform's binary. A package
/// entry is shown only where the name it guesses is found on `PATH`, and the
/// fixture's guesses none, being a package runner's own name.
#[test]
fn only_another_platforms_binary_is_not_listed() {
    let platform = super::trust::here();
    let index = registry_of(&platform);

    let listed = listed_here(&index);
    let ids: Vec<&str> = listed.iter().map(|(id, _)| *id).collect();
    let offered: Vec<&str> = listed
        .iter()
        .filter(|(_, why)| why.is_none())
        .map(|(id, _)| *id)
        .collect();

    assert_eq!(
        ids,
        [
            "hashed", "unhashed", "bzipped", "one", "two", "three", "four", "npx", "zipped"
        ]
    );
    assert!(
        listed.contains(&(
            "npx",
            Some(Unoffered::Package {
                id: agent("npx"),
                platform: platform.as_str().into(),
                runner: super::registry::Runner::Npx,
            })
        )),
        "{listed:?}"
    );
    let npx = index.entry(&agent("npx")).expect("listed");
    assert_eq!(
        OwnEntry::registry(npx, &platform).expect("read"),
        None,
        "a package runner's own name is no guess"
    );
    assert_eq!(
        offered,
        ["hashed", "bzipped", "one", "two", "three", "four"]
    );
}

#[test]
fn a_registry_target_reaches_its_archive_host_and_the_four_redirect_hosts() {
    let index = registry_of(&super::trust::here());
    let offer = index.offer(&agent("one")).expect("an offer");
    let hosted = offer.target().hosts();
    let hosts: Vec<&str> = hosted.iter().map(|host| &**host).collect();

    assert_eq!(
        hosts,
        [
            "one.example.com",
            "github.com",
            "objects.githubusercontent.com",
            "release-assets.githubusercontent.com",
            "github-releases.githubusercontent.com",
        ]
    );
    for id in ["two", "three", "four"] {
        let offer = index.offer(&agent(id)).expect("an offer");
        assert!(
            offer
                .target()
                .hosts()
                .iter()
                .any(|host| **host == format!("{id}.example.com")),
            "{id}"
        );
    }
}

#[test]
fn a_registry_index_is_bounded_and_refused_when_it_is_not_one() {
    let too_large = vec![b' '; INDEX_BYTES + 1];
    assert!(matches!(
        RegistryIndex::parse(&too_large, at(0)),
        Err(super::registry::RegistryError::Size(_))
    ));

    assert!(matches!(
        RegistryIndex::parse(b"not json", at(0)),
        Err(super::registry::RegistryError::Json(_))
    ));

    let mut future = registry(&super::trust::here());
    set(&mut future, "/version", json!("2.0.0"));
    assert!(matches!(
        RegistryIndex::parse(&serde_json::to_vec(&future).expect("JSON"), at(0)),
        Err(super::registry::RegistryError::Field { .. })
    ));

    let crowded = json!({
        "version": "1.0.0",
        "agents": (0..=super::registry::AGENTS)
            .map(|index| json!({
                "id": format!("agent-{index}"), "name": "agent", "version": "1.0.0",
                "license": "MIT", "distribution": {},
            }))
            .collect::<Vec<_>>(),
    });
    assert!(matches!(
        RegistryIndex::parse(&serde_json::to_vec(&crowded).expect("JSON"), at(0)),
        Err(super::registry::RegistryError::Agents(_))
    ));
}

#[test]
fn the_registry_adapter_builds_an_entry_the_catalog_reader_validates() {
    let index = registry_of(&super::trust::here());
    let offer = index.offer(&agent("hashed")).expect("an offer");
    let entry = offer.provisional().expect("an entry");
    let target = entry.here().expect("a target here");

    assert_eq!(entry.key, agent("hashed"));
    assert_eq!(&*entry.registry_id, "hashed");
    assert_eq!(&*entry.version, "1.0.0");
    assert_eq!(&*entry.review, super::registry::URL);
    assert_eq!(&*target.cmd, "bin/agent");
    assert_eq!(target.args, ["acp".into()]);
    let artifact = target.artifacts.first().expect("an artifact");
    assert_eq!(&*artifact.sha256, hash('a'));
    assert_eq!(artifact.size, crate::release::ARCHIVE_BYTES);
    assert_eq!(target.hosts.len(), 5);
}

#[test]
fn the_registry_adapter_refuses_what_the_catalog_reader_refuses() {
    let platform = super::trust::here();
    let build = |target: Value| {
        let index = RegistryIndex::parse(
            &serde_json::to_vec(&json!({
                "version": "1.0.0",
                "agents": [registry_agent("agent", &platform, &target)],
            }))
            .expect("JSON"),
            at(0),
        )
        .expect("an index");
        index
            .offer(&agent("agent"))
            .expect("an offer")
            .provisional()
    };

    let loader = build(registry_target(
        "https://downloads.example.com/agent.tar.gz",
        Some(&hash('a')),
    ));
    assert!(loader.is_ok(), "the base entry is valid");

    let absolute = build(json!({
        "archive": "https://downloads.example.com/agent.tar.gz",
        "cmd": "/usr/bin/agent",
        "sha256": hash('a'),
    }))
    .expect_err("an absolute command");
    assert!(absolute.to_string().contains("cmd"), "{absolute}");

    let climbing = build(json!({
        "archive": "https://downloads.example.com/agent.tar.gz",
        "cmd": "bin/../..\\..\\agent",
        "sha256": hash('a'),
    }))
    .expect_err("a command that leaves its directory");
    assert!(climbing.to_string().contains("cmd"), "{climbing}");

    let variable = build(json!({
        "archive": "https://downloads.example.com/agent.tar.gz",
        "cmd": "./agent",
        "sha256": hash('a'),
        "env": {"LD_PRELOAD": "lib.so"},
    }))
    .expect_err("a loader variable");
    assert!(variable.to_string().contains("LD_PRELOAD"), "{variable}");
}

/// A receipt of an installed registry release, surveyed at what it holds.
fn registry_receipt(version: &str, artifact: &str) -> crate::store::Receipt {
    let files: Vec<File> = [
        ("agent", 10_u64, hash('c'), true),
        ("LICENSE", 3, hash('d'), false),
    ]
    .into_iter()
    .map(|(path, size, sha256, exec)| File {
        path: path.into(),
        kind: FileKind::File {
            size,
            sha256: sha256.into(),
            exec,
        },
    })
    .collect();
    crate::store::Receipt {
        key: agent("hashed"),
        version: version.into(),
        lock_revision: None,
        listing: crate::store::Listing {
            serial: 0,
            review: super::registry::URL.into(),
            terms: None,
        },
        source: crate::store::Source::Registry,
        probe: None,
        unpacked: 13,
        decompressed: 20,
        fetched: at(0),
        origin: crate::store::Origin::Fetched,
        artifacts: vec![crate::store::Fetched {
            url: "https://downloads.example.com/agent-1.0.0.tar.gz".into(),
            hosts: vec!["downloads.example.com".into()],
            size: 100,
            sha256: artifact.into(),
        }],
        files,
    }
}

/// The probe record of the same install.
fn registry_probe() -> crate::store::Probe {
    crate::store::Probe {
        target: super::trust::here().into(),
        admits: Admits {
            name: "agent".into(),
            version: "2.0.0".into(),
        },
        auth_methods: vec![
            crate::store::ProbedMethod {
                id: "key".into(),
                name: "API key".into(),
                kind: crate::store::ProbedKind::EnvVar,
            },
            crate::store::ProbedMethod {
                id: "login".into(),
                name: "Log in".into(),
                kind: crate::store::ProbedKind::Terminal {
                    command: "agent".into(),
                    args: ["login".into()].into(),
                    env: vec![Variable {
                        name: "AGENT_LOGIN".into(),
                        value: "1".into(),
                    }],
                    tty: Some(false),
                },
            },
            crate::store::ProbedMethod {
                id: "browser".into(),
                name: "Sign in".into(),
                kind: crate::store::ProbedKind::Agent,
            },
            crate::store::ProbedMethod {
                id: "other".into(),
                name: "Other".into(),
                kind: crate::store::ProbedKind::Other,
            },
        ],
    }
}

#[test]
fn the_registry_adapter_rebuilds_an_installed_release_from_its_receipt_and_probe() {
    let index = registry_of(&super::trust::here());
    let offer = index.offer(&agent("hashed")).expect("an offer");
    let receipt = registry_receipt("1.0.0", &hash('e'));
    let probe = registry_probe();
    let entry = offer
        .installed(&receipt, &probe)
        .expect("an installed entry");
    let target = entry.here().expect("a target here");

    assert_eq!(&*entry.version, "1.0.0");
    assert_eq!(&*entry.admits.name, "agent");
    assert_eq!(&*entry.admits.version, "2.0.0");
    assert_eq!(
        entry
            .auth_methods
            .iter()
            .map(|method| &*method.id)
            .collect::<Vec<_>>(),
        ["key", "login", "browser"],
        "the variable is named from the target's one, the other kind is left out"
    );
    assert!(matches!(
        entry.auth_methods.first().map(|method| &method.method),
        Some(Method::EnvVar { var }) if &**var == "AGENT_HOME"
    ));
    assert!(matches!(
        entry.auth_methods.get(1).map(|method| &method.method),
        Some(Method::Terminal {
            route: Route::Piped,
            ..
        })
    ));
    let artifact = target.artifacts.first().expect("an artifact");
    assert_eq!(&*artifact.sha256, hash('e'), "the receipt's hash");
    assert_eq!(artifact.size, 100);
    assert_eq!(artifact.unpacked, 13);
    assert_eq!(artifact.decompressed, 20);
    assert!(matches!(&artifact.files, Files::Listed(files) if files.len() == 2));
    assert!(
        entry.content_use.is_none(),
        "the registry carries no statement"
    );
    assert_eq!(&*entry.review, super::registry::URL);
}

#[test]
fn a_registry_receipts_probe_verification_agrees_with_its_launch_refusal() {
    let target = super::registry::platform();
    let mut receipt = registry_receipt("1.0.0", &hash('e'));
    let named = |receipt: &crate::store::Receipt| {
        super::registry::receipt_mismatches(receipt, &target)
            .iter()
            .any(|one| &*one.path == "probe")
    };

    assert_eq!(
        named(&receipt),
        receipt.unprobed(&target).is_some(),
        "a receipt with no probe"
    );
    receipt.probe = Some(Box::new(registry_probe()));
    assert_eq!(
        named(&receipt),
        receipt.unprobed(&target).is_some(),
        "a probe for this machine"
    );
    let mut foreign = registry_probe();
    foreign.target = "plan9-x86_64".into();
    receipt.probe = Some(Box::new(foreign));
    assert_eq!(
        named(&receipt),
        receipt.unprobed(&target).is_some(),
        "a probe for another machine"
    );
}

#[test]
fn a_registry_rows_survey_past_the_listed_files_is_refused() {
    let index = registry_of(&super::trust::here());
    let offer = index.offer(&agent("hashed")).expect("an offer");
    let mut receipt = registry_receipt("1.0.0", &hash('e'));
    receipt.files = (0..=super::LISTED_FILES)
        .map(|index| File {
            path: format!("file-{index}").into(),
            kind: FileKind::File {
                size: 0,
                sha256: hash('a').into(),
                exec: false,
            },
        })
        .collect();

    let refused = offer
        .installed(&receipt, &registry_probe())
        .expect_err("a survey past the listed files");
    assert!(refused.to_string().contains("files"), "{refused}");
}

// A program the user installed, as a registry or catalog row proposes it: the
// name it is guessed to go by, its arguments and its environment, and nothing
// pinned.

use super::OwnEntry;

/// `entry` read as the catalog reads one, its one target moved to the
/// platform this build runs on.
#[allow(clippy::panic)]
fn entry_here(mut entry: Value) -> Entry {
    set(&mut entry, "/targets/0/platform", json!(here()));
    Entry::unsigned(entry.to_string().as_bytes()).unwrap_or_else(|why| panic!("an entry: {why}"))
}

/// A registry index of one agent published here with `target`.
#[allow(clippy::panic)]
fn registry_with(target: &Value) -> RegistryIndex {
    let index = json!({
        "version": "1.0.0",
        "agents": [registry_agent("own", &here(), target)],
    });
    RegistryIndex::parse(index.to_string().as_bytes(), at(0))
        .unwrap_or_else(|why| panic!("a registry index: {why}"))
}

/// A registry row with no SHA-256, or with an archive this crucible cannot
/// unpack, is never offered for install, yet still proposes the program the
/// user may have installed themselves, with its arguments and environment.
#[test]
fn an_own_entry_without_sha256_is_read_but_never_installable() {
    let platform = here();
    let index = registry_of(&platform);
    for (id, why) in [
        ("unhashed", "no SHA-256"),
        ("zipped", "an archive it cannot unpack"),
    ] {
        assert!(
            index.offer(&agent(id)).is_err(),
            "{id}, with {why}, is installable"
        );
        let entry = index.entry(&agent(id)).expect("listed");
        let own = OwnEntry::registry(entry, &platform)
            .expect("an own row is read")
            .expect("a binary for this platform");
        assert_eq!(own.guess(), "agent", "{id}: the file name of its cmd");
        assert_eq!(own.args(), [Box::from("acp")], "{id}");
    }
    let zipped = index.entry(&agent("zipped")).expect("listed");
    let own = OwnEntry::registry(zipped, &platform)
        .expect("read")
        .expect("a binary");
    assert_eq!(
        own.env(),
        [Variable {
            name: "AGENT_HOME".into(),
            value: "/tmp/agent".into(),
        }]
    );
    for id in ["npx", "elsewhere"] {
        let entry = index.entry(&agent(id)).expect("listed");
        assert_eq!(
            OwnEntry::registry(entry, &platform).expect("read"),
            None,
            "{id} publishes no binary for this platform"
        );
    }
    let absolute = registry_with(&json!({
        "archive": "https://downloads.example.com/agent.tar.gz",
        "cmd": "/usr/bin/agent",
    }));
    let refused = OwnEntry::registry(absolute.entry(&agent("own")).expect("listed"), &platform)
        .expect_err("a cmd outside its directory");
    assert!(refused.to_string().contains("cmd"), "{refused}");
}

/// A row that sets a variable a shell reads to choose a file of code to run
/// first is refused, in any case, as a loader's variable is.
#[test]
fn an_own_entry_setting_bash_env_or_env_is_refused() {
    let platform = here();
    for name in ["BASH_ENV", "ENV", "bash_env", "Env", "LD_PRELOAD", "PATH"] {
        let index = registry_with(&json!({
            "archive": "https://downloads.example.com/agent.tar.gz",
            "cmd": "./agent",
            "env": {name: "/tmp/hook.sh"},
        }));
        let refused = OwnEntry::registry(index.entry(&agent("own")).expect("listed"), &platform)
            .expect_err("a variable that chooses what code runs");
        assert!(
            matches!(&refused, CatalogError::Loader { name: refused, .. } if &**refused == name),
            "{name}: {refused}"
        );

        let mut signed = binary();
        set(&mut signed, "/targets/0/env", json!({name: "/tmp/hook.sh"}));
        set(&mut signed, "/targets/0/platform", json!(here()));
        if let Ok(entry) = Entry::unsigned(signed.to_string().as_bytes()) {
            let refused = OwnEntry::catalog(&entry).expect_err("a shell's variable");
            assert!(refused.to_string().contains(name), "{name}: {refused}");
        }
    }
    let index = registry_with(&json!({
        "archive": "https://downloads.example.com/agent.tar.gz",
        "cmd": "./agent",
        "env": {"BASH_ENVIRONMENT": "x", "ENVY": "y"},
    }));
    assert!(
        OwnEntry::registry(index.entry(&agent("own")).expect("listed"), &platform).is_ok(),
        "a name that only begins like one is not refused"
    );
}

/// No row's guess is a package runner, whatever it is read from: a signed
/// catalog binary's `cmd`, a registry binary's `cmd` or a registry package's
/// name, each naming every runner in any case and with or without `.exe` or
/// `.cmd`, proposes nothing on `PATH`. A name that only begins like a runner's
/// is still guessed.
#[test]
fn a_guess_naming_a_package_runner_is_refused_on_every_path() {
    let platform = here();
    let registry_binary = |cmd: &str| {
        let index = registry_with(&json!({
            "archive": "https://downloads.example.com/agent.tar.gz",
            "cmd": cmd,
        }));
        OwnEntry::registry(index.entry(&agent("own")).expect("listed"), &platform)
            .expect("read")
            .map(|own| own.guess().to_owned())
    };
    let catalog_binary = |cmd: &str| {
        let mut signed = binary();
        set(&mut signed, "/targets/0/cmd", json!(cmd));
        OwnEntry::catalog(&entry_here(signed))
            .expect("read")
            .map(|own| own.guess().to_owned())
    };
    let package = |runner: &str, name: &str| {
        own_packaged(&json!({runner: {"package": name}})).map(|own| own.guess().to_owned())
    };

    for runner in super::own::RUNNERS {
        for name in [
            (*runner).to_owned(),
            runner.to_ascii_uppercase(),
            format!("{runner}.exe"),
            format!("{}.CMD", runner.to_ascii_uppercase()),
        ] {
            assert_eq!(
                registry_binary(&format!("./{name}")),
                None,
                "registry ./{name}"
            );
            assert_eq!(
                registry_binary(&format!("bin/{name}")),
                None,
                "registry bin/{name}"
            );
            assert_eq!(
                catalog_binary(&format!("bin/{name}")),
                None,
                "catalog bin/{name}"
            );
            assert_eq!(package("npx", &format!("{name}@1.0.0")), None, "npx {name}");
            assert_eq!(
                package("npx", &format!("@example/{name}@1.0.0")),
                None,
                "npx @example/{name}"
            );
            assert_eq!(package("uvx", &format!("{name}==1.0")), None, "uvx {name}");
        }
    }

    assert_eq!(registry_binary("./npx-agent").as_deref(), Some("npx-agent"));
    assert_eq!(registry_binary("bin/uvxagent").as_deref(), Some("uvxagent"));
    assert_eq!(catalog_binary("bin/bunny").as_deref(), Some("bunny"));
    assert_eq!(
        package("npx", "npx-agent@1.0.0").as_deref(),
        Some("npx-agent")
    );
    assert_eq!(
        package("uvx", "uvx-agent==1.0").as_deref(),
        Some("uvx-agent")
    );
}

/// A registry index of one agent, `own`, whose `distribution` is
/// `distribution`.
#[allow(clippy::panic)]
fn registry_packaged(distribution: &Value) -> RegistryIndex {
    let index = json!({
        "version": "1.0.0",
        "agents": [{
            "id": "own", "name": "own", "version": "1.0.0", "license": "MIT",
            "distribution": distribution,
        }],
    });
    RegistryIndex::parse(index.to_string().as_bytes(), at(0))
        .unwrap_or_else(|why| panic!("a registry index: {why}"))
}

/// What the registry's one agent, published as `distribution`, proposes for
/// the platform this build runs on.
#[allow(clippy::panic)]
fn own_packaged(distribution: &Value) -> Option<OwnEntry> {
    let index = registry_packaged(distribution);
    let entry = index
        .entry(&agent("own"))
        .unwrap_or_else(|| panic!("{distribution}: not listed"));
    OwnEntry::registry(entry, &here()).unwrap_or_else(|why| panic!("{distribution}: {why}"))
}

/// A registry entry published only as a package proposes the name its package
/// is guessed to go by on `PATH`: an npx package's last path segment without a
/// trailing `@<version>`, a uvx package without a trailing `==<version>`. Any
/// other version specifier, extras, an address, a path or a package runner's
/// own name gives none. A binary for this platform is guessed from first, then
/// npx, then uvx; the package's arguments and environment are kept.
#[test]
fn a_package_name_gives_its_last_segment_without_the_version() {
    for (runner, package, guess) in [
        ("npx", "@xai-official/grok@1.0.80", Some("grok")),
        ("uvx", "fast-agent-acp==0.10.1", Some("fast-agent-acp")),
        ("npx", "@google/gemini-cli", Some("gemini-cli")),
        ("npx", "cline@3.0.74", Some("cline")),
        ("uvx", "fast-agent-acp", Some("fast-agent-acp")),
        ("uvx", "fast-agent-acp>=0.10", None),
        ("uvx", "fast-agent-acp~=0.10", None),
        ("uvx", "fast-agent-acp[extra]==0.10.1", None),
        ("uvx", "fast-agent-acp==0.10.1; python_version>'3'", None),
        ("uvx", "minion-code@0.1.44", None),
        ("uvx", "git+https://example.com/agent.git", None),
        ("uvx", "./agent", None),
        ("npx", "agent@>=1.0", None),
        ("npx", "agent@^1.0", None),
        ("npx", "https://example.com/agent.tgz", None),
        ("npx", "github:example/agent", None),
        ("npx", "example/agent", None),
        ("npx", "@example/agent/more@1.0", None),
        ("npx", "./agent", None),
        ("npx", "../agent", None),
        ("npx", "/usr/lib/agent", None),
        ("npx", "..", None),
        ("npx", "-agent", None),
        ("npx", "npx@1.0.0", None),
        ("npx", "@example/UVX.cmd@1.0.0", None),
        ("uvx", "uvx==1.0", None),
    ] {
        let own = own_packaged(&json!({runner: {"package": package}}));
        assert_eq!(
            own.as_ref().map(OwnEntry::guess),
            guess,
            "{runner} {package}"
        );
    }

    let grok = own_packaged(&json!({"npx": {
        "package": "@xai-official/grok@1.0.80",
        "args": ["agent", "stdio"],
        "env": {"GROK_MODE": "acp"},
    }}))
    .expect("a guess");
    assert_eq!(grok.args(), [Box::from("agent"), Box::from("stdio")]);
    assert_eq!(
        grok.env(),
        [Variable {
            name: "GROK_MODE".into(),
            value: "acp".into(),
        }]
    );

    let both = own_packaged(&json!({
        "uvx": {"package": "second==1.0"},
        "npx": {"package": "first@1.0"},
    }))
    .expect("a guess");
    assert_eq!(both.guess(), "first", "npx before uvx");
    let binary = own_packaged(&json!({
        "binary": {here(): {
            "archive": "https://downloads.example.com/agent.tar.gz",
            "cmd": "./bin/binary",
        }},
        "npx": {"package": "first@1.0"},
    }))
    .expect("a guess");
    assert_eq!(binary.guess(), "binary", "a binary here before a package");
}

/// A registry entry published only as a package that sets a variable a loader
/// or a shell reads to choose what code runs is refused, as a binary target
/// that sets one is; a name that only begins like one is not.
#[test]
fn a_package_entry_setting_a_loader_variable_is_refused() {
    let platform = here();
    for runner in ["npx", "uvx"] {
        for name in [
            "BASH_ENV",
            "ENV",
            "bash_env",
            "LD_PRELOAD",
            "NODE_OPTIONS",
            "PYTHONPATH",
            "PATH",
        ] {
            let index = registry_packaged(&json!({runner: {
                "package": "agent",
                "env": {name: "/tmp/hook.sh"},
            }}));
            let refused =
                OwnEntry::registry(index.entry(&agent("own")).expect("listed"), &platform)
                    .expect_err("a variable that chooses what code runs");
            assert!(
                matches!(&refused, CatalogError::Loader { name: refused, .. } if &**refused == name),
                "{runner} {name}: {refused}"
            );
        }
        let kept = own_packaged(&json!({runner: {
            "package": "agent",
            "env": {"BASH_ENVIRONMENT": "x", "ENVY": "y"},
        }}));
        assert_eq!(
            kept.as_ref().map(OwnEntry::guess),
            Some("agent"),
            "{runner}: a name that only begins like one is not refused"
        );
    }
}

/// The shipped sources that name a package runner as text, each with why
/// that text starts nothing.
const NAMING_A_RUNNER: &[(&str, &str)] = &[
    (
        "crates/crucible-builtins/src/bash/wrapper.rs",
        "classifies a command a model asks the shell to run",
    ),
    (
        "crates/crucible-config/src/shape.rs",
        "shows a command an MCP server of the user's own may be",
    ),
    (
        "crates/crucible-external-harnesses/src/catalog/entry.rs",
        "reads a catalog entry's kind",
    ),
    (
        "crates/crucible-external-harnesses/src/catalog/registry.rs",
        "reads the keys a registry distribution names a runner by",
    ),
    (
        "crates/crucible-external-harnesses/src/catalog/own.rs",
        "lists the names no package is guessed to be",
    ),
];

/// Every place in `source` that names a package runner as text, as
/// `(runner, starts it)`: whether it is the program a process is made for.
fn runners_named(source: &str) -> Vec<(&'static str, bool)> {
    let squeezed: String = source.chars().filter(|c| !c.is_whitespace()).collect();
    let mut found = Vec::new();
    for runner in super::own::RUNNERS {
        for suffix in ["", ".exe", ".cmd"] {
            let quoted = format!("\"{runner}{suffix}\"");
            for (at, _) in squeezed.match_indices(&quoted) {
                let started = squeezed
                    .get(..at)
                    .is_some_and(|before| before.ends_with("new(") || before.ends_with("new(r"));
                found.push((*runner, started));
            }
        }
    }
    found
}

/// Every Rust source a shipped program is built from in the workspace at
/// `root`: each crate's `src` and the binary's, without a test module's file.
fn shipped_sources(root: &Path) -> Vec<PathBuf> {
    let mut directories = vec![root.join("src")];
    if let Ok(crates) = fs::read_dir(root.join("crates")) {
        directories.extend(crates.flatten().map(|krate| krate.path().join("src")));
    }
    let mut sources = Vec::new();
    while let Some(directory) = directories.pop() {
        let Ok(read) = fs::read_dir(&directory) else {
            continue;
        };
        for found in read.flatten() {
            let path = found.path();
            let name = found.file_name();
            if name == "tests" || name == "tests.rs" {
                continue;
            }
            if path.is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push(path);
            }
        }
    }
    sources.sort();
    sources
}

/// No shipped program starts a package runner: none makes a process for one,
/// and only the sources [`NAMING_A_RUNNER`] lists name one as text at all.
#[test]
fn nothing_runs_a_package_runner() {
    for runner in ["npx", "uvx"] {
        assert!(super::own::RUNNERS.contains(&runner), "{runner}");
    }
    assert_eq!(
        runners_named("let child = Command::new( \"npx\" ).arg(package);"),
        [("npx", true)]
    );
    assert_eq!(
        runners_named("SandboxCommand::new(\"uvx.exe\", args)"),
        [("uvx", true)]
    );
    assert_eq!(
        runners_named("match key { \"npx\" => Runner::Npx }"),
        [("npx", false)]
    );
    assert!(runners_named("Command::new(\"npxx\")").is_empty());

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sources = shipped_sources(&root);
    assert!(
        sources.len() > 100,
        "only {} sources were searched",
        sources.len()
    );
    let mut refused = Vec::new();
    for path in &sources {
        let source = fs::read_to_string(path).expect("a shipped source is text");
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let listed = NAMING_A_RUNNER.iter().any(|(listed, _)| *listed == shown);
        for (runner, started) in runners_named(&source) {
            if started || !listed {
                refused.push(format!("{shown}: {runner}"));
            }
        }
    }
    assert!(refused.is_empty(), "a package runner is named: {refused:?}");
    for (listed, why) in NAMING_A_RUNNER {
        assert!(root.join(listed).is_file(), "{listed}, which {why}");
    }
}

/// An npm or uvx catalog entry starts a pinned interpreter, not a program of
/// its own, so it proposes nothing on `PATH`; a binary entry proposes the file
/// name of its `cmd`.
#[test]
fn a_catalog_npm_or_uvx_entry_gives_no_guess() {
    for entry in [uvx(), npm()] {
        let entry = entry_here(entry);
        assert_eq!(
            OwnEntry::catalog(&entry).expect("read"),
            None,
            "{}",
            entry.key
        );
    }
    let mut goose = binary();
    set(&mut goose, "/targets/0/cmd", json!("bin/goose"));
    let goose = entry_here(goose);
    let own = OwnEntry::catalog(&goose)
        .expect("read")
        .expect("a binary here");
    assert_eq!(own.guess(), "goose");
    assert_eq!(own.args(), [Box::from("acp")]);
    assert_eq!(
        own.env(),
        [Variable {
            name: "GOOSE_MODE".into(),
            value: "acp".into(),
        }]
    );
    let elsewhere = Entry::unsigned(binary().to_string().as_bytes()).expect("an entry");
    if elsewhere.here().is_none() {
        assert_eq!(
            OwnEntry::catalog(&elsewhere).expect("read"),
            None,
            "no target here"
        );
    }
}

/// A signed entry still pins what it installs, whatever an own row may omit:
/// an artifact with no SHA-256 is refused by name.
#[test]
fn an_entry_whose_artifact_has_no_sha256_is_still_refused() {
    let refused = fetched(&without("/entries/0/targets/0/artifacts/0/sha256"));
    assert!(
        matches!(
            &refused,
            Err(CatalogError::Field { field, why: "is missing" })
                if **field == *"entries[0].targets[0].artifacts[0].sha256"
        ),
        "{:?}",
        refused.map(|_| "believed")
    );
}

/// A program the user installed is run with its own arguments, under the
/// hosts of the signed entry it stands for, and is admitted by answering in
/// the protocol with an `agentCapabilities` object, whatever it calls itself.
#[test]
fn an_own_profile_is_admitted_by_its_capabilities_and_never_by_its_name() {
    let mut goose = binary();
    set(&mut goose, "/targets/0/cmd", json!("bin/goose"));
    let goose = entry_here(goose);
    let own = OwnEntry::catalog(&goose).expect("read").expect("here");
    let profile = CatalogProfile::from_own(
        goose.key.clone(),
        &own,
        Some(&goose),
        goose.auth_methods.clone(),
    )
    .expect("a profile");
    assert_eq!(
        profile.reach(),
        CatalogProfile::from_entry(&goose, goose.here().expect("here"))
            .expect("a store profile")
            .reach()
    );
    let registry =
        CatalogProfile::from_own(goose.key.clone(), &own, None, Vec::new()).expect("a profile");
    assert!(registry.reach().is_empty() && registry.sign_in_hosts().is_empty());
    assert!(registry.state_paths().is_empty() && registry.profile_env().is_none());
    for answer in [
        json!({"protocolVersion": 1, "agentCapabilities": {}}),
        json!({"protocolVersion": 1, "agentCapabilities": {"loadSession": true},
               "agentInfo": {"name": "someone-else", "version": "0.0.1"}}),
    ] {
        let initialized = crate::acp::Initialized::read(&answer);
        assert!(profile.admits(&initialized).is_ok(), "{answer}");
    }
    for answer in [
        json!({"protocolVersion": 1}),
        json!({"protocolVersion": 1, "agentCapabilities": null}),
        json!({"protocolVersion": 1, "agentCapabilities": []}),
    ] {
        let initialized = crate::acp::Initialized::read(&answer);
        assert!(profile.admits(&initialized).is_err(), "{answer}");
    }
    let request = profile.request(Vec::new(), None);
    assert_eq!(request.arguments, ["acp"]);
}
