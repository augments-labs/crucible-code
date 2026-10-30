//! The agents door installs a release a pinned interpreter starts: its
//! interpreter and its wheels are fetched only after the yes, each wheel
//! installed under `site/` with the launcher written beside them, and the
//! version committed whole or not at all; a start runs the interpreter with
//! its own arguments, the launcher's path, then the agent's. An entry point
//! the launcher cannot be written for is refused before the question, a
//! wheel that is not its pin or lands on a path already written commits
//! nothing, and a release a pinned Node starts is still refused before it.
//! An installed version is started through its launcher or not at all: once
//! the catalog no longer says a pinned interpreter starts it, it is refused
//! before any process, and where a newer release that one starts is listed
//! instead, it still starts through its own launcher.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crucible_app::backends::{CatalogRow, Registry};
use crucible_app::catalog_fetch;
use crucible_app::client::agents::{AgentsError, How, Outcome};
use crucible_app::content_use::{Consent, Routes};
use crucible_app::launching::{Agent, Launcher, Unlaunchable, Unlaunched};
use crucible_client_api::{Command, Pending, Text};
use crucible_config::{Driver, ExternalAgent};
use crucible_external_harnesses::fake_peer;
use crucible_external_harnesses::release::proof;
use crucible_external_harnesses::{Account, Control, Reading};
use crucible_sandbox::{SandboxPolicy, SandboxService};
use crucible_sandbox_local::LocalSandbox;
use crucible_tools::Mode;
use crucible_types::AgentKey;
use serde_json::{Value, json};

use super::{
    Bench, CATALOG, Failed, Reply, Server, answering, answers, artifact, asset_path, digest,
    install, launched, peer, platform, row_of, runtime, set, shape, stating,
};

/// The interpreter's path in the version, as the catalog lists it.
fn python() -> String {
    format!("python/bin/python3{}", std::env::consts::EXE_SUFFIX)
}

/// The module the launcher imports.
const CLI: &[u8] = b"def main():\n    return 0\n";

/// A wheel of goose's module, and each file it installs.
fn wheel() -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
    proof::wheel(
        "goose_acp-1.0.dist-info",
        &[
            ("goose_acp/__init__.py", b""),
            ("goose_acp/cli.py", CLI),
            ("goose_acp-1.0.dist-info/METADATA", b"Name: goose-acp\n"),
        ],
    )
}

/// `wheel`, built at review and kept on the catalog's host, as an artifact
/// of a release lists it.
fn wheel_artifact(wheel: &(Vec<u8>, Vec<(String, Vec<u8>)>)) -> Value {
    let (bytes, installs) = wheel;
    let total: usize = installs.iter().map(|(_, bytes)| bytes.len()).sum();
    let files: Vec<Value> = installs
        .iter()
        .map(|(path, bytes)| json!({"path": path, "size": bytes.len(), "sha256": digest(bytes)}))
        .collect();
    json!({
        "url": "https://downloads.example.com/goose_acp-1.0-py3-none-any.whl",
        "sha256": "a".repeat(64),
        "size": 1000,
        "format": "wheel",
        "unpacked": total,
        "decompressed": total,
        "files": files,
        "replaced_by": {"sha256": digest(bytes), "size": bytes.len()}
    })
}

/// `goose` 1.0.0 as a pinned interpreter starts it, calling `entry_point`:
/// the fake peer playing `played` as the interpreter, with its script beside
/// it, and `wheel`, each kept on `server`.
fn interpreted(
    server: &Server,
    entry_point: &str,
    played: &(Vec<u8>, Vec<u8>),
    wheel: &(Vec<u8>, Vec<(String, Vec<u8>)>),
) -> Value {
    let (program, script) = played;
    for bytes in [program, script, &wheel.0] {
        server.serve(&asset_path(bytes), Reply::Body(bytes.clone()));
    }
    stating(json!({
        "key": "goose",
        "registry_id": "goose-acp",
        "version": "1.0.0",
        "kind": "uvx",
        "license": "Apache-2.0",
        "review": "https://github.com/augments-labs/crucible-agent-catalog/pull/12",
        "admits": {"name": "goose", "version": "1.0.0"},
        "targets": [{
            "platform": platform(),
            "cmd": python(),
            "interpreter_args": ["-I"],
            "entry_point": entry_point,
            "args": ["acp"],
            "env": {"GOOSE_NO_UPDATE": "1"},
            "hosts": ["downloads.example.com"],
            "artifacts": [
                artifact(&python(), program),
                artifact("python/bin/script", script),
                wheel_artifact(wheel),
            ]
        }]
    }))
}

