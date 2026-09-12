//! Commands that can write, run side by side, and how each one's ending completes.
//!
//! The lock a publication holds is the only thing two writers share, so these
//! hold it themselves, the way another command's publication would, and watch a
//! writer end while it is held: waiting its turn, stopped, killed, refused, or
//! prepared across it. A test takes the lock only after its own writer has
//! started, because preparation waits for it too — except the read-only case,
//! which may take it first, since a command with no writable root never asks for
//! it, and the tests of preparation itself, which hold the lock first on purpose
//! to watch what preparation does when it cannot have it.

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

/// Waits until `process` reads as ended, so that what is done next happens
/// after every fact its ending has recorded and before it publishes.
fn once_ended(process: &mut dyn SandboxProcess, patience: Duration) {
    let deadline = Instant::now() + patience;
    while !process.ended() {
        assert!(Instant::now() < deadline, "the command did not end");
        thread::sleep(Duration::from_millis(10));
    }
}

/// Says that nothing of `sandbox` is left in this user's state directory.
///
/// A stage left there is read by every later preparation as a lifecycle needing
/// recovery, which refuses it.
fn nothing_left_of(sandbox: SandboxId) {
    let stage = super::transaction::stage_root(sandbox).expect("the stage path");
    assert!(
        !stage.exists(),
        "a stage was left behind, which refuses every later preparation: {}",
        stage.display()
    );
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
        // It moves the root's generation first, as a publication does, because
        // that is what a command being prepared across one has to notice.
        thread::sleep(Duration::from_millis(500));
        super::generations::advance(&state, &[super::generations::key(&root)])
            .expect("a publication moves the root's generation");
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

/// Fills `audit` until it holds `wanted` facts, so that the next fact recorded
/// after that finds it full.
fn fill_audit(audit: &crucible_sandbox::SandboxAudit, wanted: usize) {
    while audit.records().expect("audit records").len() < wanted {
        audit
            .record(
                SandboxId::new(),
                crucible_sandbox::SandboxFactKind::Lifecycle(SandboxLifecycle::Prepared),
            )
            .expect("a fact fits while the collector has room");
    }
}

#[test]
fn a_writer_prepared_while_a_publication_will_not_end_is_refused_rather_than_waiting() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-writer-refused-by-a-stuck-publication");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    // Held for longer than a preparation may wait, the way another crucible of
    // this user, holding it for a whole run, would hold it.
    let publishing = held_publication(&sample);
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("a writer");
    session.materialize().expect("materialized workspace");

    let (told, hears) = std::sync::mpsc::channel();
    let writer = thread::spawn(move || {
        let outcome = session.start(command("printf 'after\\n' > after.txt"));
        told.send(outcome.err().map(|problem| problem.to_string()))
            .expect("the test hears how the writer went");
    });
    let outcome = hears.recv_timeout(Duration::from_secs(20));
    drop(publishing);
    writer.join().expect("the writer thread");

    let refused = outcome
        .expect("a writer waited on a publication that never ends")
        .expect("a writer is refused while another publication holds the lock");
    assert!(refused.contains("concurrency"), "{refused}");
    assert!(!sample.root().join("after.txt").exists());
}

#[test]
fn a_publication_that_cannot_record_its_fact_still_says_what_the_command_did() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-publication-fact-unrecorded");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    let writer = request(&sample, SandboxManifest::empty());
    let sandbox = writer.id();
    let audit = writer.audit().clone();
    let mut session = service.prepare(writer).expect("a writer");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command("read go; printf 'after\\n' > after.txt").spoken_to())
        .expect("started command");
    let_go(process.as_mut());
    // Filled once the command has ended, so that one slot is left at the
    // publication: its start takes that, and the fact that it finished finds the
    // collector full.
    once_ended(process.as_mut(), Duration::from_secs(5));
    fill_audit(&audit, crucible_sandbox::MAX_SANDBOX_AUDIT_FACTS - 1);

    let status = ended_within(process.as_mut(), Duration::from_secs(5));

    let stopped = process.stop();
    assert!(status.success(), "{status}");
    assert_eq!(
        std::fs::read_to_string(sample.root().join("after.txt")).expect("published file"),
        "after\n"
    );
    assert!(
        stopped.is_err(),
        "a fact that could not be recorded was never reported"
    );
    nothing_left_of(sandbox);
}

