//! What a call leaves behind once its output has been collected: nothing.
//!
//! A command's output is read by tasks the call starts and awaits, so each of
//! these runs a call on a runtime of one thread that nothing else shares, and
//! looks at that runtime once the call is over. A flood, a stall, a call
//! dropped part way and a descendant holding the pipes each leave no task
//! alive there, and each call but the flood, whose command is stopped too
//! soon to measure, hands that one thread back while its command runs. The
//! call dropped part way is driven through the wait itself, so that what it
//! asks of its command can be seen.

use std::ffi::OsString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crucible_runtime::{BoxFuture, Cancel};
use crucible_sandbox::{
    SandboxCommand, SandboxEnvironment, SandboxInspection, SandboxManifest, SandboxOutput,
    SandboxPolicy, SandboxProcess, SandboxRequest, SandboxService, SandboxUsage, SandboxViolation,
};
use crucible_tools::{ToolContext, Unwatched};
use crucible_types::{Ancestry, SandboxId, ToolId};

use super::super::{Bash, output};
use super::{Tool, ToolError, ToolOutput, alone, compatible, quiesced};
use crate::sample::{Sample, allowed};

/// What a call answered on a runtime of one thread, and how many times a
/// clock beside it on that thread ticked while it ran.
struct Beside {
    answered: Result<ToolOutput, ToolError>,
    ticks: usize,
}

/// How often the clock beside a call ticks.
const TICKING: Duration = Duration::from_millis(10);

/// Runs `args` on `runtime` beside a clock, under `context`.
///
/// The clock is a task on the same one thread, so it ticks only while the
/// call has handed that thread back: a collector that kept the thread until
/// the command ended leaves it at nothing, however long the command ran.
fn beside_a_clock(
    runtime: &tokio::runtime::Runtime,
    tool: &Bash,
    context: &ToolContext<'_>,
    args: &str,
) -> Beside {
    let ticks = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&ticks);
    let clock = runtime.spawn(async move {
        loop {
            tokio::time::sleep(TICKING).await;
            counting.fetch_add(1, Ordering::Relaxed);
        }
    });
    let answered = runtime.block_on(tool.run(allowed(tool, args), context));
    clock.abort();
    Beside {
        answered,
        ticks: ticks.load(Ordering::Relaxed),
    }
}

/// Asserts the clock beside a call went on ticking while it ran.
fn handed_back(beside: &Beside) {
    assert!(
        beside.ticks >= 3,
        "the call held the thread polling it while the command ran: {} tick(s)",
        beside.ticks
    );
}

/// A command admitted the way the tool admits one, through this machine's
/// confinement with confinement switched off, and handed back inside
/// [`Recorded`].
fn admitted(sample: &Sample, line: &str, stopped: &Arc<AtomicBool>) -> Box<dyn SandboxProcess> {
    let policy = SandboxPolicy::standard(&sample.workspace())
        .expect("a standard policy for the fixture workspace")
        .with_enabled(false);
    let command = SandboxCommand::new(
        super::super::shell::find(|name| std::env::var_os(name)).expect("a POSIX shell"),
        [OsString::from("-c"), OsString::from(line)],
        SandboxEnvironment::new(std::iter::empty::<(&str, &std::ffi::OsStr)>())
            .expect("the command environment"),
    )
    .expect("the sandbox command");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("bash"),
        policy,
        SandboxManifest::empty(),
    );
    let mut session = crucible_runtime::answered!(crate::sample::sandbox().prepare(request))
        .expect("a prepared session");
    crucible_runtime::answered!(session.materialize()).expect("an empty manifest");
    let process = crucible_runtime::answered!(session.start(command)).expect("the child started");
    Box::new(Recorded {
        inner: process,
        stopped: Arc::clone(stopped),
    })
}

/// A command whose stop is recorded before it is passed on.
///
/// Its backend ends it again when it is dropped, whoever held it, so that a
/// command outlives nothing is not by itself proof that the wait ended it:
/// the record is what says the wait did, before letting go of it.
struct Recorded {
    inner: Box<dyn SandboxProcess>,
    stopped: Arc<AtomicBool>,
}

impl SandboxProcess for Recorded {
    fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
        self.inner.take_stdin()
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.inner.take_stdout()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.inner.take_stderr()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.inner.try_wait()
    }

    fn ended(&mut self) -> bool {
        self.inner.ended()
    }

    fn stop(&mut self) -> BoxFuture<'_, std::io::Result<()>> {
        self.stopped.store(true, Ordering::Release);
        self.inner.stop()
    }

    fn inspection(&self) -> &SandboxInspection {
        self.inner.inspection()
    }

    fn usage(&self) -> SandboxUsage {
        self.inner.usage()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        self.inner.violation()
    }
}