/// Where version 1.0.0 is put.
fn version(bench: &Bench) -> PathBuf {
    bench.programs().join("1.0.0")
}

/// What the catalog's host was asked for once each of `assets` was fetched
/// after the catalog.
fn catalog_then(assets: &[&[u8]]) -> Vec<String> {
    CATALOG
        .iter()
        .map(|path| (*path).to_owned())
        .chain(assets.iter().map(|bytes| asset_path(bytes)))
        .collect()
}

/// What goose, the agent `row` gives, says when its version is asked of
/// `executable`, with the sandbox off, or why it was not started: a step run
/// to its end, so the peer standing in for the interpreter has kept the
/// arguments it was given.
fn versioned(
    bench: &Bench,
    row: CatalogRow,
    executable: &Path,
) -> Result<Result<Reading, Unlaunched>, Failed> {
    let consent = Consent::new(Routes::production().cataloging(row.warned().ok().cloned()));
    consent.give("agent:goose");
    let registry: &'static Registry = Box::leak(Box::new(
        Registry::new(&[]).cataloging(Box::leak(Box::new([row]))),
    ));
    let key = AgentKey::new("goose")?;
    let instance = ExternalAgent::unwritten(
        "work",
        Driver::Catalog,
        &executable.to_string_lossy(),
        None,
        Some(&key),
    )
    .ok_or("a catalog instance naming its agent")?;
    let agent = Agent::configured(&instance, registry, |_| None).ok_or("its catalog row")?;
    let runtime = runtime()?;
    let handle = runtime.handle().clone();
    let policy = SandboxPolicy::standard(&bench.tree.workspace()?)?.with_enabled(false);
    let sandbox: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(handle.clone()));
    let state = catalog_fetch::state_in(bench.home.path());
    let launcher = Launcher::new(
        sandbox,
        handle,
        &policy,
        &consent,
        &state,
        bench.home.sessions(),
    );
    let control = Control::new().ok_or("a control")?;
    let mut dialect = agent.row().dialect(Mode::Ask, None, None)?;
    Ok(runtime.block_on(launcher.account(
        &agent,
        &control,
        dialect.as_mut(),
        Account::Version,
        &|_| {},
    )))
}

/// Asserts that nothing of goose is kept and the user file is `before`.
fn unchanged(bench: &Bench, before: &[u8], what: &str) -> Result<(), Failed> {
    assert!(!version(bench).exists(), "{what}");
    assert!(
        bench
            .stored()
            .iter()
            .all(|name| name == ".lock" || name == ".staging"),
        "{what}: {:?}",
        bench.stored()
    );
    let staging = bench.programs().join(".staging");
    if staging.exists() {
        assert!(fs::read_dir(staging)?.next().is_none(), "{what}");
    }
    assert_eq!(fs::read(bench.user())?, before, "{what}");
    Ok(())
}