#[test]
fn a_writer_publishes_nothing_of_a_root_another_publication_touched_while_it_ran() {
    // A command sees its root through an overlay, so another command's
    // publication into that root shows in what the command itself is scanned as
    // holding. If the root is then back as this command found it — because the
    // other publication rolled back, or because what it wrote was written over
    // again — the baseline check sees nothing wrong, and the difference the scan
    // carries is published as this command's own work.
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-root-touched-while-it-ran");
    sample.write("shared.txt", "baseline\n");
    // The root has a history before this command starts, so what refuses it is
    // the count moving again rather than an entry appearing where there was
    // none.
    let state = super::transaction::state_directory(&request(&sample, SandboxManifest::empty()))
        .expect("transaction state");
    let held = held_publication(&sample);
    super::generations::advance(
        &state,
        std::slice::from_ref(&super::generations::key(sample.root())),
    )
    .expect("a publication before this command");
    drop(held);
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("a writer");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command("read go; printf 'mine\n' > mine.txt").spoken_to())
        .expect("started command");

    // Another command publishes into the same root while the first one runs.
    let mut other = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("another writer into the same root");
    other.materialize().expect("the other writer materialized");
    let (status, _, _) = finish(
        other
            .start(command("printf 'theirs\n' > shared.txt"))
            .expect("the other writer started"),
    );
    assert!(status.success(), "{status}");

    let_go(process.as_mut());
    once_ended(process.as_mut(), Duration::from_secs(5));
    // Put the root back the way the first command found it, so that nothing but
    // the generation says a publication happened at all.
    sample.write("shared.txt", "baseline\n");

    let deadline = Instant::now() + Duration::from_secs(5);
    let refused = loop {
        match process.try_wait() {
            Err(problem) => break problem,
            Ok(None) => {}
            Ok(Some(status)) => panic!(
                "a writer published across another command's publication: {status}, and shared.txt now holds {:?}",
                std::fs::read_to_string(sample.root().join("shared.txt")).unwrap_or_default()
            ),
        }
        assert!(Instant::now() < deadline, "the command did not end");
        thread::sleep(Duration::from_millis(10));
    };

    assert!(refused.to_string().contains("writable root"), "{refused}");
    assert_eq!(
        std::fs::read_to_string(sample.root().join("shared.txt")).expect("the root's own content"),
        "baseline\n",
        "another command's content was published as this one's"
    );
    assert!(!sample.root().join("mine.txt").exists());
}

/// What this user's state directory remembers, as bytes and as a moment.
fn generations_as_they_stand(
    state: &std::path::Path,
) -> (Option<Vec<u8>>, Option<std::time::SystemTime>) {
    let file = state.join("publications");
    (
        std::fs::read(&file).ok(),
        std::fs::metadata(&file)
            .ok()
            .and_then(|at| at.modified().ok()),
    )
}

#[test]
fn a_command_with_nothing_to_publish_leaves_the_generations_alone() {
    // A command with no writable root is let in without the lock, on purpose:
    // it has nothing to publish into. Anything it writes to the file it writes
    // with nothing held, and a copy taken before somebody else's publication
    // moved the counter would put that publication's witness back.
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-reader-leaves-the-generations");
    sample.write("seen.txt", "seen\n");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
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
    .expect("a policy that writes nothing");
    let reader = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("bash"),
        policy,
        SandboxManifest::empty(),
    );
    let state = super::transaction::state_directory(&reader).expect("transaction state");
    // Something to erase: a publication's witness, already recorded.
    let held = held_publication(&sample);
    super::generations::advance(
        &state,
        std::slice::from_ref(&super::generations::key(sample.root())),
    )
    .expect("a publication moves the root's generation");
    drop(held);
    let before = generations_as_they_stand(&state);

    let mut session = service.prepare(reader).expect("a reader");
    session.materialize().expect("materialized workspace");
    let (status, _, _) = finish(
        session
            .start(command("cat seen.txt"))
            .expect("started command"),
    );

    assert!(status.success(), "{status}");
    assert_eq!(
        before,
        generations_as_they_stand(&state),
        "a command with nothing to publish rewrote the generations, holding nothing"
    );
}

#[test]
fn a_writer_publishes_nothing_when_the_generations_cannot_be_read() {
    // A line this cannot read is not an absence. Read as one, it says no
    // publication has touched the root, which is the one answer that lets a
    // command through.
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-generations-unreadable");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    let writer = request(&sample, SandboxManifest::empty());
    let state = super::transaction::state_directory(&writer).expect("transaction state");
    std::fs::create_dir_all(&state).expect("the state directory");
    std::fs::write(state.join("publications"), "this is not a generation\n")
        .expect("a file nothing can read");
    let mut session = service.prepare(writer).expect("a writer");
    session.materialize().expect("materialized workspace");

    let refused = session
        .start(command("printf 'mine\\n' > mine.txt"))
        .err()
        .map(|problem| problem.to_string())
        .unwrap_or_default();

    // Taken away before anything asserts: the state directory is this user's,
    // shared by every test in this process, and a file none of them can read
    // would fail all of them.
    std::fs::remove_file(state.join("publications")).expect("the unreadable file is taken away");
    assert!(
        refused.contains("generations"),
        "a file that could not be read was taken for an empty one: {refused}"
    );
    assert!(!sample.root().join("mine.txt").exists());
}

