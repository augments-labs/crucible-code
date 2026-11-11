//! The Antigravity driver as the launch holds it: the release the catalog
//! offers, the user's own program found as `agy`, and the profile each needs.

use super::*;

/// The built-in Antigravity row alone.
static ANTIGRAVITY_ONLY: [Row; 1] = [ANTIGRAVITY];

/// The registry holding it.
pub(super) static ANTIGRAVITY_REGISTRY: Registry = Registry::new(&ANTIGRAVITY_ONLY);

/// The catalog's entry for `version` of Antigravity.
pub(crate) fn antigravity_release(version: &str) -> serde_json::Value {
    release_of(
        "antigravity",
        version,
        (
            &format!("https://dl.google.com/agy-extensions/releases/linux/agy-{version}.zip"),
            &"a".repeat(64),
            1000,
        ),
        [("agy_acp_server.par", 700), ("localharness_external", 300)],
        &[],
    )
}

/// What the fake peer says to Antigravity's status check once it is
/// initialized as `version`: it initializes, then opens a session.
fn antigravity_answering(version: &str) -> Reply {
    Reply::new()
        .hear(1)
        .say(acp::initialized(
            "antigravity-acp",
            version,
            "oauth-personal",
        ))
        .hear(1)
        .say(acp::opened(1))
}

/// The status of the Antigravity the peer playing `script` is, as the steps of
/// the registry holding its row read it, with the trust state `sample` holds.
async fn antigravity_status(sample: &Sample, script: &Script) -> (Reading, usize) {
    let here = sample.root().join("peer");
    let program = fake_peer::install(&here, "antigravity", script).expect("the fake peer");
    let consent = Consent::new(Routes::production());
    consent.give("agent:antigravity");
    let policy = conversation(sample);
    let state = sample.state();
    let sessions = sample.sessions();
    let steps = Steps::new(
        Launcher::new(
            local(),
            Handle::current(),
            &policy,
            &consent,
            &state,
            &sessions,
        ),
        Arc::new(SandboxEnablement::new(false, false)),
        &ANTIGRAVITY_REGISTRY,
        None,
    );
    let entry = ExternalAgent::unwritten(
        "space",
        Driver::Antigravity,
        &program.to_string_lossy(),
        None,
        None,
    )
    .expect("an instance of a built-in driver");

    let read = steps.account(&entry, Account::Status).await;

    (read, fake_peer::played(&here))
}

/// The status of the release of Antigravity the peer playing `script` is,
/// kept in the install store in the directory of `version` and given a
/// profile directory, as the steps of the registry holding its row read it,
/// with the trust state `sample` holds.
async fn stored_antigravity_status(
    sample: &Sample,
    version: &str,
    script: &Script,
) -> (Reading, usize) {
    let here = sample.home().join("programs/antigravity").join(version);
    let program = fake_peer::install(&here, "agy_acp_server.par", script).expect("the fake peer");
    let profile = sample.home().join("agents/space");
    let steps = antigravity_steps(sample, None);

    let read = antigravity_read(&steps, &program.to_string_lossy(), Some(&profile)).await;

    (read, fake_peer::played(&here))
}

/// The version a release in the install store answers as is admitted when
/// the trust state lists it, and is sent nothing more when it does not: a
/// release the catalog reviewed since this build was made is run without a
/// release of crucible.
#[tokio::test(flavor = "multi_thread")]
async fn antigravity_is_admitted_as_a_version_the_catalog_lists_and_no_other() {
    let unlisted = Sample::new("launch-antigravity-unlisted");
    let (refused, heard) = stored_antigravity_status(
        &unlisted,
        "1.3.0",
        &Script::of([antigravity_answering("1.4.1").hear_out()]),
    )
    .await;
    let listed = Sample::new("launch-antigravity-listed");
    trusting(&listed, 2, &[antigravity_release("1.4.1")]);
    let (admitted, _) = stored_antigravity_status(
        &listed,
        "1.4.1",
        &Script::of([antigravity_answering("1.4.1")]),
    )
    .await;
    let snapshot = Sample::new("launch-antigravity-snapshot");
    let version = snapshot_version().unwrap_or("1.3.0");
    let (older, _) = stored_antigravity_status(
        &snapshot,
        version,
        &Script::of([antigravity_answering(version)]),
    )
    .await;

    let Reading::Unreadable(failure) = refused else {
        panic!("{refused:?}");
    };
    assert!(failure.reason().as_str().contains("1.4.1"), "{failure:?}");
    assert_eq!(heard, 1, "nothing more was sent");
    assert!(matches!(admitted, Reading::Signed(_)), "{admitted:?}");
    if snapshot_version().is_some() {
        assert!(matches!(older, Reading::Signed(_)), "{older:?}");
    }
}

