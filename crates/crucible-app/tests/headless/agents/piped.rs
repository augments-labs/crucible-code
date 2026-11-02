//! A sign-in the agent runs as a command with no real terminal, through the
//! agents door: refused with nobody lent to carry it to and where the agent's
//! own word differs from its signed entry, carried both ways where neither, and
//! believed only as the agent reads afterwards.

use crucible_sandbox::SandboxDomainPattern;

use super::*;

/// A method that needs a real terminal stays refused as this door not running
/// it, and with a channel lent it is no different.
#[test]
fn a_method_that_needs_a_real_terminal_is_refused_however_much_is_lent() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-real-terminal")?;
    let mut release = signing_in_by(&bench, &Peer::of([signed_in_after()]))?;
    set(&mut release, "/auth_methods/0/route", json!("terminal"));
    let root = bench.with_installed(&release)?;
    let person = Person::default();

    let refused = bench.ask_piped(
        &mut answering(&bench.server, None),
        (local_of(&runtime), false, true),
        (&release, Some(&person)),
        &runtime,
    )?;

    assert!(
        matches!(&refused, Err(AgentsError::Unsupported(_))),
        "{}",
        shape(&refused)
    );
    assert_eq!(fake_peer::played(&root), 0);
    Ok(())
}

/// A value a person types that nothing kept may carry.
const SECRET_LINE: &str = "typed-5ec7e7-c0ffee";

/// A person lent to a sign-in run as a command: keeps what it is told, and
/// sends each line once what it waits for has been said.
#[derive(Default)]
struct Person {
    told: Mutex<Vec<SaidLine>>,
    typing: Mutex<std::collections::VecDeque<(&'static str, Typing)>>,
}

impl Person {
    fn typing(typing: impl IntoIterator<Item = (&'static str, Typing)>) -> Self {
        Self {
            told: Mutex::default(),
            typing: Mutex::new(typing.into_iter().collect()),
        }
    }

    fn told(&self) -> Vec<SaidLine> {
        self.told
            .lock()
            .map(|told| told.clone())
            .unwrap_or_default()
    }

    fn told_line(&self, line: &str) -> bool {
        self.told().iter().any(|told| told.as_str() == line)
    }

    /// Whether anything told holds `value`; a bool, so that a failure repeats
    /// nothing of it.
    fn told_of(&self, value: &str) -> bool {
        self.told().iter().any(|told| told.as_str().contains(value))
    }

    fn next(&self) -> Option<Typing> {
        let mut typing = self.typing.lock().ok()?;
        let (waited, _) = typing.front()?;
        if !waited.is_empty() && !self.told_of(waited) {
            return None;
        }
        typing.pop_front().map(|(_, line)| line)
    }
}

impl SignInChannel for Person {
    fn said(&self, line: SaidLine) {
        if let Ok(mut told) = self.told.lock() {
            told.push(line);
        }
    }

