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

/// `entry` signed for the agent keyed `key`.
pub(super) fn rekeyed(mut entry: Value, key: &str) -> Value {
    set(&mut entry, "/key", json!(key));
    set(&mut entry, "/registry_id", json!(key));
    set(&mut entry, "/admits/name", json!(key));
    entry
}

/// The consent [`shipped`] makes, having `route`'s yes.
pub(super) fn shipped_given(route: &str) -> Result<Consent, Failed> {
    let consent = shipped()?;
    consent.give(route);
    Ok(consent)
}

/// `bench`'s door asked to install version 1.0.0 of the agent `key`, the sandbox
/// on, over `consent`.
pub(super) fn installed_over(
    bench: &Bench,
    key: &str,
    consent: &Consent,
    front: &mut Answering<'_>,
    runtime: &tokio::runtime::Runtime,
) -> Result<Result<Outcome, AgentsError>, Failed> {
    let sandbox: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));
    let command = Command::InstallAgent {
        agent: name(key)?,
        version: name("1.0.0")?,
    };
    bench.ask_consenting((command, front), (true, consent), (runtime, sandbox, None))
}

/// A release of `key` that answers initializing as that agent, and is smoke
/// tested where it may be.
fn answering_as(bench: &Bench, key: &str, stated: bool) -> Result<Value, Failed> {
    let played = peer(&Peer::once(Played::new().hear(1).say(acp::initialized(
        key,
        "1.0.0",
        "agent-login",
    ))))?;
    let mut release = rekeyed(bench.server.peer_release("1.0.0", &played), key);
    if !stated && let Some(object) = release.as_object_mut() {
        object.remove("content_use");
    }
    Ok(release)
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
        let entry = rekeyed(goose("1.0.0", ONE), key);
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

/// A signed entry keyed `claude` backs no built-in, so the yes to Claude Code's
/// route does not stand for it: its release is installed without its program
/// being started, stated or not, and the question and the outcome say why. The
/// entry that backs Antigravity is smoke tested under that route's yes.
#[test]
fn a_smoke_of_an_entry_keyed_for_a_built_in_it_does_not_back_is_refused_under_that_yes()
-> Result<(), Failed> {
    let runtime = tokio::runtime::Runtime::new()?;
    if !enforcing(&runtime)? {
        return Ok(());
    }
    for (case, stated) in [("stated", true), ("unstated", false)] {
        let bench = Bench::new(&format!("agents-shipped-smoke-claude-{case}"))?;
        let release = answering_as(&bench, "claude", stated)?;
        bench
            .server
            .catalog(1, std::slice::from_ref(&release), &json!([]))?;
        let consent = shipped_given("agent:claude")?;
        let mut front = answering(&bench.server, Some("Install"));

        let installed = installed_over(&bench, "claude", &consent, &mut front, &runtime)?;

        let Ok(Outcome::Moved {
            unsmoked: Some(why),
            ..
        }) = &installed
        else {
            return Err(format!("{case}: smoked or refused: {}", shape(&installed)).into());
        };
        assert!(
            why.contains("is not its entry's own"),
            "{case}: refused for another reason"
        );
        assert!(
            facts(&front)?
                .iter()
                .any(|fact| fact.starts_with("Not smoke-tested")
                    && fact.contains("is not its entry's own")),
            "{case}: {:?}",
            facts(&front)
        );
    }

    let bench = Bench::new("agents-shipped-smoke-antigravity")?;
    let release = answering_as(&bench, "antigravity", true)?;
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    let consent = shipped_given("agent:antigravity")?;
    let mut front = answering(&bench.server, Some("Install"));

    let installed = installed_over(&bench, "antigravity", &consent, &mut front, &runtime)?;

    assert!(
        matches!(&installed, Ok(Outcome::Moved { unsmoked: None, .. })),
        "{}",
        shape(&installed)
    );
    Ok(())
}

/// A key past the ceiling is learned by nothing, and keeps the refusal of a route
/// this build does not ask about, whatever yes it has.
#[test]
fn a_smoke_of_a_key_past_the_ceiling_keeps_its_refusal() -> Result<(), Failed> {
    let runtime = tokio::runtime::Runtime::new()?;
    if !enforcing(&runtime)? {
        return Ok(());
    }
    let bench = Bench::new("agents-shipped-smoke-past")?;
    let peer_at = bench.tree.0.join("peer");
    let program = fake_peer::install(&peer_at, "goose", &Peer::of([]))?
        .to_string_lossy()
        .into_owned();
    let consent = shipped_given("agent:past")?;
    for at in 0..CATALOGED {
        let key = format!("goose-{at}");
        let row = row_of(&stating(rekeyed(goose("1.0.0", ONE), &key)))?;
        let _asked = start((&bench, &consent), row, (&key, &program), &runtime)?;
    }
    let release = answering_as(&bench, "past", true)?;
    bench
        .server
        .catalog(1, std::slice::from_ref(&release), &json!([]))?;
    let mut front = answering(&bench.server, Some("Install"));

    let installed = installed_over(&bench, "past", &consent, &mut front, &runtime)?;

    let Ok(Outcome::Moved {
        unsmoked: Some(why),
        ..
    }) = &installed
    else {
        return Err(format!("smoked or refused: {}", shape(&installed)).into());
    };
    assert!(
        why.contains("is not one this build asks about"),
        "refused for another reason"
    );
    Ok(())
}

/// A conversation of this build's own registry, on `consent` and over `bench`'s
/// home, that has not been asked to move.
fn on_the_shipped_registry(
    bench: &Bench,
    tree: &Tree,
    consent: &Consent,
    runtime: &tokio::runtime::Runtime,
) -> Result<crucible_app::Conversation, Failed> {
    let policy = SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(false);
    let state = catalog_fetch::state_in(bench.home.path());
    let sessions = tree.sessions();
    let steps = Steps::new(
        Launcher::new(
            Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone())),
            runtime.handle().clone(),
            &policy,
            consent,
            &state,
            &sessions,
        ),
        Arc::new(SandboxEnablement::new(false, false)),
        &crucible_app::backends::REGISTRY,
        None,
    );
    Ok(
        crate::conversation(tree, crate::Script::new(Vec::new()), false)?
            .consenting(consent.clone())
            .stepping(steps),
    )
}

