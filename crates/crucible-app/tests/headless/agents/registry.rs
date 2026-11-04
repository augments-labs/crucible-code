//! A release the ACP Registry lists: a later session lists, launches,
//! updates, rolls back and signs in to it from the cached index and the
//! receipt's probe record, with no signed catalog and no trust state, and the
//! registry's generic content-use warning holds its route.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crucible_app::backends::{AgentRow, CatalogRow, REGISTRY, Unbelieved, believed_registry};
use crucible_app::catalog_fetch::{self, RegistrySource, Source};
use crucible_app::client::agents::{
    AgentsError, How, Outcome, Shelf, Watching, run, sign_in_methods,
};
use crucible_app::content_use::{Consent, Routes};
use crucible_app::launching::Agent;
use crucible_client_api::{Capabilities, Capability, Command, Correlation, Request};
use crucible_config::{ExternalAgent, Home, Settings};
use crucible_external_harnesses::catalog::registry::{self, RegistryIndex};
use crucible_external_harnesses::catalog::{Admits, KEYS, Keys, Trust};
use crucible_external_harnesses::store::{
    Fetched, Listing, Probe, ProbedKind, ProbedMethod, Provenance, Source as Released, Store,
    Version,
};
use crucible_http::ProxyEnv;
use crucible_runtime::Cancel;
use crucible_types::AgentKey;
use serde_json::{Value, json};

use super::{Answering, Bench, Failed, digest, name, on_runtime, platform};

/// The archive the registry lists `goose` at, on a host of its own.
const ARCHIVE: &str = "https://downloads.example.com/goose-1.0.0.tar.gz";

/// A registry index listing `goose` at `version` for this machine, with one
/// target and one environment variable, an unhashed one, and a second agent
/// whose id is a built-in driver's word.
fn registry_json(version: &str) -> Value {
    let target = |cmd: &str| {
        json!({
            "archive": ARCHIVE,
            "cmd": cmd,
            "sha256": "a".repeat(64),
            "args": ["acp"],
            "env": {"GOOSE_HOME": "/tmp/goose"},
        })
    };
    json!({
        "version": "1.0.0",
        "agents": [
            {
                "id": "goose",
                "name": "Goose",
                "version": version,
                "license": "Apache-2.0",
                "license_url": "https://example.com/terms",
                "distribution": {"binary": {platform(): target("./goose")}},
            },
            {
                "id": "unhashed",
                "name": "Unhashed",
                "version": "1.0.0",
                "license": "MIT",
                "distribution": {"binary": {platform(): {
                    "archive": ARCHIVE,
                    "cmd": "./unhashed",
                }}},
            },
            {
                "id": "claude",
                "name": "Claude",
                "version": "1.0.0",
                "license": "MIT",
                "distribution": {"binary": {platform(): target("./claude")}},
            },
        ],
    })
}

/// Writes the cached registry index under `home`, fetched now.
#[allow(clippy::expect_used)] // a fixture that cannot be written is the test's own failure
fn cache(home: &Home, index: &Value) {
    let path = catalog_fetch::registry_in(home.path()).join("registry.json");
    fs::create_dir_all(path.parent().expect("a directory")).expect("the directory");
    fs::write(&path, serde_json::to_vec(index).expect("JSON")).expect("the cache");
}

/// The probe record an install of `goose` at `version` stored.
fn probe(version: &str) -> Probe {
    Probe {
        target: platform().into(),
        admits: Admits {
            name: "goose".into(),
            version: version.into(),
        },
        auth_methods: vec![
            ProbedMethod {
                id: "key".into(),
                name: "API key".into(),
                kind: ProbedKind::EnvVar,
            },
            ProbedMethod {
                id: "login".into(),
                name: "Log in".into(),
                kind: ProbedKind::Terminal {
                    command: "goose".into(),
                    args: ["login".into()].into(),
                    env: Vec::new(),
                    tty: Some(true),
                },
            },
        ],
    }
}

/// Installs `goose` at `version` into `home`'s store as a release the ACP
/// Registry lists, with `program` written where its entry starts it, and gives
/// the executable.
fn installed(home: &Home, version: &str, program: &[u8]) -> Result<PathBuf, Failed> {
    let store = Store::new(home.path().join("programs"));
    let key = AgentKey::new("goose")?;
    let staging = store.stage(&key)?;
    let path = staging.directory().join("goose");
    fs::write(&path, program)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    }
    let bytes = u64::try_from(program.len()).unwrap_or(u64::MAX);
    let files = vec![crucible_external_harnesses::catalog::File {
        path: "goose".into(),
        kind: crucible_external_harnesses::catalog::FileKind::File {
            size: bytes,
            sha256: digest(program).into(),
            exec: true,
        },
    }];
    let version = Version::new(version, None)?;
    let named = version.version().to_owned();
    store.commit(
        staging,
        &version,
        &files,
        Provenance {
            listing: Listing {
                serial: 0,
                review: registry::URL.into(),
                terms: None,
            },
            source: Released::Registry,
            probe: Some(probe(&named)),
            decompressed: bytes,
            fetched: SystemTime::now(),
            artifacts: vec![Fetched {
                url: ARCHIVE.into(),
                hosts: vec!["downloads.example.com".into()],
                size: bytes,
                sha256: digest(program).into(),
            }],
        },
    )?;
    Ok(store.directory(&key, &version).join("goose"))
}

