//! A command's status answers at once while the cancel of a violation runs,
//! its stop ends within a bound whatever that cancel does, and dropping it
//! leaves nothing it started running.

use super::*;

use crucible_sandbox::SandboxSpeech;

/// How long the cancel below takes: long enough that a status or a stop which
/// waited behind it cannot pass for one that did not.
const CANCEL_TAKES: Duration = Duration::from_secs(3);

/// How long a status may take to answer. A look at the leader is a few system
/// calls; anything near this bound waited on something.
const ANSWERS_WITHIN: Duration = Duration::from_millis(250);

/// How long a stop may take while a cancel it cannot shorten runs: its kill,
/// its reap and its wait for the cancel, each bounded, with room for a loaded
/// machine.
const STOPS_WITHIN: Duration = Duration::from_millis(1500);

/// A command that runs until it is stopped, and says nothing.
#[cfg(unix)]
fn lingering() -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "sleep 30"]);
    command
}

/// A command that runs until it is stopped, and says nothing.
#[cfg(windows)]
fn lingering() -> Command {
    let mut command = Command::new("cmd.exe");
    std::os::windows::process::CommandExt::raw_arg(
        command.args(["/d", "/c"]),
        "for /l %i in (0,0,1) do @rem",
    );
    command
}

/// What the cancel below has done: begun, and ended.
#[derive(Default)]
struct Cancelling {
    begun: AtomicBool,
    ended: AtomicBool,
}

impl Cancelling {
    /// Waits, within `bound`, for `flag` to be raised.
    fn waited(flag: &AtomicBool, bound: Duration) -> bool {
        let deadline = Instant::now() + bound;
        while !flag.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(SUPERVISE);
        }
        true
    }
}

/// A command cut at once by its command-time limit, whose backend's cancel
/// takes [`CANCEL_TAKES`] and does nothing to it: only the kill after the
/// cancel ends it.
fn cut_slowly(
    cancelling: &Arc<Cancelling>,
) -> Result<LocalProcess, crucible_sandbox::SandboxError> {
    let mut plan = testing_plan(SandboxSpeech::Closed, None)?;
    plan.limits.command_time = Some(Duration::from_millis(1));
    let seen = Arc::clone(cancelling);
    plan.canceller = Some(Box::new(move |_leader| {
        seen.begun.store(true, Ordering::Release);
        thread::sleep(CANCEL_TAKES);
        seen.ended.store(true, Ordering::Release);
        Ok(())
    }));
    spawn_local(lingering(), plan)
}

#[test]
fn a_status_asked_while_a_violation_is_cancelled_answers_at_once() {
    let cancelling = Arc::new(Cancelling::default());
    let mut process = cut_slowly(&cancelling).expect("a command");
    assert!(
        Cancelling::waited(&cancelling.begun, Duration::from_secs(10)),
        "the violation's cancel never began"
    );

    let asked = Instant::now();
    let status = process.try_wait();
    let took = asked.elapsed();

    assert!(
        took < ANSWERS_WITHIN,
        "the status waited {took:?} behind the violation's cancel"
    );
    assert!(
        matches!(status, Ok(None)),
        "a command the cancel has not ended yet is still running: {status:?}"
    );
    assert_eq!(process.violation(), Some(SandboxViolation::CommandTime));

    // The kill after the cancel is what ends it, and its end is then seen.
    let deadline = Instant::now() + CANCEL_TAKES + Duration::from_secs(5);
    loop {
        match process.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(SUPERVISE),
            other => panic!("the killed command's end was not seen: {other:?}"),
        }
    }
    process.stop().expect("cleanup");
}

#[test]
fn a_stop_while_a_violation_is_cancelled_ends_within_its_bound() {
    let cancelling = Arc::new(Cancelling::default());
    let mut process = cut_slowly(&cancelling).expect("a command");
    assert!(
        Cancelling::waited(&cancelling.begun, Duration::from_secs(10)),
        "the violation's cancel never began"
    );

    let asked = Instant::now();
    let stopped = process.stop();
    let took = asked.elapsed();

    assert!(
        took < STOPS_WITHIN,
        "the stop waited {took:?} behind the violation's cancel"
    );
    assert!(
        !cancelling.ended.load(Ordering::Acquire),
        "the cancel ended before the stop, which then proves nothing"
    );
    // The cancel is still running on a thread the process started, so its
    // cleanup is not yet confirmed, however dead the command is.
    assert!(
        stopped.is_err(),
        "a stop cannot confirm cleanup while its cancel still runs"
    );
    assert_eq!(process.inspection().cleanup(), SandboxCleanup::Failed);
    assert!(
        matches!(process.try_wait(), Ok(Some(_))),
        "the stop killed and reaped the command"
    );

    assert!(
        Cancelling::waited(&cancelling.ended, CANCEL_TAKES + Duration::from_secs(5)),
        "the cancel never ended"
    );
    process
        .stop()
        .expect("once the cancel has ended, a stop confirms cleanup");
    assert_eq!(process.inspection().cleanup(), SandboxCleanup::Complete);
}

