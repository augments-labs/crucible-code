//! Real startup failures use owned children and staging roots. The native stop
//! fault leaves its child unreaped; rescue runs after the bounded observation.

use super::super::*;

fn full_audit(plan: &SpawnPlan) -> io::Result<()> {
    for _ in 0..crucible_core::MAX_SANDBOX_AUDIT_FACTS {
        plan.audit
            .record(
                plan.sandbox,
                SandboxFactKind::Lifecycle(SandboxLifecycle::Prepared),
            )
            .map_err(io::Error::other)?;
    }
    Ok(())
}

fn fail_stop(_scope: &Scope, _child: &mut Child) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "injected startup scope stop",
    ))
}

#[test]
fn startup_stop_failure_is_bounded_and_quarantines_owned_resources() -> io::Result<()> {
    let sample = crate::sample::Sample::new("startup-stop-failure");
    let root = sample.root().join("stage");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("payload"), "owned staging data")?;
    let marker = sample.root().join("owned-child-pid");
    let plan = testing_plan(
        crucible_core::SandboxSpeech::Held,
        Some(Stage::new(root.clone())),
    )
    .map_err(io::Error::other)?;
    full_audit(&plan)?;
    let active = Arc::clone(&plan.reservation.active);
    // The fixture itself expires if setup fails before rescue learns its PID.
    // This timeout exceeds the asserted startup bound and is not cleanup proof.
    let mut command = Command::new("/bin/bash");
    command
        .args([
            "-c",
            "printf '%s' \"$$\" > \"$1\"; read -t 5 line",
            "fixture",
        ])
        .arg(&marker);
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let result = spawn_inner(command, plan, fail_stop).map(drop);
        let _ = send.send(result);
    });
    // The current bug blocks in wait while its stdin writer keeps read alive.
    // The fixed owner returns promptly without asserting that this child died.
    let returned = receive.recv_timeout(Duration::from_secs(1));
    let bounded = returned.is_ok();
    let deadline = Instant::now() + Duration::from_secs(3);
    let pid = loop {
        if let Ok(raw) = std::fs::read_to_string(&marker)
            && let Ok(raw) = raw.parse::<i32>()
            && let Some(pid) = rustix::process::Pid::from_raw(raw)
        {
            break Some(pid);
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::sleep(SUPERVISE);
    };
    // This PID belongs to our still-unreaped child; it cannot be reused before
    // either the baseline waiter or this rescue reaps it. Signal exactly once.
    if let Some(pid) = pid {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
    }
    let result = match returned {
        Ok(result) => result,
        Err(_) => receive
            .recv_timeout(Duration::from_secs(2))
            .map_err(io::Error::other)?,
    };
    worker
        .join()
        .map_err(|_| io::Error::other("startup fixture panicked"))?;
    if let Some(pid) = pid {
        let deadline = Instant::now() + REAP;
        loop {
            match rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::NOHANG) {
                Ok(None) if Instant::now() < deadline => thread::sleep(SUPERVISE),
                _ => break,
            }
        }
    }
    assert!(pid.is_some(), "fixture must identify its owned child");
    assert_eq!(
        (
            bounded,
            matches!(result, Err(crucible_core::SandboxError::Lifecycle(_))),
            root.join("payload").exists(),
            active.load(Ordering::Acquire),
        ),
        (true, true, true, 1),
        "startup cleanup must return within its bound and retain unknown-live resources: {result:?}"
    );
    Ok(())
}

#[test]
fn startup_audit_failure_with_proved_cleanup_releases_resources() -> io::Result<()> {
    let sample = crate::sample::Sample::new("startup-audit-cleanup");
    let root = sample.root().join("stage");
    std::fs::create_dir(&root)?;
    let plan = testing_plan(
        crucible_core::SandboxSpeech::Held,
        Some(Stage::new(root.clone())),
    )
    .map_err(io::Error::other)?;
    full_audit(&plan)?;
    let active = Arc::clone(&plan.reservation.active);
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "read line"]);
    let result = spawn_local(command, plan);
    assert!(matches!(result, Err(crucible_core::SandboxError::Audit(_))));
    assert!(!root.exists());
    assert_eq!(active.load(Ordering::Acquire), 0);
    Ok(())
}

#[test]
fn startup_spawn_failure_preserves_failed_stage_cleanup_and_capacity() -> io::Result<()> {
    let sample = crate::sample::Sample::new("startup-stage-failure");
    let root = sample.root().join("stage-file");
    std::fs::write(&root, "a regular file is not a staging tree")?;
    let plan = testing_plan(
        crucible_core::SandboxSpeech::Closed,
        Some(Stage::new(root.clone())),
    )
    .map_err(io::Error::other)?;
    let active = Arc::clone(&plan.reservation.active);
    let result = spawn_local(Command::new(sample.root().join("absent-program")), plan);
    assert!(matches!(
        result,
        Err(crucible_core::SandboxError::Lifecycle(_))
    ));
    assert!(root.exists());
    assert_eq!(active.load(Ordering::Acquire), 1);
    Ok(())
}
