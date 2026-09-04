//! Failure regressions exercise the production process owner with per-instance
//! native-operation faults. Rescue cleanup is outside every observed assertion.

use super::*;

struct Fixture {
    process: Option<LocalProcess>,
    unreaped: Option<rustix::process::Pid>,
    active: Arc<AtomicUsize>,
}

impl Fixture {
    fn new(stage: Option<Stage>) -> Self {
        let mut command = Command::new("/bin/sh");
        // The owned stdin writer keeps this builtin alive without descendants.
        command.args(["-c", "read line"]);
        let process = testing_local(command, crucible_core::SandboxSpeech::Held, stage)
            .expect("production process fixture");
        let active = Arc::clone(&process.reservation.as_ref().expect("reservation").active);
        Self {
            process: Some(process),
            unreaped: None,
            active,
        }
    }

    fn process(&mut self) -> &mut LocalProcess {
        self.process.as_mut().expect("owned process")
    }

    fn lose_unconfirmed_owner(&mut self) {
        let mut process = self.process.take().expect("owned process");
        process.test_stop = fail_stop;
        process.test_reap = fail_reap;
        self.unreaped = rustix::process::Pid::from_raw(
            i32::try_from(process.child.id()).expect("fixture process identifier"),
        );
        // Both injected operations leave this child unreaped, so the rescue
        // PID cannot be reused before Fixture::drop reaps its owned child.
        drop(process);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut process) = self.process.take() {
            // Bypass injected faults and any broken cached cleanup state. This
            // guard also rescues the intentionally failing pre-fix candidate.
            let _ = stop_scope(&process.scope, &mut process.child);
            let _ = reap(&mut process.child, &mut process.status);
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

fn stage(sample: &crate::sample::Sample) -> std::path::PathBuf {
    let root = sample.root().join("stage");
    std::fs::create_dir(&root).expect("stage directory");
    std::fs::write(root.join("marker"), "owned staging data").expect("stage marker");
    root
}

#[test]
fn repeated_stage_failure_never_becomes_success() {
    let sample = crate::sample::Sample::new("cleanup-repeated-stage-failure");
    let root = sample.root().join("stage-file");
    std::fs::write(&root, "a regular file cannot be removed as a tree").expect("stage error");
    let mut fixture = Fixture::new(Some(Stage::new(root)));
    fixture.process().stop().expect_err("first cleanup fails");
    fixture
        .process()
        .stop()
        .expect_err("unchanged cleanup still fails");
    assert_eq!(
        fixture.process().inspection().cleanup(),
        SandboxCleanup::Failed
    );
    assert_eq!(
        fixture.active.load(Ordering::Acquire),
        1,
        "failed cleanup keeps its slot"
    );
}

#[test]
fn repaired_stage_cleanup_is_retried_and_audited() {
    let sample = crate::sample::Sample::new("cleanup-recovered-stage");
    let root = sample.root().join("stage-file");
    std::fs::write(&root, "stage error").expect("stage error");
    let mut fixture = Fixture::new(Some(Stage::new(root.clone())));
    let audit = fixture.process().control.audit.clone();
    fixture.process().stop().expect_err("first cleanup fails");
    std::fs::remove_file(&root).expect("repair failed resource");
    std::fs::create_dir(&root).expect("recoverable stage");
    fixture
        .process()
        .stop()
        .expect("retry removes repaired stage");
    assert!(
        !root.exists(),
        "successful retry must actually clean the stage"
    );
    assert_eq!(fixture.active.load(Ordering::Acquire), 0);
    assert_eq!(
        fixture.process().inspection().cleanup(),
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
}

#[test]
fn failed_scope_stop_preserves_staging_and_admission() {
    let sample = crate::sample::Sample::new("cleanup-live-scope");
    let root = stage(&sample);
    let mut fixture = Fixture::new(Some(Stage::new(root.clone())));
    fixture.process().test_stop = fail_stop;
    fixture.process().test_reap = fail_reap;
    fixture
        .process()
        .stop()
        .expect_err("scope remains unconfirmed");
    assert!(
        fixture
            .process()
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
}

#[test]
fn failed_reap_preserves_staging_and_admission() {
    let sample = crate::sample::Sample::new("cleanup-unreaped-scope");
    let root = stage(&sample);
    let mut fixture = Fixture::new(Some(Stage::new(root.clone())));
    fixture.process().test_reap = fail_reap;
    fixture
        .process()
        .stop()
        .expect_err("leader remains unconfirmed");
    assert!(
        root.join("marker").exists(),
        "unconfirmed reap retains its stage"
    );
    assert_eq!(fixture.active.load(Ordering::Acquire), 1);
}

#[test]
fn dropping_an_unconfirmed_scope_keeps_the_service_slot_and_stage() {
    let sample = crate::sample::Sample::new("cleanup-owner-loss");
    let root = stage(&sample);
    let mut fixture = Fixture::new(Some(Stage::new(root.clone())));
    fixture.lose_unconfirmed_owner();
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
}

#[test]
fn a_recoverable_scope_failure_is_retried() {
    let mut fixture = Fixture::new(None);
    fixture.process().test_stop = fail_stop;
    fixture.process().test_reap = fail_reap;
    fixture.process().stop().expect_err("first stop fails");
    fixture.process().test_stop = stop_scope;
    fixture.process().test_reap = reap;
    fixture
        .process()
        .stop()
        .expect("second stop confirms cleanup");
    assert!(
        fixture.process().status.is_some(),
        "retry must really reap the leader"
    );
    assert_eq!(fixture.active.load(Ordering::Acquire), 0);
}

fn supervisor_failure(fixture: &mut Fixture) {
    let process = fixture.process();
    process.supervisor = Some(
        Supervisor::start(
            Arc::clone(&process.control),
            process.terminator,
            None,
            None,
            process.child.id(),
        )
        .expect("real supervisor"),
    );
    process
        .control
        .record_failure(&io::Error::other("injected supervisor failure"));
}

#[test]
fn joining_a_failed_supervisor_cannot_claim_complete_cleanup() {
    let mut fixture = Fixture::new(None);
    supervisor_failure(&mut fixture);
    fixture
        .process()
        .stop()
        .expect_err("stored supervisor failure");
    assert!(
        fixture
            .process()
            .supervisor
            .as_ref()
            .expect("supervisor")
            .thread
            .is_none()
    );
    assert_eq!(
        fixture.process().inspection().cleanup(),
        SandboxCleanup::Failed
    );
    fixture
        .process()
        .stop()
        .expect_err("stored failure remains visible");
}

#[test]
fn cached_leader_status_cannot_hide_a_failed_stop() {
    let mut fixture = Fixture::new(None);
    supervisor_failure(&mut fixture);
    fixture
        .process()
        .stop()
        .expect_err("stop observes supervisor failure");
    assert!(
        fixture.process().status.is_some(),
        "the real leader was reaped"
    );
    fixture
        .process()
        .try_wait()
        .expect_err("cached status cannot erase the failure");
}