/// What a missing Antigravity is told to do is what the trust state offers:
/// the command that installs the release, and where the state cannot be used,
/// that it is to be repaired.
#[test]
fn a_missing_antigravity_is_told_what_the_trust_state_offers() {
    let sample = Sample::new("launch-antigravity-missing");
    let consent = Consent::new(Routes::production());
    let route = Route::External {
        serves: Serves::Provider("google"),
        instance: "space".into(),
        driver: Driver::Antigravity,
        executable: "no-such-antigravity-program".into(),
        profile: None,
        model: None,
        effort: None,
        location: None,
        enabled: true,
    };
    let registry = &ANTIGRAVITY_REGISTRY;
    let missing = Agent::routed(&route, registry, |_| {
        Some(OsString::from(sample.root().join("empty")))
    })
    .expect("a row");
    let said = |trust: &Trust| match missing.program(&consent, trust) {
        Err(Unlaunched::NotInstalled([_, next])) => next,
        other => panic!("{other:?}"),
    };

    let offered = said(&trusting(&sample, 2, &[antigravity_release("1.4.1")]));
    let file = trust_of(&sample).file();
    std::fs::create_dir_all(file.parent().expect("a directory")).expect("a directory");
    std::fs::write(&file, "not a trust state").expect("the file");
    let unusable = said(&trust_of(&sample));

    assert!(
        offered.contains("crucible agents install antigravity"),
        "{offered}"
    );
    assert!(
        unusable.contains(&file.display().to_string()) && unusable.contains("Repair or remove"),
        "{unusable}"
    );
}

/// A run is admitted as the releases on offer when it is launched, not when
/// its agent was selected: a catalog taken in since is the one it is held to.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_is_admitted_as_the_releases_on_offer_when_it_is_launched() {
    let sample = Sample::new("launch-antigravity-fresh");
    let consent = Consent::new(Routes::production());
    let route = Route::External {
        serves: Serves::Provider("google"),
        instance: "space".into(),
        driver: Driver::Antigravity,
        executable: "antigravity".into(),
        profile: None,
        model: None,
        effort: None,
        location: None,
        enabled: true,
    };
    let agent = Agent::routed(&route, &ANTIGRAVITY_REGISTRY, |_| None).expect("a row");
    let policy = conversation(&sample);
    let state = sample.state();
    let sessions = sample.sessions();
    let runs = Runs::new(
        Launcher::new(
            local(),
            Handle::current(),
            &policy,
            &consent,
            &state,
            &sessions,
        ),
        Arc::new(SandboxEnablement::new(false, false)),
        agent,
        "space",
        &Controls::default(),
    );
    let answers = |version: &str| {
        let mut dialect = runs.dialect(Mode::Ask).expect("asking");
        let _ = dialect.request(Shape {
            workspace: Some(Path::new("/work")),
            ..Shape::default()
        });
        let _ = dialect
            .open(&TurnRequest::new(TurnId::FIRST, "hello"))
            .expect("a first request");
        let frame = acp::initialized("antigravity-acp", version, "oauth-personal");
        dialect.heard(&frame).expect("an answer")
    };

    let before = answers("1.4.1");
    trusting(&sample, 2, &[antigravity_release("1.4.1")]);
    let after = answers("1.4.1");

    assert!(matches!(before.as_slice(), [Act::Fail(_)]), "{before:?}");
    assert!(matches!(after.as_slice(), [Act::Send(_)]), "{after:?}");
}

