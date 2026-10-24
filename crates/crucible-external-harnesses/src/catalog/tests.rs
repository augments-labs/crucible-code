//! A catalog is believed only signed, bounded, current and in a known schema,
//! and is parsed into every field an entry carries. What a user is offered
//! from it is decided by the trust state and the snapshot, whatever the
//! catalogs a machine has seen since.

use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crucible_types::AgentKey;
use serde_json::{Value, json};

use super::fixture::{Signed, signed};
use super::trust::{DIRECTORY, FILE, REVOKED_CAP, STATE_BYTES, here, platform};
use super::{
    Admits, AssetRef, AuthMethod, Catalog, CatalogError, ContentUse, Disposition, FileKind, Files,
    Format, GRACE, Held, INDEX_BYTES, KEYS, Keys, Kind, Launch, Lock, Method, Route, SCHEMA_SHOWN,
    SNAPSHOT, Script, Trust, TrustError, VERSION_BYTES, Variable,
};
use crate::antigravity::Manifest;
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
                {"package": "node_modules/d", "event": "install", "disposition": "built"},
                {
                    "package": "node_modules/e", "event": "postinstall", "disposition": "local",
                    "hosts": ["*.example.com"], "result": {"sha256": hash('4'), "size": 900}
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
fn fetched(catalog: &Value) -> Result<Catalog, CatalogError> {
    let signed = sign(catalog);
    Catalog::verify(
        &signed.index,
        &signed.signature,
        &signed.keys,
        Held(0),
        SystemTime::now(),
    )
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
                script("node_modules/d", "install", Disposition::Built),
                script(
                    "node_modules/e",
                    "postinstall",
                    Disposition::Local {
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
            json!("tar.bz2"),
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

/// `listed`, then what the snapshot offers of Antigravity on this platform.
fn then_the_snapshot(listed: &[&str]) -> Vec<String> {
    listed
        .iter()
        .copied()
        .chain(Manifest::REVIEWED.here().map(|pin| pin.version))
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
fn the_interim_snapshot_agrees_with_the_reviewed_manifest_field_for_field() {
    let [entry] = SNAPSHOT.entries() else {
        panic!("one entry");
    };

    assert_eq!(Manifest::REVIEWED.versions, [&*entry.version]);
    assert!(SNAPSHOT.serial() >= 1, "a serial below it can be refused");
    assert!(SNAPSHOT.revoked().is_empty());
    assert_eq!(entry.key.as_str(), "antigravity");
    assert_eq!(&*entry.version, "1.3.0");
    assert_eq!(entry.lock_revision, None);
    assert_eq!(entry.kind, Kind::Binary);
    assert_eq!(&*entry.admits.name, Manifest::REVIEWED.name);
    assert_eq!(&*entry.admits.version, "1.3.0");
    assert_eq!(entry.targets.len(), Manifest::REVIEWED.platforms.len());
    for (os, arch, pin) in Manifest::REVIEWED.platforms {
        let spelled = platform(os, arch);
        let Some(target) = entry.targets.iter().find(|t| *t.platform == *spelled) else {
            panic!("a target for {spelled}");
        };
        assert_eq!(&*target.cmd, pin.executable.name, "{spelled}");
        assert_eq!(target.launch, Launch::Binary, "{spelled}");
        let [archive] = target.artifacts.as_slice() else {
            panic!("one archive for {spelled}");
        };
        assert_eq!(&*archive.url, pin.url, "{spelled}");
        assert_eq!(&*archive.sha256, pin.sha256, "{spelled}");
        assert_eq!(archive.size, pin.archive, "{spelled}");
        assert_eq!(archive.format, Format::Zip, "{spelled}");
        assert_eq!(archive.mirror, None, "{spelled}");
        assert_eq!(archive.replaced_by, None, "{spelled}");
        assert_eq!(
            archive.unpacked,
            pin.executable.bytes + pin.companion.bytes,
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
                (pin.executable.name, Some((pin.executable.bytes, true))),
                (pin.companion.name, Some((pin.companion.bytes, true))),
            ],
            "{spelled}"
        );
    }
}

#[test]
fn nothing_taken_in_offers_the_snapshots_release_and_touches_no_disk() {
    let owned = Owned::new("trust-fresh");

    let trust = Trust::load(&owned.0, taken());

    let pins = trust.offered(&agent("antigravity"));
    let reviewed = Manifest::REVIEWED.here();
    assert_eq!(pins.len(), usize::from(reviewed.is_some()));
    if let (Some(pin), Some(offered)) = (reviewed, pins.first()) {
        assert_eq!(offered.key.as_str(), "antigravity");
        assert_eq!(&*offered.version, pin.version);
        assert_eq!(offered.lock_revision, None);
        assert_eq!(&*offered.target.cmd, pin.executable.name);
        let [archive] = offered.target.artifacts.as_slice() else {
            panic!("one archive");
        };
        assert_eq!(
            (&*archive.url, &*archive.sha256, archive.size),
            (pin.url, pin.sha256, pin.archive)
        );
    }
    assert!(offered(&trust, "goose").is_empty());
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
fn a_state_that_is_not_the_trust_state_is_read_as_deleted() {
    let ours = Owned::new("trust-garbage");
    let mut held = trust(&ours, taken());
    believe(
        &mut held,
        &listing(5, vec![release("goose", "1.4.1", None)], Vec::new()),
        taken(),
    );
    let file = ours.0.join(DIRECTORY).join(FILE);
    let good = fs::read(&file).unwrap_or_default();
    let oversize = vec![b' '; STATE_BYTES + 1];
    let other_format = String::from_utf8_lossy(&good).replacen(r#""format":1"#, r#""format":2"#, 1);
    assert_ne!(other_format.as_bytes(), good, "the format is written");

    for garbage in [
        b"not json".to_vec(),
        b"[]".to_vec(),
        b"{}".to_vec(),
        good.get(..good.len() / 2).unwrap_or_default().to_vec(),
        other_format.into_bytes(),
        oversize,
    ] {
        fs::write(&file, &garbage).unwrap_or_default();

        let read = trust(&ours, taken());

        assert!(offered(&read, "goose").is_empty());
        assert_eq!(offered(&read, "antigravity"), then_the_snapshot(&[]));
    }
    // And the next catalog taken in replaces it.
    let mut replaced = trust(&ours, taken());
    believe(
        &mut replaced,
        &listing(2, vec![release("goose", "1.4.1", None)], Vec::new()),
        taken(),
    );
    assert_eq!(offered(&trust(&ours, taken()), "goose"), ["1.4.1"]);
}

#[test]
fn a_state_that_cannot_be_kept_changes_nothing_that_was_believed() {
    let owned = Owned::new("trust-unkept");
    fs::write(owned.0.join(DIRECTORY), b"a file where the directory goes").unwrap_or_default();
    let mut held = trust(&owned, taken());

    let refused = take_in(
        &mut held,
        &listing(5, vec![release("goose", "1.4.1", None)], Vec::new()),
        taken(),
    );

    assert!(matches!(refused, Err(TrustError::Io(_))), "{refused:?}");
    assert!(offered(&held, "goose").is_empty());
    // Nor was the serial taken: the same catalog is believed once it can be.
    fs::remove_file(owned.0.join(DIRECTORY)).unwrap_or_default();
    believe(
        &mut held,
        &listing(5, vec![release("goose", "1.4.1", None)], Vec::new()),
        taken(),
    );
    assert_eq!(offered(&held, "goose"), ["1.4.1"]);
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
    let own = keys();
    let replacement: &'static [&'static str] = Box::leak(Box::new(["another key", the_key(&own)]));
    let replacement = Keys::new(replacement);
    let signed = sign(&listing(
        3,
        vec![release("goose", "1.4.1", None)],
        Vec::new(),
    ));

    let taken_in = reloaded.take_in(&signed.index, &signed.signature, &replacement, taken());

    assert!(taken_in.is_ok(), "{taken_in:?}");
    // What the old keys made held is not kept: not the release they listed,
    // not the revocation they made.
    assert_eq!(offered(&reloaded, "antigravity"), then_the_snapshot(&[]));
    assert_eq!(offered(&reloaded, "goose"), ["1.4.1"]);
    assert_eq!(offered(&trust(&owned, taken()), "goose"), ["1.4.1"]);
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