/// The configuration naming the instance `work` of `goose` at `executable`.
fn configure(home: &Home, executable: &Path) -> Result<(), Failed> {
    fs::write(
        crucible_config::user(home),
        serde_json::to_vec_pretty(&json!({
            "externalAgents": {
                "work": {
                    "driver": "catalog",
                    "agent": "goose",
                    "executable": executable.to_string_lossy(),
                }
            }
        }))?,
    )?;
    Ok(())
}

/// Makes the signed catalog's trust state unusable, so any path that reads it
/// refuses the release.
fn untrusted(home: &Home) -> Result<(), Failed> {
    let state = catalog_fetch::state_in(home.path()).join("agent-catalog");
    fs::create_dir_all(&state)?;
    fs::write(state.join("trust.json"), b"not a trust state")?;
    Ok(())
}

/// The shelf a door is lent at `home`, with no launcher, pointing its registry
/// at an address nothing serves: the cache is what is read.
fn shelf(home: &Home) -> Shelf<'_> {
    Shelf {
        catalog: Source {
            base: "http://127.0.0.1:1/catalog/".into(),
            keys: Keys::new(&[]),
            hosts: &[],
            proxy: ProxyEnv::default(),
        },
        registry: Some(RegistrySource {
            url: "http://127.0.0.1:1/registry.json".into(),
            hosts: &[],
            proxy: ProxyEnv::default(),
        }),
        home,
        smoke: None,
        steps: None,
    }
}

fn ask(command: Command) -> Request {
    Request::new(
        Capabilities::none().with(Capability::Install),
        Correlation::new(1),
        command,
    )
}

/// Runs `command` at the door over `bench`'s home, with `front` answering.
fn door(
    bench: &Bench,
    front: &mut Answering<'_>,
    command: Command,
) -> Result<Result<Outcome, AgentsError>, Failed> {
    let request = ask(command);
    let shelf = shelf(&bench.home);
    on_runtime(run(
        &request,
        &shelf,
        front,
        Watching {
            progress: &mut |_| {},
            cancel: &Cancel::new(),
            link: &|_| {},
            sign_in: None,
            terminal: None,
        },
    ))
}

/// The configured instance `name` of `home`.
fn instance(home: &Home, name: &str) -> Result<ExternalAgent, Failed> {
    Settings::read_home(home)?
        .external_agents()
        .into_iter()
        .find(|one| one.name() == name)
        .ok_or_else(|| "the instance is not configured".into())
}