#[test]
fn a_root_is_remembered_by_where_it_is_rather_than_by_what_it_is_called() {
    // A mount's destination is the sandbox's name for a root, not the root. Two
    // names for one root give it two counts, and a command that ran across the
    // other name's publication reads its own as unmoved and is let through.
    //
    // The workspace root is writable here too, because a writable mount needs a
    // writable authority above it — but it is remembered under its own path,
    // which is not the one this looks for. What this looks for can only have
    // come from the mount.
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-root-under-another-name");
    let mounted = sample.root().join("data");
    std::fs::create_dir(&mounted).expect("a directory to mount");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    let manifest = SandboxManifest::new([crucible_sandbox::SandboxManifestEntry::mount(
        mounted.clone(),
        "data",
        SandboxFilesystemAccess::ReadWrite,
        crucible_sandbox::SandboxFilesystemProvenance::Manifest,
    )
    .expect("a writable mount")])
    .expect("a manifest of one mount");
    let writer = request(&sample, manifest);
    let state = super::transaction::state_directory(&writer).expect("transaction state");
    let mut session = service.prepare(writer).expect("a writer through a mount");
    session.materialize().expect("materialized workspace");
    let (status, _, errors) = finish(
        session
            .start(command(
                "printf 'mine\n' > /crucible/manifest/data/mine.txt",
            ))
            .expect("started command"),
    );

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    let standing = super::generations::current(
        &state,
        std::slice::from_ref(&super::generations::key(&mounted)),
    )
    .expect("the generations");
    assert!(
        standing.first().copied().flatten().is_some(),
        "the root was remembered by the name the sandbox gave it, not by where it is"
    );
}

#[test]
fn a_writer_publishes_over_a_root_whose_publications_all_happened_before_it() {
    // The generation says when, not whether. A root this user has published into
    // before is ordinary; what matters is that nothing moved between the
    // baseline and the publication.
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-root-with-a-history");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    let mut earlier = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("an earlier writer");
    earlier
        .materialize()
        .expect("the earlier writer materialized");
    let (status, _, _) = finish(
        earlier
            .start(command("printf 'first\n' > first.txt"))
            .expect("the earlier writer started"),
    );
    assert!(status.success(), "{status}");

    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("a writer after it");
    session.materialize().expect("materialized workspace");
    let (status, _, _) = finish(
        session
            .start(command("printf 'second\n' > second.txt"))
            .expect("started command"),
    );

    assert!(status.success(), "{status}");
    assert_eq!(
        std::fs::read_to_string(sample.root().join("second.txt")).expect("published file"),
        "second\n"
    );
}

#[test]
fn a_writer_publishes_nothing_when_a_publication_touched_a_root_it_knew_before() {
    // The same as the fresh-root case, but where the root already carried a
    // generation: what refuses this command is that the generation moved, not
    // merely that one appeared.
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-root-generation-moved-again");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    let state = super::transaction::state_directory(&request(&sample, SandboxManifest::empty()))
        .expect("transaction state");
    let key = super::generations::key(sample.root());
    let held = held_publication(&sample);
    super::generations::advance(&state, std::slice::from_ref(&key))
        .expect("the root has a history");
    drop(held);

    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("a writer");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command("read go; printf 'mine\n' > mine.txt").spoken_to())
        .expect("started command");
    let held = held_publication(&sample);
    super::generations::advance(&state, std::slice::from_ref(&key))
        .expect("another publication touches it");
    drop(held);
    let_go(process.as_mut());

    let deadline = Instant::now() + Duration::from_secs(5);
    let refused = loop {
        match process.try_wait() {
            Err(problem) => break problem,
            Ok(None) => {}
            Ok(Some(status)) => panic!("a writer published across another publication: {status}"),
        }
        assert!(Instant::now() < deadline, "the command did not end");
        thread::sleep(Duration::from_millis(10));
    };

    assert!(
        refused.to_string().contains("touched a writable root"),
        "{refused}"
    );
    assert!(!sample.root().join("mine.txt").exists());
}