/// `bench` with goose 1.0.0 installed from a catalog that offers it, its one
/// instance `work` run from there; the instance, the release's directory, and
/// the release as published.
fn installed_from_the_catalog(bench: &Bench) -> Result<(ExternalAgent, PathBuf, Value), Failed> {
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
    let instance = instance_of("goose", &bench.executable()?)?;
    Ok((instance, root, release))
}

/// What moving a conversation of the shipped registry onto `instance` says,
/// where it is refused, with nothing started and nothing asked.
fn refused_onto(bench: &Bench, instance: &ExternalAgent, root: &Path) -> Result<String, Failed> {
    let runtime = tokio::runtime::Runtime::new()?;
    let tree = Tree::new(&format!(
        "agents-shipped-refused-{}",
        REFUSALS.fetch_add(1, Ordering::Relaxed)
    ))?;
    let consent = shipped()?;
    let mut conversation = on_the_shipped_registry(bench, &tree, &consent, &runtime)?;
    let moved =
        runtime.block_on(conversation.onto(&tree.sessions(), &tree.workspace()?, None, instance));
    assert_eq!(fake_peer::played(root), 0, "started though it was refused");
    assert!(
        conversation.launching().is_none(),
        "moved though it was refused"
    );
    moved
        .map(drop)
        .err()
        .ok_or_else(|| "it was not refused".into())
}

static REFUSALS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The catalog republished as `entries`, revoking `revoked`, and fetched as
/// the door fetches it.
fn republished(bench: &Bench, entries: &[Value], revoked: &Value) -> Result<(), Failed> {
    bench.server.catalog(2, entries, revoked)?;
    let (listed, _) = bench.ask(
        Command::AgentCatalog,
        &mut answering(&bench.server, None),
        false,
    )?;
    if listed.is_err() {
        return Err(format!("the catalog was not fetched: {}", shape(&listed)).into());
    }
    Ok(())
}