#[test]
fn a_later_session_lists_launches_updates_rolls_back_and_signs_in_from_the_registry()
-> Result<(), Failed> {
    let bench = Bench::new("registry-later-session")?;
    cache(&bench.home, &registry_json("2.0.0"));
    untrusted(&bench.home)?;
    let previous = installed(&bench.home, "1.0.0", b"goose 1.0.0")?;
    let current = installed(&bench.home, "2.0.0", b"goose 2.0.0")?;
    configure(&bench.home, &current)?;
    let state = catalog_fetch::state_in(bench.home.path());
    let mut front = Answering {
        choice: Some("Roll back"),
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    // It lists.
    let outcome = door(&bench, &mut front, Command::AgentCatalog)?;
    let Ok(Outcome::Catalog { agents, .. }) = outcome else {
        panic!("no listing: {outcome:?}");
    };
    let goose = agents
        .iter()
        .find(|listed| listed.agent.as_str() == "goose")
        .expect("goose listed");
    let offered: Vec<&str> = goose.offered.iter().map(|version| &**version).collect();
    assert_eq!(offered, ["2.0.0"]);
    assert_eq!(&*goose.license, "Apache-2.0");
    assert!(
        !agents
            .iter()
            .any(|listed| listed.agent.as_str() == "claude")
    );
    assert!(
        !agents
            .iter()
            .any(|listed| listed.agent.as_str() == "unhashed"),
        "an unhashed target is absent"
    );

    // It launches from the cached index and the receipt's probe record, with a
    // trust state that cannot be used and no signed catalog at all.
    let configured = instance(&bench.home, "work")?;
    let agent = Agent::believing(&configured, &REGISTRY, &state, |name| {
        std::env::var_os(name)
    })
    .map_err(|why| why.to_string())?;
    let consent = Consent::new(Routes::production());
    let trust = Trust::load(&state, SystemTime::now());
    assert!(trust.unusable().is_some(), "the trust state is unusable");
    let program = agent
        .program(&consent, &trust)
        .map_err(|why| why.to_string())?;
    assert_eq!(program.path(), current);
    let (started, stored) = agent
        .release(
            &consent,
            &trust,
            bench.programs().parent().expect("programs"),
        )
        .map_err(|why| why.to_string())?;
    assert_eq!(started.path(), current);
    assert!(stored.is_some());
    let AgentRow::Catalog(row) = agent.row() else {
        panic!("not a catalog row");
    };
    let warned = row.warned().expect("the generic warning");
    assert_eq!(&*warned.route, "agent:goose");
    assert_eq!(&*warned.warning.caution, "no content statement");
    assert_eq!(&*warned.warning.link, registry::URL);
    assert!(!warned.warning.read.is_empty());
    assert_eq!(row.source(), Released::Registry);

    // It updates to the version it already runs.
    let outcome = door(
        &bench,
        &mut front,
        Command::UpdateAgent {
            agent: name("goose")?,
        },
    )?;
    assert!(
        matches!(&outcome, Ok(Outcome::Current { version, .. }) if &**version == "2.0.0"),
        "{outcome:?}"
    );

    // It rolls back to the version before, through the registry source, and
    // the trust state that cannot be used refuses nothing.
    let outcome = door(
        &bench,
        &mut front,
        Command::RollbackAgent {
            agent: name("goose")?,
        },
    )?;
    let Ok(Outcome::Moved { how, .. }) = outcome else {
        panic!("no rollback: {outcome:?}");
    };
    assert_eq!(how, How::RolledBack);
    assert_eq!(Path::new(&bench.executable()?), previous);

    // It signs in by the methods the probe recorded, naming the variable from
    // the target's one.
    let methods: Vec<String> = sign_in_methods(&bench.home, "work")?
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(methods, ["key", "login"]);
    Ok(())
}

#[test]
fn a_reserved_driver_word_and_an_unhashed_target_get_no_registry_row() -> Result<(), Failed> {
    let bench = Bench::new("registry-reserved")?;
    let index = RegistryIndex::parse(
        &serde_json::to_vec(&registry_json("1.0.0"))?,
        SystemTime::now(),
    )?;
    let key = AgentKey::new("claude")?;
    let store = Store::new(bench.home.path().join("programs"));
    assert!(matches!(
        believed_registry(
            &index,
            &store,
            &key,
            Path::new("/nowhere/claude"),
            &probe("1.0.0"),
        ),
        Err(Unbelieved::Reserved { .. })
    ));
    let goose = AgentKey::new("goose")?;
    assert!(matches!(
        believed_registry(
            &index,
            &store,
            &goose,
            Path::new("/nowhere/goose"),
            &probe("1.0.0"),
        ),
        Err(Unbelieved::NotInstalled { .. })
    ));
    let unhashed = AgentKey::new("unhashed")?;
    let why = index
        .offer(&unhashed)
        .expect_err("an unhashed target")
        .to_string();
    assert!(why.contains("SHA-256"), "{why}");
    Ok(())
}

#[test]
fn the_registry_row_is_held_by_the_registrys_generic_warning_and_no_key() {
    assert_eq!(KEYS, Keys::new(&[]));
    let index = RegistryIndex::parse(
        &serde_json::to_vec(&registry_json("1.0.0")).expect("JSON"),
        SystemTime::now(),
    )
    .expect("an index");
    let offer = index
        .offer(&AgentKey::new("goose").expect("a key"))
        .expect("an offer")
        .provisional()
        .expect("an entry");
    let row = CatalogRow::registry(&offer, offer.here().expect("a target"), "2026-11-04".into())
        .expect("a row");
    let warned = row.warned().expect("the generic warning");
    assert_eq!(
        warned.warning.sentence.as_ref(),
        "This agent's vendor has not stated what it does with what you send. Installing and \
         starting it sends your prompts, files and commands to the vendor."
    );
    assert_eq!(&*warned.warning.source, "the ACP Registry entry");
    assert_eq!(&*warned.warning.read, "2026-11-04");
    assert!(warned.origins.is_empty());
}

#[test]
fn an_install_of_an_unhashed_registry_target_is_refused_naming_the_hash() -> Result<(), Failed> {
    let bench = Bench::new("registry-unhashed-install")?;
    cache(&bench.home, &registry_json("1.0.0"));
    let mut front = Answering {
        choice: None,
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };
    let outcome = door(
        &bench,
        &mut front,
        Command::InstallAgent {
            agent: name("unhashed")?,
            version: name("1.0.0")?,
        },
    )?;
    let Err(AgentsError::Unregistered { why, .. }) = outcome else {
        panic!("no refusal: {outcome:?}");
    };
    assert!(why.to_string().contains("SHA-256"), "{why}");
    Ok(())
}