#[test]
fn a_release_a_pinned_interpreter_starts_is_installed_after_the_yes_and_started_through_its_launcher()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-uvx")?;
    let played = peer(&answers("1.0.0"))?;
    let wheel = wheel();
    let release = interpreted(&bench.server, "goose_acp.cli:main", &played, &wheel);
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    let program = version(&bench).join(python());
    bench.configure(&program, &json!({}))?;
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(
            &outcome,
            Ok(Outcome::Moved { how: How::Installed, version, instances, downloaded: true, .. })
                if &**version == "1.0.0" && instances.len() == 1
        ),
        "{}",
        shape(&outcome)
    );
    // Nothing of the release before the answer; all of it after.
    let [(Pending::Install { .. }, asked)] = front.put.as_slice() else {
        return Err("one install question was put".into());
    };
    assert_eq!(asked, &CATALOG);
    assert_eq!(
        bench.server.asked(),
        catalog_then(&[&played.0, &played.1, &wheel.0])
    );
    let version = version(&bench);
    assert_eq!(fs::read(&program)?, played.0);
    assert_eq!(fs::read(version.join("site/goose_acp/cli.py"))?, CLI);
    assert!(
        fs::read_to_string(version.join("launch.py"))?.contains("\"goose_acp.cli:main\""),
        "the launcher imports the entry point's module"
    );
    assert_eq!(bench.executable()?, program.to_string_lossy());

    // Started, the interpreter is given its own arguments, then the
    // launcher, then the agent's.
    let read = versioned(&bench, row_of(&release)?, &program)??;

    assert!(
        matches!(read, Reading::Version(_)),
        "it did not answer initialize"
    );
    let launcher = fs::canonicalize(version.join("launch.py"))?;
    let started = fake_peer::argued(&version.join("python/bin"));
    assert_eq!(
        started.first().map(Vec::as_slice),
        Some(
            [
                "-I".to_owned(),
                launcher.to_string_lossy().into_owned(),
                "acp".to_owned(),
            ]
            .as_slice()
        )
    );

    // Committed with its launcher, so asked again it is the version in
    // place, and nothing of the release is fetched.
    bench.server.forget();
    let mut again = answering(&bench.server, Some("Install"));
    let (kept, _) = bench.ask(install("1.0.0")?, &mut again, false)?;

    assert!(
        matches!(
            &kept,
            Ok(Outcome::Moved {
                downloaded: false,
                ..
            })
        ),
        "{}",
        shape(&kept)
    );
    assert_eq!(bench.server.asked(), CATALOG);
    Ok(())
}

#[test]
fn a_no_to_a_release_a_pinned_interpreter_starts_fetches_none_of_it_and_shows_how_it_starts()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-uvx-no")?;
    let release = interpreted(
        &bench.server,
        "goose_acp.cli:main",
        &peer(&answers("1.0.0"))?,
        &wheel(),
    );
    bench.server.catalog(1, &[release], &json!([]))?;
    bench.configure(&version(&bench).join(python()), &json!({}))?;
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, None);

    let (outcome, told) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(outcome, Ok(Outcome::Undecided)),
        "{}",
        shape(&outcome)
    );
    let [(Pending::Install { facts, .. }, asked)] = front.put.as_slice() else {
        return Err("one install question was put".into());
    };
    assert_eq!(asked, &CATALOG);
    let started = format!("Started as: {} -I launch.py acp", python());
    assert!(
        facts.iter().map(Text::as_str).any(|fact| fact == started),
        "{started:?} in {facts:?}"
    );
    assert_eq!(bench.server.asked(), CATALOG);
    assert!(told.is_empty());
    unchanged(&bench, &before, "declined")
}

#[test]
fn an_entry_point_the_launcher_cannot_be_written_for_is_refused_before_the_question()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-uvx-entry-point")?;
    // The catalog reads it as a module and an attribute; neither side of a
    // launcher's import may hold a hyphen.
    let release = interpreted(
        &bench.server,
        "goose-acp.cli:main",
        &peer(&answers("1.0.0"))?,
        &wheel(),
    );
    bench.server.catalog(1, &[release], &json!([]))?;
    bench.configure(&version(&bench).join(python()), &json!({}))?;
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert_eq!(shape(&outcome), "Err(Unpacked)");
    let said = outcome.err().map(|why| why.to_string()).unwrap_or_default();
    assert!(
        said.contains("entry_point"),
        "the refusal does not name the field"
    );
    assert!(front.put.is_empty());
    assert_eq!(bench.server.asked(), CATALOG);
    unchanged(&bench, &before, "refused")
}

