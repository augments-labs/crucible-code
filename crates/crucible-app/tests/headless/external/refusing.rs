//! What a launch that cannot happen is asked about.
//!
//! Each case here holds a conversation whose user file names an instance of
//! Claude Code whose program is nowhere on the `PATH`, and whose route either
//! has no yes or has one already. A command that would start that program is
//! refused for it being missing, before the route is put and before anything
//! is written; nobody is asked to agree to a launch that cannot happen, and
//! the command ends undecided, as it does where a person declines. The consent
//! here has no file to write a yes to, so what a yes would have recorded is
//! not asserted; only that the route is not put where it has no yes, and the
//! command's outcome. The case whose route has its yes also asserts that the
//! user file is left as it was and that the agent's program is not started.
//!
//! A sign-out is refused the same way where any instance on the profile has a
//! run that could not be confirmed stopped, since the agent's program would
//! run beside it: the cases at the end of this file make such a run with a
//! sandbox that never confirms a stop, and hold the user file, the
//! conversation's own run and the agent's program to being left alone.

use std::path::Path;
use std::sync::Arc;

use crucible_app::backends::Registry;
use crucible_app::client;
use crucible_app::launching::{Launcher, Steps};
use crucible_app::outside::Relaunch;
use crucible_client_api::{
    Capabilities as Asking, Command, LoginOutcome, LogoutOutcome, Name, Outcome, Pending,
};
use crucible_config::{Driver, ExternalAgent};
use crucible_external_harnesses::fake_peer::{self, Reply, scripts::claude};
use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxCommand, SandboxEnablement, SandboxError,
    SandboxInput, SandboxInspection, SandboxLaunch, SandboxOutput, SandboxPolicy, SandboxProcess,
    SandboxRequest, SandboxService, SandboxSession, SandboxUsage, SandboxViolation,
};
use crucible_sandbox_local::LocalSandbox;
use crucible_tools::Mode;

use super::{Asked, Outside, Picking, SHARED, accounting, connected, row, runtime, turn};
use crate::{Failed, Tree};

/// The user file naming one instance of Claude Code, run as a program that is
/// not installed.
const MISSING: &str = r#"{
  "externalAgents": {"work": {"driver": "claude", "executable": "no-such-claude-program"}},
  "updates": {"check": "auto"}
}
"#;

/// Whether `put` holds the question for the route of an installed agent.
fn asked_route(put: &[Pending]) -> bool {
    put.iter().any(|pending| {
        matches!(pending, Pending::Warning { route, .. } if route.as_str() == "agent:claude")
    })
}

