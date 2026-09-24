//! Failure regressions exercise the production process owner with per-instance
//! native-operation faults. Rescue cleanup is outside every observed assertion.

use super::super::*;

struct Fixture {
    process: Option<LocalProcess>,
    unreaped: Option<rustix::process::Pid>,
    active: Arc<AtomicUsize>,
}

impl Fixture {
    fn new(stage: Option<Stage>) -> io::Result<Self> {
        let mut command = Command::new("/bin/sh");
        // The owned stdin writer keeps this builtin alive without descendants.
        command.args(["-c", "read line"]);
        let process = testing_local(command, crucible_sandbox::SandboxSpeech::Held, stage)
            .map_err(io::Error::other)?;
        let active = Arc::clone(
            &process
                .reservation
                .as_ref()
                .ok_or_else(|| io::Error::other("fixture has no reservation"))?
                .active,
        );
        Ok(Self {
            process: Some(process),
            unreaped: None,
            active,
        })
    }

    fn process(&mut self) -> io::Result<&mut LocalProcess> {
        self.process
            .as_mut()
            .ok_or_else(|| io::Error::other("fixture has no process"))
    }

    fn lose_unconfirmed_owner(&mut self) -> io::Result<()> {
        let mut process = self
            .process
            .take()
            .ok_or_else(|| io::Error::other("fixture has no process"))?;
        process.test_stop = fail_stop;
        process.test_reap = fail_reap;
        let leader = watched(&process.watched)?.child.id();
        self.unreaped =
            rustix::process::Pid::from_raw(i32::try_from(leader).map_err(io::Error::other)?);
        // Both injected operations leave this child unreaped, so the rescue
        // PID cannot be reused before Fixture::drop reaps its owned child.
        drop(process);
        Ok(())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut process) = self.process.take() {
            // Bypass injected faults and any broken cached cleanup state. This
            // guard also rescues the intentionally failing pre-fix candidate.
            if let Ok(mut watched) = watched(&process.watched) {
                let Watched {
                    child,
                    scope,
                    status,
                    ..
                } = &mut *watched;
                let _ = stop_scope(scope, child);
                let _ = reap(child, status);
            }
            process.test_stop = stop_scope;
            process.test_reap = reap;
        }
        if let Some(pid) = self.unreaped.take() {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            let deadline = Instant::now() + REAP;
            loop {
                match rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::NOHANG) {
                    Ok(None) if Instant::now() < deadline => thread::sleep(SUPERVISE),
                    _ => break,
                }
            }
        }
    }
}

fn fail_stop(_scope: &Scope, _child: &mut Child) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "injected scope stop",
    ))
}

fn fail_reap(_child: &mut Child, _status: &mut Option<ExitStatus>) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "injected leader reap",
    ))
}

fn stage(sample: &crate::sample::Sample) -> io::Result<std::path::PathBuf> {
    let root = sample.root().join("stage");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("marker"), "owned staging data")?;
    Ok(root)
}

#[test]
fn repeated_stage_failure_never_becomes_success() -> io::Result<()> {
    let sample = crate::sample::Sample::new("cleanup-repeated-stage-failure");
    let root = sample.root().join("stage-file");
    std::fs::write(&root, "a regular file cannot be removed as a tree").expect("stage error");
    let mut fixture = Fixture::new(Some(Stage::new(root)))?;
    fixture.process()?.stop().expect_err("first cleanup fails");
    fixture
        .process()?
        .stop()
        .expect_err("unchanged cleanup still fails");
    assert_eq!(
        fixture.process()?.inspection().cleanup(),
        SandboxCleanup::Failed
    );
    assert_eq!(
        fixture.active.load(Ordering::Acquire),
        1,
        "failed cleanup keeps its slot"
    );
    Ok(())
}

