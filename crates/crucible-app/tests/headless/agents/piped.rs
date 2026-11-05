//! A sign-in the agent runs as a command with no real terminal, through the
//! agents door: refused with nobody lent to carry it to and where the agent's
//! own word differs from its signed entry, carried both ways where neither, and
//! believed only as the agent reads afterwards.

#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
use crucible_app::client::agents::HandoverFailed;
use crucible_sandbox::SandboxDomainPattern;

use super::*;

/// What a front end lends the door for a sign-in: a channel for a command over
/// pipes, a terminal for one that needs a real one, or neither.
type Lent<'a> = (
    Option<&'a dyn SignInChannel>,
    Option<&'a dyn SignInTerminal>,
);

/// A method that needs a real terminal is refused where no front end lent a
/// handover, before anything is asked or started, and a channel lent for the
/// piped route is no handover.
#[test]
#[cfg(unix)]
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
        Some(&person),
        &runtime,
    )?;

    assert!(
        matches!(&refused, Err(AgentsError::Unlent(_))),
        "{}",
        shape(&refused)
    );
    assert_eq!(fake_peer::played(&root), 0);
    Ok(())
}

/// A front end lent a terminal, keeping what the door asked of it.
#[derive(Default)]
#[cfg(unix)]
struct Handed {
    offered: Mutex<Vec<u32>>,
    resumed: std::sync::atomic::AtomicUsize,
    /// Whether each child was stopped when the handover was asked for.
    stopped: Mutex<Vec<bool>>,
    /// Whether to refuse the handover, as a terminal that cannot be given.
    refuse: std::sync::atomic::AtomicBool,
}

#[cfg(unix)]
impl Handed {
    fn offered(&self) -> Vec<u32> {
        self.offered
            .lock()
            .map(|offered| offered.clone())
            .unwrap_or_default()
    }

    fn stopped(&self) -> Vec<bool> {
        self.stopped
            .lock()
            .map(|stopped| stopped.clone())
            .unwrap_or_default()
    }
}

/// Whether `pid` is stopped now, as a handover must find the child before it
/// gives it the terminal.
#[cfg(target_os = "linux")]
fn stopped(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .and_then(|(_, rest)| rest.trim_start().chars().next())
        })
        == Some('T')
}

#[cfg(all(unix, not(target_os = "linux")))]
fn stopped(_: u32) -> bool {
    true
}

