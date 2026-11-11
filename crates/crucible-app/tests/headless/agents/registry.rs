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