#[test]
fn a_wheel_that_is_not_its_pin_or_lands_where_another_artifact_wrote_commits_nothing()
-> Result<(), Failed> {
    type Altered = fn(&Server, &mut Value, &[u8]);
    let played = peer(&answers("1.0.0"))?;
    let wheel = wheel();
    let cases: [(&str, Altered, &str); 2] = [
        (
            "a wheel of other bytes at its pinned size",
            |server, _, wheel| {
                server.serve(&asset_path(wheel), Reply::Body(vec![b' '; wheel.len()]));
            },
            "Err(Download)",
        ),
        (
            "an interpreter that holds a file the wheel installs",
            |server, release, _| {
                server.serve(&asset_path(CLI), Reply::Body(CLI.to_vec()));
                if let Some(artifacts) = release
                    .pointer_mut("/targets/0/artifacts")
                    .and_then(Value::as_array_mut)
                {
                    artifacts.insert(2, artifact("site/goose_acp/cli.py", CLI));
                }
            },
            "Err(Unpacked)",
        ),
    ];
    for (index, (what, altered, refused)) in cases.into_iter().enumerate() {
        let bench = Bench::new(&format!("agents-door-uvx-uncommitted-{index}"))?;
        let mut release = interpreted(&bench.server, "goose_acp.cli:main", &played, &wheel);
        altered(&bench.server, &mut release, &wheel.0);
        bench.server.catalog(1, &[release], &json!([]))?;
        bench.configure(&version(&bench).join(python()), &json!({}))?;
        let before = fs::read(bench.user())?;
        let mut front = answering(&bench.server, Some("Install"));

        let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

        assert_eq!(shape(&outcome), refused, "{what}");
        assert_eq!(front.put.len(), 1, "{what}");
        unchanged(&bench, &before, what)?;
    }
    Ok(())
}

#[test]
fn a_version_whose_launcher_is_gone_is_refused_before_any_process() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-uvx-unlaunchable")?;
    let played = peer(&answers("1.0.0"))?;
    let release = interpreted(&bench.server, "goose_acp.cli:main", &played, &wheel());
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    let program = version(&bench).join(python());
    bench.configure(&program, &json!({}))?;
    let (installed, _) = bench.ask(
        install("1.0.0")?,
        &mut answering(&bench.server, Some("Install")),
        false,
    )?;
    assert!(
        matches!(installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    fs::remove_file(version(&bench).join("launch.py"))?;

    let (ended, _) = launched(&bench, row_of(&release)?, &program)?;

    assert!(
        matches!(ended, Err(Unlaunched::Launcher { .. })),
        "a start without its launcher was not refused as one"
    );
    assert!(fake_peer::argued(&version(&bench).join("python/bin")).is_empty());
    Ok(())
}

/// `goose` at `version` as a pinned Node starts it.
fn noded(version: &str) -> Value {
    let mut release = stating(super::goose(version, b"node"));
    set(&mut release, "/kind", json!("npm"));
    if let Some(target) = release
        .pointer_mut("/targets/0")
        .and_then(Value::as_object_mut)
    {
        target.insert("node_args".into(), json!([]));
        target.insert("bin".into(), json!("index.js"));
        target.insert(
            "lock".into(),
            json!({"sha256": "b".repeat(64), "size": 100, "unpack_rules": 1}),
        );
        target.insert("scripts".into(), json!([]));
    }
    release
}

#[test]
fn a_release_a_pinned_node_starts_is_refused_before_the_question() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-npm")?;
    let release = noded("1.0.0");
    bench.server.catalog(1, &[release], &json!([]))?;
    bench.configure(&bench.program("1.0.0"), &json!({}))?;
    let before = fs::read(bench.user())?;
    let mut front = answering(&bench.server, Some("Install"));

    let (outcome, _) = bench.ask(install("1.0.0")?, &mut front, false)?;

    assert!(
        matches!(&outcome, Err(AgentsError::Unsupported(why)) if why.contains("Node") && !why.contains("interpreter")),
        "{}",
        shape(&outcome)
    );
    assert!(front.put.is_empty());
    assert_eq!(bench.server.asked(), CATALOG);
    assert_eq!(fs::read(bench.user())?, before);
    Ok(())
}