#[cfg(unix)]
impl SignInTerminal for Handed {
    fn handed(&self, pid: u32) -> Result<(), HandoverFailed> {
        if let Ok(mut offered) = self.offered.lock() {
            offered.push(pid);
        }
        if let Ok(mut stopped_now) = self.stopped.lock() {
            stopped_now.push(stopped(pid));
        }
        if self.refuse.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(HandoverFailed::new("this terminal cannot be given"));
        }
        Ok(())
    }

    fn resumed(&self) {
        self.resumed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
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
/// records a method run as a command over pipes.
fn signing_in_by(bench: &Bench, script: &Peer) -> Result<Value, Failed> {
    signing_in_with(
        bench.server.peer_release("1.0.0", &peer(script)?),
        &command(),
    )
}

/// The same, with the method's route the real terminal, which the door runs
/// unconfined and hands this process's terminal to.
#[cfg(unix)]
fn signing_in_at_terminal(bench: &Bench, script: &Peer) -> Result<Value, Failed> {
    let mut release = signing_in_by(bench, script)?;
    set(&mut release, "/auth_methods/0/route", json!("terminal"));
    Ok(release)
}

/// `release` with the one method `terminal`, which runs `by` through the door.
fn signing_in_with(mut release: Value, by: &str) -> Result<Value, Failed> {
    let object = release.as_object_mut().ok_or("a release is an object")?;
    object.insert(
        "auth_methods".into(),
        json!([{
            "id": "terminal",
            "kind": "terminal",
            "name": "Sign in at a terminal",
            "command": by,
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
        self.with_installed_at(release, &command())
    }

    /// [`Bench::with_installed`], the instance running `program` of the
    /// version.
    fn with_installed_at(&self, release: &Value, program: &str) -> Result<PathBuf, Failed> {
        self.server
            .catalog(1, std::slice::from_ref(release), &json!([]))?;
        let root = self.programs().join("1.0.0");
        self.configure(&root.join(program), &json!({}))?;
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
        channel: Option<&dyn SignInChannel>,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Result<Outcome, AgentsError>, Failed> {
        self.ask_piped_lent(front, (sandbox, confined, yes), (channel, None), runtime)
    }

    /// The door asked as [`Bench::ask_piped`] asks it, with `terminal` lent.
    #[cfg(unix)]
    fn ask_piped_terminal(
        &self,
        front: &mut Answering<'_>,
        (sandbox, confined, yes): (Arc<dyn SandboxService>, bool, bool),
        terminal: &dyn SignInTerminal,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Result<Outcome, AgentsError>, Failed> {
        self.ask_piped_lent(
            front,
            (sandbox, confined, yes),
            (None, Some(terminal)),
            runtime,
        )
    }

    /// The door asked with each of `channel` and `terminal` lent, where there
    /// is one of each.
    fn ask_piped_lent(
        &self,
        front: &mut Answering<'_>,
        sandbox: (Arc<dyn SandboxService>, bool, bool),
        lent: (Option<&dyn SignInChannel>, Option<&dyn SignInTerminal>),
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Result<Outcome, AgentsError>, Failed> {
        let steps = self.piped_steps(sandbox.clone(), &REGISTRY, runtime)?;
        self.ask_piped_beside(front, sandbox, (lent, &steps), runtime)
    }

    /// The steps of a conversation on `sandbox` that confines its commands
    /// where `confined`, whose route for `goose` has its yes where `yes`, and
    /// which knows the agents of `registry`.
    fn piped_steps(
        &self,
        (sandbox, confined, yes): (Arc<dyn SandboxService>, bool, bool),
        registry: &'static Registry,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Steps, Failed> {
        self.piped_steps_on(&piped_consent(yes)?, (sandbox, confined), registry, runtime)
    }

    /// [`Bench::piped_steps`], its yeses those of `consent`.
    fn piped_steps_on(
        &self,
        consent: &Consent,
        (sandbox, confined): (Arc<dyn SandboxService>, bool),
        registry: &'static Registry,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Steps, Failed> {
        let policy = SandboxPolicy::standard(&self.tree.workspace()?)?.with_enabled(confined);
        let state = catalog_fetch::state_in(self.home.path());
        Ok(Steps::new(
            Launcher::new(
                sandbox,
                runtime.handle().clone(),
                &policy,
                consent,
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
        (lent, steps): (Lent<'_>, &Steps),
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Result<Outcome, AgentsError>, Failed> {
        self.ask_piped_on(
            (front, request(sign_in("work", "terminal")?)),
            (&piped_consent(yes)?, sandbox, confined),
            (lent, steps),
            runtime,
        )
    }

    /// The door asked `request` in the conversation whose steps are `steps`,
    /// its yeses those of `consent`.
    fn ask_piped_on(
        &self,
        (front, request): (&mut dyn Front, Request),
        (consent, sandbox, confined): (&Consent, Arc<dyn SandboxService>, bool),
        (lent, steps): (Lent<'_>, &Steps),
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Result<Outcome, AgentsError>, Failed> {
        let policy = SandboxPolicy::standard(&self.tree.workspace()?)?.with_enabled(confined);
        let state = catalog_fetch::state_in(self.home.path());
        let lent_launcher = Launcher::new(
            sandbox,
            runtime.handle().clone(),
            &policy,
            consent,
            &state,
            self.home.sessions(),
        );
        let lending = Shelf {
            catalog: self.server.source(),
            registry: None,
            home: &self.home,
            smoke: Some(&lent_launcher),
            steps: Some(steps),
        };
        let cancel = Cancel::new();
        let (channel, terminal) = lent;
        Ok(runtime.block_on(run(
            &request,
            &lending,
            front,
            Watching {
                progress: &mut |_| {},
                cancel: &cancel,
                link: &|_| {},
                sign_in: channel,
                terminal,
            },
        )))
    }
}

/// The steps of a conversation whose sandbox is required by its project file,
/// whatever its user chose.
#[cfg(unix)]
fn required_steps(bench: &Bench, runtime: &tokio::runtime::Runtime) -> Result<Steps, Failed> {
    let policy = SandboxPolicy::standard(&bench.tree.workspace()?)?.with_enabled(true);
    let state = catalog_fetch::state_in(bench.home.path());
    Ok(Steps::new(
        Launcher::new(
            local_of(runtime),
            runtime.handle().clone(),
            &policy,
            &piped_consent(true)?,
            &state,
            bench.home.sessions(),
        ),
        Arc::new(SandboxEnablement::new(false, true)),
        &REGISTRY,
        None,
    ))
}

/// A sign-in at a real terminal is refused while the sandbox is required,
/// before anything is asked or started, and the terminal is never lent.
#[test]
#[cfg(unix)]
fn a_real_terminal_sign_in_is_refused_while_the_sandbox_is_required() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-terminal-required")?;
    let release = signing_in_at_terminal(&bench, &Peer::of([signed_in_after()]))?;
    let root = bench.with_installed(&release)?;
    let steps = required_steps(&bench, &runtime)?;
    let handed = Handed::default();
    let mut front = answering(&bench.server, None);

    let refused = bench.ask_piped_on(
        (front_ref(&mut front), request(sign_in("work", "terminal")?)),
        (&piped_consent(true)?, local_of(&runtime), true),
        ((None, Some(&handed)), &steps),
        &runtime,
    )?;

    assert!(
        matches!(&refused, Err(AgentsError::Unconfined(_))),
        "{}",
        shape(&refused)
    );
    assert!(handed.offered().is_empty(), "the terminal was handed over");
    assert_eq!(fake_peer::played(&root), 0, "something started");
    Ok(())
}

/// A sign-in at a real terminal runs the catalog's command with the terminal
/// handed to it, holds the child stopped until the handover has succeeded and
/// the instance's control across it, takes the terminal back, and is believed
/// only as the agent is read afterwards.
#[test]
#[cfg(unix)]
fn a_real_terminal_sign_in_runs_with_a_handover_and_is_read_as_signed_in() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-terminal-ran")?;
    let release = signing_in_at_terminal(
        &bench,
        &Peer::of([
            signed_in_after(),
            Played::new()
                .pause(Duration::from_millis(50))
                .say("the terminal says hello"),
            signed_in_after(),
        ]),
    )?;
    let root = bench.with_installed(&release)?;
    let handed = Handed::default();

    let signed = bench.ask_piped_terminal(
        &mut answering(&bench.server, Some("Run it")),
        (local_of(&runtime), false, true),
        &handed,
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
    assert_eq!(handed.offered().len(), 1, "the child was not handed over");
    assert!(handed.offered().first().is_some_and(|pid| *pid > 0));
    assert_eq!(
        handed.stopped(),
        [true],
        "the child was not held stopped until the terminal was its own"
    );
    assert_eq!(
        handed.resumed.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "the terminal was not taken back once"
    );
    assert_eq!(fake_peer::played(&root), 3);
    Ok(())
}

/// A real-terminal sign-in starts its recorded command with the release's
/// updater-off variable, as the piped route does: the agent's updater cannot
/// run during the unconfined sign-in.
#[test]
#[cfg(unix)]
fn a_real_terminal_sign_in_turns_the_agents_updater_off() -> Result<(), Failed> {
    use std::os::unix::fs::PermissionsExt as _;

    let runtime = runtime()?;
    let bench = Bench::new("agents-door-terminal-updater")?;
    let method = json!({
        "type": "terminal",
        "id": "terminal",
        "name": "Sign in at a terminal",
        "command": "login.sh",
        "args": ["login"],
        "env": {"SIGNING": "in"},
        "route": "terminal",
    });
    let status = || {
        Played::new()
            .hear(1)
            .say(offered(&method))
            .hear(1)
            .say(acp::opened(1))
    };
    let mut release = signing_in_by(&bench, &Peer::of([status(), status()]))?;
    set(&mut release, "/auth_methods/0/command", json!("login.sh"));
    set(&mut release, "/auth_methods/0/route", json!("terminal"));
    if let Some(object) = release.as_object_mut() {
        object.insert(
            "updater_off".into(),
            json!({"name": "GOOSE_NO_UPDATE", "value": "1"}),
        );
    }
    let root = bench.with_installed(&release)?;
    let script = root.join("login.sh");
    fs::write(
        &script,
        "#!/bin/sh\nprintf %s \"$GOOSE_NO_UPDATE\" > ../updater\n",
    )?;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;

    let signed = bench.ask_piped_terminal(
        &mut answering(&bench.server, Some("Run it")),
        (local_of(&runtime), false, true),
        &Handed::default(),
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
        "{signed:?}",
    );
    assert_eq!(
        fs::read_to_string(bench.programs().join("updater"))?,
        "1",
        "the sign-in ran with the agent's updater left on"
    );
    Ok(())
}

/// A handover that cannot give the terminal refuses the sign-in and kills the
/// child's whole group: nothing is left running with this process's group in
/// the foreground.
#[test]
#[cfg(unix)]
fn a_handover_that_fails_refuses_and_kills_the_group() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-terminal-unhanded")?;
    let release = signing_in_at_terminal(
        &bench,
        &Peer::of([
            signed_in_after(),
            Played::new()
                .pause(Duration::from_secs(30))
                .say("the terminal was never given"),
            signed_in_after(),
        ]),
    )?;
    bench.with_installed(&release)?;
    let handed = Handed::default();
    handed
        .refuse
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let refused = bench.ask_piped_terminal(
        &mut answering(&bench.server, Some("Run it")),
        (local_of(&runtime), false, true),
        &handed,
        &runtime,
    )?;

    assert!(
        matches!(&refused, Err(AgentsError::Piped(Unpiped::Unhanded(_)))),
        "{}",
        shape(&refused)
    );
    assert_eq!(handed.offered().len(), 1, "no child was handed over");
    assert_eq!(
        handed.resumed.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "the terminal was not taken back once"
    );
    let pid = handed.offered().first().copied().ok_or("no child's pid")?;
    let pid = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or("a pid this process can name")?;
    let until = Instant::now() + Duration::from_secs(5);
    while rustix::process::test_kill_process_group(pid).is_ok() {
        if Instant::now() >= until {
            return Err("the child's group outlived a failed handover".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Not held: that the command never started. It is stopped from the parent
    // once `spawn` returns, so it may start (and count itself as played) in
    // the moment before, as the module above documents. What it must not do is
    // run on, and that is held above: a child that was not stopped is refused
    // as one that would not stop, never as a failed handover, and one not
    // killed outlives the group check.
    Ok(())
}

/// `front` as the shared reference [`Bench::ask_piped_on`] takes.
#[cfg(unix)]
fn front_ref<'a>(front: &'a mut Answering<'_>) -> &'a mut dyn Front {
    front
}
fn piped_consent(yes: bool) -> Result<Consent, Failed> {
    let consent = super::shipped_hold::shipped()?;
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
        None,
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
    let steps = bench.piped_steps((Arc::clone(&local), false, true), registry, &runtime)?;
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
        ((Some(&person as &dyn SignInChannel), None), &steps),
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
        Some(&person),
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
        Some(&person),
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
        Some(&person),
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
            Some(&person),
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
        Some(&Person::default()),
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
        Some(&Person::default()),
        &runtime,
    )?;

    assert!(
        matches!(&refused, Err(AgentsError::Piped(Unpiped::Outside(_)))),
        "{}",
        shape(&refused)
    );
    assert_eq!(
        fake_peer::played(&root),
        0,
        "refused before anything starts"
    );
    Ok(())
}

/// A method offering `by`, the peer that would be asked of it.
fn offering(by: &str) -> Result<(Vec<u8>, Vec<u8>), Failed> {
    let mut method = terminal_method();
    set(&mut method, "/command", json!(by));
    peer(&Peer::of([Played::new().hear(1).say(offered(&method))]))
}

/// Asserts the door refuses signing in by a command that is the program
/// the release starts led, as `program` installed: run bare, it would load from
/// the working directory, so nothing is started.
fn refused_as_the_program_led(
    bench: &Bench,
    program: &str,
    runtime: &tokio::runtime::Runtime,
) -> Result<(), Failed> {
    let refused = bench.ask_piped(
        &mut answering(&bench.server, None),
        (local_of(runtime), false, true),
        Some(&Person::default()),
        runtime,
    )?;

    assert!(
        matches!(&refused, Err(AgentsError::Piped(Unpiped::Outside(_)))),
        "{}",
        shape(&refused)
    );
    let beside = bench
        .programs()
        .join("1.0.0")
        .join(program)
        .parent()
        .map(Path::to_path_buf)
        .ok_or("the program lies in a directory")?;
    assert_eq!(
        fake_peer::played(&beside),
        0,
        "refused before anything starts"
    );
    Ok(())
}

/// The pinned interpreter takes `-I` and its launcher from every other start,
/// so a sign-in command naming it is refused rather than started without them.
#[test]
fn a_command_that_is_the_pinned_interpreter_is_not_run() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-piped-interpreter")?;
    let program = uvx::python();
    let release = signing_in_with(
        uvx::interpreted_as(&bench.server, &offering(&program)?),
        &program,
    )?;
    bench.with_installed_at(&release, &program)?;

    refused_as_the_program_led(&bench, &program, &runtime)
}

/// The pinned Node takes its preload from every other start, so a sign-in
/// command naming it is refused rather than started without it.
#[cfg(feature = "proof")]
#[test]
fn a_command_that_is_the_pinned_node_is_not_run() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-piped-node")?;
    let program = npm::node();
    let release = signing_in_with(npm::noded_as(&bench.server, &offering(&program)?), &program)?;
    bench.with_installed_at(&release, &program)?;

    refused_as_the_program_led(&bench, &program, &runtime)
}

/// A link to the pinned interpreter is the pinned interpreter.
#[cfg(unix)]
#[test]
fn a_command_that_links_to_the_pinned_interpreter_is_not_run() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-piped-interpreter-link")?;
    let program = uvx::python();
    let link = format!("{program}-again");
    let release = signing_in_with(uvx::interpreted_as(&bench.server, &offering(&link)?), &link)?;
    let root = bench.with_installed_at(&release, &program)?;
    linked(&root, &root.join(&program), &link)?;

    refused_as_the_program_led(&bench, &program, &runtime)
}

/// A link in the version that leads out of it is refused however executable
/// what it leads to is, and what it leads to is not run.
#[cfg(unix)]
#[test]
fn a_command_that_links_out_of_the_version_is_not_run() -> Result<(), Failed> {
    const LINK: &str = "leading-out";
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-piped-link-out")?;
    let release = signing_in_with(bench.server.peer_release("1.0.0", &offering(LINK)?), LINK)?;
    let root = bench.with_installed(&release)?;
    let elsewhere = Tree::new("agents-door-piped-link-out-elsewhere")?;
    let outside = fake_peer::install(&elsewhere.0, "goose", &Peer::of([signed_in_after()]))?;
    linked(&root, &outside, LINK)?;

    let refused = bench.ask_piped(
        &mut answering(&bench.server, None),
        (local_of(&runtime), false, true),
        Some(&Person::default()),
        &runtime,
    )?;

    assert!(
        matches!(&refused, Err(AgentsError::Piped(Unpiped::Outside(_)))),
        "{}",
        shape(&refused)
    );
    assert_eq!(fake_peer::played(&elsewhere.0), 0, "what it leads to ran");
    assert_eq!(
        fake_peer::played(&root),
        0,
        "refused before anything starts"
    );
    Ok(())
}

/// A link at `link` under `root` to `target`, the version's own modes put back,
/// as something planted there after it was installed would stand.
#[cfg(unix)]
fn linked(root: &Path, target: &Path, link: &str) -> Result<(), Failed> {
    use std::os::unix::fs::PermissionsExt;

    let at = root.join(link);
    let parent = at.parent().ok_or("a link lies in a directory")?;
    let kept = fs::metadata(parent)?.permissions();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755))?;
    std::os::unix::fs::symlink(target, &at)?;
    fs::set_permissions(parent, kept)?;
    Ok(())
}

/// A script in a version a pinned interpreter starts is still a program of the
/// version: it is not what the start leads, and it runs with the arguments the
/// record gives and nothing leading them.
#[test]
fn a_program_beside_the_pinned_interpreter_is_run_with_the_records_arguments() -> Result<(), Failed>
{
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-piped-beside-interpreter")?;
    let program = uvx::python();
    let script = format!("python/bin/login{}", std::env::consts::EXE_SUFFIX);
    let mut method = terminal_method();
    set(&mut method, "/command", json!(script));
    let played = peer(&Peer::of([
        Played::new().hear(1).say(offered(&method)),
        Played::new().say("open the link"),
        signed_in_after(),
    ]))?;
    let mut release = signing_in_with(uvx::interpreted_as(&bench.server, &played), &script)?;
    release
        .pointer_mut("/targets/0/artifacts")
        .and_then(Value::as_array_mut)
        .ok_or("a release lists its artifacts")?
        .push(artifact(&script, &played.0));
    let root = bench.with_installed_at(&release, &program)?;

    let signed = bench.ask_piped(
        &mut answering(&bench.server, None),
        (local_of(&runtime), false, true),
        Some(&Person::default()),
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
    let started = fake_peer::argued(&root.join("python/bin"));
    assert_eq!(
        started.get(1).map(Vec::as_slice),
        Some(["login".to_owned()].as_slice()),
        "{started:?}"
    );
    Ok(())
}

/// The sign-in reaches the hosts its entry names and nothing else: the sandbox
/// is asked for those alone, and the command cannot connect to an address that
/// is open to this machine. Runs where the sandbox holds a program to its
/// network and can limit it to hosts, as `CRUCIBLE_TEST_REQUIRE_ENFORCING_SANDBOX`
/// makes a job insist of the first; elsewhere a policy reaches the same hosts by
/// the launcher's own test, and a sandbox that can only close the network refuses
/// the command, as the tests after this one prove.
#[test]
fn a_command_reaches_only_the_hosts_its_entry_signs_in_at() -> Result<(), Failed> {
    let runtime = runtime()?;
    if !enforcing(&runtime)? || !limits_to_hosts(&runtime) {
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
        Some(&person),
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

/// Whether this machine's sandbox can limit a program's network to hosts rather
/// than only close it.
fn limits_to_hosts(runtime: &tokio::runtime::Runtime) -> bool {
    let service = LocalSandbox::new().watching_on(runtime.handle().clone());
    runtime
        .block_on(service.probe())
        .is_ok_and(|(_, capabilities)| {
            capabilities.claim(SandboxFeature::NetworkAllowlist) == SandboxCapability::Enforced
        })
}

/// A sandbox that can close the network and cannot limit it to hosts, as the
/// Windows one: it keeps what each start asked of it, refuses a grant of hosts
/// as that one does, and runs anything else as this machine's sandbox does, so
/// that this runs wherever that one holds a program at all.
struct ClosingOnly {
    local: Arc<dyn SandboxService>,
    prepared: Arc<Mutex<Vec<Prepared>>>,
}

impl SandboxService for ClosingOnly {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        self.local.probe()
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async move {
            let network = request.policy().network().clone();
            if let Ok(mut prepared) = self.prepared.lock() {
                prepared.push((network.clone(), None));
            }
            if matches!(network, SandboxNetworkPolicy::Domains(_)) {
                return Err(SandboxError::Unsupported {
                    feature: SandboxFeature::NetworkAllowlist,
                });
            }
            self.local.prepare(request).await
        })
    }
}

/// Asserts the door, on `sandbox`, refuses `release`'s command for the hosts its
/// entry names: an error comes back, the person is told nothing, and the last
/// start asked for, the command's, asked for those hosts.
fn refused_for_its_hosts(
    bench: &Bench,
    (sandbox, prepared): (Arc<dyn SandboxService>, &Mutex<Vec<Prepared>>),
    runtime: &tokio::runtime::Runtime,
) -> Result<(), Failed> {
    let person = Person::default();

    let refused = bench.ask_piped(
        &mut answering(&bench.server, None),
        (sandbox, true, true),
        Some(&person),
        runtime,
    )?;

    assert!(
        matches!(&refused, Err(AgentsError::Piped(Unpiped::Unlaunched(_)))),
        "{}",
        shape(&refused)
    );
    assert!(person.told().is_empty(), "the person was told something");
    let prepared: Vec<Prepared> = prepared.lock().map(|kept| kept.clone()).unwrap_or_default();
    assert_eq!(prepared.len(), 2, "the agent's read and then the command");
    let (reach, _) = prepared.get(1).ok_or("the command was not asked for")?;
    let SandboxNetworkPolicy::Domains(domains) = reach else {
        return Err("the command was not asked for with its sign-in hosts".into());
    };
    assert!(
        domains
            .allowed()
            .iter()
            .map(SandboxDomainPattern::as_str)
            .eq(["accounts.vendor.example"]),
        "the command was asked for with other hosts"
    );
    Ok(())
}

/// A sandbox that can close the network but not limit it to hosts refuses a
/// command whose entry names hosts: it is neither widened nor run closed.
#[test]
fn a_command_whose_entry_names_hosts_is_refused_where_they_cannot_be_held() -> Result<(), Failed> {
    let runtime = runtime()?;
    if !enforcing(&runtime)? {
        return Ok(());
    }
    let bench = Bench::new("agents-door-piped-closing-only")?;
    // One reply to every start, since nothing may write where it is installed.
    let release = signing_in_by(
        &bench,
        &Peer::once(Played::new().hear(1).say(offered(&terminal_method()))),
    )?;
    bench.with_installed(&release)?;
    let prepared: Arc<Mutex<Vec<Prepared>>> = Arc::default();
    let sandbox = ClosingOnly {
        local: local_of(&runtime),
        prepared: Arc::clone(&prepared),
    };

    refused_for_its_hosts(&bench, (Arc::new(sandbox), &prepared), &runtime)
}

/// The same refusal from this machine's own sandbox, where it enforces a closed
/// network and its filesystem rules and cannot limit a network to hosts: Windows
/// today. Nothing here runs where it can.
#[test]
fn a_command_whose_entry_names_hosts_is_refused_by_a_sandbox_that_only_closes() -> Result<(), Failed>
{
    let runtime = runtime()?;
    if !enforcing(&runtime)? || limits_to_hosts(&runtime) {
        return Ok(());
    }
    let bench = Bench::new("agents-door-piped-only-closes")?;
    // One reply to every start, since nothing may write where it is installed.
    let release = signing_in_by(
        &bench,
        &Peer::once(Played::new().hear(1).say(offered(&terminal_method()))),
    )?;
    bench.with_installed(&release)?;
    let prepared: Arc<Mutex<Vec<Prepared>>> = Arc::default();
    let sandbox = Recording {
        local: local_of(&runtime),
        settings: bench.tree.0.join("nothing"),
        refusing: false,
        prepared: Arc::clone(&prepared),
    };

    refused_for_its_hosts(&bench, (Arc::new(sandbox), &prepared), &runtime)
}

/// An entry that names no hosts asks for a closed network, which such a sandbox
/// can hold, so its command is run.
#[test]
fn a_command_whose_entry_names_no_hosts_is_run_where_hosts_cannot_be_held() -> Result<(), Failed> {
    let runtime = runtime()?;
    if !enforcing(&runtime)? {
        return Ok(());
    }
    let bench = Bench::new("agents-door-piped-no-hosts")?;
    // One reply to every start, since nothing may write where it is installed.
    let mut release = signing_in_by(
        &bench,
        &Peer::once(Played::new().hear(1).say(offered(&terminal_method()))),
    )?;
    release
        .as_object_mut()
        .ok_or("a release is an object")?
        .remove("sign_in_hosts");
    bench.with_installed(&release)?;
    let prepared: Arc<Mutex<Vec<Prepared>>> = Arc::default();
    let sandbox = ClosingOnly {
        local: local_of(&runtime),
        prepared: Arc::clone(&prepared),
    };
    let person = Person::typing([("", Typing::Line("begin".to_owned()))]);

    // Whether the agent reads as signed in afterwards is another test's.
    let ran = bench.ask_piped(
        &mut answering(&bench.server, None),
        (Arc::new(sandbox), true, true),
        Some(&person),
        &runtime,
    )?;

    assert!(
        !matches!(&ran, Err(AgentsError::Piped(Unpiped::Unlaunched(_)))),
        "{}",
        shape(&ran)
    );
    assert!(!person.told().is_empty(), "the command said nothing");
    let prepared: Vec<Prepared> = prepared.lock().map(|kept| kept.clone()).unwrap_or_default();
    assert!(
        matches!(prepared.get(1), Some((SandboxNetworkPolicy::Closed, _))),
        "the command was asked for with more than a closed network"
    );
    Ok(())
}

/// A client that answers the question a route is put in with `yes`, keeping
/// each route it was asked about.
struct Answers {
    yes: bool,
    routes: Vec<String>,
}

impl Front for Answers {
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Decision>> {
        Box::pin(async move {
            let Shown::Warning(warned) = shown else {
                return None;
            };
            self.routes.push(warned.route.to_string());
            let id = pending.id();
            Some(if self.yes {
                Decision::Accepted { id }
            } else {
                Decision::Declined { id }
            })
        })
    }

    fn refused(&mut self, _refusal: Refusal) {}
}

/// A catalog agent's sign-in on the build's own consent asks its route's
/// question: a no starts nothing, and a yes, written down through that consent,
/// starts it.
#[test]
fn a_catalog_agents_sign_in_asks_its_route_and_runs_only_after_the_yes() -> Result<(), Failed> {
    let runtime = runtime()?;
    let bench = Bench::new("agents-door-piped-shipped")?;
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
    let consent = super::shipped_hold::shipped()?;
    consent.keeps_in(bench.user());
    let local = local_of(&runtime);
    let steps = bench.piped_steps_on(&consent, (Arc::clone(&local), false), &REGISTRY, &runtime)?;
    let person = Person::typing([("code: ", Typing::Line("the-code".to_owned()))]);
    let asking = || -> Result<Request, Failed> {
        Ok(Request::new(
            Capabilities::none()
                .with(Capability::Install)
                .with(Capability::Permissions),
            Correlation::new(1),
            sign_in("work", "terminal")?,
        ))
    };

    let mut no = Answers {
        yes: false,
        routes: Vec::new(),
    };
    let declined = bench.ask_piped_on(
        (&mut no, asking()?),
        (&consent, Arc::clone(&local), false),
        ((Some(&person as &dyn SignInChannel), None), &steps),
        &runtime,
    )?;
    assert!(
        matches!(declined, Ok(Outcome::Undecided)),
        "{}",
        shape(&declined)
    );
    assert_eq!(no.routes, ["agent:goose"]);
    assert_eq!(fake_peer::played(&root), 0, "started before the yes");

    let mut yes = Answers {
        yes: true,
        routes: Vec::new(),
    };
    let signed = bench.ask_piped_on(
        (&mut yes, asking()?),
        (&consent, local, false),
        ((Some(&person as &dyn SignInChannel), None), &steps),
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
    assert_eq!(yes.routes, ["agent:goose"]);
    assert!(fake_peer::played(&root) > 0, "not started after the yes");
    Ok(())
}