/// A start of an Antigravity whose trust state cannot be used says so, naming
/// the file to repair or remove, rather than that no release is offered.
#[tokio::test(flavor = "multi_thread")]
async fn a_found_antigravity_with_an_unusable_trust_state_is_refused_naming_the_file() {
    let sample = Sample::new("launch-antigravity-untrusted");
    let file = trust_of(&sample).file();
    std::fs::create_dir_all(file.parent().expect("a directory")).expect("a directory");
    std::fs::write(&file, "not a trust state").expect("the file");

    let (read, heard) =
        antigravity_status(&sample, &Script::of([antigravity_answering("1.3.0")])).await;

    let Reading::Unreadable(failure) = read else {
        panic!("{read:?}");
    };
    let said = failure.reason().as_str();
    assert!(said.contains(&file.display().to_string()), "{said}");
    assert!(said.contains("Repair or remove"), "{said}");
    assert_eq!(heard, 0, "nothing was started");
}

/// The wire a run is made with follows the release its program is: the
/// version directory it is kept in, not the newest one on offer.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_is_started_with_the_arguments_of_the_release_its_program_is() {
    let sample = Sample::new("launch-antigravity-directory");
    let release = |version: &str, args: &[&str]| {
        release_of(
            "antigravity",
            version,
            (
                &format!("https://dl.google.com/agy-extensions/releases/linux/agy-{version}.zip"),
                &"a".repeat(64),
                1000,
            ),
            [("agy_acp_server.par", 700), ("localharness_external", 300)],
            args,
        )
    };
    trusting(
        &sample,
        2,
        &[release("1.4.1", &["--new"]), release("1.3.0", &["--old"])],
    );
    let consent = Consent::new(Routes::production());
    let policy = conversation(&sample);
    let state = sample.state();
    let sessions = sample.sessions();
    let arguments = |version: &str| {
        let program = sample
            .root()
            .join("programs/antigravity")
            .join(version)
            .join("agy_acp_server.par");
        std::fs::create_dir_all(program.parent().expect("a directory")).expect("a directory");
        std::fs::write(&program, "#!/bin/sh\n").expect("the program");
        #[cfg(unix)]
        std::fs::set_permissions(
            &program,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .expect("executable");
        let route = Route::External {
            serves: Serves::Provider("google"),
            instance: "space".into(),
            driver: Driver::Antigravity,
            executable: program.to_string_lossy().into(),
            profile: None,
            model: None,
            effort: None,
            location: None,
            enabled: true,
        };
        let agent = Agent::routed(&route, &ANTIGRAVITY_REGISTRY, |_| None).expect("a row");
        let runs = Runs::new(
            Launcher::new(
                local(),
                Handle::current(),
                &policy,
                &consent,
                &state,
                &sessions,
            ),
            Arc::new(SandboxEnablement::new(false, false)),
            agent,
            "space",
            &Controls::default(),
        );
        runs.dialect(Mode::Ask)
            .expect("asking")
            .request(Shape::default())
            .arguments
    };

    assert_eq!(arguments("1.4.1"), ["--new"]);
    assert_eq!(arguments("1.3.0"), ["--old"]);
}

/// The steps of the registry holding Antigravity's row alone, with a yes to
/// Antigravity, each run unconfined and looking for programs on `path`.
pub(crate) fn antigravity_steps(sample: &Sample, path: Option<OsString>) -> Steps {
    let consent = Consent::new(Routes::production());
    consent.give("agent:antigravity");
    let policy = conversation(sample);
    let state = sample.state();
    let sessions = sample.sessions();
    Steps::new(
        Launcher::new(
            local(),
            Handle::current(),
            &policy,
            &consent,
            &state,
            &sessions,
        ),
        Arc::new(SandboxEnablement::new(false, false)),
        &ANTIGRAVITY_REGISTRY,
        path,
    )
}

/// The instance `space` of Antigravity, its program `executable` and its
/// profile directory `profile`.
fn antigravity_entry(executable: &str, profile: Option<&Path>) -> ExternalAgent {
    let profile = profile.map(|profile| profile.to_string_lossy().into_owned());
    ExternalAgent::unwritten(
        "space",
        Driver::Antigravity,
        executable,
        profile.as_deref(),
        None,
    )
    .expect("an instance of a built-in driver")
}