#[test]
fn repaired_stage_cleanup_is_retried_and_audited() -> io::Result<()> {
    let sample = crate::sample::Sample::new("cleanup-recovered-stage");
    let root = sample.root().join("stage-file");
    std::fs::write(&root, "stage error").expect("stage error");
    let mut fixture = Fixture::new(Some(Stage::new(root.clone())))?;
    let audit = fixture.process()?.control.audit.clone();
    fixture.process()?.stop().expect_err("first cleanup fails");
    std::fs::remove_file(&root).expect("repair failed resource");
    std::fs::create_dir(&root).expect("recoverable stage");
    fixture
        .process()?
        .stop()
        .expect("retry removes repaired stage");
    assert!(
        !root.exists(),
        "successful retry must actually clean the stage"
    );
    assert_eq!(fixture.active.load(Ordering::Acquire), 0);
    assert_eq!(
        fixture.process()?.inspection().cleanup(),
        SandboxCleanup::Complete
    );
    let cleanup: Vec<_> = audit
        .records()
        .expect("audit")
        .iter()
        .filter_map(|record| {
            if let SandboxFactKind::Cleanup(state) = record.fact().kind() {
                Some(*state)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(cleanup, [SandboxCleanup::Failed, SandboxCleanup::Complete]);
    Ok(())
}

#[test]
fn failed_scope_stop_preserves_staging_and_admission() -> io::Result<()> {
    let sample = crate::sample::Sample::new("cleanup-live-scope");
    let root = stage(&sample)?;
    let mut fixture = Fixture::new(Some(Stage::new(root.clone())))?;
    fixture.process()?.test_stop = fail_stop;
    fixture.process()?.test_reap = fail_reap;
    fixture
        .process()?
        .stop()
        .expect_err("scope remains unconfirmed");
    assert!(
        watched(&fixture.process()?.watched)?
            .child
            .try_wait()
            .expect("owned child state")
            .is_none()
    );
    assert!(
        root.join("marker").exists(),
        "live workload must retain its stage"
    );
    assert_eq!(fixture.active.load(Ordering::Acquire), 1);
    assert!(Reservation::take(Arc::clone(&fixture.active), 1).is_err());
    Ok(())
}

#[test]
fn failed_reap_preserves_staging_and_admission() -> io::Result<()> {
    let sample = crate::sample::Sample::new("cleanup-unreaped-scope");
    let root = stage(&sample)?;
    let mut fixture = Fixture::new(Some(Stage::new(root.clone())))?;
    fixture.process()?.test_reap = fail_reap;
    fixture
        .process()?
        .stop()
        .expect_err("leader remains unconfirmed");
    assert!(
        root.join("marker").exists(),
        "unconfirmed reap retains its stage"
    );
    assert_eq!(fixture.active.load(Ordering::Acquire), 1);
    Ok(())
}

#[test]
fn dropping_an_unconfirmed_scope_keeps_the_service_slot_and_stage() -> io::Result<()> {
    let sample = crate::sample::Sample::new("cleanup-owner-loss");
    let root = stage(&sample)?;
    let mut fixture = Fixture::new(Some(Stage::new(root.clone())))?;
    fixture.lose_unconfirmed_owner()?;
    assert_eq!(
        fixture.active.load(Ordering::Acquire),
        1,
        "Drop must quarantine admission capacity"
    );
    assert!(Reservation::take(Arc::clone(&fixture.active), 1).is_err());
    assert!(
        root.join("marker").exists(),
        "Stage::drop must preserve unconfirmed workload data"
    );
    Ok(())
}

#[test]
fn a_recoverable_scope_failure_is_retried() -> io::Result<()> {
    let mut fixture = Fixture::new(None)?;
    fixture.process()?.test_stop = fail_stop;
    fixture.process()?.test_reap = fail_reap;
    fixture.process()?.stop().expect_err("first stop fails");
    fixture.process()?.test_stop = stop_scope;
    fixture.process()?.test_reap = reap;
    fixture
        .process()?
        .stop()
        .expect("second stop confirms cleanup");
    assert!(
        fixture.process()?.reaped(),
        "retry must really reap the leader"
    );
    assert_eq!(fixture.active.load(Ordering::Acquire), 0);
    Ok(())
}

/// A failure the status task records, as a violation's kill that could not
/// be sent records one.
fn status_task_failure(fixture: &mut Fixture) -> io::Result<()> {
    fixture
        .process()?
        .control
        .record_failure(&io::Error::other("injected status task failure"));
    Ok(())
}

#[test]
fn a_failed_status_task_cannot_claim_complete_cleanup() -> io::Result<()> {
    let mut fixture = Fixture::new(None)?;
    status_task_failure(&mut fixture)?;
    fixture
        .process()?
        .stop()
        .expect_err("stored status task failure");
    assert_eq!(
        fixture.process()?.inspection().cleanup(),
        SandboxCleanup::Failed
    );
    fixture
        .process()?
        .stop()
        .expect_err("stored failure remains visible");
    Ok(())
}

#[test]
fn cached_leader_status_cannot_hide_a_failed_stop() -> io::Result<()> {
    let mut fixture = Fixture::new(None)?;
    status_task_failure(&mut fixture)?;
    fixture
        .process()?
        .stop()
        .expect_err("stop observes the status task's failure");
    assert!(fixture.process()?.reaped(), "the real leader was reaped");
    fixture
        .process()?
        .try_wait()
        .expect_err("cached status cannot erase the failure");
    Ok(())
}

#[test]
fn panicked_cancel_failure_survives_consumed_join() -> io::Result<()> {
    let mut fixture = Fixture::new(None)?;
    let process = fixture.process()?;
    watched(&process.watched)?.cancel =
        Some(thread::spawn(|| panic!("owned cancel panic fixture")));
    process
        .stop()
        .expect_err("joining the panicked thread fails");
    assert!(
        watched(&process.watched)?.cancel.is_none(),
        "the failing join was consumed"
    );
    process
        .stop()
        .expect_err("a consumed join cannot erase its failure");
    process
        .try_wait()
        .expect_err("a reaped leader cannot hide the failed join");
    assert_eq!(process.inspection().cleanup(), SandboxCleanup::Failed);
    Ok(())
}

#[test]
fn a_status_task_gone_before_its_command_is_a_failure_no_status_hides() -> io::Result<()> {
    let mut fixture = Fixture::new(None)?;
    let process = fixture.process()?;
    // What a runtime shut down under the task, or a panic in it, leaves: the
    // task dropped before its command ended, with no stop begun.
    process
        .watch
        .as_ref()
        .ok_or_else(|| io::Error::other("fixture has no status task"))?
        .abort();
    // EOF ends the real shell's read builtin without an injected wait result.
    process.stdin.take();
    let deadline = Instant::now() + REAP;
    loop {
        match process.try_wait() {
            Err(_) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(SUPERVISE),
            other => panic!("expected the lost status task after real exit, got {other:?}"),
        }
    }
    process
        .stop()
        .expect_err("cleanup retains the lost status task");
    assert!(process.reaped(), "the real leader was reaped");
    process
        .try_wait()
        .expect_err("cached status cannot erase the lost status task");
    assert_eq!(process.inspection().cleanup(), SandboxCleanup::Failed);
    Ok(())
}

#[test]
fn a_stop_ends_the_status_task() -> io::Result<()> {
    let mut fixture = Fixture::new(None)?;
    let process = fixture.process()?;
    process.stop()?;
    let deadline = Instant::now() + REAP;
    while !process
        .watch
        .as_ref()
        .is_some_and(tokio::task::JoinHandle::is_finished)
    {
        assert!(
            Instant::now() < deadline,
            "the status task outlived the stop"
        );
        thread::sleep(SUPERVISE);
    }
    Ok(())
}

/// A stop whose scope cleanup and input thread both failed reports both: the
/// scope's failure as the error, and the thread's beside it, so a caller that
/// retries the stop knows the thread was not joined either.
#[test]
fn a_stop_reports_a_failed_input_thread_beside_a_failed_scope() {
    let scope = io::Error::new(io::ErrorKind::PermissionDenied, "scope");
    let input = io::Error::new(io::ErrorKind::TimedOut, "input");

    let both = stopped_with_input(Err(scope), Err(input)).expect_err("two failures");

    assert_eq!(both.kind(), io::ErrorKind::PermissionDenied);
    let said = both.to_string();
    assert!(
        said.contains("PermissionDenied") && said.contains("TimedOut"),
        "{said}"
    );
    let input_only = stopped_with_input(Ok(()), Err(io::Error::from(io::ErrorKind::TimedOut)))
        .expect_err("the input's failure");
    assert_eq!(input_only.kind(), io::ErrorKind::TimedOut);
    stopped_with_input(Ok(()), Ok(())).expect("nothing failed");
}
