//! How the user's own program of a catalog agent is signed in: by a key kept
//! for it at `/login`, given at every start and withheld from what it says,
//! and never by a command its entry gives.

use crucible_app::client::agents::AgentsError;
use crucible_external_harnesses::fake_peer::{self, Reply as Played, Script as Peer, scripts::acp};
use serde_json::json;

use super::{Bench, ONE, answering, shape, sign_in, signing_in};
use crate::{Failed, Tree};

/// The user's own program is signed in by no command, even one its catalog
/// entry gives: the terminal method is refused before anything is asked or
/// started, saying to sign the program in itself in a terminal as its entry
/// says.
#[test]
fn an_own_programs_terminal_method_is_refused_for_it_to_run_itself() -> Result<(), Failed> {
    let bench = Bench::new("agents-door-sign-in-own-terminal")?;
    bench
        .server
        .catalog(1, &[signing_in("goose", ONE)], &json!([]))?;
    let made = Tree::new("agents-door-sign-in-own-terminal-peer")?;
    let program = fake_peer::install(
        &made.0,
        "goose",
        &Peer::once(
            Played::new()
                .hear(1)
                .say(acp::initialized("goose", "1.0.0", "terminal")),
        ),
    )?;
    bench.configure(&program, &json!({}))?;
    let mut front = answering(&bench.server, Some("Yes"));

    let (outcome, _) = bench.ask(sign_in("work", "terminal")?, &mut front, false)?;

    let refused = match &outcome {
        Err(AgentsError::Piped(why)) => why.to_string(),
        other => {
            return Err(format!(
                "the own program's command was not refused: {}",
                shape(other)
            )
            .into());
        }
    };
    assert!(refused.contains("your own program"), "{refused}");
    assert!(refused.contains("in a terminal"), "{refused}");
    assert!(refused.contains("Sign in at a terminal"), "{refused}");
    assert!(front.put.is_empty(), "something was asked");
    assert_eq!(fake_peer::played(&made.0), 0, "something started");
    Ok(())
}

/// The user's own program of an agent whose catalog entry gives a key method
/// is given the key kept for that method at launch, in the variable the entry
/// names, and that variable's value is withheld from what the program says.
#[cfg(unix)]
#[test]
fn a_kept_key_is_given_to_your_own_program_and_withheld_from_what_it_says() -> Result<(), Failed> {
    use std::fs;

    use crucible_app::client::agents::{Typed, keep_key};

    use super::{KEY, SAYING_THE_KEY};

    let bench = Bench::new("agents-own-key-launch")?;
    bench
        .server
        .catalog(1, &[signing_in("goose", ONE)], &json!([]))?;
    let made = Tree::new("agents-own-key-launch-program")?;
    let program = made.0.join("goose");
    fs::write(&program, SAYING_THE_KEY)?;
    fs::set_permissions(
        &program,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )?;
    bench.configure(&program, &json!({}))?;
    // Asked first, so the catalog is fetched and offers the agent.
    let (unstored, _) = bench.ask(
        sign_in("work", "api-key")?,
        &mut answering(&bench.server, None),
        false,
    )?;
    assert!(
        matches!(&unstored, Err(AgentsError::Unstored { .. })),
        "{}",
        shape(&unstored)
    );

    let unkeyed = said_by_your_own(&bench, &program)?;
    keep_key(&bench.home, "work", "api-key", Typed::new(KEY.to_owned()))?;
    let keyed = said_by_your_own(&bench, &program)?;

    // What the program said, as it is kept, is compared without being printed.
    assert!(
        unkeyed == "k",
        "your own program was given a key before one was kept"
    );
    assert!(
        keyed == "k[withheld]",
        "the kept key was not given to your own program, or was not withheld"
    );
    Ok(())
}

/// The version the user's own goose at `program`, run for `work` in `bench`
/// with the sandbox off, says it is, as what it says is kept.
#[cfg(unix)]
fn said_by_your_own(bench: &Bench, program: &std::path::Path) -> Result<String, Failed> {
    use std::sync::Arc;
    use std::time::SystemTime;

    use crucible_app::backends::{AgentRow, REGISTRY, Sandboxing};
    use crucible_app::catalog_fetch;
    use crucible_app::content_use::{Consent, Routes};
    use crucible_app::launching::{Agent, Launcher};
    use crucible_config::{Driver, ExternalAgent};
    use crucible_external_harnesses::catalog::Trust;
    use crucible_external_harnesses::{Account, Control, Reading};
    use crucible_sandbox::{SandboxPolicy, SandboxService};
    use crucible_sandbox_local::LocalSandbox;
    use crucible_tools::Mode;
    use crucible_types::AgentKey;

    use super::runtime;

    let key = AgentKey::new("goose")?;
    let instance = ExternalAgent::unwritten(
        "work",
        Driver::Catalog,
        &program.to_string_lossy(),
        None,
        Some(&key),
    )
    .ok_or("a catalog instance naming its agent")?;
    let state = catalog_fetch::state_in(bench.home.path());
    let agent = Agent::believing(&instance, &REGISTRY, &state, |_: &str| None)
        .map_err(|why| why.to_string())?;
    assert!(
        matches!(agent.row(), AgentRow::Catalog(row) if row.is_own()),
        "the program was not run as your own"
    );
    let consent = Consent::new(Routes::production());
    consent.give(agent.row().route());
    let runtime = runtime()?;
    let handle = runtime.handle().clone();
    let policy = SandboxPolicy::standard(&bench.tree.workspace()?)?.with_enabled(false);
    let sandbox: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(handle.clone()));
    let launcher = Launcher::new(
        sandbox,
        handle,
        &policy,
        &consent,
        &state,
        bench.home.sessions(),
    );
    let control = Control::new().ok_or("a control")?;
    let trust = Trust::load(&state, SystemTime::now());
    let mut dialect = agent.dialect(Mode::Ask, &trust, &bench.programs(), Sandboxing::default())?;
    let read = runtime.block_on(launcher.account(
        &agent,
        &control,
        dialect.as_mut(),
        Account::Version,
        &|_| {},
    ))?;
    match read {
        Reading::Version(version) => Ok(version.as_str().to_owned()),
        _ => Err("your own program did not say its version".into()),
    }
}
