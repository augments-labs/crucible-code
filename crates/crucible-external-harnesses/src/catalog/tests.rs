//! A catalog is believed only signed, bounded, current and in a known schema,
//! and is parsed into every field an entry carries.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::fixture::{Signed, signed};
use super::{
    Admits, AssetRef, AuthMethod, Catalog, CatalogError, ContentUse, Disposition, FileKind, Files,
    Format, GRACE, Held, INDEX_BYTES, KEYS, Kind, Launch, Lock, Method, Route, Script, Variable,
};

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
    for schema in [json!(2), json!(0), json!("1")] {
        let signed = sign(&with("/schema", schema));

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
    for (field, pointer, to) in cases {
        let refused = fetched(&with(&pointer, to));

        assert!(
            matches!(&refused, Err(CatalogError::Field { field: named, .. }) if **named == *field),
            "{field}: {refused:?}"
        );
    }
}