/// What the status step of the instance `space` of Antigravity, its program
/// `executable` and its profile directory `profile`, reads through `steps`.
async fn antigravity_read(steps: &Steps, executable: &str, profile: Option<&Path>) -> Reading {
    steps
        .account(&antigravity_entry(executable, profile), Account::Status)
        .await
}

/// What the user's own Antigravity says to the status check once it is
/// initialized: it answers as itself, then opens a session.
fn answering_as(name: &str, version: &str) -> Reply {
    Reply::new()
        .hear(1)
        .say(acp::initialized(name, version, "oauth-personal"))
        .hear(1)
        .say(acp::opened(1))
}

/// The user's own program is found as `agy` on `PATH` for an instance that
/// names it `antigravity`, and is started as `agy acp`.
#[tokio::test(flavor = "multi_thread")]
async fn antigravity_is_found_as_agy_and_started_with_acp() {
    let sample = Sample::new("launch-antigravity-agy");
    let here = sample.root().join("bin");
    fake_peer::install(
        &here,
        "agy",
        &Script::of([answering_as("Antigravity ACP Server", "1.5.5")]),
    )
    .expect("the fake peer");
    let steps = antigravity_steps(&sample, Some(OsString::from(&here)));

    let read = antigravity_read(&steps, "antigravity", None).await;

    assert!(matches!(read, Reading::Signed(_)), "{read:?}");
    assert_eq!(fake_peer::argued(&here), [["acp"]]);
}

/// The user's own `agy` uses no release, so a trust state that cannot be used
/// does not hold it back: it is started as `agy acp` and admitted by what it
/// answers.
#[tokio::test(flavor = "multi_thread")]
async fn an_own_agy_is_admitted_while_the_trust_state_cannot_be_used() {
    let sample = Sample::new("launch-own-agy-untrusted");
    let file = trust_of(&sample).file();
    std::fs::create_dir_all(file.parent().expect("a directory")).expect("a directory");
    std::fs::write(&file, "not a trust state").expect("the file");
    let here = sample.root().join("bin");
    let program = fake_peer::install(
        &here,
        "agy",
        &Script::of([answering_as("Antigravity ACP Server", "1.5.5")]),
    )
    .expect("the fake peer");
    let steps = antigravity_steps(&sample, None);

    let read = antigravity_read(&steps, &program.to_string_lossy(), None).await;

    assert!(matches!(read, Reading::Signed(_)), "{read:?}");
    assert_eq!(fake_peer::argued(&here), [["acp"]]);
}

/// The status the user's own Antigravity, outside the install store, reads
/// as when it answers initializing with `reply`, and how many times it was
/// started.
async fn own_antigravity_status(name: &str, reply: Reply) -> (Reading, usize) {
    let sample = Sample::new(name);
    let here = sample.root().join("bin");
    let program = fake_peer::install(&here, "agy", &Script::of([reply])).expect("the fake peer");
    let steps = antigravity_steps(&sample, None);

    let read = antigravity_read(&steps, &program.to_string_lossy(), None).await;

    (read, fake_peer::played(&here))
}

