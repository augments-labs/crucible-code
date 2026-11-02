//! A catalog agent held on the consent a build shares, as [`Services`] makes
//! it: its route is told to that consent by the row a launch or a question is
//! about, its start asks and runs nothing before a yes, and an entry whose
//! route it cannot hold is refused, whatever yes is given.
//!
//! [`Services`]: crucible_app::services::Services

use crucible_external_harnesses::Host;

use super::*;

/// The consent [`Services`] makes, the one every door and turn of a run shares.
///
/// [`Services`]: crucible_app::services::Services
pub(super) fn shipped() -> Result<Consent, Failed> {
    let (consent, stopped) = crucible_app::services::serving(|services| services.consent().clone());
    stopped.map_err(|unfinished| format!("the services did not stop: {unfinished:?}"))?;
    Ok(consent)
}

/// The instance `work` of the catalog agent `key`, run as `program`.
fn instance_of(key: &str, program: &str) -> Result<ExternalAgent, Failed> {
    let key = AgentKey::new(key)?;
    Ok(
        ExternalAgent::unwritten("work", Driver::Catalog, program, None, Some(&key))
            .ok_or("a catalog instance naming its agent")?,
    )
}

/// A registry listing `row` alone, for the conversation that asks after it.
fn listing(row: CatalogRow) -> &'static Registry {
    Box::leak(Box::new(
        Registry::new(&[]).cataloging(Box::leak(Box::new([row]))),
    ))
}

/// What a start of `row`, an instance of its key run as `program`, comes to
/// over `consent`.
fn start(
    (bench, consent): (&Bench, &Consent),
    row: CatalogRow,
    (key, program): (&str, &str),
    runtime: &tokio::runtime::Runtime,
) -> Result<Result<Host, Unlaunched>, Failed> {
    let agent = Agent::configured(&instance_of(key, program)?, listing(row), |_| None)
        .ok_or("its catalog row")?;
    let handle = runtime.handle().clone();
    let policy = SandboxPolicy::standard(&bench.tree.workspace()?)?.with_enabled(false);
    let sandbox: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(handle.clone()));
    let state = catalog_fetch::state_in(bench.home.path());
    let launcher = Launcher::new(
        sandbox,
        handle,
        &policy,
        consent,
        &state,
        bench.home.sessions(),
    );
    let control = Control::new().ok_or("a control")?;
    let mut dialect = agent.row().dialect(Mode::Ask, None, None)?;
    Ok(runtime.block_on(launcher.launch(&agent, &control, dialect.as_mut(), None)))
}

/// A catalog agent's start on the build's own consent asks its route's question
/// and starts nothing, and starts once that route has its yes.
#[test]
fn a_catalog_agents_start_asks_its_route_and_runs_only_after_the_yes() -> Result<(), Failed> {
    let runtime = tokio::runtime::Runtime::new()?;
    let bench = Bench::new("agents-shipped-start")?;
    let release = bench.server.peer_release(
        "1.0.0",
        &peer(&Peer::of([acp::opening(&acp::offering(
            "goose",
            "1.0.0",
            &json!([]),
        ))]))?,
    );
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    let root = bench.programs().join("1.0.0");
    bench.configure(&root.join(command()), &json!({}))?;
    let (installed, _) = bench.ask(
        install("1.0.0")?,
        &mut answering(&bench.server, Some("Install")),
        false,
    )?;
    if !matches!(installed, Ok(Outcome::Moved { .. })) {
        return Err(format!("installed: {}", shape(&installed)).into());
    }
    let program = bench.executable()?;
    let consent = shipped()?;

    let asked = start(
        (&bench, &consent),
        row_of(&release)?,
        ("goose", &program),
        &runtime,
    )?;

    let Err(Unlaunched::Asks(warned)) = &asked else {
        return Err(format!("not asked: {:?}", asked.map(drop)).into());
    };
    assert_eq!(warned.route, "agent:goose");
    assert_eq!(fake_peer::played(&root), 0, "started before the yes");

    consent.give("agent:goose");
    // Kept while it is looked for: a run dropped is a program stopped.
    let answered = start(
        (&bench, &consent),
        row_of(&release)?,
        ("goose", &program),
        &runtime,
    )?;
    assert!(answered.is_ok(), "{:?}", answered.map(drop));
    assert!(started(&root, 1), "not started after the yes");
    Ok(())
}