/// A catalog agent installed by a shipped build is one a conversation of that
/// build moves onto: its row is built from the catalog believed, which teaches
/// its route, and it is asked about before anything is started.
#[test]
fn a_conversation_of_the_shipped_registry_moves_onto_an_installed_catalog_agent_and_asks_first()
-> Result<(), Failed> {
    let runtime = tokio::runtime::Runtime::new()?;
    let bench = Bench::new("agents-shipped-onto")?;
    let tree = Tree::new("agents-shipped-onto-tree")?;
    let (instance, root, _) = installed_from_the_catalog(&bench)?;
    let consent = shipped()?;
    let mut conversation = on_the_shipped_registry(&bench, &tree, &consent, &runtime)?;

    let moved =
        runtime.block_on(conversation.onto(&tree.sessions(), &tree.workspace()?, None, &instance));

    assert!(moved.is_ok(), "{:?}", moved.err());
    assert_eq!(conversation.launching().as_deref(), Some("agent:goose"));
    let asked = conversation.unlaunched().ok_or("nothing was asked")?;
    assert_eq!(asked.route, "agent:goose");
    assert_eq!(fake_peer::played(&root), 0, "started before the yes");
    Ok(())
}

/// A version the catalog revoked is refused, saying what to run.
#[test]
fn a_revoked_version_is_refused_naming_the_command_that_moves_it() -> Result<(), Failed> {
    let bench = Bench::new("agents-shipped-revoked")?;
    let (instance, root, _) = installed_from_the_catalog(&bench)?;
    republished(
        &bench,
        &[goose("2.0.0", ONE)],
        &json!([{"key": "goose", "version": "1.0.0", "reason": "It wrote outside its directory."}]),
    )?;

    let said = refused_onto(&bench, &instance, &root)?;

    assert!(said.contains("revoked"), "the refusal says why");
    assert!(
        said.contains("crucible agents update goose"),
        "the refusal names the command"
    );
    assert!(!said.contains("no launch"), "the refusal is the old one");
    Ok(())
}

/// A version the catalog no longer offers is refused, saying what to run.
#[test]
fn an_unoffered_version_is_refused_naming_the_command_that_moves_it() -> Result<(), Failed> {
    let bench = Bench::new("agents-shipped-unoffered")?;
    let (instance, root, _) = installed_from_the_catalog(&bench)?;
    republished(&bench, &[goose("2.0.0", ONE)], &json!([]))?;

    let said = refused_onto(&bench, &instance, &root)?;

    assert!(said.contains("no longer offers"), "the refusal says why");
    assert!(
        said.contains("crucible agents update goose"),
        "the refusal names the command"
    );
    Ok(())
}

/// With no catalog believed to offer the agent nothing is fetched to find out:
/// the refusal says what fetches one.
#[test]
fn an_agent_no_believed_catalog_offers_is_refused_with_no_fetch_naming_the_command()
-> Result<(), Failed> {
    let bench = Bench::new("agents-shipped-no-catalog")?;
    let (instance, root, _) = installed_from_the_catalog(&bench)?;
    fs::remove_dir_all(catalog_fetch::state_in(bench.home.path()))?;
    let before = bench.server.asked().len();

    let said = refused_onto(&bench, &instance, &root)?;

    assert!(
        said.contains("crucible agents search"),
        "the refusal names the command"
    );
    assert_eq!(bench.server.asked().len(), before, "a start fetched");
    Ok(())
}

/// A program the configuration alone names gets no row, though its key is one
/// the catalog offers: an executable outside the store, and one in the version
/// that is not the program its entry's `cmd` names.
#[test]
fn a_program_that_is_not_the_installed_entrys_gets_no_row_however_it_is_configured()
-> Result<(), Failed> {
    let bench = Bench::new("agents-shipped-own-program")?;
    let (_, root, _) = installed_from_the_catalog(&bench)?;
    let elsewhere = bench.tree.0.join("own");
    let own = fake_peer::install(&elsewhere, "goose", &Peer::of([]))?;

    let outside = refused_onto(
        &bench,
        &instance_of("goose", &own.to_string_lossy())?,
        &elsewhere,
    )?;
    assert!(
        outside.contains("crucible agents install goose"),
        "the refusal names the command"
    );

    let beside = root.join("script");
    let other = refused_onto(
        &bench,
        &instance_of("goose", &beside.to_string_lossy())?,
        &root,
    )?;
    assert!(
        other.contains("crucible agents install goose"),
        "the refusal names the command"
    );
    Ok(())
}

