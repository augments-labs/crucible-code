//! Commands that can write, run side by side, and how each one's ending completes.
//!
//! The lock a publication holds is the only thing two writers share, so these
//! hold it themselves, the way another command's publication would, and watch a
//! writer end while it is held: waiting its turn, stopped, killed, refused, or
//! prepared across it. A test takes the lock only after its own writer has
//! started, because preparation waits for it too.

use std::process::ExitStatus;
use std::thread;
use std::time::{Duration, Instant};

use crucible_sandbox::{
    SandboxCleanup, SandboxFilesystemAccess, SandboxFilesystemRule, SandboxLifecycle,
    SandboxManifest, SandboxNetworkPolicy, SandboxPolicy, SandboxProcess, SandboxRequest,
    SandboxResourceLimits, SandboxService,
};
use crucible_types::{Ancestry, SandboxId, ToolId};

use super::tests::{command, finish, lifecycles, request};
use crate::LocalSandbox;
use crate::sample::{Sample, skipped_without_enforcement};

/// This user's publication lock, held the way a publication in progress holds it.
///
/// Retried rather than taken once: a publication from another test may hold it
/// for a moment, and a single attempt would make that a failure of this one.
fn held_publication(sample: &Sample) -> super::transaction::Lease {
    let state = super::transaction::state_directory(&request(sample, SandboxManifest::empty()))
        .expect("transaction state");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(lease) =
            super::transaction::Lease::try_acquire_in(&state).expect("the publication lock")
        {
            return lease;
        }
        assert!(
            Instant::now() < deadline,
            "another publication held the lock throughout"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

/// Lets a command started spoken to, and waiting on `read go`, go on.
fn let_go(process: &mut dyn SandboxProcess) {
    let mut input = process.take_stdin().expect("the command's input");
    std::io::Write::write_all(&mut input, b"go\n").expect("the command is let go");
}

/// How `process` ended, once it has, within `patience`.
fn ended_within(process: &mut dyn SandboxProcess, patience: Duration) -> ExitStatus {
    let deadline = Instant::now() + patience;
    loop {
        match process.try_wait() {
            Ok(Some(status)) => return status,
            Ok(None) => {}
            Err(problem) => panic!("the command's ending went wrong: {problem}"),
        }
        assert!(
            Instant::now() < deadline,
            "the command did not end within {patience:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_writer_left_running_does_not_keep_another_from_writing() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let running = Sample::new("sandbox-writer-left-running");
    let beside = Sample::new("sandbox-writer-beside-it");
    let mut first = service
        .prepare(request(&running, SandboxManifest::empty()))
        .expect("first writer");
    first.materialize().expect("first writer materialized");
    let mut held = first
        .start(command("sleep 30"))
        .expect("first writer running");

    let mut second = service
        .prepare(request(&beside, SandboxManifest::empty()))
        .expect("a second writer is prepared while the first runs");
    second.materialize().expect("second writer materialized");
    let (status, _, _) = finish(
        second
            .start(command("printf 'beside\\n' > beside.txt"))
            .expect("second writer started"),
    );

    assert!(status.success(), "{status}");
    assert_eq!(
        std::fs::read_to_string(beside.root().join("beside.txt")).expect("published file"),
        "beside\n"
    );
    assert!(
        held.try_wait().expect("first writer").is_none(),
        "the first writer ended before the second published"
    );
    held.stop().expect("first writer cleanup");
}

#[test]
fn a_writer_that_ends_while_another_publishes_waits_its_turn() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-writer-waits-its-turn");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("a writer");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command("read go; printf 'after\\n' > after.txt").spoken_to())
        .expect("started command");
    // Held once the writer has started, the way another command's publication
    // would be: a writer is only ever prepared between publications.
    let publishing = held_publication(&sample);
    let_go(process.as_mut());
    let held_until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < held_until {
        assert!(
            process.try_wait().expect("wait").is_none(),
            "a writer published while another held the lock"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!sample.root().join("after.txt").exists());

    drop(publishing);
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = process.try_wait().expect("wait") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the writer never published once the lock was free"
        );
        thread::sleep(Duration::from_millis(10));
    };
    process.stop().expect("cleanup");
    assert!(status.success(), "{status}");
    assert_eq!(
        std::fs::read_to_string(sample.root().join("after.txt")).expect("published file"),
        "after\n"
    );
}

#[test]
fn of_two_writers_into_one_root_the_one_that_ends_later_publishes_nothing() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-writers-into-one-root");
    let slower_request = request(&sample, SandboxManifest::empty());
    let audit = slower_request.audit().clone();
    let mut slower = service.prepare(slower_request).expect("a slower writer");
    slower.materialize().expect("slower writer materialized");
    let mut late = slower
        .start(command("read go; printf 'late\\n' > late.txt").spoken_to())
        .expect("slower writer started");
    let mut faster = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("a faster writer into the same root");
    faster.materialize().expect("faster writer materialized");
    let (status, _, _) = finish(
        faster
            .start(command("printf 'early\\n' > early.txt"))
            .expect("faster writer started"),
    );
    assert!(status.success(), "{status}");

    let mut input = late.take_stdin().expect("the slower writer's input");
    std::io::Write::write_all(&mut input, b"go\n").expect("the slower writer is let go");
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(5);
    let refused = loop {
        match late.try_wait() {
            Err(problem) => break problem,
            Ok(None) => {}
            Ok(Some(status)) => {
                panic!("a writer published over a root that changed under it: {status}")
            }
        }
        assert!(
            Instant::now() < deadline,
            "the slower writer did not terminate"
        );
        thread::sleep(Duration::from_millis(10));
    };
    // Asked again while a publication holds the lock. A refused ending is over:
    // it answers the same, and waits for nothing.
    let publishing = held_publication(&sample);
    let again = late.try_wait();
    drop(publishing);
    assert_eq!(
        again.as_ref().map_err(ToString::to_string),
        Err(refused.to_string()),
        "a refused ending answered differently when asked again"
    );
    late.stop()
        .expect("a refused writer's cleanup is confirmed");
    assert_eq!(late.inspection().cleanup(), SandboxCleanup::Complete);
    let lifecycles = lifecycles(&audit);
    assert!(
        lifecycles.contains(&SandboxLifecycle::RolledBack),
        "{lifecycles:?}"
    );
    assert!(
        !lifecycles.contains(&SandboxLifecycle::Quarantined),
        "{lifecycles:?}"
    );
    assert_eq!(
        std::fs::read_to_string(sample.root().join("early.txt")).expect("earlier publication"),
        "early\n"
    );
    assert!(!sample.root().join("late.txt").exists());
}

#[test]
fn a_writer_that_ended_while_another_publishes_reads_as_ended_until_it_publishes() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-writer-ended-beside-a-publication");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("a writer");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command("read go; printf 'after\\n' > after.txt").spoken_to())
        .expect("started command");
    let publishing = held_publication(&sample);
    let_go(process.as_mut());

    let deadline = Instant::now() + Duration::from_secs(3);
    while !process.ended() {
        assert!(
            Instant::now() < deadline,
            "a writer that ended never read as ended while it waited to publish"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        process.try_wait().expect("wait").is_none(),
        "a writer published while another held the lock"
    );
    assert!(!sample.root().join("after.txt").exists());

    drop(publishing);
    let status = ended_within(process.as_mut(), Duration::from_secs(3));
    process.stop().expect("cleanup");
    assert!(status.success(), "{status}");
    assert_eq!(
        std::fs::read_to_string(sample.root().join("after.txt")).expect("published file"),
        "after\n"
    );
}

#[test]
fn stopping_a_writer_that_ended_while_another_publishes_discards_it_cleanly() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-writer-stopped-after-it-ended");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    let writer = request(&sample, SandboxManifest::empty());
    let audit = writer.audit().clone();
    let mut session = service.prepare(writer).expect("a writer");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command("read go; printf 'after\\n' > after.txt").spoken_to())
        .expect("started command");
    let publishing = held_publication(&sample);
    let_go(process.as_mut());
    // The moment it takes to end. How long does not matter, only that it has.
    let settled = Instant::now() + Duration::from_secs(1);
    while !process.ended() && Instant::now() < settled {
        thread::sleep(Duration::from_millis(10));
    }

    let stopped = process.stop();
    drop(publishing);

    stopped.expect("a writer stopped after it ended confirms its cleanup");
    assert_eq!(process.inspection().cleanup(), SandboxCleanup::Complete);
    assert!(
        !sample.root().join("after.txt").exists(),
        "a stopped writer published"
    );
    let lifecycles = lifecycles(&audit);
    assert!(
        lifecycles.contains(&SandboxLifecycle::RolledBack),
        "{lifecycles:?}"
    );
    assert!(
        !lifecycles.contains(&SandboxLifecycle::Quarantined),
        "{lifecycles:?}"
    );
}

