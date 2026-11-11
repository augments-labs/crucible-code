//! A release the ACP Registry lists: a later session lists, launches,
//! updates, rolls back and signs in to it from the cached index and the
//! receipt's probe record, with no signed catalog and no trust state, and the
//! registry's generic content-use warning holds its route.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crucible_app::backends::{AgentRow, CatalogRow, REGISTRY, Unbelieved, believed_registry};
use crucible_app::catalog_fetch::{self, RegistrySource, Source};
use crucible_app::client::agents::{
    AgentsError, How, Outcome, Shelf, Typed, Watching, keep_key, run, runs_a_command,
    sign_in_methods,
};
use crucible_app::client::{Front, Shown};
use crucible_app::content_use::{Consent, Routes};
use crucible_app::launching::Agent;
use crucible_client_api::{
    Capabilities, Capability, Command, Correlation, Decision, Pending, PendingId, Refusal, Request,
    Said,
};
use crucible_config::{ExternalAgent, Home, Settings};
use crucible_external_harnesses::catalog::fixture::signed;
use crucible_external_harnesses::catalog::registry::{self, RegistryIndex};
use crucible_external_harnesses::catalog::{Admits, KEYS, Keys, Trust};
use crucible_external_harnesses::fake_peer;
use crucible_external_harnesses::store::{
    Fetched, Listing, Probe, ProbedKind, ProbedMethod, Provenance, Source as Released, Store,
    Version,
};
use crucible_http::ProxyEnv;
use crucible_runtime::{BoxFuture, Cancel};
use crucible_types::AgentKey;
use serde_json::{Value, json};

use super::{
    Answering, Bench, Failed, ONE, Reply, digest, entry, name, on_runtime, platform, set, shape,
};

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
fn cache(home: &Home, index: &Value) {
    cache_at(home, SystemTime::now(), index);
}

/// Writes the cached registry index under `home`, fetched at `at`.
#[allow(clippy::expect_used)] // a fixture that cannot be written is the test's own failure
fn cache_at(home: &Home, at: SystemTime, index: &Value) {
    let path = catalog_fetch::registry_in(home.path()).join("registry.json");
    fs::create_dir_all(path.parent().expect("a directory")).expect("the directory");
    fs::write(&path, serde_json::to_vec(index).expect("JSON")).expect("the cache");
    fs::File::options()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_modified(at))
        .expect("the cache time");
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
    installed_as(home, version, program, Released::Registry)
}

/// Installs `goose` at `version` as `source` describes its release, with
/// `program` written where its entry starts it, and gives the executable.
fn installed_as(
    home: &Home,
    version: &str,
    program: &[u8],
    source: Released,
) -> Result<PathBuf, Failed> {
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
                serial: if source == Released::Registry { 0 } else { 2 },
                review: if source == Released::Registry {
                    registry::URL.into()
                } else {
                    "https://github.com/augments-labs/crucible-agent-catalog/pull/12".into()
                },
                terms: None,
            },
            source,
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

/// Installs `goose` as a registry release whose receipt carries `probe`, which
/// may be absent or taken for another platform.
fn installed_with_probe(
    home: &Home,
    version: &str,
    program: &[u8],
    probe: Option<Probe>,
) -> Result<PathBuf, Failed> {
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
            probe,
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
    configure_with(home, executable, false)
}