    fn line(&self) -> BoxFuture<'_, Option<Typing>> {
        Box::pin(async move {
            loop {
                if let Some(next) = self.next() {
                    return Some(next);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
    }
}

/// What the agent says of its sign-in method on the wire, as `signing_in_by`'s
/// entry records it.
fn terminal_method() -> Value {
    json!({
        "type": "terminal",
        "id": "terminal",
        "name": "Sign in at a terminal",
        "command": command(),
        "args": ["login"],
        "env": {"SIGNING": "in"},
    })
}

/// The agent's answer to initializing, offering `method`.
fn offered(method: &Value) -> String {
    acp::offering("goose", "1.0.0", &json!([method]))
}

/// Answers a start that reads whether the agent is signed in, as signed in.
fn signed_in_after() -> Played {
    Played::new()
        .hear(1)
        .say(offered(&terminal_method()))
        .hear(1)
        .say(acp::opened(1))
}

/// A release of goose that is the fake peer playing `script`, whose entry
/// records `terminal_method` as a method run with no real terminal.
fn signing_in_by(bench: &Bench, script: &Peer) -> Result<Value, Failed> {
    let mut release = bench.server.peer_release("1.0.0", &peer(script)?);
    let object = release.as_object_mut().ok_or("a release is an object")?;
    object.insert(
        "auth_methods".into(),
        json!([{
            "id": "terminal",
            "kind": "terminal",
            "name": "Sign in at a terminal",
            "command": command(),
            "args": ["login"],
            "env": {"SIGNING": "in"},
            "route": "piped",
        }]),
    );
    object.insert("sign_in_hosts".into(), json!(["accounts.vendor.example"]));
    Ok(release)
}

impl Bench {
    /// `release` of goose published, installed through the door and configured
    /// as the instance `work`, with nothing of it started.
    fn with_installed(&self, release: &Value) -> Result<PathBuf, Failed> {
        self.server
            .catalog(1, std::slice::from_ref(release), &json!([]))?;
        let root = self.programs().join("1.0.0");
        self.configure(&root.join(command()), &json!({}))?;
        let (installed, _) = self.ask(
            install("1.0.0")?,
            &mut answering(&self.server, Some("Install")),
            false,
        )?;
        if !matches!(installed, Ok(Outcome::Moved { .. })) {
            return Err(format!("installed: {}", shape(&installed)).into());
        }
        Ok(root)
    }

    /// The door asked to sign `work` in by its method `terminal`, lent
    /// `channel`, under `sandbox` confined or not, the route having its yes
    /// where `yes`.
    fn ask_piped(
        &self,
        front: &mut Answering<'_>,
        (sandbox, confined, yes): (Arc<dyn SandboxService>, bool, bool),
        (release, channel): (&Value, Option<&dyn SignInChannel>),
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Result<Outcome, AgentsError>, Failed> {
        let steps = self.piped_steps(
            (Arc::clone(&sandbox), confined, yes),
            release,
            &REGISTRY,
            runtime,
        )?;
        self.ask_piped_beside(
            front,
            (sandbox, confined, yes),
            (release, channel, &steps),
            runtime,
        )
    }

    /// The steps of a conversation on `sandbox` that confines its commands
    /// where `confined`, whose route for `release` has its yes where `yes`, and
    /// which knows the agents of `registry`.
    fn piped_steps(
        &self,
        (sandbox, confined, yes): (Arc<dyn SandboxService>, bool, bool),
        release: &Value,
        registry: &'static Registry,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Steps, Failed> {
        let consent = piped_consent(release, yes)?;
        let policy = SandboxPolicy::standard(&self.tree.workspace()?)?.with_enabled(confined);
        let state = catalog_fetch::state_in(self.home.path());
        Ok(Steps::new(
            Launcher::new(
                sandbox,
                runtime.handle().clone(),
                &policy,
                &consent,
                &state,
                self.home.sessions(),
            ),
            Arc::new(SandboxEnablement::new(confined, false)),
            registry,
            None,
        ))
    }

    /// The door asked as [`Bench::ask_piped`] asks it, in the conversation
    /// whose steps are `steps`.
    fn ask_piped_beside(
        &self,
        front: &mut Answering<'_>,
        (sandbox, confined, yes): (Arc<dyn SandboxService>, bool, bool),
        (release, channel, steps): (&Value, Option<&dyn SignInChannel>, &Steps),
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Result<Outcome, AgentsError>, Failed> {
        let consent = piped_consent(release, yes)?;
        let policy = SandboxPolicy::standard(&self.tree.workspace()?)?.with_enabled(confined);
        let state = catalog_fetch::state_in(self.home.path());
        let lent = Launcher::new(
            sandbox,
            runtime.handle().clone(),
            &policy,
            &consent,
            &state,
            self.home.sessions(),
        );
        let lending = Shelf {
            catalog: self.server.source(),
            home: &self.home,
            smoke: Some(&lent),
            steps: Some(steps),
        };
        let request = request(sign_in("work", "terminal")?);
        let cancel = Cancel::new();
        Ok(runtime.block_on(run(
            &request,
            &lending,
            front,
            Watching {
                progress: &mut |_| {},
                cancel: &cancel,
                link: &|_| {},
                sign_in: channel,
            },
        )))
    }
}

/// The yeses of a conversation for the route of `release`, given where `yes`.
fn piped_consent(release: &Value, yes: bool) -> Result<Consent, Failed> {
    let row = row_of(release)?;
    let consent = Consent::new(Routes::production().cataloging(row.warned().ok().cloned()));
    if yes {
        consent.give("agent:goose");
    }
    Ok(consent)
}

/// The local sandbox of `runtime`, as a door's launcher is given one.
fn local_of(runtime: &tokio::runtime::Runtime) -> Arc<dyn SandboxService> {
    Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()))
}

/// With nobody lent to carry it to, a method run as a command is refused saying
/// a terminal is needed, before the route is asked about or anything started.
#[test]
fn a_command_with_no_channel_is_refused_before_anything_is_asked_or_started() -> Result<(), Failed>
{
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-piped-unlent")?;
    let release = signing_in_by(&bench, &Peer::of([signed_in_after()]))?;
    let root = bench.with_installed(&release)?;
    let mut front = answering(&bench.server, None);

    let refused = bench.ask_piped(
        &mut front,
        (local_of(&runtime), false, false),
        (&release, None),
        &runtime,
    )?;

    let Err(unlent @ AgentsError::Unlent(_)) = &refused else {
        return Err(shape(&refused).into());
    };
    assert!(
        unlent.to_string().contains("terminal"),
        "the refusal does not say a terminal is needed"
    );
    assert!(front.put.is_empty(), "something was asked");
    assert_eq!(fake_peer::played(&root), 0, "something was started");
    Ok(())
}

/// A method run as a command is refused beside a live run of its instance as
/// one the agent carries out is, with someone lent to carry it to and no yes
/// for its route, before the route is asked about or the command started.
#[test]
fn a_command_beside_a_live_run_is_refused_before_anything_is_asked_or_started() -> Result<(), Failed>
{
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-piped-live")?;
    let release = signing_in_by(&bench, &Peer::of([signed_in_after()]))?;
    let root = bench.with_installed(&release)?;

    let registry: &'static Registry = Box::leak(Box::new(
        Registry::new(&[]).cataloging(Box::leak(Box::new([row_of(&release)?]))),
    ));
    let key = AgentKey::new("goose")?;
    let instance = ExternalAgent::unwritten(
        "work",
        Driver::Catalog,
        &bench.executable()?,
        None,
        Some(&key),
    )
    .ok_or("a catalog instance naming its agent")?;
    let agent = Agent::configured(&instance, registry, |_| None).ok_or("its catalog row")?;
    let local = local_of(&runtime);
    let steps = bench.piped_steps(
        (Arc::clone(&local), false, true),
        &release,
        registry,
        &runtime,
    )?;
    let run = runtime.block_on(
        steps
            .runs(agent, "work")
            .launch(Mode::Ask, None, None, None),
    )?;
    assert!(started(&root, 1), "the run was not started");

    let person = Person::default();
    let mut front = answering(&bench.server, None);
    let refused = bench.ask_piped_beside(
        &mut front,
        (local, false, false),
        (&release, Some(&person), &steps),
        &runtime,
    )?;

    let Err(AgentsError::Running(live)) = &refused else {
        return Err(shape(&refused).into());
    };
    assert!(
        live.iter().any(|live| &**live == "work"),
        "the refusal does not name the instance with the run"
    );
    assert!(front.put.is_empty(), "a route was asked");
    assert!(person.told().is_empty(), "the command was carried");
    assert_eq!(fake_peer::played(&root), 1, "a second process was started");
    drop(front);
    runtime.block_on(run.close())?;
    Ok(())
}

/// A sign-in carried end to end: what the command says reaches the person, what
/// they type reaches the command, and what is believed is the agent's own word
/// when it is read afterwards.
#[test]
fn a_command_is_carried_to_the_channel_and_back_and_then_read_as_signed_in() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-piped-carried")?;
    let release = signing_in_by(
        &bench,
        &Peer::of([
            Played::new().hear(1).say(offered(&terminal_method())),
            Played::new()
                .say("open the link")
                .prompt("code: ")
                .hear(1)
                .say("thanks"),
            signed_in_after(),
        ]),
    )?;
    let root = bench.with_installed(&release)?;
    let person = Person::typing([("code: ", Typing::Line("the-code".to_owned()))]);