#[test]
fn a_writer_killed_while_another_publishes_ends_without_waiting_for_it() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-writer-killed-beside-a-publication");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("a writer");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command("read go; printf 'killed\\n' > killed.txt; kill -KILL $$").spoken_to())
        .expect("started command");
    let publishing = held_publication(&sample);
    let_go(process.as_mut());

    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ended_within(process.as_mut(), Duration::from_secs(3))
    }));
    drop(publishing);

    let status = ended.expect("a writer with nothing to publish waited for another's publication");
    process.stop().expect("cleanup");
    assert!(!status.success(), "{status}");
    assert!(
        !sample.root().join("killed.txt").exists(),
        "a killed writer published"
    );
}

#[test]
fn a_writer_stopped_while_it_ran_still_answers_how_it_ended() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-writer-stopped-while-running");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("a writer");
    session.materialize().expect("materialized workspace");
    let mut process = session.start(command("sleep 30")).expect("started command");

    process.stop().expect("cleanup");

    let status = process
        .try_wait()
        .expect("a stopped command still answers how it ended")
        .expect("a stopped command has ended");
    assert!(!status.success(), "{status}");
}

#[test]
fn a_writer_that_wrote_nothing_ends_cleanly_after_another_published_into_its_root() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-idle-writer-beside-a-publication");
    let mut idle_session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("a writer that will write nothing");
    idle_session.materialize().expect("materialized workspace");
    let mut idle = idle_session
        .start(command("read go; true").spoken_to())
        .expect("started command");
    let mut busy = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("a writer into the same root");
    busy.materialize().expect("materialized workspace");
    let (status, _, _) = finish(
        busy.start(command("printf 'beside\\n' > beside.txt"))
            .expect("started command"),
    );
    assert!(status.success(), "{status}");

    let_go(idle.as_mut());
    let status = ended_within(idle.as_mut(), Duration::from_secs(5));

    idle.stop().expect("cleanup");
    assert!(status.success(), "{status}");
    assert_eq!(
        std::fs::read_to_string(sample.root().join("beside.txt")).expect("the other publication"),
        "beside\n"
    );
}