#[test]
fn a_call_dropped_while_its_command_runs_ends_it_and_leaves_nothing_behind() {
    // Its output is collected by tasks the wait starts and awaits, so the
    // wait hands the thread polling it back while the command runs, and is
    // dropped there when whoever awaits it stops waiting: a lone call's own
    // deadline does exactly that. The one thread of this runtime is the proof
    // of the first: a wait that kept it until the command ended would let no
    // timer fire before the sleep below was over. Dropped, the wait ends the
    // command — asked of the command itself, and seen in a descendant that
    // never gets to write its marker — and gives up on its readers, and
    // nothing it started is left on the runtime.
    let sample = Sample::new("bash-owned-dropped");
    let stopped = Arc::new(AtomicBool::new(false));
    let process = admitted(
        &sample,
        "printf begun; (sleep 1; touch outlived) & sleep 5",
        &stopped,
    );
    let runtime = alone();
    let cancel = Cancel::new();

    let started = Instant::now();
    let answered = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_millis(300),
            output::collect(
                process,
                &output::Waiting {
                    allowed: Duration::from_secs(30),
                    cancel: &cancel,
                    watch: &Unwatched,
                    leaving: None,
                },
            ),
        )
        .await
    });

    assert!(
        answered.is_err(),
        "the wait held the thread polling it until the command ended"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the wait was not dropped until the command ended"
    );
    assert!(
        stopped.load(Ordering::Acquire),
        "the dropped wait let go of its command without ending it"
    );
    quiesced(&runtime, "the call was dropped");
    thread::sleep(Duration::from_secs(2));
    assert!(
        !sample.root().join("outlived").exists(),
        "a descendant of the command outlived the dropped call"
    );
}

#[test]
fn a_command_that_floods_its_pipes_leaves_nothing_behind_once_stopped() {
    // Both pipes, far past what the call keeps and past the ceiling that
    // stops the command: the readers drain as fast as it writes, and what the
    // call answers is bounded however much went through them.
    let sample = Sample::new("bash-owned-flood");
    let tool = compatible(&sample);
    let runtime = alone();

    let beside = beside_a_clock(
        &runtime,
        &tool,
        &crate::sample::context(),
        r#"{"command":"yes flood 1>&2 & yes flood","timeout":5}"#,
    );

    let output = beside.answered.expect("the command was stopped, not lost");
    assert!(output.is_failed(), "{}", output.text());
    assert!(output.text().contains("[stopped: "), "{}", output.text());
    assert!(
        output.text().len() <= crate::bound::OUTPUT,
        "the answer outgrew its ceiling: {} bytes",
        output.text().len()
    );
    quiesced(&runtime, "a flood was stopped");
}

#[test]
fn a_command_that_stalls_until_the_turn_is_stopped_leaves_nothing_behind() {
    // Something printed, and then nothing for as long as anybody would wait:
    // the readers wait on quiet pipes, and the call waits beside them without
    // holding its thread, until the user stops the turn.
    let sample = Sample::new("bash-owned-stall");
    let tool = compatible(&sample);
    let runtime = alone();
    let cancel = Cancel::new();
    let stopper = cancel.clone();
    let stopping = thread::spawn(move || {
        thread::sleep(Duration::from_millis(500));
        stopper.request();
    });

    let beside = beside_a_clock(
        &runtime,
        &tool,
        &crate::sample::cancelled_by(&cancel),
        r#"{"command":"printf begun; sleep 30"}"#,
    );
    stopping.join().expect("the turn was stopped");

    assert!(
        matches!(beside.answered, Err(ToolError::Cancelled(ref tool)) if &**tool == "bash"),
        "{:?}",
        beside.answered
    );
    handed_back(&beside);
    quiesced(&runtime, "a stalled command's turn was stopped");
}

#[test]
fn a_command_whose_descendant_holds_its_pipes_leaves_nothing_behind() {
    // The shell exits while something it started still holds both pipes.
    // Ending what it left running is what lets the readers reach the end, and
    // once the call has answered, neither they nor the descendant are left.
    let sample = Sample::new("bash-owned-descendant");
    let tool = compatible(&sample);
    let runtime = alone();

    let beside = beside_a_clock(
        &runtime,
        &tool,
        &crate::sample::context(),
        r#"{"command":"(sleep 30 &) ; printf started; sleep 1"}"#,
    );

    handed_back(&beside);
    let output = beside.answered.expect("the command ran");
    assert_eq!(output.text(), "started");
    quiesced(
        &runtime,
        "a command whose descendant held its pipes answered",
    );
}