    let signed = bench.ask_piped(
        &mut answering(&bench.server, None),
        (local_of(&runtime), false, true),
        (&release, Some(&person)),
        &runtime,
    )?;

    assert!(
        matches!(
            &signed,
            Ok(Outcome::SignedIn {
                by: Signed::Command,
                ..
            })
        ),
        "{}",
        shape(&signed)
    );
    assert!(person.told_line("open the link"), "{:?}", person.told());
    assert!(person.told_line("code: "), "{:?}", person.told());
    assert!(person.told_line("thanks"), "{:?}", person.told());
    assert!(
        fake_peer::heard(&root)
            .iter()
            .any(|line| line == "the-code"),
        "the line typed did not reach the command"
    );
    let argued = fake_peer::argued(&root);
    assert_eq!(
        argued.get(1).map(Vec::as_slice),
        Some(&["login".to_owned()][..])
    );
    assert_eq!(fake_peer::played(&root), 3);
    Ok(())
}

/// A line marked a secret reaches the command and nothing of it comes back
/// through the door, in what it tells the channel or in how it ends.
#[test]
fn a_secret_line_reaches_the_command_and_never_comes_back_through_the_door() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-piped-secret")?;
    let release = signing_in_by(
        &bench,
        &Peer::of([
            Played::new().hear(1).say(offered(&terminal_method())),
            Played::new()
                .prompt("password: ")
                .echo(1)
                .say(format!("signed in with {SECRET_LINE}"))
                .exit(1),
        ]),
    )?;
    let root = bench.with_installed(&release)?;
    let person = Person::typing([(
        "password: ",
        Typing::Secret(Typed::new(SECRET_LINE.to_owned())),
    )]);