#[test]
fn a_writer_prepared_while_another_publishes_takes_its_baseline_after_that_publication() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-writer-prepared-during-a-publication");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    let state = super::transaction::state_directory(&request(&sample, SandboxManifest::empty()))
        .expect("transaction state");
    let root = sample.root().clone();
    let (locked, lock_held) = std::sync::mpsc::channel();
    let publishing = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let lease = loop {
            if let Some(lease) =
                super::transaction::Lease::try_acquire_in(&state).expect("the publication lock")
            {
                break lease;
            }
            assert!(
                Instant::now() < deadline,
                "another publication held the lock throughout"
            );
            thread::sleep(Duration::from_millis(10));
        };
        locked.send(()).expect("the test is told the lock is held");
        // A publication in progress: the root changes while the lock is held.
        thread::sleep(Duration::from_millis(500));
        std::fs::write(root.join("published.txt"), "published\n").expect("a publication's write");
        drop(lease);
    });
    lock_held.recv().expect("the lock is held");

    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("a writer prepared while another publishes");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command("printf 'mine\\n' > mine.txt"))
        .expect("started command");
    let ended = ended_within(process.as_mut(), Duration::from_secs(5));
    process.stop().expect("cleanup");
    publishing.join().expect("the publication finished");

    assert!(ended.success(), "{ended}");
    assert_eq!(
        std::fs::read_to_string(sample.root().join("mine.txt")).expect("published file"),
        "mine\n"
    );
    assert_eq!(
        std::fs::read_to_string(sample.root().join("published.txt"))
            .expect("the other publication"),
        "published\n"
    );
}

#[test]
fn a_read_only_command_ends_while_another_publishes() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-reader-beside-a-publication");
    sample.write("seen.txt", "seen\n");
    let base = SandboxPolicy::standard(&sample.workspace()).expect("base policy");
    let policy = SandboxPolicy::new(
        true,
        base.filesystem().iter().map(|rule| {
            if rule.access() == SandboxFilesystemAccess::ReadWrite {
                SandboxFilesystemRule::new(
                    rule.path(),
                    SandboxFilesystemAccess::ReadOnly,
                    rule.provenance(),
                )
                .expect("the same root, read-only")
            } else {
                rule.clone()
            }
        }),
        sample.root().clone(),
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("a read-only policy");
    let reader = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("reader"),
        policy,
        SandboxManifest::empty(),
    );
    let publishing = held_publication(&sample);

    let finished = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut session = service
            .prepare(reader)
            .expect("a reader prepared while another publishes");
        session.materialize().expect("materialized workspace");
        finish(
            session
                .start(command("cat seen.txt"))
                .expect("started command"),
        )
    }));
    drop(publishing);

    let (status, output, errors) =
        finished.expect("a reader, with nothing to publish, waited for another's publication");
    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(output, b"seen\n");
}