/// A conversation on a catalog agent names the route of the agent's key, and is
/// asked about it on the build's own consent before anything is launched.
#[cfg(feature = "proof")]
#[test]
fn a_catalog_conversation_is_asked_about_its_agents_route_before_anything_launches()
-> Result<(), Failed> {
    let runtime = tokio::runtime::Runtime::new()?;
    let bench = Bench::new("agents-shipped-conversation")?;
    let tree = Tree::new("agents-shipped-conversation-tree")?;
    let peer_at = bench.tree.0.join("peer");
    let program = fake_peer::install(&peer_at, "goose", &Peer::of([]))?;
    let registry = listing(row_of(&stating(goose("1.0.0", ONE)))?);
    let consent = shipped()?;
    let policy = SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(false);
    let state = catalog_fetch::state_in(bench.home.path());
    let sessions = tree.sessions();
    let steps = Steps::new(
        Launcher::new(
            Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone())),
            runtime.handle().clone(),
            &policy,
            &consent,
            &state,
            &sessions,
        ),
        Arc::new(SandboxEnablement::new(false, false)),
        registry,
        None,
    );
    let mut conversation = crucible_app::proof::agents(
        crate::conversation(&tree, crate::Script::new(Vec::new()), false)?,
        registry,
    )
    .consenting(consent.clone())
    .stepping(steps);

    let moved = runtime.block_on(conversation.onto(
        &sessions,
        &tree.workspace()?,
        None,
        &instance_of("goose", &program.to_string_lossy())?,
    ));
    assert!(moved.is_ok(), "{:?}", moved.err());

    assert_eq!(conversation.launching().as_deref(), Some("agent:goose"));
    let asked = conversation.unlaunched().ok_or("nothing was asked")?;
    assert_eq!(asked.route, "agent:goose");
    assert_eq!(fake_peer::played(&peer_at), 0, "started before the yes");
    Ok(())
}

/// An entry with no statement is refused whatever yes its route has; a route
/// past the bound, and one restating a route the build warns about, are refused
/// the same, and the yes given to them lets nothing start.
#[test]
fn a_route_the_consent_cannot_hold_is_refused_whatever_yes_it_has() -> Result<(), Failed> {
    let runtime = tokio::runtime::Runtime::new()?;
    let bench = Bench::new("agents-shipped-refused")?;
    let peer_at = bench.tree.0.join("peer");
    let program = fake_peer::install(&peer_at, "goose", &Peer::of([]))?
        .to_string_lossy()
        .into_owned();
    let keyed = |key: &str, stated: bool| -> Result<CatalogRow, Failed> {
        let mut entry = goose("1.0.0", ONE);
        set(&mut entry, "/key", json!(key));
        set(&mut entry, "/registry_id", json!(key));
        set(&mut entry, "/admits/name", json!(key));
        row_of(&if stated { stating(entry) } else { entry })
    };
    let consent = shipped()?;

    consent.give("agent:goose");
    let unstated = start(
        (&bench, &consent),
        keyed("goose", false)?,
        ("goose", &program),
        &runtime,
    )?
    .map(drop);
    assert!(
        matches!(unstated, Err(Unlaunched::Unstated { .. })),
        "{unstated:?}"
    );

    consent.give("agent:claude");
    let restated = start(
        (&bench, &consent),
        keyed("claude", true)?,
        ("claude", &program),
        &runtime,
    )?
    .map(drop);
    assert!(
        matches!(&restated, Err(Unlaunched::Unknown(route)) if route == "agent:claude"),
        "{restated:?}"
    );

    for at in 0..CATALOGED {
        let key = format!("goose-{at}");
        let held = start(
            (&bench, &consent),
            keyed(&key, true)?,
            (&key, &program),
            &runtime,
        )?
        .map(drop);
        assert!(matches!(held, Err(Unlaunched::Asks(_))), "{at}: {held:?}");
    }
    consent.give("agent:past");
    let past = start(
        (&bench, &consent),
        keyed("past", true)?,
        ("past", &program),
        &runtime,
    )?
    .map(drop);
    assert!(
        matches!(&past, Err(Unlaunched::Unknown(route)) if route == "agent:past"),
        "{past:?}"
    );
    assert_eq!(fake_peer::played(&peer_at), 0, "something was started");
    Ok(())
}