/// The user's own Antigravity is admitted by what it answers, whatever
/// release it names itself as, and refused where it lacks the usual
/// sign-in, signing out, or the protocol version this client speaks.
#[tokio::test(flavor = "multi_thread")]
async fn an_antigravity_outside_the_store_is_admitted_by_what_it_answers() {
    let capabilities = serde_json::json!({
        "loadSession": true,
        "sessionCapabilities": {"resume": {}},
        "auth": {"logout": {}},
    });
    let answered = |protocol: u64, capabilities: &serde_json::Value, method: &str| {
        Reply::new()
            .hear(1)
            .say(acp::response(
                0,
                &serde_json::json!({
                    "protocolVersion": protocol,
                    "agentInfo": {"name": "Antigravity ACP Server", "version": "1.5.5"},
                    "agentCapabilities": capabilities,
                    "authMethods": [{"id": method, "name": "Sign in"}],
                }),
            ))
            .hear_out()
    };
    let mut no_logout = capabilities.clone();
    if let Some(fields) = no_logout.as_object_mut() {
        fields.insert("auth".to_owned(), serde_json::json!({}));
    }

    let (admitted, _) = own_antigravity_status(
        "launch-own-admitted",
        answering_as("Antigravity ACP Server", "1.5.5"),
    )
    .await;
    let refused = [
        (
            "business",
            own_antigravity_status(
                "launch-own-business",
                answered(1, &capabilities, "oauth-business"),
            )
            .await,
        ),
        (
            "logout",
            own_antigravity_status(
                "launch-own-logout",
                answered(1, &no_logout, "oauth-personal"),
            )
            .await,
        ),
        (
            "protocol",
            own_antigravity_status(
                "launch-own-protocol",
                answered(2, &capabilities, "oauth-personal"),
            )
            .await,
        ),
    ];

    assert!(matches!(admitted, Reading::Signed(_)), "{admitted:?}");
    for (case, (read, played)) in refused {
        assert!(matches!(read, Reading::Unreadable(_)), "{case}: {read:?}");
        assert_eq!(played, 1, "{case}: it was started once");
    }
}

/// An instance of Claude Code whose `executable` is a name its row does not
/// know, such as `cc`, is launched as the program of that name on `PATH`.
#[tokio::test(flavor = "multi_thread")]
async fn a_claude_named_cc_is_launched() {
    let sample = Sample::new("launch-cc");
    let here = sample.root().join("bin");
    fake_peer::install(
        &here,
        "cc",
        &Script::of([Reply::new().hear(1).say("say hello").say("end")]),
    )
    .expect("the fake peer");
    let consent = granted();
    let policy = conversation(&sample);
    let state = sample.state();
    let sessions = sample.sessions();
    let launcher = Launcher::new(
        local(),
        Handle::current(),
        &policy,
        &consent,
        &state,
        &sessions,
    );
    let control = Control::new().expect("a control");
    let named = Agent::routed(
        &route(Path::new("cc"), None),
        &crate::backends::REGISTRY,
        |name| (name == "PATH").then(|| OsString::from(&here)),
    )
    .expect("Claude Code's row");

    let mut host = launcher
        .launch(&named, &control, &mut Plain, None)
        .await
        .expect("started");
    let ended = host
        .turn(
            &mut Plain,
            &SessionRef::new("one").expect("a name"),
            &TurnRequest::new(TurnId::FIRST, "go"),
            &Kept::default(),
        )
        .await;
    host.stop(crucible_external_harnesses::STOP_GRACE)
        .await
        .expect("stopped");

    assert_eq!(ended, Ok(Outcome::Answered));
    assert_eq!(fake_peer::played(&here), 1);
}

/// A program anywhere in the install store is the store's, even in another
/// agent's directory or in no version, and is refused as one no release is
/// rather than taken for the user's own; only one outside the store is own.
#[tokio::test(flavor = "multi_thread")]
async fn a_program_anywhere_in_the_store_is_not_own() {
    let sample = Sample::new("launch-origin");
    let programs = sample.home().join("programs");
    let elsewhere = fake_peer::install(&programs.join("claude/1.0.0"), "agy", &Script::of([]))
        .expect("the fake peer");
    let versionless = fake_peer::install(&programs.join("antigravity"), "agy", &Script::of([]))
        .expect("the fake peer");
    let own = fake_peer::install(&sample.root().join("peer"), "agy", &Script::of([]))
        .expect("the fake peer");
    let steps = antigravity_steps(&sample, None);
    let row = AgentRow::Built(&ANTIGRAVITY);
    let profile = sample.home().join("agents/space");

    assert_eq!(Origin::of(&programs, &elsewhere), Origin::Stored);
    assert_eq!(Origin::of(&programs, &versionless), Origin::Stored);
    assert_eq!(Origin::of(&programs, &own), Origin::Own);
    for stored in [&elsewhere, &versionless] {
        let entry = antigravity_entry(&stored.to_string_lossy(), Some(&profile));
        let refused = steps.refusal(&row, &entry);
        assert!(
            matches!(refused, Some(Unlaunched::Unstored { .. })),
            "{}: {refused:?}",
            stored.display()
        );
    }
}