#[test]
fn a_writer_publishes_nothing_when_a_publication_touched_its_root_as_it_ran() {
    // The witness the baseline check lacks. A publication into this root moved
    // its generation while the command ran, so what the command was scanned as
    // holding may be that publication's work rather than its own — however the
    // root looks now.
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-root-generation-moved");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    let mut session = service
        .prepare(request(&sample, SandboxManifest::empty()))
        .expect("a writer");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command("read go; printf 'mine\n' > mine.txt").spoken_to())
        .expect("started command");

    let state = super::transaction::state_directory(&request(&sample, SandboxManifest::empty()))
        .expect("transaction state");
    let held = held_publication(&sample);
    super::generations::advance(&state, &[super::generations::key(sample.root())])
        .expect("a publication moves the root's generation");
    drop(held);

    let_go(process.as_mut());
    let deadline = Instant::now() + Duration::from_secs(5);
    let refused = loop {
        match process.try_wait() {
            Err(problem) => break problem,
            Ok(None) => {}
            Ok(Some(status)) => panic!("a writer published across another publication: {status}"),
        }
        assert!(Instant::now() < deadline, "the command did not end");
        thread::sleep(Duration::from_millis(10));
    };

    assert!(
        refused.to_string().contains("touched a writable root"),
        "{refused}"
    );
    assert!(!sample.root().join("mine.txt").exists());
}

#[test]
fn a_publication_that_cannot_ask_for_admission_says_the_same_thing_twice() {
    use std::os::unix::fs::PermissionsExt as _;

    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-admission-unavailable");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    let writer = request(&sample, SandboxManifest::empty());
    let audit = writer.audit().clone();
    let mut session = service.prepare(writer).expect("a writer");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command("read go; printf 'after\n' > after.txt").spoken_to())
        .expect("started command");
    let_go(process.as_mut());
    once_ended(process.as_mut(), Duration::from_secs(5));
    // Neither held nor free: a lock that cannot be opened at all is the one
    // admission failure a caller cannot act on. The collector is full as well,
    // so the cleanup that follows fails with something else again — which is how
    // an ending that answers two different things shows itself.
    let state = super::transaction::state_directory(&request(&sample, SandboxManifest::empty()))
        .expect("transaction state");
    let lock = state.join("writable.lock");
    let restore = std::fs::metadata(&lock).expect("the lock").permissions();
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o000))
        .expect("an unopenable lock");
    fill_audit(&audit, crucible_sandbox::MAX_SANDBOX_AUDIT_FACTS);

    let first = process
        .try_wait()
        .expect_err("an admission nobody can ask for is an ending that went wrong");
    let again = process.try_wait();

    std::fs::set_permissions(&lock, restore).expect("the lock is restored");
    assert_eq!(
        again.as_ref().map_err(ToString::to_string),
        Err(first.to_string()),
        "an ending that went wrong answered differently when asked again"
    );
    // What comes back is read by the model, and where this user's state
    // directory is belongs in the audit rather than in a tool result.
    assert!(
        !first.to_string().contains("/var/tmp"),
        "the refusal carries this user's state directory: {first}"
    );
    assert!(!sample.root().join("after.txt").exists());
}

#[test]
fn a_publication_whose_start_cannot_be_recorded_discards_what_the_command_wrote() {
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let sample = Sample::new("sandbox-publication-start-unrecorded");
    let _serial = super::transaction::TestSerialLease::acquire().expect("test writer coordination");
    let writer = request(&sample, SandboxManifest::empty());
    let sandbox = writer.id();
    let audit = writer.audit().clone();
    let mut session = service.prepare(writer).expect("a writer");
    session.materialize().expect("materialized workspace");
    let mut process = session
        .start(command("read go; printf 'after\\n' > after.txt").spoken_to())
        .expect("started command");
    let_go(process.as_mut());
    // Filled once the command has ended, so that the publication's own start is
    // the fact that finds the collector full.
    once_ended(process.as_mut(), Duration::from_secs(5));
    fill_audit(&audit, crucible_sandbox::MAX_SANDBOX_AUDIT_FACTS);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match process.try_wait() {
            Err(_) => break,
            Ok(None) => {}
            Ok(Some(status)) => panic!("a publication nobody could record went ahead: {status}"),
        }
        assert!(Instant::now() < deadline, "the command did not end");
        thread::sleep(Duration::from_millis(10));
    }
    let again = process.try_wait();

    // Both looks answer the collector's one fixed sentence, so their agreeing
    // says nothing about which error was settled. That rule is held by
    // `a_publication_that_cannot_ask_for_admission_says_the_same_thing_twice`,
    // where the two differ. What this holds is the discard.
    assert!(
        again.is_err(),
        "a publication nobody could record went ahead"
    );
    assert!(!sample.root().join("after.txt").exists());
    drop(process);
    nothing_left_of(sandbox);
}