/// A connect through, or a sign-out of, an instance whose program is not
/// installed, with its route unanswered, does not put the route to the client
/// and ends undecided. Before the refusal came first the route was put, its
/// accept could not be kept, and the command ended some other way.
#[test]
fn a_launch_that_cannot_happen_is_refused_before_the_route_is_asked() -> Result<(), Failed> {
    type Made = fn(Name) -> Command;
    let cases: [(&str, Made, Vec<Option<&'static str>>, Outcome); 2] = [
        (
            "connect",
            |instance| Command::Connect {
                instance: Some(instance),
            },
            vec![Some("Accept"), None],
            Outcome::Login(LoginOutcome::Undecided),
        ),
        (
            "sign-out",
            |instance| Command::SignOut { instance },
            vec![Some(client::accounts::SIGN_OUT), Some("Accept"), None],
            Outcome::Logout(LogoutOutcome::Undecided),
        ),
    ];
    let mut failures = Vec::new();
    for (case, command, choices, undecided) in cases {
        let tree = Tree::new(&format!("external-refused-first-{case}"))?;
        let script = connected([claude::clean(), claude::signed_in()]);
        let mut outside = Outside::new(&tree, &script)?;
        outside.unanswer();
        let asked = Asked {
            installed: (MISSING, "work"),
            command: &command,
            asking: Asking::every(),
            after: None,
        };
        let mut picking = Picking::of(choices);

        let (outcome, _file) = accounting(&tree, &mut outside, &asked, &mut picking)?;

        if asked_route(&picking.put) {
            failures.push(format!(
                "{case}: the agent's route was asked about for a program that is not installed"
            ));
        }
        if outcome != undecided {
            failures.push(format!(
                "{case}: ended {outcome:?} rather than undecided, with the route unanswered"
            ));
        }
    }
    // Every case is run, so that a failure in one does not hide the other.
    assert!(failures.is_empty(), "{failures:#?}");
    Ok(())
}

/// A sign-out of an instance whose program is not installed, with its route
/// already answered, is refused before it writes anything, as it is where the
/// route has no yes: nothing could be signed out, so no instance on the
/// profile is disabled and nothing is started.
#[test]
fn a_sign_out_whose_program_is_missing_writes_nothing_on_an_answered_route() -> Result<(), Failed> {
    let missing = SHARED.replace(
        r#""executable": "claude""#,
        r#""executable": "no-such-claude-program""#,
    );
    let tree = Tree::new("external-sign-out-missing-answered")?;
    let script = connected([claude::clean(), Reply::new(), claude::signed_out()]);
    let mut outside = Outside::new(&tree, &script)?;
    let asked = Asked {
        installed: (&missing, "work"),
        command: &|instance| Command::SignOut { instance },
        asking: Asking::every(),
        after: None,
    };
    // Signs out, then goes back from whatever it is asked next.
    let mut picking = Picking::of([Some(client::accounts::SIGN_OUT), None]);
    for _ in 0..500 {
        if fake_peer::played(&outside.peer) >= 3 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let before = fake_peer::played(&outside.peer);

    let (outcome, file) = accounting(&tree, &mut outside, &asked, &mut picking)?;

    assert_eq!(file, missing, "nothing was written");
    assert_eq!(
        fake_peer::played(&outside.peer) - before,
        0,
        "the agent's program was started"
    );
    assert_eq!(outcome, Outcome::Logout(LogoutOutcome::Undecided));
    let told = picking.put.get(1).is_some_and(|put| match put {
        Pending::Account { asks, facts, .. } => {
            asks.as_str() == "Claude Code cannot be started."
                && facts.iter().any(|fact| {
                    fact.as_str()
                        .starts_with("Agent: work, run as no-such-claude-program")
                })
        }
        _ => false,
    });
    assert!(told, "{:?}", picking.put);
    Ok(())
}

/// A sandbox whose processes run as the local one's do, but which can never
/// confirm that one of them stopped.
struct Stubborn(Arc<dyn SandboxService>);

impl SandboxService for Stubborn {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        self.0.probe()
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async move {
            let session: Box<dyn SandboxSession> =
                Box::new(StubbornSession(self.0.prepare(request).await?));
            Ok(session)
        })
    }
}

/// Its session.
struct StubbornSession(Box<dyn SandboxSession>);

impl SandboxSession for StubbornSession {
    fn inspection(&self) -> &SandboxInspection {
        self.0.inspection()
    }

    fn materialize(&mut self) -> BoxFuture<'_, Result<(), SandboxError>> {
        self.0.materialize()
    }

    fn stage<'a>(
        self: Box<Self>,
        command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Box<dyn SandboxLaunch>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let launch: Box<dyn SandboxLaunch> =
                Box::new(StubbornLaunch(self.0.stage(command).await?));
            Ok(launch)
        })
    }
}

/// Its staged launch.
struct StubbornLaunch(Box<dyn SandboxLaunch>);

impl SandboxLaunch for StubbornLaunch {
    fn inspection(&self) -> &SandboxInspection {
        self.0.inspection()
    }

    fn release<'a>(self: Box<Self>) -> BoxFuture<'a, Result<Box<dyn SandboxProcess>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let process: Box<dyn SandboxProcess> =
                Box::new(StubbornProcess(self.0.release().await?));
            Ok(process)
        })
    }
}