/// A shell that starts a sleeper of its own in the background, prints the
/// sleeper's process id, and waits for it.
#[cfg(unix)]
fn with_a_descendant() -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "sleep 30 & echo $!; wait"]);
    command
}

/// Whether `pid` still names a running process. A zombie waiting for its new
/// parent to collect it is not running.
#[cfg(target_os = "linux")]
fn running(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat.split_whitespace().nth(2) != Some("Z"),
        Err(_) => false,
    }
}

/// Whether `pid` still names a running process: one this user may signal.
#[cfg(all(unix, not(target_os = "linux")))]
fn running(pid: u32) -> bool {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

/// Reads `output` until it holds a line, within `bound`.
#[cfg(unix)]
fn first_line(output: &mut dyn SandboxOutput, bound: Duration) -> io::Result<String> {
    let deadline = Instant::now() + bound;
    let mut said = Vec::new();
    let mut buffer = [0_u8; 64];
    while !said.contains(&b'\n') {
        if Instant::now() >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        match output.read_ready(&mut buffer)? {
            SandboxRead::Bytes(count)
            | SandboxRead::Limited {
                retained: count, ..
            } => said.extend_from_slice(
                buffer
                    .get(..count)
                    .ok_or_else(|| io::Error::other("more bytes reported than read"))?,
            ),
            SandboxRead::Pending => thread::sleep(SUPERVISE),
            SandboxRead::End => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
        }
    }
    String::from_utf8(said).map_err(io::Error::other)
}

#[cfg(unix)]
#[test]
fn dropping_a_command_leaves_no_descendant_running() {
    let mut plan = testing_plan(SandboxSpeech::Closed, None).expect("a plan");
    plan.limits.command_time = Some(Duration::from_mins(1));
    let mut process = spawn_local(with_a_descendant(), plan).expect("a command");
    let mut output = process.take_stdout().expect("its output");
    let pid: u32 = first_line(output.as_mut(), Duration::from_secs(10))
        .expect("the descendant's id")
        .trim()
        .parse()
        .expect("a process id");
    assert!(running(pid), "the descendant was not started");

    let dropped = Instant::now();
    drop(process);
    assert!(
        dropped.elapsed() < STOPS_WITHIN,
        "dropping the command took {:?}",
        dropped.elapsed()
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while running(pid) {
        assert!(
            Instant::now() < deadline,
            "descendant {pid} outlived the dropped command"
        );
        thread::sleep(SUPERVISE);
    }
    drop(output);
}

/// A `cmd` that starts another in the background and then runs until it is
/// stopped. The descendant writes `alive.txt` in its working directory as soon
/// as it runs, and `survived.txt` about three seconds later.
#[cfg(windows)]
fn with_a_descendant() -> Command {
    let mut command = Command::new("cmd.exe");
    std::os::windows::process::CommandExt::raw_arg(
        command.args(["/d", "/c"]),
        "start \"\" /b cmd /d /c \"echo alive>alive.txt & ping -n 4 127.0.0.1 >NUL & echo survived>survived.txt\" & ping -n 60 127.0.0.1 >NUL",
    );
    command
}

#[cfg(windows)]
#[test]
fn dropping_a_command_leaves_no_descendant_running() {
    let sample = crate::sample::Sample::new("process-dropped-descendant");
    let mut plan = testing_plan(SandboxSpeech::Closed, None).expect("a plan");
    plan.limits.command_time = Some(Duration::from_mins(1));
    let mut command = with_a_descendant();
    command.current_dir(sample.root());
    let process = spawn_local(command, plan).expect("a command");

    // The descendant is running: it has written the first of its two files.
    let alive = sample.root().join("alive.txt");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !alive.exists() {
        assert!(Instant::now() < deadline, "the descendant never ran");
        thread::sleep(SUPERVISE);
    }
    assert!(
        !sample.root().join("survived.txt").exists(),
        "the descendant finished before the command was dropped"
    );

    let dropped = Instant::now();
    drop(process);
    assert!(
        dropped.elapsed() < STOPS_WITHIN,
        "dropping the command took {:?}",
        dropped.elapsed()
    );

    // Past the moment the descendant would have written its second file, had
    // it lived.
    thread::sleep(Duration::from_secs(6));
    assert!(
        !sample.root().join("survived.txt").exists(),
        "a descendant outlived the dropped command"
    );
}