/// A key spelling a built-in driver's word or the native runner's name is no
/// catalog row, whatever a catalog signs: it would answer under a name other
/// routes, sessions and credentials are kept by.
#[test]
fn a_key_spelling_a_reserved_word_gets_no_row() -> Result<(), Failed> {
    let bench = Bench::new("agents-shipped-reserved")?;
    let (instance, root, _) = installed_from_the_catalog(&bench)?;
    for word in ["claude", "antigravity", "native"] {
        let said = refused_onto(&bench, &instance_of(word, instance.executable())?, &root)?;
        assert!(said.contains("is kept for"), "the refusal says why");
    }
    Ok(())
}

/// A route the consent cannot hold reaches a refusal before anything starts,
/// and not a conversation that would go on unasked.
#[test]
fn a_route_past_the_ceiling_refuses_the_move_before_anything_starts() -> Result<(), Failed> {
    let runtime = tokio::runtime::Runtime::new()?;
    let bench = Bench::new("agents-shipped-ceiling")?;
    let tree = Tree::new("agents-shipped-ceiling-tree")?;
    let (instance, root, _) = installed_from_the_catalog(&bench)?;
    let consent = shipped()?;
    let policy = SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(false);
    let state = catalog_fetch::state_in(bench.home.path());
    let sessions = tree.sessions();
    let full = Steps::new(
        Launcher::new(
            Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone())),
            runtime.handle().clone(),
            &policy,
            &consent,
            &state,
            &sessions,
        ),
        Arc::new(SandboxEnablement::new(false, false)),
        &crucible_app::backends::REGISTRY,
        None,
    );
    for at in 0..CATALOGED {
        let key = format!("filler{at}");
        let row = row_of(&rekeyed(stating(goose("1.0.0", ONE)), &key))?;
        // Asking teaches, as a start's first question does.
        let _ = full.refusal(&AgentRow::Catalog(row), &instance_of(&key, "absent")?);
    }
    let mut conversation = on_the_shipped_registry(&bench, &tree, &consent, &runtime)?;

    let moved =
        runtime.block_on(conversation.onto(&tree.sessions(), &tree.workspace()?, None, &instance));

    let said = moved.map(drop).err().ok_or("it was not refused")?;
    assert!(said.contains("agent:goose"), "the refusal names the route");
    assert!(conversation.unlaunched().is_none(), "asked about it");
    assert_eq!(fake_peer::played(&root), 0, "started though it was refused");
    Ok(())
}

/// A run restarted over a session on a catalog agent reads the store and the
/// local trust state alone: nothing is fetched, the route is the session's, and
/// the question comes before anything starts.
#[test]
fn a_session_on_a_catalog_agent_resumes_with_no_fetch_and_asks_first() -> Result<(), Failed> {
    let runtime = tokio::runtime::Runtime::new()?;
    let bench = Bench::new("agents-shipped-resume")?;
    let tree = Tree::new("agents-shipped-resume-tree")?;
    let (_, root, _) = installed_from_the_catalog(&bench)?;
    let (sessions, workspace) = (tree.sessions(), tree.workspace()?);
    let entries = crucible_config::Settings::read_home(&bench.home)?.external_agents();
    let instance = entries.first().ok_or("the instance in the user file")?;
    let consent = shipped()?;
    let mut first = on_the_shipped_registry(&bench, &tree, &consent, &runtime)?;
    let moved = runtime.block_on(first.onto(&sessions, &workspace, None, instance));
    assert!(moved.is_ok(), "{:?}", moved.err());
    let id = first
        .session()
        .id()
        .cloned()
        .ok_or("a session on the agent")?;
    let before = bench.server.asked().len();
    // The run ends, which lets go of its log.
    drop(first);

    let consent = shipped()?;
    let mut restarted = on_the_shipped_registry(&bench, &tree, &consent, &runtime)?;
    let resumed = runtime.block_on(restarted.resume(&sessions, &workspace, &id, &entries));

    assert!(resumed.is_ok(), "{:?}", resumed.err());
    assert_eq!(bench.server.asked().len(), before, "a restart fetched");
    assert_eq!(restarted.launching().as_deref(), Some("agent:goose"));
    let asked = restarted.unlaunched().ok_or("nothing was asked")?;
    assert_eq!(asked.route, "agent:goose");
    assert_eq!(fake_peer::played(&root), 0, "started before the yes");
    Ok(())
}