    let refused = bench.ask_piped(
        &mut answering(&bench.server, None),
        (local_of(&runtime), false, true),
        (&release, Some(&person)),
        &runtime,
    )?;

    assert!(
        fake_peer::heard(&root)
            .iter()
            .any(|line| line == SECRET_LINE),
        "the command was not given the line"
    );
    assert!(!person.told_of(SECRET_LINE), "it was carried back");
    assert!(
        person.told_of("signed in with"),
        "what surrounds it was not"
    );
    let Err(failed) = &refused else {
        return Err(shape(&refused).into());
    };
    assert!(matches!(failed, AgentsError::Piped(Unpiped::Failed)));
    assert!(
        !failed.to_string().contains(SECRET_LINE),
        "an error repeats it"
    );
    assert!(
        !format!("{failed:?}").contains(SECRET_LINE),
        "an error repeats it"
    );
    Ok(())
}

/// What the command writes reaches the channel bounded through the door, and
/// the cut is said in the line it fell in.
#[test]
fn what_a_command_says_is_bounded_on_its_way_through_the_door_and_the_cut_is_reported()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-piped-bounded")?;
    let release = signing_in_by(
        &bench,
        &Peer::of([
            Played::new().hear(1).say(offered(&terminal_method())),
            Played::new()
                .flood(fake_peer::Stream::Out, 16 * 1024)
                .say("")
                .say("after"),
            signed_in_after(),
        ]),
    )?;
    bench.with_installed(&release)?;
    let person = Person::default();

    let signed = bench.ask_piped(
        &mut answering(&bench.server, None),
        (local_of(&runtime), false, true),
        (&release, Some(&person)),
        &runtime,
    )?;

    assert!(
        matches!(&signed, Ok(Outcome::SignedIn { .. })),
        "{}",
        shape(&signed)
    );
    let told = person.told();
    assert!(
        told.iter()
            .all(|line| line.as_str().len() <= crucible_external_harnesses::PIPED_LINE_BYTES)
    );
    assert!(told.iter().any(SaidLine::incomplete), "no cut was reported");
    assert!(person.told_line("after"), "{told:?}");
    Ok(())
}

/// The agent's own word comes first: what it says of the method now, differing
/// from the catalog's record in any part, or not saying it at all, or saying it
/// needs a real terminal, refuses the sign-in before the command is started.
#[test]
fn a_live_answer_that_differs_from_the_record_is_refused_before_the_command_runs()
-> Result<(), Failed> {
    let runtime = runtime()?;
    let differing = [
        ("command", {
            let mut method = terminal_method();
            set(&mut method, "/command", json!("elsewhere"));
            method
        }),
        ("args", {
            let mut method = terminal_method();
            set(&mut method, "/args", json!(["login", "--more"]));
            method
        }),
        ("env", {
            let mut method = terminal_method();
            set(&mut method, "/env", json!({}));
            method
        }),
        ("route", {
            let mut method = terminal_method();
            if let Some(object) = method.as_object_mut() {
                object.insert("_meta".into(), json!({"tty": true}));
            }
            method
        }),
        ("absent", json!({"id": "browser", "name": "In a browser"})),
    ];
    for (name, method) in differing {
        let bench = Bench::new(&format!("agents-door-piped-differs-{name}"))?;
        let release = signing_in_by(
            &bench,
            &Peer::of([
                Played::new().hear(1).say(offered(&method)),
                Played::new().say("the command ran"),
            ]),
        )?;
        let root = bench.with_installed(&release)?;
        let person = Person::default();

        let refused = bench.ask_piped(
            &mut answering(&bench.server, None),
            (local_of(&runtime), false, true),
            (&release, Some(&person)),
            &runtime,
        )?;

        assert!(
            matches!(&refused, Err(AgentsError::Piped(Unpiped::Disagrees(_)))),
            "{name}: {}",
            shape(&refused)
        );
        assert_eq!(
            fake_peer::played(&root),
            1,
            "{name}: the command was started"
        );
        assert!(person.told().is_empty(), "{name}: the channel was told");
    }
    Ok(())
}