/// The sentence a store release with no profile directory is refused with.
const UNPROFILED: &str = "keeps its sign-in in a profile directory of its own, so it is never \
                          one another Google product shares";

/// A release of Antigravity in the install store whose instance names no
/// profile directory is refused when it is launched, saying why, and is not
/// started.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_antigravity_without_a_profile_is_refused_at_launch() {
    let sample = Sample::new("launch-antigravity-unprofiled");
    let version = snapshot_version().unwrap_or("1.3.0");
    let here = sample.home().join("programs/antigravity").join(version);
    let program = fake_peer::install(
        &here,
        "agy_acp_server.par",
        &Script::of([antigravity_answering(version)]),
    )
    .expect("the fake peer");
    let steps = antigravity_steps(&sample, None);
    let entry = antigravity_entry(&program.to_string_lossy(), None);

    let refused = steps.refusal(&AgentRow::Built(&ANTIGRAVITY), &entry);
    let read = antigravity_read(&steps, &program.to_string_lossy(), None).await;

    let said = refused
        .map(|refused| refused.to_string())
        .unwrap_or_default();
    assert!(said.contains(UNPROFILED), "{said}");
    let Reading::Unreadable(failure) = read else {
        panic!("{read:?}");
    };
    assert!(
        failure.reason().as_str().contains(UNPROFILED),
        "{failure:?}"
    );
    assert_eq!(fake_peer::played(&here), 0, "nothing was started");
}

/// The user's own Antigravity with no profile directory is not refused for
/// it: it uses the sign-in its user already gave it.
#[tokio::test(flavor = "multi_thread")]
async fn an_own_antigravity_without_a_profile_is_not_refused_for_it() {
    let sample = Sample::new("launch-own-unprofiled");
    let here = sample.root().join("bin");
    let program = fake_peer::install(
        &here,
        "agy",
        &Script::of([answering_as("Antigravity ACP Server", "1.5.5")]),
    )
    .expect("the fake peer");
    let steps = antigravity_steps(&sample, None);
    let entry = antigravity_entry(&program.to_string_lossy(), None);

    let refused = steps.refusal(&AgentRow::Built(&ANTIGRAVITY), &entry);
    let read = antigravity_read(&steps, &program.to_string_lossy(), None).await;

    assert!(refused.is_none(), "{refused:?}");
    assert!(matches!(read, Reading::Signed(_)), "{read:?}");
}

/// A profile directory the user gives their own `agy` by hand is the home it
/// is started with, and it is still started as `agy acp`.
#[tokio::test(flavor = "multi_thread")]
async fn an_own_agy_given_a_profile_directory_by_hand_gets_gemini_home() {
    let sample = Sample::new("launch-own-profiled");
    let program = fake_peer::install(&sample.root().join("bin"), "agy", &Script::of([]))
        .expect("the fake peer");
    let profile = sample.home().join("agents/mine");
    let route = Route::External {
        serves: Serves::Provider("google"),
        instance: "space".into(),
        driver: Driver::Antigravity,
        executable: program.to_string_lossy().into(),
        profile: Some(profile.to_string_lossy().into()),
        model: None,
        effort: None,
        location: None,
        enabled: true,
    };
    let agent = Agent::routed(&route, &ANTIGRAVITY_REGISTRY, |_| None).expect("a row");
    let consent = Consent::new(Routes::production());
    let policy = conversation(&sample);
    let state = sample.state();
    let sessions = sample.sessions();
    let workspace = sample.root();
    let shape = agent.shape(None, &workspace);
    let runs = Runs::new(
        Launcher::new(
            local(),
            Handle::current(),
            &policy,
            &consent,
            &state,
            &sessions,
        ),
        Arc::new(SandboxEnablement::new(false, false)),
        agent.clone(),
        "space",
        &Controls::default(),
    );

    let request = runs.dialect(Mode::Ask).expect("asking").request(shape);

    assert_eq!(request.arguments, ["acp"]);
    let home = request
        .environment
        .iter()
        .find_map(|(name, value)| (name == "GEMINI_HOME").then_some(value.as_str()));
    assert_eq!(home, Some(&*profile.to_string_lossy()));
}