/// Installs goose 1.0.0 as a pinned interpreter starts it, the fake peer
/// playing `played` standing in for that interpreter, then takes in the
/// catalog of serial 2, which lists `entries` instead; and gives the release
/// installed and its interpreter's path.
fn superseded(
    bench: &Bench,
    played: &(Vec<u8>, Vec<u8>),
    entries: &[Value],
) -> Result<(Value, PathBuf), Failed> {
    let release = interpreted(&bench.server, "goose_acp.cli:main", played, &wheel());
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    let program = version(bench).join(python());
    bench.configure(&program, &json!({}))?;
    let (installed, _) = bench.ask(
        install("1.0.0")?,
        &mut answering(&bench.server, Some("Install")),
        false,
    )?;
    assert!(
        matches!(installed, Ok(Outcome::Moved { .. })),
        "{}",
        shape(&installed)
    );
    assert!(version(bench).join("launch.py").is_file());
    bench.server.catalog(2, entries, &json!([]))?;
    let (listed, _) = bench.ask(
        Command::AgentCatalog,
        &mut answering(&bench.server, None),
        false,
    )?;
    assert!(
        matches!(
            listed,
            Ok(Outcome::Catalog {
                unrefreshed: None,
                ..
            })
        ),
        "{}",
        shape(&listed)
    );
    Ok((release, program))
}

/// Once the catalog no longer says a pinned interpreter starts it, a version
/// that holds its launcher is not started at all, rather than as a program of
/// its own: withdrawn, and listed instead at a newer release a program of its
/// own or a pinned Node starts, it is refused before any process.
#[test]
fn a_version_holding_its_launcher_is_refused_once_the_catalog_no_longer_says_how_it_starts()
-> Result<(), Failed> {
    let played = peer(&answers("1.0.0"))?;
    let cases: [(&str, Vec<Value>); 3] = [
        ("withdrawn", Vec::new()),
        (
            "a program of its own",
            vec![stating(super::goose("2.0.0", b"goose 2.0.0"))],
        ),
        ("a pinned Node", vec![noded("2.0.0")]),
    ];
    let mut found = Vec::new();
    for (index, (what, entries)) in cases.iter().enumerate() {
        let bench = Bench::new(&format!("agents-door-uvx-unlisted-{index}"))?;
        let (release, program) = superseded(&bench, &played, entries)?;

        let ended = versioned(&bench, row_of(&release)?, &program)?;

        let refused = matches!(
            ended,
            Err(Unlaunched::Launcher {
                why: Unlaunchable::Unlisted,
                ..
            })
        );
        found.push((
            *what,
            refused,
            fake_peer::argued(&version(&bench).join("python/bin")),
        ));
    }

    // Each refused as a start the catalog no longer says, and the
    // interpreter never started, so it kept no arguments.
    let expected: Vec<_> = cases
        .iter()
        .map(|(what, _)| (*what, true, Vec::<Vec<String>>::new()))
        .collect();
    assert_eq!(found, expected);
    Ok(())
}

/// A version the catalog no longer lists, where a newer release a pinned
/// interpreter starts is listed instead, is still started through its own
/// launcher, isolated.
#[test]
fn a_version_superseded_by_a_newer_interpreted_release_starts_through_its_own_launcher()
-> Result<(), Failed> {
    let bench = Bench::new("agents-door-uvx-superseded")?;
    let played = peer(&answers("1.0.0"))?;
    let mut newer = interpreted(
        &bench.server,
        "goose_acp.cli:main",
        &peer(&answers("2.0.0"))?,
        &wheel(),
    );
    set(&mut newer, "/version", json!("2.0.0"));
    set(&mut newer, "/admits/version", json!("2.0.0"));
    let (release, program) = superseded(&bench, &played, &[newer])?;

    let read = versioned(&bench, row_of(&release)?, &program)??;

    assert!(
        matches!(read, Reading::Version(_)),
        "it did not answer initialize"
    );
    let launcher = fs::canonicalize(version(&bench).join("launch.py"))?;
    let started = fake_peer::argued(&version(&bench).join("python/bin"));
    assert_eq!(
        started.first().map(Vec::as_slice),
        Some(
            [
                "-I".to_owned(),
                launcher.to_string_lossy().into_owned(),
                "acp".to_owned(),
            ]
            .as_slice()
        )
    );
    Ok(())
}