/// A command that ends well is not taken at its word: what reads the agent
/// afterwards decides, as it does for a sign-in the agent carried out.
#[test]
fn a_command_that_ended_well_is_not_believed_over_what_the_agent_says_after() -> Result<(), Failed>
{
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-piped-unbelieved")?;
    let release = signing_in_by(
        &bench,
        &Peer::of([
            Played::new().hear(1).say(offered(&terminal_method())),
            Played::new().say("all done"),
            Played::new()
                .hear(1)
                .say(offered(&terminal_method()))
                .hear(1)
                .say(acp::sign_in_required(1)),
        ]),
    )?;
    bench.with_installed(&release)?;

    let unverified = bench.ask_piped(
        &mut answering(&bench.server, None),
        (local_of(&runtime), false, true),
        (&release, Some(&Person::default())),
        &runtime,
    )?;

    assert!(
        matches!(&unverified, Err(AgentsError::Unverified(_))),
        "{}",
        shape(&unverified)
    );
    Ok(())
}

/// A command the entry records that is no program in the version installed is
/// not run from anywhere else.
#[test]
fn a_command_that_is_no_program_of_the_version_installed_is_not_run() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-piped-outside")?;
    let mut method = terminal_method();
    set(&mut method, "/command", json!("absent"));
    let mut release = signing_in_by(
        &bench,
        &Peer::of([Played::new().hear(1).say(offered(&method))]),
    )?;
    set(&mut release, "/auth_methods/0/command", json!("absent"));
    let root = bench.with_installed(&release)?;

    let refused = bench.ask_piped(
        &mut answering(&bench.server, None),
        (local_of(&runtime), false, true),
        (&release, Some(&Person::default())),
        &runtime,
    )?;

    assert!(
        matches!(&refused, Err(AgentsError::Piped(Unpiped::Outside(_)))),
        "{}",
        shape(&refused)
    );
    assert_eq!(fake_peer::played(&root), 1);
    Ok(())
}

/// The sign-in reaches the hosts its entry names and nothing else: the sandbox
/// is asked for those alone, and the command cannot connect to an address that
/// is open to this machine. Runs where the sandbox holds a program to its
/// network, as `CRUCIBLE_TEST_REQUIRE_ENFORCING_SANDBOX` makes a job insist;
/// elsewhere a policy reaches the same hosts by the launcher's own test.
#[test]
fn a_command_reaches_only_the_hosts_its_entry_signs_in_at() -> Result<(), Failed> {
    let runtime = runtime()?;
    if !enforcing(&runtime)? {
        return Ok(());
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let bench = Bench::new("agents-door-piped-reach")?;
    // One reply to every start, since nothing may write where it is installed:
    // each start goes as far as it is taken.
    let release = signing_in_by(
        &bench,
        &Peer::once(
            Played::new()
                .hear(1)
                .say(offered(&terminal_method()))
                .reach(listener.local_addr()?, "held in", "reached it")
                .hear(1)
                .say(acp::opened(1)),
        ),
    )?;
    bench.with_installed(&release)?;
    let prepared = Arc::default();
    let sandbox = Recording {
        local: local_of(&runtime),
        settings: bench.tree.0.join("nothing"),
        refusing: false,
        prepared: Arc::clone(&prepared),
    };
    let person = Person::typing([
        ("", Typing::Line("begin".to_owned())),
        ("held in", Typing::Line("end".to_owned())),
    ]);

    // How the agent reads afterwards does not decide what it could reach.
    let _read = bench.ask_piped(
        &mut answering(&bench.server, None),
        (Arc::new(sandbox), true, true),
        (&release, Some(&person)),
        &runtime,
    )?;

    assert!(person.told_line("held in"), "{:?}", person.told());
    assert!(!person.told_line("reached it"), "it connected");
    let prepared: Vec<Prepared> = prepared.lock().map(|kept| kept.clone()).unwrap_or_default();
    let (reach, _) = prepared.get(1).ok_or("the command was not started")?;
    let SandboxNetworkPolicy::Domains(domains) = reach else {
        return Err("the command was not given its sign-in hosts".into());
    };
    let hosts: Vec<&str> = domains
        .allowed()
        .iter()
        .map(SandboxDomainPattern::as_str)
        .collect();
    assert_eq!(hosts, ["accounts.vendor.example"]);
    Ok(())
}