/// The configuration naming the instance `work` of `goose` at `executable`,
/// with `agents.update` set to `reviewed` where `reviewed` is so.
fn configure_with(home: &Home, executable: &Path, reviewed: bool) -> Result<(), Failed> {
    let mut document = json!({
        "externalAgents": {
            "work": {
                "driver": "catalog",
                "agent": "goose",
                "executable": executable.to_string_lossy(),
            }
        }
    });
    if reviewed && let Some(object) = document.as_object_mut() {
        object.insert("agents".into(), json!({"update": "reviewed"}));
    }
    fs::write(
        crucible_config::user(home),
        serde_json::to_vec_pretty(&document)?,
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

/// The shelf a door is lent at `home`, with no launcher, reading the registry
/// from `registry`.
fn shelf_at<'a>(home: &'a Home, registry: &str) -> Shelf<'a> {
    Shelf {
        catalog: Source {
            base: "http://127.0.0.1:1/catalog/".into(),
            keys: Keys::new(&[]),
            hosts: &[],
            proxy: ProxyEnv::default(),
        },
        registry: Some(RegistrySource {
            url: registry.into(),
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

/// Runs `command` at the door over `bench`'s home, with `front` answering and
/// the registry read from `registry`.
fn door_at(
    bench: &Bench,
    front: &mut dyn Front,
    command: Command,
    registry: &str,
) -> Result<Result<Outcome, AgentsError>, Failed> {
    let request = ask(command);
    let shelf = shelf_at(&bench.home, registry);
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

/// Runs `command` at the door over `bench`'s home, with `front` answering.
fn door(
    bench: &Bench,
    front: &mut dyn Front,
    command: Command,
) -> Result<Result<Outcome, AgentsError>, Failed> {
    door_at(bench, front, command, "http://127.0.0.1:1/registry.json")
}

/// A front that records the facts of every install question it is put and
/// answers with `choice`, or declines where there is none.
struct Recording {
    choice: Option<&'static str>,
    facts: Vec<String>,
}

impl Front for Recording {
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Decision>> {
        Box::pin(async move {
            if let Shown::Install(confirmation) = shown {
                self.facts.push(confirmation.facts.join(" "));
            }
            let id = pending.id();
            Some(match self.choice {
                Some(choice) => Decision::Picked {
                    id: PendingId::new(id.number()),
                    choice: Said::new(choice).ok()?,
                },
                None => Decision::Declined { id },
            })
        })
    }

    fn refused(&mut self, _refusal: Refusal) {}
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

    // A key a built-in driver is spelled with is no registry row at all: an
    // install or update of it is refused, and the built-in's route is not
    // shadowed.
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
            agent: name("claude")?,
            version: name("1.0.0")?,
        },
    )?;
    assert!(
        matches!(outcome, Err(AgentsError::Unoffered { .. })),
        "{outcome:?}"
    );
    let outcome = door(
        &bench,
        &mut front,
        Command::UpdateAgent {
            agent: name("claude")?,
        },
    )?;
    assert!(
        matches!(outcome, Err(AgentsError::Unlisted(_))),
        "{outcome:?}"
    );
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

/// `index` with no agents, as a registry that has withdrawn every one.
#[allow(clippy::expect_used)] // a fixture that cannot be edited is the test's own failure
fn empty_registry(mut index: Value) -> Value {
    let object = index.as_object_mut().expect("an index object");
    object.insert("agents".into(), json!([]));
    index
}

#[test]
fn a_reviewed_update_of_a_registry_release_is_still_asked() -> Result<(), Failed> {
    let bench = Bench::new("registry-reviewed-update")?;
    let now = SystemTime::now();
    cache_at(
        &bench.home,
        now - catalog_fetch::REGISTRY_REUSE - Duration::from_mins(1),
        &registry_json("2.0.0"),
    );
    let current = installed(&bench.home, "1.0.0", b"goose 1.0.0")?;
    configure_with(&bench.home, &current, true)?;
    let mut front = Recording {
        choice: None,
        facts: Vec::new(),
    };

    let outcome = door_at(
        &bench,
        &mut front,
        Command::UpdateAgent {
            agent: name("goose")?,
        },
        // Nothing serves the registry: the stale cache is used, not verified.
        &bench.server.at("/registry.json"),
    )?;

    assert_eq!(
        front.facts.len(),
        1,
        "the update question was not asked: {outcome:?}"
    );
    let said = front.facts.first().expect("a question fact");
    assert!(
        said.contains("Moves work from 1.0.0 onto 2.0.0"),
        "the update question is missing: {said}"
    );
    assert!(
        said.contains("Source: ACP Registry, read "),
        "the source and its date are missing: {said}"
    );
    assert!(
        said.contains("not verified: "),
        "the unverified line is missing: {said}"
    );
    assert!(said.contains("not reviewed by Crucible"), "{said}");
    assert!(matches!(outcome, Ok(Outcome::Undecided)), "{outcome:?}");
    Ok(())
}

#[test]
fn a_stale_registry_cache_is_listed_as_not_verified() -> Result<(), Failed> {
    let bench = Bench::new("registry-unverified-listing")?;
    cache_at(
        &bench.home,
        SystemTime::now() - catalog_fetch::REGISTRY_REUSE - Duration::from_mins(1),
        &registry_json("1.0.0"),
    );
    let mut front = Answering {
        choice: None,
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let outcome = door_at(
        &bench,
        &mut front,
        Command::AgentCatalog,
        &bench.server.at("/registry.json"),
    )?;

    let Ok(Outcome::Catalog {
        agents,
        registry: Some(line),
        ..
    }) = outcome
    else {
        panic!("no listing line: {outcome:?}");
    };
    assert!(line.contains("not verified"), "{line}");
    assert!(
        agents.iter().any(|listed| listed.agent.as_str() == "goose"),
        "the cached index was not listed"
    );
    Ok(())
}

#[test]
fn a_registry_index_is_fetched_once_within_its_five_minutes() -> Result<(), Failed> {
    let bench = Bench::new("registry-fetch-once")?;
    bench.server.serve(
        "/registry.json",
        Reply::Body(serde_json::to_vec(&registry_json("1.0.0"))?),
    );
    let url = bench.server.at("/registry.json");
    let mut front = Answering {
        choice: None,
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let _ = door_at(&bench, &mut front, Command::AgentCatalog, &url)?;
    let _ = door_at(&bench, &mut front, Command::AgentCatalog, &url)?;

    let asked = bench
        .server
        .asked()
        .iter()
        .filter(|path| path.as_str() == "/registry.json")
        .count();
    assert_eq!(asked, 1, "the five-minute cache was not reused");
    Ok(())
}

#[test]
fn a_signed_offer_is_not_shadowed_by_the_registry() -> Result<(), Failed> {
    let bench = Bench::new("registry-signed-first")?;
    let current = installed_as(&bench.home, "1.0.0", ONE, Released::Catalog)?;
    configure(&bench.home, &current)?;
    let catalog = json!({
        "schema": 1,
        "serial": 2,
        "entries": [entry("goose", "1.0.0", ONE)],
        "revoked": [],
    });
    let signed = signed(&serde_json::to_vec(&catalog)?);
    let state = catalog_fetch::state_in(bench.home.path());
    let now = SystemTime::now();
    let mut trust = Trust::load(&state, now);
    trust.take_in(&signed.index, &signed.signature, &signed.keys, now)?;
    cache(&bench.home, &registry_json("2.0.0"));
    let mut front = Answering {
        choice: None,
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let outcome = door(&bench, &mut front, Command::AgentCatalog)?;
    let Ok(Outcome::Catalog { agents, .. }) = outcome else {
        panic!("no listing: {outcome:?}");
    };
    let goose = agents
        .iter()
        .find(|listed| listed.agent.as_str() == "goose")
        .expect("goose listed");
    assert_eq!(
        goose
            .offered
            .iter()
            .map(|version| &**version)
            .collect::<Vec<_>>(),
        ["1.0.0"],
        "the registry shadowed the signed offer"
    );

    let outcome = door(
        &bench,
        &mut front,
        Command::UpdateAgent {
            agent: name("goose")?,
        },
    )?;
    assert!(
        matches!(&outcome, Ok(Outcome::Current { version, .. }) if &**version == "1.0.0"),
        "{outcome:?}"
    );

    // A signed install with a registry index read says nothing of the
    // registry's source.
    let mut asked = Recording {
        choice: None,
        facts: Vec::new(),
    };
    let outcome = door(
        &bench,
        &mut asked,
        Command::InstallAgent {
            agent: name("goose")?,
            version: name("1.0.0")?,
        },
    )?;
    assert_eq!(asked.facts.len(), 1, "no install question: {outcome:?}");
    let said = asked.facts.first().expect("a question fact");
    assert!(said.contains("already installed"), "{said}");
    assert!(!said.contains("Source: ACP Registry"), "{said}");
    assert!(!said.contains("not reviewed by Crucible"), "{said}");
    assert!(matches!(outcome, Ok(Outcome::Undecided)), "{outcome:?}");
    Ok(())
}

#[test]
fn a_rollback_names_the_registrys_actual_reason() -> Result<(), Failed> {
    let bench = Bench::new("registry-rollback-reason")?;
    cache(&bench.home, &registry_json("2.0.0"));
    let previous = installed(&bench.home, "1.0.0", b"goose 1.0.0")?;
    let current = installed(&bench.home, "2.0.0", b"goose 2.0.0")?;
    configure(&bench.home, &current)?;
    // The registry withdraws every agent after the install.
    cache(&bench.home, &empty_registry(registry_json("2.0.0")));
    let mut front = Recording {
        choice: None,
        facts: Vec::new(),
    };

    let outcome = door(
        &bench,
        &mut front,
        Command::RollbackAgent {
            agent: name("goose")?,
        },
    )?;

    assert_eq!(front.facts.len(), 1, "no rollback question: {outcome:?}");
    let said = front.facts.first().expect("a question fact");
    assert!(
        said.contains("lists no agent goose"),
        "the registry reason is missing: {said}"
    );
    assert!(
        !said.contains("no longer lists"),
        "the old false reason is still said: {said}"
    );
    assert!(matches!(outcome, Ok(Outcome::Undecided)), "{outcome:?}");
    let _ = previous;
    Ok(())
}

#[test]
fn the_registry_sign_in_row_reads_its_route_and_refuses_a_terminal_method() -> Result<(), Failed> {
    let bench = Bench::new("registry-sign-in-row")?;
    cache(&bench.home, &registry_json("2.0.0"));
    let current = installed(&bench.home, "2.0.0", b"goose 2.0.0")?;
    configure(&bench.home, &current)?;

    assert!(
        runs_a_command(&bench.home, "work", "login")?,
        "the registry row's terminal method was not read"
    );
    let mut front = Recording {
        choice: None,
        facts: Vec::new(),
    };
    let outcome = door(
        &bench,
        &mut front,
        Command::SignInAgent {
            instance: name("work")?,
            method: name("login")?,
        },
    )?;
    // A terminal route is refused before the front end on every platform: the
    // channel that would carry it on Unix, and the platform's own sentence
    // where it has no real terminal.
    #[cfg(unix)]
    match outcome {
        Err(AgentsError::Unlent(method)) => assert_eq!(&*method, "login"),
        other => panic!("the terminal method was not refused: {other:?}"),
    }
    #[cfg(windows)]
    match outcome {
        Err(AgentsError::Unsupported(why)) => assert!(
            why.contains("a sign-in at a real terminal"),
            "the platform's terminal refusal: {why}"
        ),
        other => panic!("the terminal method was not refused: {other:?}"),
    }
    #[cfg(not(any(unix, windows)))]
    match outcome {
        Err(AgentsError::Unlent(method)) => assert_eq!(&*method, "login"),
        Err(AgentsError::Unsupported(why)) => {
            assert!(why.contains("a real terminal"), "{why}");
        }
        other => panic!("the terminal method was not refused: {other:?}"),
    }

    // The typed path: the probe's env-var method names the target's one
    // variable, and a kept key is read back without starting the vendor's
    // program.
    keep_key(
        &bench.home,
        "work",
        "key",
        Typed::new("sk-goose-0123456789".to_owned()),
    )?;
    let outcome = door(
        &bench,
        &mut front,
        Command::SignInAgent {
            instance: name("work")?,
            method: name("key")?,
        },
    )?;
    assert!(matches!(outcome, Ok(Outcome::Kept { .. })), "{outcome:?}");
    Ok(())
}

#[test]
fn a_registry_install_from_a_provisional_offer_downloads_after_the_question() -> Result<(), Failed>
{
    let bench = Bench::new("registry-provisional-install")?;
    cache(&bench.home, &registry_json("2.0.0"));
    let current = installed(&bench.home, "1.0.0", b"goose 1.0.0")?;
    configure(&bench.home, &current)?;
    let mut front = Answering {
        choice: Some("Install"),
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let outcome = door(
        &bench,
        &mut front,
        Command::InstallAgent {
            agent: name("goose")?,
            version: name("2.0.0")?,
        },
    )?;

    let Err(AgentsError::Download(_)) = outcome else {
        panic!("the provisional install did not try the download: {outcome:?}");
    };
    assert!(!front.put.is_empty(), "the question was not put");
    assert_eq!(
        Path::new(&bench.executable()?),
        current,
        "an instance moved onto an unlaunchable version"
    );
    assert!(
        !bench.stored().iter().any(|name| name == "2.0.0"),
        "a provisional version was committed"
    );
    Ok(())
}

#[test]
fn a_registry_install_with_no_index_names_the_unread_line() -> Result<(), Failed> {
    let bench = Bench::new("registry-no-cache")?;
    let url = bench.server.at("/registry.json");
    let mut front = Answering {
        choice: None,
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let outcome = door_at(
        &bench,
        &mut front,
        Command::InstallAgent {
            agent: name("goose")?,
            version: name("1.0.0")?,
        },
        &url,
    )?;
    let Err(AgentsError::UnreadInstall { why, .. }) = outcome else {
        panic!("the install did not name the unread line: {outcome:?}");
    };
    assert!(why.contains("404"), "{why}");

    let outcome = door_at(
        &bench,
        &mut front,
        Command::UpdateAgent {
            agent: name("goose")?,
        },
        &url,
    )?;
    let Err(AgentsError::UnreadUpdate { why, .. }) = outcome else {
        panic!("the update did not name the unread line: {outcome:?}");
    };
    assert!(why.contains("404"), "{why}");
    Ok(())
}

#[test]
fn a_stale_cache_install_and_update_name_the_day_it_was_read() -> Result<(), Failed> {
    let bench = Bench::new("registry-stale-cache-date")?;
    // 20_733 days after 1970-01-01 is 2026-10-07.
    let read = UNIX_EPOCH + Duration::from_hours(20_733 * 24);
    cache_at(&bench.home, read, &empty_registry(registry_json("1.0.0")));
    let url = bench.server.at("/registry.json");
    let mut front = Answering {
        choice: None,
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };

    let outcome = door_at(
        &bench,
        &mut front,
        Command::InstallAgent {
            agent: name("goose")?,
            version: name("1.0.0")?,
        },
        &url,
    )?;
    let Err(AgentsError::UnreadInstall { why, .. }) = outcome else {
        panic!("the install did not name the unread line: {outcome:?}");
    };
    assert!(why.contains("2026-10-07"), "{why}");
    assert!(why.contains("not verified"), "{why}");

    let outcome = door_at(
        &bench,
        &mut front,
        Command::UpdateAgent {
            agent: name("goose")?,
        },
        &url,
    )?;
    let Err(AgentsError::UnreadUpdate { why, .. }) = outcome else {
        panic!("the update did not name the unread line: {outcome:?}");
    };
    assert!(why.contains("2026-10-07"), "{why}");
    assert!(why.contains("not verified"), "{why}");
    Ok(())
}

/// Installing a registry release downloads its one archive within the
/// registry's bounded path, surveys it from the file it landed in, runs the
/// vendor's program once in the closed sandbox, and stores the probe record in
/// the receipt; a later session rebuilds the row from that record without
/// starting the vendor again.
#[test]
fn a_registry_install_surveys_probes_and_stores_the_record() -> Result<(), Failed> {
    use crucible_external_harnesses::fake_peer::scripts::acp;
    use crucible_external_harnesses::fake_peer::{self, Script};
    use crucible_external_harnesses::store::Probe;

    let bench = Bench::new("registry-install-probed")?;
    let built = bench.home.path().join("built");
    let initialized = acp::offering("goose-acp", "1.0.0", &json!([]));
    let program = fake_peer::install(
        &built,
        "goose",
        &Script::once(
            crucible_external_harnesses::fake_peer::Reply::new()
                .hear(1)
                .say(&initialized),
        ),
    )?;
    let program_name = program
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .expect("a program name")
        .to_owned();
    let script = fs::read(built.join("script"))?;
    let archive = fake_peer::archive(&[(&program_name, &fs::read(&program)?), ("script", &script)]);
    let digest = digest(&archive);
    bench
        .server
        .serve(&format!("/catalog/{digest}"), Reply::Body(archive.clone()));
    let mut index = registry_json("1.0.0");
    let target = index
        .pointer_mut(&format!("/agents/0/distribution/binary/{}", platform()))
        .expect("the goose target");
    target["archive"] = json!("https://downloads.example.com/goose-1.0.0.zip");
    target["cmd"] = json!(format!("./{program_name}"));
    target["sha256"] = json!(digest);
    bench
        .server
        .serve("/registry.json", Reply::Body(serde_json::to_vec(&index)?));

    let shelf = Shelf {
        catalog: bench.server.source(),
        registry: Some(RegistrySource {
            url: bench.server.at("/registry.json"),
            hosts: &[],
            proxy: ProxyEnv::default(),
        }),
        home: &bench.home,
        smoke: None,
        steps: None,
    };
    let mut front = Recording {
        choice: Some("Install"),
        facts: Vec::new(),
    };
    let outcome = on_runtime(run(
        &ask(Command::InstallAgent {
            agent: name("goose")?,
            version: name("1.0.0")?,
        }),
        &shelf,
        &mut front,
        Watching {
            progress: &mut |_| {},
            cancel: &Cancel::new(),
            link: &|_| {},
            sign_in: None,
            terminal: None,
        },
    ))?;
    let Ok(Outcome::Moved {
        downloaded: true,
        probe: Some(probe),
        ..
    }) = outcome
    else {
        panic!("no probed install: {outcome:?}");
    };
    assert_eq!(&*probe.admits.name, "goose-acp");
    assert_eq!(&*probe.admits.version, "1.0.0");
    assert_eq!(&*probe.target, platform());

    // The question named the download and the probe.
    let said = front.facts.first().expect("a question fact");
    assert!(said.contains("download"), "{said}");
    assert!(said.contains("survey"), "{said}");
    assert!(said.contains("probe"), "{said}");

    // The receipt stores what the probe answered, under the platform it was
    // taken for.
    let store = Store::new(bench.home.path().join("programs"));
    let version = Version::new("1.0.0", None)?;
    let receipt = store
        .receipt(&AgentKey::new("goose")?, &version)?
        .expect("a receipt");
    let Probe {
        target: recorded,
        admits,
        ..
    } = receipt.probe.as_deref().expect("a probe record");
    assert_eq!(&**recorded, platform());
    assert_eq!(&*admits.name, "goose-acp");

    // A later session rebuilds the row from the cached index and the receipt,
    // starting the vendor's program not at all.
    let root = store.directory(&AgentKey::new("goose")?, &version);
    let played = fake_peer::played(&root);
    let installed = root.join(&program_name);
    configure(&bench.home, &installed)?;
    let configured = instance(&bench.home, "work")?;
    let agent = Agent::believing(
        &configured,
        &REGISTRY,
        &catalog_fetch::state_in(bench.home.path()),
        |name| (name == "PATH").then(|| std::ffi::OsString::from("/bin")),
    )
    .map_err(|why| why.to_string())?;
    assert_eq!(
        fake_peer::played(&root),
        played,
        "a later session re-probed"
    );
    assert!(
        agent.row().reaches().is_empty(),
        "a registry row added runtime hosts"
    );
    let AgentRow::Catalog(row) = agent.row() else {
        panic!("not a catalog row");
    };
    assert!(row.warned().is_ok());

    // A second add of the installed version is kept: nothing is downloaded,
    // the installed vendor is not probed again, and the held version is not
    // committed over. A script that exits at every start proves the probe did
    // not run.
    let archive_path = format!("/catalog/{digest}");
    let asked = bench
        .server
        .asked()
        .iter()
        .filter(|path| *path == &archive_path)
        .count();
    fs::write(
        root.join("script"),
        Script::of(Vec::<crucible_external_harnesses::fake_peer::Reply>::new()).encoded(),
    )?;
    let mut again = Recording {
        choice: Some("Add"),
        facts: Vec::new(),
    };
    let outcome = on_runtime(run(
        &ask(Command::AddAgent {
            agent: name("goose")?,
            version: name("1.0.0")?,
        }),
        &shelf,
        &mut again,
        Watching {
            progress: &mut |_| {},
            cancel: &Cancel::new(),
            link: &|_| {},
            sign_in: None,
            terminal: None,
        },
    ))?;
    let Ok(Outcome::Moved {
        how: How::Added,
        downloaded: false,
        probe: Some(kept),
        ..
    }) = outcome
    else {
        panic!("the second add downloaded, probed or was held: {outcome:?}");
    };
    assert_eq!(&*kept.admits.name, "goose-acp");
    assert_eq!(
        bench
            .server
            .asked()
            .iter()
            .filter(|path| *path == &archive_path)
            .count(),
        asked,
        "the second add downloaded the archive"
    );
    let said = again.facts.first().expect("a question fact");
    assert!(said.contains("already installed"), "{said}");
    Ok(())
}

/// A registry archive that surveys to more files than a catalog row carries is
/// refused before anything is committed, rather than installing a release no
/// later session can rebuild.
#[test]
fn a_registry_archive_past_the_catalog_file_list_is_refused_before_a_commit() -> Result<(), Failed>
{
    use crucible_external_harnesses::catalog::LISTED_FILES;

    let bench = Bench::new("registry-file-list-bound")?;
    let names: Vec<String> = (0..=LISTED_FILES)
        .map(|at| format!("file-{at:03}"))
        .collect();
    let bodies: Vec<Vec<u8>> = names.iter().map(|_| Vec::new()).collect();
    let members: Vec<(&str, &[u8])> = names
        .iter()
        .map(String::as_str)
        .zip(bodies.iter().map(Vec::as_slice))
        .collect();
    let archive = fake_peer::archive(&members);
    let digest = digest(&archive);
    bench
        .server
        .serve(&format!("/catalog/{digest}"), Reply::Body(archive));
    let mut index = registry_json("1.0.0");
    let target = index
        .pointer_mut(&format!("/agents/0/distribution/binary/{}", platform()))
        .expect("the goose target");
    target["archive"] = json!("https://downloads.example.com/goose-1.0.0.zip");
    target["sha256"] = json!(digest);
    bench
        .server
        .serve("/registry.json", Reply::Body(serde_json::to_vec(&index)?));

    let shelf = Shelf {
        catalog: bench.server.source(),
        registry: Some(RegistrySource {
            url: bench.server.at("/registry.json"),
            hosts: &[],
            proxy: ProxyEnv::default(),
        }),
        home: &bench.home,
        smoke: None,
        steps: None,
    };
    let mut front = Recording {
        choice: Some("Install"),
        facts: Vec::new(),
    };
    let outcome = on_runtime(run(
        &ask(Command::InstallAgent {
            agent: name("goose")?,
            version: name("1.0.0")?,
        }),
        &shelf,
        &mut front,
        Watching {
            progress: &mut |_| {},
            cancel: &Cancel::new(),
            link: &|_| {},
            sign_in: None,
            terminal: None,
        },
    ))?;
    let Err(AgentsError::TooManyFiles { files, bound, .. }) = outcome else {
        panic!("the file list was not refused: {outcome:?}");
    };
    assert_eq!(files, LISTED_FILES + 1);
    assert_eq!(bound, LISTED_FILES);
    assert!(
        Store::new(bench.home.path().join("programs"))
            .installed(&AgentKey::new("goose")?)?
            .is_empty(),
        "a refused archive committed a version"
    );
    Ok(())
}

/// A registry receipt with no probe for this platform is refused at launch and
/// names `crucible agents probe`, rather than starting the vendor unrecorded.
#[test]
fn a_registry_release_with_no_probe_record_is_refused_naming_the_probe_command()
-> Result<(), Failed> {
    let bench = Bench::new("registry-missing-probe")?;
    cache(&bench.home, &registry_json("1.0.0"));
    let current = installed_with_probe(&bench.home, "1.0.0", b"goose 1.0.0", None)?;
    configure(&bench.home, &current)?;
    let configured = instance(&bench.home, "work")?;
    let refused = Agent::believing(
        &configured,
        &REGISTRY,
        &catalog_fetch::state_in(bench.home.path()),
        |name| (name == "PATH").then(|| std::ffi::OsString::from("/bin")),
    )
    .expect_err("a release with no probe record was believed");
    let said = refused.to_string();
    assert!(said.contains("crucible agents probe"), "{said}");
    Ok(())
}

/// `verify` names a registry receipt's changed file, its source when the
/// receipt no longer records the registry document, and its probe record when
/// it is missing or was taken for another machine.
#[test]
fn verify_names_a_registry_receipts_changed_file_source_and_probe() -> Result<(), Failed> {
    let bench = Bench::new("registry-verify-record")?;
    let program = installed(&bench.home, "1.0.0", b"goose 1.0.0")?;
    let root = program.parent().ok_or("a version directory")?.to_owned();
    let receipt = root.join(".receipt");
    let verify = || -> Result<Command, Failed> {
        Ok(Command::VerifyAgent {
            agent: name("goose")?,
        })
    };
    let mut quiet = Answering {
        choice: None,
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };
    let named = |outcome: &Result<Outcome, AgentsError>| -> Vec<String> {
        let Ok(Outcome::Verified { versions, .. }) = outcome else {
            return Vec::new();
        };
        versions
            .iter()
            .flat_map(|(_, differs)| differs.iter().map(|one| one.path.to_string()))
            .collect()
    };

    let (whole, _) = bench.ask(verify()?, &mut quiet, false)?;
    assert_eq!(named(&whole), Vec::<String>::new(), "{}", shape(&whole));

    fs::write(&program, b"goose 1.0.1")?;
    let (changed, _) = bench.ask(verify()?, &mut quiet, false)?;
    assert_eq!(named(&changed), ["goose"], "{}", shape(&changed));
    fs::write(&program, b"goose 1.0.0")?;

    // A stub in `.bin` is a file the receipt names, so one changed byte in it
    // is named too.
    let stub = root
        .join(crucible_external_harnesses::release::BIN)
        .join("peer");
    fs::create_dir_all(stub.parent().ok_or("a stub directory")?)?;
    let stub_bytes = b"#!/bin/sh\nexec node peer.js\n";
    fs::write(&stub, stub_bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755))?;
    }
    let mut document: Value = serde_json::from_slice(&fs::read(&receipt)?)?;
    let probed = document
        .pointer("/probe")
        .cloned()
        .ok_or("the receipt's probe")?;
    document
        .pointer_mut("/files")
        .and_then(Value::as_array_mut)
        .ok_or("the receipt's files")?
        .push(json!({
            "path": ".bin/peer",
            "kind": "file",
            "size": stub_bytes.len(),
            "sha256": digest(stub_bytes),
            "exec": true,
        }));
    fs::write(&receipt, serde_json::to_vec(&document)?)?;
    let (stubbed, _) = bench.ask(verify()?, &mut quiet, false)?;
    assert_eq!(named(&stubbed), Vec::<String>::new(), "{}", shape(&stubbed));
    fs::write(&stub, b"#!/bin/sh\nexec node stolen.js\n")?;
    let (stub_changed, _) = bench.ask(verify()?, &mut quiet, false)?;
    assert_eq!(
        named(&stub_changed),
        [".bin/peer"],
        "{}",
        shape(&stub_changed)
    );
    fs::write(&stub, stub_bytes)?;

    set(&mut document, "/source", json!("catalog"));
    fs::write(&receipt, serde_json::to_vec(&document)?)?;
    let (sourced, _) = bench.ask(verify()?, &mut quiet, false)?;
    assert_eq!(named(&sourced), ["source"], "{}", shape(&sourced));

    set(&mut document, "/source", json!("registry"));
    set(&mut document, "/probe", Value::Null);
    fs::write(&receipt, serde_json::to_vec(&document)?)?;
    let (unprobed, _) = bench.ask(verify()?, &mut quiet, false)?;
    assert_eq!(named(&unprobed), ["probe"], "{}", shape(&unprobed));

    set(
        &mut document,
        "/probe",
        json!({
            "target": "plan9-x86_64",
            "admits": {"name": "goose", "version": "1.0.0"},
            "auth_methods": [],
        }),
    );
    fs::write(&receipt, serde_json::to_vec(&document)?)?;
    let (elsewhere, _) = bench.ask(verify()?, &mut quiet, false)?;
    assert_eq!(named(&elsewhere), ["probe"], "{}", shape(&elsewhere));

    // The registry has no catalog serial, so a receipt carrying one is named
    // at source even with its probe for this machine back in place.
    set(&mut document, "/probe", probed);
    set(&mut document, "/serial", json!(1));
    fs::write(&receipt, serde_json::to_vec(&document)?)?;
    let (serialed, _) = bench.ask(verify()?, &mut quiet, false)?;
    assert_eq!(named(&serialed), ["source"], "{}", shape(&serialed));
    Ok(())
}

/// `/catalog` says why the registry could not be read even where it lists no
/// registry agent, as the field's doc promises.
#[test]
fn a_catalog_with_no_registry_index_says_why() -> Result<(), Failed> {
    let bench = Bench::new("registry-no-index-listing")?;
    let url = bench.server.at("/registry.json");
    let mut front = Answering {
        choice: None,
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };
    let outcome = door_at(&bench, &mut front, Command::AgentCatalog, &url)?;
    let Ok(Outcome::Catalog {
        registry: Some(line),
        ..
    }) = outcome
    else {
        panic!("no unread line: {outcome:?}");
    };
    assert!(line.contains("404"), "{line}");
    Ok(())
}

/// A probe taken for another platform is not shown as this release's, and the
/// launch refuses it: the display and the launch read the receipt's own target.
#[test]
fn a_probe_for_another_platform_is_not_shown_or_launched() -> Result<(), Failed> {
    let bench = Bench::new("registry-other-platform")?;
    cache(&bench.home, &registry_json("1.0.0"));
    let mut other = probe("1.0.0");
    other.target = "plan9-x86_64".into();
    let current = installed_with_probe(&bench.home, "1.0.0", b"goose 1.0.0", Some(other))?;
    configure(&bench.home, &current)?;
    let mut front = Answering {
        choice: None,
        server: &bench.server,
        put: Vec::new(),
        meanwhile: None,
    };
    let outcome = door(&bench, &mut front, Command::AgentCatalog)?;
    let Ok(Outcome::Catalog { agents, .. }) = outcome else {
        panic!("no listing: {outcome:?}");
    };
    let goose = agents
        .iter()
        .find(|listed| listed.agent.as_str() == "goose")
        .expect("goose listed");
    assert!(
        goose.probed.is_none(),
        "another platform's probe was shown: {:?}",
        goose.probed
    );
    let configured = instance(&bench.home, "work")?;
    let refused = Agent::believing(
        &configured,
        &REGISTRY,
        &catalog_fetch::state_in(bench.home.path()),
        |name| (name == "PATH").then(|| std::ffi::OsString::from("/bin")),
    )
    .expect_err("another platform's probe was believed");
    let said = refused.to_string();
    assert!(said.contains("crucible agents probe"), "{said}");
    Ok(())
}

/// The configuration naming the instance `work` of `goose` started as
/// `executable`, written as a command to look for on `PATH`.
fn configure_command(home: &Home, executable: &str) -> Result<(), Failed> {
    let document = json!({
        "externalAgents": {
            "work": {"driver": "catalog", "agent": "goose", "executable": executable}
        }
    });
    fs::write(
        crucible_config::user(home),
        serde_json::to_vec_pretty(&document)?,
    )?;
    Ok(())
}

/// A registry agent whose program the user installed on `PATH` is refused,
/// naming the command that tries it, until it is tried; trying it starts it
/// once, confined with the network closed and its directory read-only, and
/// keeps what it answered, and a later session then runs it as the user's own
/// program without starting it.
#[test]
fn a_registry_agent_on_path_is_run_as_the_users_own_program_once_probed() -> Result<(), Failed> {
    use crucible_app::own_program::{self, Places};

    let bench = Bench::new("registry-own-on-path")?;
    let (tools, program, configured) = own_on_path(&bench)?;
    let state = catalog_fetch::state_in(bench.home.path());
    let path = tools.clone().into_os_string();
    let lookup = |name: &str| (name == "PATH").then(|| path.clone());

    let unprobed = Agent::believing(&configured, &REGISTRY, &state, lookup)
        .expect_err("an own program with no probe record was believed");
    assert!(
        matches!(unprobed, Unbelieved::OwnUnprobed { .. }),
        "{unprobed}"
    );
    assert!(
        unprobed.to_string().contains("crucible agents try goose"),
        "{unprobed}"
    );
    assert_eq!(fake_peer::played(&tools), 0, "belief started the program");

    let user = bench.tree.0.join("user");
    let record = own_program::tried(
        &Places {
            home: &user,
            crucible: bench.home.path(),
            sessions: bench.home.sessions(),
        },
        &configured,
        &["work"],
        lookup,
        &crucible_app::probing::local,
        Duration::from_secs(20),
    )
    .map_err(|why| why.to_string())?;
    assert_eq!(record.name(), "goose-acp");
    assert_eq!(record.version(), "1.2.3");
    // The peer counts its starts in its own directory, which the probe gives
    // it to read alone.
    assert_eq!(
        fake_peer::played(&tools),
        0,
        "the probed program wrote into its own directory"
    );

    let agent =
        Agent::believing(&configured, &REGISTRY, &state, lookup).map_err(|why| why.to_string())?;
    let AgentRow::Catalog(row) = agent.row() else {
        panic!("not a catalog row");
    };
    assert!(row.is_own(), "the program was taken for a release");
    assert!(row.warned().is_ok());
    assert_eq!(
        agent.installed(),
        Some(program.as_path()),
        "the instance was not started as the program found on PATH"
    );
    assert_eq!(fake_peer::played(&tools), 0, "belief started the program");
    Ok(())
}

/// Caches the registry listing `goose`, puts the user's own `goose` that
/// answers `initialize` in a directory of its own, and configures the
/// instance `work` to look for it on `PATH`: that directory, the program and
/// the instance.
fn own_on_path(bench: &Bench) -> Result<(PathBuf, PathBuf, ExternalAgent), Failed> {
    use crucible_external_harnesses::fake_peer::scripts::acp;
    use crucible_external_harnesses::fake_peer::{Reply as Said, Script};

    cache(&bench.home, &registry_json("1.0.0"));
    let tools = bench.tree.0.join("tools");
    let program = fake_peer::install(
        &tools,
        "goose",
        &Script::once(
            Said::new()
                .hear(1)
                .say(acp::offering("goose-acp", "1.2.3", &json!([]))),
        ),
    )?;
    configure_command(&bench.home, "goose")?;
    Ok((tools, program, instance(&bench.home, "work")?))
}

/// A sandbox that cannot say it closes the network, as one whose backend is
/// missing: it starts nothing.
struct Unclosing;

impl crucible_sandbox::SandboxService for Unclosing {
    fn probe(
        &self,
    ) -> BoxFuture<
        '_,
        Result<
            (
                crucible_sandbox::SandboxBackendIdentity,
                crucible_sandbox::SandboxCapabilities,
            ),
            crucible_sandbox::SandboxError,
        >,
    > {
        Box::pin(async {
            Err(crucible_sandbox::SandboxError::Unsupported {
                feature: crucible_sandbox::SandboxFeature::NetworkDeny,
            })
        })
    }

    fn prepare(
        &self,
        _: crucible_sandbox::SandboxRequest,
    ) -> BoxFuture<
        '_,
        Result<Box<dyn crucible_sandbox::SandboxSession>, crucible_sandbox::SandboxError>,
    > {
        Box::pin(async {
            Err(crucible_sandbox::SandboxError::Unsupported {
                feature: crucible_sandbox::SandboxFeature::NetworkDeny,
            })
        })
    }
}

/// Where the sandbox cannot close the network, the user's own program is not
/// started, nothing is recorded, and the refusal points at what the sandbox
/// needs; belief then still refuses it as untried.
#[test]
fn an_own_program_is_not_probed_where_the_sandbox_cannot_close_the_network() -> Result<(), Failed> {
    use crucible_app::own_program::{self, Places, Unowned};

    let bench = Bench::new("registry-own-unclosable")?;
    let (tools, _, configured) = own_on_path(&bench)?;
    let path = tools.clone().into_os_string();
    let lookup = |name: &str| (name == "PATH").then(|| path.clone());
    let user = bench.tree.0.join("user");
    let refused = own_program::tried(
        &Places {
            home: &user,
            crucible: bench.home.path(),
            sessions: bench.home.sessions(),
        },
        &configured,
        &["work"],
        lookup,
        &|_| std::sync::Arc::new(Unclosing),
        Duration::from_secs(20),
    )
    .expect_err("an own program was probed where the network cannot be closed");
    assert!(matches!(refused, Unowned::Unclosable(_)), "{refused}");
    assert!(refused.to_string().contains("Turning it on"), "{refused}");
    assert_eq!(fake_peer::played(&tools), 0, "the program was started");
    let state = catalog_fetch::state_in(bench.home.path());
    let unprobed = Agent::believing(&configured, &REGISTRY, &state, lookup)
        .expect_err("an own program refused its probe was believed");
    assert!(
        matches!(unprobed, Unbelieved::OwnUnprobed { .. }),
        "{unprobed}"
    );
    Ok(())
}

/// The user's own Antigravity under the sandbox with no profile directory
/// cannot reach the sign-in it keeps in `~/.gemini`, and one made there is
/// not kept, so it is offered no Google account or key to sign in by: not
/// as a move of the signed-out question, not by the door's list, and a
/// sign-in by one named anyway is refused as no method of the agent's.
#[test]
fn an_own_agy_under_the_sandbox_is_offered_no_key_method() -> Result<(), Failed> {
    use crate::external::choosing::{Started, own_agy_connected};
    use crucible_app::client::accounts::CHECK_AGAIN;

    let tree = crate::Tree::new("registry-own-agy-no-key")?;
    let (_, choices, _) = own_agy_connected(&tree, Started::Sandboxed)?;

    assert_eq!(choices, [vec![CHECK_AGAIN, "Cancel"]]);
    let home = tree.home()?;
    assert_eq!(sign_in_methods(&home, "space")?, Vec::<Box<str>>::new());
    for method in ["gemini-api-key", "oauth-personal"] {
        match runs_a_command(&home, "space", method) {
            Err(AgentsError::NoMethod {
                method: refused, ..
            }) => {
                assert!(refused.contains(method), "{refused}");
            }
            other => panic!("{method} was not refused: {other:?}"),
        }
    }
    Ok(())
}

/// The user's own Antigravity under the sandbox with no profile directory is
/// read signed out and offered no Google account to sign in by, since a
/// sign-in made where it cannot reach `~/.gemini` would be lost. In place of
/// the line telling the user to sign in at the agent, it is told the two ways
/// it can be signed in: turning the sandbox off, with what that costs every
/// command and agent, and the store release; where a project requires the
/// sandbox, only the store release.
#[test]
fn an_own_agy_under_the_sandbox_is_offered_no_sign_in_it_would_lose() -> Result<(), Failed> {
    use crate::external::choosing::{Started, own_agy_connected};
    use crucible_app::client::accounts::CHECK_AGAIN;

    let tree = crate::Tree::new("registry-own-agy-would-lose")?;
    let (_, choices, written) = own_agy_connected(&tree, Started::Sandboxed)?;
    assert_eq!(choices, [vec![CHECK_AGAIN, "Cancel"]]);
    let screen = written.join("\n");
    assert!(
        !screen.contains("open the agent and sign in there"),
        "{screen}"
    );
    let how = screen
        .lines()
        .find(|line| line.starts_with("fact: Sign in:"))
        .ok_or("no line says how to sign in")?;
    for said in [
        "~/.gemini",
        "turn the sandbox off",
        "docs/security/sandboxing.md",
        "removes confinement from every command and agent",
        "store release",
    ] {
        assert!(how.contains(said), "{said} is not said: {how}");
    }
    let shipped = crucible_external_harnesses::catalog::SNAPSHOT
        .entries()
        .first()
        .ok_or("the snapshot's entry")?;
    let answered = format!(
        "fact: Your own program answered as {}, version {}.",
        shipped.admits.name, shipped.admits.version
    );
    assert!(
        screen.contains(&answered),
        "{answered} is not said: {screen}"
    );
    let lost = "is gone when that start ends";
    assert_eq!(
        screen.contains(lost),
        cfg!(target_os = "linux"),
        "the lost sign-in is said where the sandbox gives a private home alone: {screen}"
    );

    let tree = crate::Tree::new("registry-own-agy-would-lose-required")?;
    let (_, choices, written) = own_agy_connected(&tree, Started::Required)?;
    assert_eq!(choices, [vec![CHECK_AGAIN, "Cancel"]]);
    let how = written
        .iter()
        .flat_map(|question| question.lines())
        .find(|line| line.starts_with("fact: Sign in:"))
        .ok_or("no line says how to sign in")?;
    assert!(!how.contains("turn the sandbox off"), "{how}");
    assert!(how.contains("store release"), "{how}");
    Ok(())
}

/// Caches the registry listing `goose`, puts the user's own `goose` in a
/// directory of its own, answering `initialize` at every start with
/// `methods` to sign in by, configures the instance `work` to start it by
/// its whole path, and probes it as `crucible agents try` does: that
/// directory, the program and the instance.
fn own_probed(bench: &Bench, methods: &Value) -> Result<(PathBuf, PathBuf, ExternalAgent), Failed> {
    use crucible_external_harnesses::fake_peer::scripts::acp;
    use crucible_external_harnesses::fake_peer::{Reply as Said, Script};

    own_probed_with(
        bench,
        &Script::once(
            Said::new()
                .hear(1)
                .say(acp::offering("goose-acp", "1.2.3", methods)),
        ),
    )
}

/// [`own_probed`], the program playing `script`.
fn own_probed_with(
    bench: &Bench,
    script: &crucible_external_harnesses::fake_peer::Script,
) -> Result<(PathBuf, PathBuf, ExternalAgent), Failed> {
    cache(&bench.home, &registry_json("1.0.0"));
    let tools = bench.tree.0.join("tools");
    let user = bench.tree.0.join("user");
    let (program, configured) = own_probed_in(bench, &tools, &user, script)?;
    Ok((tools, program, configured))
}

/// Puts the user's own `goose`, playing `script`, in `directory`, configures
/// the instance `work` to start it by its whole path, and probes it as
/// `crucible agents try` does with `home` for the user's home, against the
/// registry already cached.
fn own_probed_in(
    bench: &Bench,
    directory: &Path,
    home: &Path,
    script: &crucible_external_harnesses::fake_peer::Script,
) -> Result<(PathBuf, ExternalAgent), Failed> {
    use crucible_app::own_program::{self, Places};

    let program = fake_peer::install(directory, "goose", script)?;
    configure_command(&bench.home, &program.to_string_lossy())?;
    let configured = instance(&bench.home, "work")?;
    own_program::tried(
        &Places {
            home,
            crucible: bench.home.path(),
            sessions: bench.home.sessions(),
        },
        &configured,
        &["work"],
        |_| None,
        &crucible_app::probing::local,
        Duration::from_secs(20),
    )
    .map_err(|why| why.to_string())?;
    Ok((program, configured))
}

/// No sign-in command is run for the user's own program: a terminal method
/// its answer offers is refused before anything is asked or started, saying
/// to sign in with the program itself in a terminal and quoting what the
/// method says of itself with its control characters escaped.
#[test]
fn an_own_program_is_never_signed_in_by_a_command() -> Result<(), Failed> {
    let bench = Bench::new("registry-own-no-command")?;
    let methods = json!([{
        "type": "terminal",
        "id": "login",
        "name": "Log in",
        "description": "Run goose login\u{1b}[2J at a terminal",
        "command": "goose",
        "args": ["login"],
    }]);
    let (tools, _, _) = own_probed(&bench, &methods)?;
    let mut front = Recording {
        choice: Some("Yes"),
        facts: Vec::new(),
    };

    let outcome = door(
        &bench,
        &mut front,
        Command::SignInAgent {
            instance: name("work")?,
            method: name("login")?,
        },
    )?;

    let refused = match outcome {
        Err(AgentsError::Piped(why)) => why.to_string(),
        other => panic!("the own program's command was not refused: {other:?}"),
    };
    assert!(refused.contains("your own program"), "{refused}");
    assert!(refused.contains("in a terminal"), "{refused}");
    assert!(refused.contains("Run goose login"), "{refused}");
    assert!(!refused.contains('\u{1b}'), "{refused}");
    assert!(front.facts.is_empty(), "something was asked");
    assert_eq!(fake_peer::played(&tools), 0, "something started");
    Ok(())
}

/// What a connect to the instance `work` of the user's own `goose`, probed
/// and read signed out at its status check, tells a person, with the
/// sandbox on where `sandboxed` holds and the user file as it is: each
/// account question written out.
fn own_signed_out_screen(bench: &Bench, sandboxed: bool) -> Result<Vec<String>, Failed> {
    use crucible_external_harnesses::fake_peer::scripts::acp;

    own_signed_out_answering(
        bench,
        sandboxed,
        &acp::offering(
            "goose-acp",
            "1.2.3",
            &json!([{"id": "browser", "name": "Browser"}]),
        ),
    )
}

/// [`own_signed_out_screen`], the program answering `initialize` with
/// `answer` at every start.
fn own_signed_out_answering(
    bench: &Bench,
    sandboxed: bool,
    answer: &str,
) -> Result<Vec<String>, Failed> {
    use crucible_app::launching::{Launcher, Steps};
    use crucible_external_harnesses::fake_peer::scripts::acp;
    use crucible_external_harnesses::fake_peer::{Reply as Said, Script};
    use crucible_sandbox::{SandboxEnablement, SandboxPolicy};

    // Read-only under the sandbox, the peer plays the one reply at every
    // start: the probe hears `initialize` alone, a status check opens a
    // session too and is told it is signed out.
    let reply = Said::new()
        .hear(1)
        .say(answer)
        .hear(1)
        .say(acp::sign_in_required(1))
        .hear_out();
    let (_, _, configured) = own_probed_with(bench, &Script::once(reply))?;
    let runtime = super::runtime()?;
    let state = catalog_fetch::state_in(bench.home.path());
    let believed = Agent::believing(&configured, &REGISTRY, &state, |_| None)
        .map_err(|why| why.to_string())?;
    let consent = super::shipped_hold::shipped_given(believed.row().route())?;
    let policy = SandboxPolicy::standard(&bench.tree.workspace()?)?.with_enabled(sandboxed);
    let steps = Steps::new(
        Launcher::new(
            crucible_app::probing::local(runtime.handle().clone()),
            runtime.handle().clone(),
            &policy,
            &consent,
            &state,
            bench.home.sessions(),
        ),
        std::sync::Arc::new(SandboxEnablement::new(sandboxed, false)),
        &REGISTRY,
        None,
    );
    let mut conversation =
        crate::conversation(&bench.tree, crate::Script::new(Vec::new()), false)?.stepping(steps);
    let file = fs::read_to_string(bench.user())?;
    let (_, written) = crate::external::choosing::connect_cancelled(
        &bench.tree,
        &mut conversation,
        (&file, "work"),
    )?;
    Ok(written)
}

/// The user's own program of a registry agent is told, read signed out,
/// what reaches it of a sign-in: under the sandbox, that it starts signed out
/// since its registry entry names no place it keeps one in; with the sandbox
/// off, that a sign-in in a variable the user exported is not seen, and on
/// Linux that one in a desktop keyring or a relocated `XDG_CONFIG_HOME` or
/// `XDG_DATA_HOME` is not either.
#[test]
fn an_own_program_under_the_sandbox_is_told_it_starts_signed_out() -> Result<(), Failed> {
    let bench = Bench::new("registry-own-signed-out")?;
    let screen = own_signed_out_screen(&bench, true)?.join("\n");
    assert!(screen.contains("is not signed in."), "{screen}");
    assert!(
        screen.contains("starts signed out") && screen.contains("registry entry"),
        "{screen}"
    );

    let bench = Bench::new("registry-own-signed-out-unconfined")?;
    let screen = own_signed_out_screen(&bench, false)?.join("\n");
    assert!(!screen.contains("starts signed out"), "{screen}");
    assert!(
        screen.contains("a variable you exported is not seen"),
        "{screen}"
    );
    assert_eq!(
        screen.contains("desktop keyring") && screen.contains("XDG_CONFIG_HOME"),
        cfg!(target_os = "linux"),
        "{screen}"
    );
    Ok(())
}

/// The user's own program read signed out is shown the name and version it
/// answered with, its control characters escaped, or `did not say` for each
/// it gave none of, with the sandbox on and off alike.
#[test]
fn an_own_program_is_shown_the_name_and_version_it_answered() -> Result<(), Failed> {
    use crucible_external_harnesses::fake_peer::scripts::acp;

    let methods = json!([{"id": "browser", "name": "Browser"}]);
    let named = "goose\u{1b}[31m-acp";
    let escaped = crucible_types::shown::escaped(named);
    for sandboxed in [true, false] {
        let bench = Bench::new(&format!("registry-own-answered-{sandboxed}"))?;
        let screen =
            own_signed_out_answering(&bench, sandboxed, &acp::offering(named, "1.2.3", &methods))?
                .join("\n");
        assert!(screen.contains("is not signed in."), "{screen}");
        assert!(
            screen.contains(&format!(
                "fact: Your own program answered as {escaped}, version 1.2.3."
            )),
            "{screen}"
        );
        assert!(!screen.contains('\u{1b}'), "{screen}");

        let bench = Bench::new(&format!("registry-own-answered-silent-{sandboxed}"))?;
        let screen = own_signed_out_answering(
            &bench,
            sandboxed,
            &acp::response(
                0,
                &json!({
                    "protocolVersion": 1,
                    "agentCapabilities": {},
                    "authMethods": methods,
                }),
            ),
        )?
        .join("\n");
        assert!(screen.contains("is not signed in."), "{screen}");
        assert!(
            screen.contains("fact: Your own program answered as did not say, version did not say."),
            "{screen}"
        );
    }
    Ok(())
}

/// What a start was given: its arguments and its environment, by name.
#[derive(Debug, Clone)]
struct Seen {
    /// Its arguments.
    arguments: Vec<String>,
    /// Its environment, each variable's name and value.
    environment: Vec<(String, String)>,
}

/// A sandbox that starts each child as the one it wraps does, keeping what
/// each was started with.
struct Watched {
    /// The sandbox that starts them.
    local: std::sync::Arc<dyn crucible_sandbox::SandboxService>,
    /// What each start was given, in order.
    seen: std::sync::Arc<std::sync::Mutex<Vec<Seen>>>,
}

impl crucible_sandbox::SandboxService for Watched {
    fn probe(
        &self,
    ) -> BoxFuture<
        '_,
        Result<
            (
                crucible_sandbox::SandboxBackendIdentity,
                crucible_sandbox::SandboxCapabilities,
            ),
            crucible_sandbox::SandboxError,
        >,
    > {
        self.local.probe()
    }

    fn prepare(
        &self,
        request: crucible_sandbox::SandboxRequest,
    ) -> BoxFuture<
        '_,
        Result<Box<dyn crucible_sandbox::SandboxSession>, crucible_sandbox::SandboxError>,
    > {
        Box::pin(async move {
            let session: Box<dyn crucible_sandbox::SandboxSession> = Box::new(WatchedSession(
                self.local.prepare(request).await?,
                self.seen.clone(),
            ));
            Ok(session)
        })
    }
}

/// Its session, which keeps what the command it stages is given.
struct WatchedSession(
    Box<dyn crucible_sandbox::SandboxSession>,
    std::sync::Arc<std::sync::Mutex<Vec<Seen>>>,
);

impl crucible_sandbox::SandboxSession for WatchedSession {
    fn inspection(&self) -> &crucible_sandbox::SandboxInspection {
        self.0.inspection()
    }

    fn materialize(&mut self) -> BoxFuture<'_, Result<(), crucible_sandbox::SandboxError>> {
        self.0.materialize()
    }

    fn stage<'a>(
        self: Box<Self>,
        command: crucible_sandbox::SandboxCommand,
    ) -> BoxFuture<
        'a,
        Result<Box<dyn crucible_sandbox::SandboxLaunch>, crucible_sandbox::SandboxError>,
    >
    where
        Self: 'a,
    {
        if let Ok(mut seen) = self.1.lock() {
            seen.push(Seen {
                arguments: command
                    .arguments()
                    .iter()
                    .map(|argument| argument.to_string_lossy().into_owned())
                    .collect(),
                environment: command
                    .environment()
                    .iter()
                    .map(|(name, value)| (name.to_owned(), value.to_string_lossy().into_owned()))
                    .collect(),
            });
        }
        self.0.stage(command)
    }
}

/// What starting the configured instance `work` of the user's own `goose`
/// in `bench` comes to when it is started under the sandbox and takes a turn:
/// how the turn ended, else why it did not; and what each start, a probe
/// made first among them, was given.
fn own_started(
    bench: &Bench,
    configured: &ExternalAgent,
) -> Result<(Result<String, String>, Vec<Seen>), Failed> {
    own_started_in(bench, configured, None)
}

/// [`own_started`] by a launcher told of `home`, where one is given, as the
/// user's home in place of the shell's.
fn own_started_in(
    bench: &Bench,
    configured: &ExternalAgent,
    home: Option<&Path>,
) -> Result<(Result<String, String>, Vec<Seen>), Failed> {
    own_started_by(bench, configured, true, |launcher| match home {
        Some(home) => launcher.homed(home),
        None => launcher,
    })
}

/// [`own_started`] with the sandbox on where `sandboxed`, by the launcher
/// `told` makes of the one it is given.
fn own_started_by(
    bench: &Bench,
    configured: &ExternalAgent,
    sandboxed: bool,
    told: impl FnOnce(crucible_app::launching::Launcher<'_>) -> crucible_app::launching::Launcher<'_>,
) -> Result<(Result<String, String>, Vec<Seen>), Failed> {
    use crucible_app::launching::Launcher;
    use crucible_execution::{SessionRef, TurnRequest};
    use crucible_external_harnesses::Control;
    use crucible_sandbox::SandboxPolicy;
    use crucible_types::TurnId;

    let runtime = super::runtime()?;
    let state = catalog_fetch::state_in(bench.home.path());
    let agent =
        Agent::believing(configured, &REGISTRY, &state, |_| None).map_err(|why| why.to_string())?;
    let consent = super::shipped_hold::shipped_given(agent.row().route())?;
    let policy = SandboxPolicy::standard(&bench.tree.workspace()?)?.with_enabled(sandboxed);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let launcher = Launcher::new(
        std::sync::Arc::new(Watched {
            local: crucible_app::probing::local(runtime.handle().clone()),
            seen: seen.clone(),
        }),
        runtime.handle().clone(),
        &policy,
        &consent,
        &state,
        bench.home.sessions(),
    );
    let launcher = told(launcher);
    let control = Control::new().ok_or("a control")?;
    let trust = Trust::load(&state, SystemTime::now());
    let programs = bench.home.path().join("programs");
    let mut dialect = agent.dialect(
        crucible_tools::Mode::Ask,
        &trust,
        &programs,
        crucible_app::backends::Sandboxing {
            on: sandboxed,
            required: false,
        },
    )?;
    let outcome = runtime.block_on(async {
        let mut host = launcher
            .launch(&agent, &control, dialect.as_mut(), None)
            .await
            .map_err(|why| why.to_string())?;
        let turned = host
            .turn(
                dialect.as_mut(),
                &SessionRef::new("one").ok_or("a session's name")?,
                &TurnRequest::new(TurnId::FIRST, "go"),
                &Unwatched,
            )
            .await;
        drop(host.stop(crucible_external_harnesses::STOP_GRACE).await);
        turned
            .map(|outcome| format!("{outcome:?}"))
            .map_err(|failure| failure.reason().as_str().to_owned())
    });
    let seen = seen.lock().map(|seen| seen.clone()).unwrap_or_default();
    Ok((outcome, seen))
}

/// Where a turn's events go when none of them is looked at.
struct Unwatched;

impl crucible_execution::Sink for Unwatched {
    fn post(&self, _: crucible_execution::Event) {}
}

/// A program that answers initializing with `initialized`, then opens a
/// session and answers the turn taken in it, at every start.
fn turning(initialized: &str) -> crucible_external_harnesses::fake_peer::Script {
    use crucible_external_harnesses::fake_peer::Script;
    use crucible_external_harnesses::fake_peer::scripts::acp;

    Script::once(acp::opening(initialized).say(acp::stopped(2, "end_turn")))
}

/// The record kept for the instance `work` of the user's own `goose` started
/// as the registry fixture's entry says, where one is kept.
fn own_record(bench: &Bench) -> Option<crucible_app::own_program::Record> {
    own_record_setting(bench, &[])
}

/// [`own_record`] for an entry that also sets each of `more`, a name and its
/// value, after the fixture's own variable.
fn own_record_setting(
    bench: &Bench,
    more: &[(&str, &str)],
) -> Option<crucible_app::own_program::Record> {
    use crucible_external_harnesses::catalog::Variable;

    let env = [("GOOSE_HOME", "/tmp/goose")]
        .iter()
        .chain(more)
        .map(|(name, value)| Variable {
            name: (*name).into(),
            value: (*value).into(),
        })
        .collect::<Vec<_>>();
    crucible_app::own_program::read(
        bench.home.path(),
        &crucible_app::own_program::Instance {
            name: "work",
            driver: "catalog",
            agent: Some("goose"),
            args: &["acp".into()],
            env: &env,
        },
    )
}

/// Moves the modification time of the program at `program` to `to`, as a
/// program that updates itself in place does, leaving its bytes as they are.
fn retimed(program: &Path, to: SystemTime) -> Result<(), Failed> {
    fs::File::options()
        .write(true)
        .open(program)?
        .set_modified(to)?;
    Ok(())
}

/// The user's own program is started with the arguments its entry gives it
/// for this platform and the variables its entry sets, as the store's
/// release of the same entry is.
#[test]
fn an_own_program_is_started_with_its_entrys_arguments() -> Result<(), Failed> {
    use crucible_external_harnesses::fake_peer::scripts::acp;

    let bench = Bench::new("registry-own-arguments")?;
    let (_, _, configured) = own_probed_with(
        &bench,
        &turning(&acp::offering("goose-acp", "1.2.3", &json!([]))),
    )?;
    let (started, seen) = own_started(&bench, &configured)?;
    assert_eq!(started.as_deref(), Ok("Answered"), "{seen:?}");
    let [launch] = seen.as_slice() else {
        panic!("not started once, unprobed: {seen:?}");
    };
    assert_eq!(launch.arguments, ["acp"], "{launch:?}");
    assert!(
        launch
            .environment
            .contains(&("GOOSE_HOME".to_owned(), "/tmp/goose".to_owned())),
        "{launch:?}"
    );
    Ok(())
}

/// The user's own program changed since its probe, as one that updates
/// itself is, is probed again, confined, before it is started: one that
/// still answers as an agent is started and its record kept fresh; one that
/// no longer does is not started, and its record is removed.
#[test]
fn an_own_program_changed_since_its_probe_is_probed_again_before_its_launch() -> Result<(), Failed>
{
    use crucible_external_harnesses::fake_peer::scripts::acp;

    let bench = Bench::new("registry-own-changed")?;
    let (tools, program, configured) = own_probed_with(
        &bench,
        &turning(&acp::offering("goose-acp", "1.2.3", &json!([]))),
    )?;
    let canonical = fs::canonicalize(&program)?;
    let (unchanged, seen) = own_started(&bench, &configured)?;
    assert_eq!(unchanged.as_deref(), Ok("Answered"));
    assert_eq!(seen.len(), 1, "an unchanged program was probed again");

    retimed(&program, SystemTime::now() - Duration::from_hours(1))?;
    assert!(
        own_record(&bench).is_some_and(|record| !record.current(&canonical)),
        "the record was not stale"
    );
    let (changed, seen) = own_started(&bench, &configured)?;
    assert_eq!(changed.as_deref(), Ok("Answered"), "{seen:?}");
    assert_eq!(seen.len(), 2, "a changed program was not probed first");
    assert!(
        own_record(&bench).is_some_and(|record| record.current(&canonical)),
        "the record was not kept fresh"
    );

    fake_peer::install(
        &tools,
        "goose",
        &turning(&acp::response(
            0,
            &json!({"protocolVersion": 2, "agentCapabilities": {}}),
        )),
    )?;
    let (refused, seen) = own_started(&bench, &configured)?;
    let refused = refused.expect_err("a program that no longer admits was started");
    assert!(refused.contains("protocol version"), "{refused}");
    assert_eq!(seen.len(), 1, "the program was started past its probe");
    assert!(
        own_record(&bench).is_none(),
        "the record of a program that no longer admits was kept"
    );
    Ok(())
}

/// The user's own program is judged against the user's own home at its
/// launch, and at the probe a change since its probe brings, never against a
/// home its entry gives it: one in the user's home is refused at its launch
/// though its entry sets a home elsewhere, and one moved there since its probe
/// is refused by its probe again, leaving no record.
#[test]
fn an_own_program_is_judged_against_the_users_home_not_its_entrys() -> Result<(), Failed> {
    use crucible_external_harnesses::fake_peer::scripts::acp;

    let bench = Bench::new("registry-own-entry-home")?;
    let user = bench.tree.0.join("user");
    let elsewhere = bench.tree.0.join("elsewhere");
    let given = elsewhere.to_str().ok_or("a home that is text")?;
    let mut index = registry_json("1.0.0");
    let env = index
        .pointer_mut(&format!("/agents/0/distribution/binary/{}/env", platform()))
        .ok_or("the goose target's variables")?;
    env["HOME"] = json!(given);
    cache(&bench.home, &index);
    let script = turning(&acp::offering("goose-acp", "1.2.3", &json!([])));
    let setting = [("HOME", given)];

    // Probed while the user's home was elsewhere, a program in the home the
    // launch is told of is refused there, and its record is left as it was.
    let (_, configured) = own_probed_in(&bench, &user, &elsewhere, &script)?;
    let (started, seen) = own_started_in(&bench, &configured, Some(&user))?;
    let refused = started.expect_err("a program in the user's home was started");
    assert!(refused.contains("your home"), "{refused}");
    assert!(seen.is_empty(), "started: {seen:?}");
    assert!(
        own_record_setting(&bench, &setting).is_some(),
        "the record went"
    );

    // Probed outside the user's home and moved into it since, it is probed
    // again against that home, refused, and its record removed.
    let tools = bench.tree.0.join("tools");
    own_probed_in(&bench, &tools, &user, &script)?;
    let moved = user.join(format!("goose{}", std::env::consts::EXE_SUFFIX));
    configure_command(&bench.home, &moved.to_string_lossy())?;
    let configured = instance(&bench.home, "work")?;
    let (started, seen) = own_started_in(&bench, &configured, Some(&user))?;
    let refused = started.expect_err("a program moved into the user's home was started");
    assert!(refused.contains("changed since it was probed"), "{refused}");
    assert!(refused.contains("your home"), "{refused}");
    assert!(seen.is_empty(), "started: {seen:?}");
    assert!(
        own_record_setting(&bench, &setting).is_none(),
        "the record of a program refused by its probe again was kept"
    );
    Ok(())
}

/// The launcher `launcher` is, knowing no home of the user's.
fn unhomed(
    launcher: crucible_app::launching::Launcher<'_>,
) -> crucible_app::launching::Launcher<'_> {
    launcher.unhomed()
}

/// The user's home is asked for only where it is used: with no home of the
/// user's known, an own program whose record holds starts with the sandbox
/// off, and is refused, starting nothing, with it on, where its directory
/// would be judged against that home.
#[test]
fn an_own_program_needs_a_home_only_where_its_directory_is_judged() -> Result<(), Failed> {
    use crucible_external_harnesses::fake_peer::scripts::acp;

    let bench = Bench::new("registry-own-unhomed")?;
    let script = turning(&acp::offering("goose-acp", "1.2.3", &json!([])));
    let (_, _, configured) = own_probed_with(&bench, &script)?;

    let (started, _) = own_started_by(&bench, &configured, false, unhomed)?;
    started.map_err(|why| format!("refused with the sandbox off: {why}"))?;

    let (started, seen) = own_started_by(&bench, &configured, true, unhomed)?;
    let refused = started.expect_err("judged against no home, the program was started");
    assert!(
        refused.contains("no home directory of yours is known"),
        "{refused}"
    );
    assert!(refused.contains("USERPROFILE on Windows"), "{refused}");
    assert!(seen.is_empty(), "started: {seen:?}");
    Ok(())
}

/// The user's own program is taken for an agent by the protocol it answers
/// with and its capabilities, at its probe and at every start, never by its
/// name and version: one that names neither is admitted, recorded as saying
/// nothing of them; one that answers another protocol, or no capabilities,
/// is refused; and a start whose answer differs from the record replaces it
/// and goes on.
#[test]
fn an_own_program_with_no_agent_info_is_admitted_by_its_capabilities() -> Result<(), Failed> {
    use crucible_app::own_program::DID_NOT_SAY;
    use crucible_external_harnesses::fake_peer::scripts::acp;

    let answering = |result: Value| turning(&acp::response(0, &result));
    for (unadmitted, why) in [
        (
            json!({"protocolVersion": 2, "agentCapabilities": {}}),
            "protocol version",
        ),
        (json!({"protocolVersion": 1}), "capabilities"),
    ] {
        let bench = Bench::new("registry-own-unadmitted")?;
        let refused = own_probed_with(&bench, &answering(unadmitted.clone()))
            .expect_err("a program that is no agent was admitted");
        assert!(refused.to_string().contains(why), "{refused}");
        assert!(own_record(&bench).is_none(), "{unadmitted} was recorded");
    }

    let bench = Bench::new("registry-own-no-agent-info")?;
    let (tools, program, configured) = own_probed_with(
        &bench,
        &answering(json!({"protocolVersion": 1, "agentCapabilities": {}})),
    )?;
    let record = own_record(&bench).ok_or("nothing was recorded")?;
    assert_eq!(record.name(), DID_NOT_SAY);
    assert_eq!(record.version(), DID_NOT_SAY);
    let (admitted, _) = own_started(&bench, &configured)?;
    assert_eq!(admitted.as_deref(), Ok("Answered"));

    // What it answers changes where the program itself does not, so each
    // start below is held to its answer, not probed again.
    let stamped = fs::metadata(&program)?.modified()?;
    let answers = |result: Value| -> Result<(), Failed> {
        fake_peer::install(&tools, "goose", &answering(result))?;
        retimed(&program, stamped)
    };
    for (unadmitted, why) in [
        (
            json!({"protocolVersion": 2, "agentCapabilities": {}}),
            "protocol version",
        ),
        (json!({"protocolVersion": 1}), "capabilities"),
    ] {
        answers(unadmitted.clone())?;
        let (refused, seen) = own_started(&bench, &configured)?;
        assert_eq!(seen.len(), 1, "{unadmitted} was probed again");
        let refused = refused.expect_err("a start that is no agent was admitted");
        assert!(refused.contains(why), "{unadmitted}: {refused}");
    }

    answers(json!({
        "protocolVersion": 1,
        "agentInfo": {"name": "goose-acp", "version": "2.0.0"},
        "agentCapabilities": {"loadSession": true},
    }))?;
    let (admitted, seen) = own_started(&bench, &configured)?;
    assert_eq!(admitted.as_deref(), Ok("Answered"));
    assert_eq!(seen.len(), 1, "an unchanged program was probed again");
    let record = own_record(&bench).ok_or("the record was not kept")?;
    assert_eq!(record.name(), "goose-acp", "the record was not replaced");
    assert_eq!(record.version(), "2.0.0");
    Ok(())
}

/// What a turn of the user's own Antigravity, started under the sandbox,
/// `required` by a project where it says, with no profile directory, comes
/// to when the program answers the turn with the link to sign in: how the
/// turn ended, else why it did not.
fn own_agy_turned(tree: &crate::Tree, required: bool) -> Result<Result<String, String>, Failed> {
    use crucible_app::access::Route;
    use crucible_app::backends::{Registry, Row, Sandboxing};
    use crucible_app::launching::Launcher;
    use crucible_config::{Driver, Serves};
    use crucible_execution::{SessionRef, TurnRequest};
    use crucible_external_harnesses::Control;
    use crucible_external_harnesses::acp::SignIn;
    use crucible_external_harnesses::antigravity::Manifest;
    use crucible_external_harnesses::catalog::SNAPSHOT;
    use crucible_external_harnesses::fake_peer::Script;
    use crucible_external_harnesses::fake_peer::scripts::acp;
    use crucible_sandbox::SandboxPolicy;
    use crucible_types::TurnId;

    let shipped = SNAPSHOT.entries().first().ok_or("the snapshot's entry")?;
    let reviewed = acp::initialized(
        &shipped.admits.name,
        &shipped.admits.version,
        Manifest::REVIEWED.sign_in.first().map_or("", SignIn::id),
    );
    let link = "Open the following link to authenticate the ACP server: \
                https://accounts.google.com/o/oauth2/v2/auth?state=s";
    let program = fake_peer::install(
        &tree.0.join("peer"),
        "antigravity",
        &Script::once(acp::opening(&reviewed).say(link)),
    )?;
    let built = REGISTRY
        .built(Driver::Antigravity)
        .ok_or("Antigravity has no row")?;
    // The peer reaches no host.
    let row: &'static Row = Box::leak(Box::new(Row {
        reaches: &[],
        ..*built
    }));
    let executable = program.to_string_lossy().into_owned();
    let route = Route::External {
        serves: Serves::Provider("google"),
        instance: "space".into(),
        driver: Driver::Antigravity,
        executable: executable.as_str().into(),
        profile: None,
        model: None,
        effort: None,
        location: None,
        enabled: true,
    };
    let registry = Registry::new(std::slice::from_ref(row));
    let agent = Agent::routed(&route, &registry, |_| None).ok_or("Antigravity's route")?;

    let runtime = super::runtime()?;
    let home = tree.home()?;
    let state = catalog_fetch::state_in(home.path());
    let consent = super::shipped_hold::shipped_given(row.route)?;
    let policy = SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(true);
    let sessions = tree.sessions();
    let launcher = Launcher::new(
        crucible_app::probing::local(runtime.handle().clone()),
        runtime.handle().clone(),
        &policy,
        &consent,
        &state,
        &sessions,
    );
    let control = Control::new().ok_or("a control")?;
    let trust = Trust::load(&state, SystemTime::now());
    let mut dialect = agent.dialect(
        crucible_tools::Mode::Ask,
        &trust,
        &home.path().join("programs"),
        Sandboxing { on: true, required },
    )?;
    Ok(runtime.block_on(async {
        let mut host = launcher
            .launch(&agent, &control, dialect.as_mut(), None)
            .await
            .map_err(|why| why.to_string())?;
        let turned = host
            .turn(
                dialect.as_mut(),
                &SessionRef::new("one").ok_or("a session's name")?,
                &TurnRequest::new(TurnId::FIRST, "go"),
                &Unwatched,
            )
            .await;
        drop(host.stop(crucible_external_harnesses::STOP_GRACE).await);
        turned
            .map(|outcome| format!("{outcome:?}"))
            .map_err(|failure| failure.reason().as_str().to_owned())
    }))
}

/// A turn of the user's own Antigravity under the sandbox with no profile
/// directory, which the program answers by asking for a sign-in, fails naming
/// the two ways it can be signed in rather than `/login`: turning the sandbox
/// off, with what that costs every command and agent, and the store release.
/// Where a project requires the sandbox, only the store release is named.
#[test]
fn an_own_agy_turn_without_a_sign_in_names_the_two_ways() -> Result<(), Failed> {
    let tree = crate::Tree::new("registry-own-agy-turn")?;
    let failed = own_agy_turned(&tree, false)?
        .err()
        .ok_or("the turn did not fail")?;
    assert!(!failed.contains("/login"), "{failed}");
    for said in [
        "~/.gemini",
        "turn the sandbox off",
        "docs/security/sandboxing.md",
        "every command and agent in the conversation",
        "store release",
    ] {
        assert!(failed.contains(said), "{said} is not said: {failed}");
    }

    let tree = crate::Tree::new("registry-own-agy-turn-required")?;
    let failed = own_agy_turned(&tree, true)?
        .err()
        .ok_or("the turn did not fail")?;
    assert!(!failed.contains("/login"), "{failed}");
    assert!(!failed.contains("turn the sandbox off"), "{failed}");
    assert!(failed.contains("store release"), "{failed}");
    Ok(())
}