/// Its process, which never reads as ended and whose stop always fails.
struct StubbornProcess(Box<dyn SandboxProcess>);

impl SandboxProcess for StubbornProcess {
    fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
        self.0.take_stdin()
    }

    fn take_async_stdin(&mut self) -> Option<Box<dyn SandboxInput>> {
        self.0.take_async_stdin()
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.0.take_stdout()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.0.take_stderr()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        Ok(None)
    }

    fn stop(&mut self) -> BoxFuture<'_, std::io::Result<()>> {
        Box::pin(async { Err(std::io::Error::other("the scope did not answer")) })
    }

    fn inspection(&self) -> &SandboxInspection {
        self.0.inspection()
    }

    fn usage(&self) -> SandboxUsage {
        self.0.usage()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        self.0.violation()
    }
}

/// `outside` with a run of the entry `instance` launched and closed through
/// steps that can never confirm a stop, so that the run is one that may still
/// be running, and the conversation's account steps taken through those steps.
/// The peer has started the run by the time this returns.
fn left_running(tree: &Tree, outside: Outside, instance: &str) -> Result<Outside, Failed> {
    let Outside {
        conversation,
        peer,
        route,
        launched,
    } = outside;
    let runtime = runtime()?;
    let consent = conversation
        .consent()
        .cloned()
        .ok_or("the conversation holds no yeses")?;
    let policy = SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(false);
    let sandbox: Arc<dyn SandboxService> = Arc::new(Stubborn(Arc::new(
        LocalSandbox::new().watching_on(runtime.clone()),
    )));
    let state = tree.0.join("state");
    let steps = Steps::new(
        Launcher::new(sandbox, runtime.clone(), &policy, &consent, &state),
        Arc::new(SandboxEnablement::new(false, false)),
        Box::leak(Box::new(Registry::new(std::slice::from_ref(row()?)))),
        Some(Path::to_path_buf(&peer).into_os_string()),
    );
    let entry = ExternalAgent::unwritten(instance, Driver::Claude, "claude", None, None)
        .ok_or("an instance of a built-in driver")?;
    let (agent, _) = steps.onto(&entry, Mode::Ask)?;
    let runs = steps.runs(agent, instance);
    // The conversation's own run is counted once the peer has read its script:
    // after the probe, the status check and it.
    let wait = |until: usize| {
        for _ in 0..500 {
            if fake_peer::played(&peer) >= until {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    };
    wait(3);
    runtime.block_on(async {
        let run = runs.launch(Mode::Ask, None, None, None).await?;
        wait(4);
        if run.close().await.is_ok() {
            return Err("the run's stop was confirmed".into());
        }
        Ok::<_, Failed>(())
    })?;
    Ok(Outside {
        conversation: conversation.stepping(steps),
        peer,
        route,
        launched,
    })
}

/// Signs out `signing` of the user file `SHARED` while a run of `unconfirmed`
/// could not be confirmed stopped, choosing the sign-out and going back from
/// whatever it is asked next, and holds it to having changed nothing: the
/// user file as it was, the agent's program not started, the conversation's
/// own run still taking a turn, and the person told what to do about the run
/// without being asked to agree to the agent's route. Where the route is
/// `answered` it is the case that was shipped: nothing to ask, so only the
/// refusal keeps the instances from being written off; where it is not, the
/// route must not be put either.
fn refused_beside_an_unconfirmed_run(
    name: &str,
    (signing, unconfirmed): (&str, &str),
    answered: bool,
) -> Result<(), Failed> {
    let tree = Tree::new(name)?;
    let script = connected([
        claude::clean(),
        Reply::new().hear_out(),
        Reply::new(),
        claude::signed_out(),
    ]);
    let mut outside = left_running(&tree, Outside::new(&tree, &script)?, unconfirmed)?;
    if !answered {
        outside.unanswer();
    }
    let asked = Asked {
        installed: (SHARED, signing),
        command: &|instance| Command::SignOut { instance },
        asking: Asking::every(),
        after: None,
    };
    let mut picking = Picking::of([Some(client::accounts::SIGN_OUT), None]);
    let before = fake_peer::played(&outside.peer);

    let (outcome, file) = accounting(&tree, &mut outside, &asked, &mut picking)?;

    assert!(
        !asked_route(&picking.put),
        "the agent's route was asked about for a sign-out that could not go ahead"
    );
    assert_eq!(file, SHARED, "the user file was written");
    assert_eq!(
        fake_peer::played(&outside.peer) - before,
        0,
        "the agent's program was started"
    );
    assert_eq!(outcome, Outcome::Logout(LogoutOutcome::Undecided));
    let told = picking.put.get(1).is_some_and(|put| match put {
        Pending::Account { facts, .. } => facts
            .iter()
            .any(|fact| fact.as_str().ends_with("restart crucible")),
        _ => false,
    });
    assert!(told, "{:?}", picking.put);
    let (turned, _events) = turn(&mut outside, "again", false)?;
    assert!(
        turned.is_ok(),
        "the conversation's run was stopped: {turned:?}"
    );
    Ok(())
}

/// A sign-out of an instance whose own run could not be confirmed stopped is
/// refused before it disconnects or writes anything, and starts no agent
/// program beside that run, whether or not the agent's route has a yes.
#[test]
fn a_sign_out_whose_own_run_is_not_confirmed_stopped_changes_nothing() -> Result<(), Failed> {
    for (name, answered) in [
        ("external-sign-out-own-unstopped-answered", true),
        ("external-sign-out-own-unstopped", false),
    ] {
        refused_beside_an_unconfirmed_run(name, ("work", "work"), answered)?;
    }
    Ok(())
}

/// So is one where another instance on the same profile is the one whose run
/// could not be confirmed stopped: the agent's program signs the whole profile
/// out, beside that run.
#[test]
fn a_sign_out_beside_a_sharing_instance_not_confirmed_stopped_changes_nothing() -> Result<(), Failed>
{
    for (name, answered) in [
        ("external-sign-out-sharing-unstopped-answered", true),
        ("external-sign-out-sharing-unstopped", false),
    ] {
        refused_beside_an_unconfirmed_run(name, ("work", "spare"), answered)?;
    }
    Ok(())
}

/// A run on another profile is no reason to hold the sign-out of this one: the
/// agent's program signs only its own profile out, so every instance on this
/// one is written off and the program is started, as where no run is in doubt.
#[test]
fn a_sign_out_beside_an_unconfirmed_run_on_another_profile_goes_ahead() -> Result<(), Failed> {
    let tree = Tree::new("external-sign-out-apart-unstopped")?;
    let script = connected([
        claude::clean(),
        Reply::new().hear_out(),
        Reply::new(),
        claude::signed_out(),
    ]);
    let mut outside = left_running(&tree, Outside::new(&tree, &script)?, "apart")?;
    let asked = Asked {
        installed: (SHARED, "work"),
        command: &|instance| Command::SignOut { instance },
        asking: Asking::every(),
        after: None,
    };
    let mut picking = Picking::of([Some(client::accounts::SIGN_OUT), None]);
    let before = fake_peer::played(&outside.peer);

    let (outcome, file) = accounting(&tree, &mut outside, &asked, &mut picking)?;

    // The steps here run through the sandbox that never sees a process end,
    // so what the agent's program said is not read: the sign-out is not
    // verified, but it was not refused.
    assert!(
        matches!(
            outcome,
            Outcome::Logout(LogoutOutcome::SignOutUnverified { .. })
        ),
        "{outcome:?}"
    );
    assert_eq!(
        file,
        SHARED.replace(
            r#""executable": "claude"},"#,
            r#""executable": "claude", "enabled": false},"#
        )
    );
    assert_eq!(
        fake_peer::played(&outside.peer) - before,
        2,
        "a logout and a status step"
    );
    Ok(())
}
