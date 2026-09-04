//! Real startup failures use owned children and staging roots. Rescue holds a
//! pidfd acquired before any wait, including when testing the broken startup path.

use super::super::*;

use std::os::fd::{AsFd as _, OwnedFd};

thread_local! {
    // The hook and registration run on the same worker. Other parallel tests
    // have separate slots, and only the first stop attempt transfers a handle.
    static RESCUE: std::cell::RefCell<Option<std::sync::mpsc::SyncSender<io::Result<OwnedFd>>>> = const { std::cell::RefCell::new(None) };
}

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

fn fail_stop(_scope: &Scope, child: &mut Child) -> io::Result<()> {
    RESCUE.with_borrow_mut(|rescue| {
        if let Some(send) = rescue.take() {
            // The caller has not waited or reaped this owned Child. Capturing
            // identity here remains safe even if the broken path reaps it next.
            let handle = i32::try_from(child.id())
                .ok()
                .and_then(rustix::process::Pid::from_raw)
                .ok_or_else(|| io::Error::other("fixture child PID is invalid"))
                .and_then(|pid| {
                    rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty())
                        .map_err(io::Error::from)
                });
            let _ = send.send(handle);
        }
    });
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
    let plan = testing_plan(
        crucible_core::SandboxSpeech::Held,
        Some(Stage::new(root.clone())),
    )
    .map_err(io::Error::other)?;
    full_audit(&plan)?;
    let active = Arc::clone(&plan.reservation.active);
    // This independent expiry bounds even the broken wait path if the test
    // worker cannot hand over a pidfd. It is not evidence of successful cleanup.
    let mut command = Command::new("/bin/bash");
    command.args(["-c", "read -t 5 line"]);
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let (rescue_send, rescue_receive) = std::sync::mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        RESCUE.with_borrow_mut(|slot| *slot = Some(rescue_send));
        let result = spawn_inner(command, plan, fail_stop).map(drop);
        let _ = send.send(result);
    });
    // The current bug blocks in wait while its stdin writer keeps read alive.
    // The fixed owner returns promptly without asserting that this child died.
    let returned = receive.recv_timeout(Duration::from_secs(1));
    let bounded = returned.is_ok();
    let rescue = rescue_receive
        .recv_timeout(Duration::from_secs(1))
        .map_err(io::Error::other)
        .and_then(std::convert::identity);
    if let Ok(handle) = &rescue {
        // ESRCH is safe: the captured process may already have exited/reaped.
        // Unlike a numeric PID, this descriptor cannot select its replacement.
        let _ = rustix::process::pidfd_send_signal(handle, rustix::process::Signal::KILL);
    }
    let result = match returned {
        Ok(result) => result,
        Err(_) => receive
            .recv_timeout(Duration::from_secs(6))
            .map_err(io::Error::other)?,
    };
    worker
        .join()
        .map_err(|_| io::Error::other("startup fixture panicked"))?;
    let handle = rescue?;
    let deadline = Instant::now() + REAP;
    loop {
        match rustix::process::waitid(
            rustix::process::WaitId::PidFd(handle.as_fd()),
            rustix::process::WaitIdOptions::EXITED | rustix::process::WaitIdOptions::NOHANG,
        ) {
            Ok(Some(_)) | Err(rustix::io::Errno::CHILD) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(SUPERVISE),
            Ok(None) => return Err(io::Error::other("fixture child did not exit")),
            Err(problem) => return Err(problem.into()),
        }
    }
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
