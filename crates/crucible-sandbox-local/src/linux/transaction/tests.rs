use super::*;
use std::io::SeekFrom;

#[test]
fn background_acceptance_follows_owner_transfer_and_go() {
    let mut machine = Machine::new();
    for record in [
        Record::Initialized(InvocationMode::Background),
        Record::Prepared,
        Record::ReleaseIntent,
        Record::OwnerTransferred,
        Record::GoSentOrAmbiguous,
        Record::CallAcceptIntent,
        Record::CallAccepted([0x5a; 32]),
    ] {
        machine.push(record).expect("valid lifecycle prefix");
    }
    assert!(!machine.is_terminal());

    let mut interrupted = Machine::new();
    for record in [
        Record::Initialized(InvocationMode::Background),
        Record::Prepared,
        Record::ReleaseIntent,
        Record::OwnerTransferred,
        Record::GoSentOrAmbiguous,
        Record::AbortObserved,
    ] {
        interrupted
            .push(record)
            .expect("post-GO background recovery edge");
    }
}

#[test]
fn rejects_background_acceptance_before_go_or_without_an_owner() {
    let mut before_go = Machine::new();
    for record in [
        Record::Initialized(InvocationMode::Background),
        Record::Prepared,
        Record::ReleaseIntent,
        Record::OwnerTransferred,
    ] {
        before_go.push(record).expect("valid prefix");
    }
    assert!(before_go.push(Record::CallAcceptIntent).is_err());

    let mut no_owner = Machine::new();
    for record in [
        Record::Initialized(InvocationMode::Background),
        Record::Prepared,
        Record::ReleaseIntent,
    ] {
        no_owner.push(record).expect("valid prefix");
    }
    assert!(no_owner.push(Record::GoSentOrAmbiguous).is_err());
}

#[test]
fn detachable_lifecycles_choose_exactly_one_terminal_or_acceptance_path_after_go() {
    let prefix = [
        Record::Initialized(InvocationMode::Detachable),
        Record::Prepared,
        Record::ReleaseIntent,
        Record::OwnerTransferred,
        Record::GoSentOrAmbiguous,
    ];
    let mut terminal = Machine::new();
    for record in prefix {
        terminal.push(record).expect("detachable prefix");
    }
    terminal
        .push(Record::CommandExited)
        .expect("foreground terminal path");
    assert!(terminal.push(Record::CallAcceptIntent).is_err());

    let mut accepted = Machine::new();
    for record in prefix {
        accepted.push(record).expect("detachable prefix");
    }
    accepted
        .push(Record::CallAcceptIntent)
        .expect("detach intent");
    accepted
        .push(Record::CallAccepted([0x6b; 32]))
        .expect("detach acceptance");
    assert!(accepted.push(Record::CommandExited).is_ok());
}

#[test]
fn positive_publication_requires_contiguous_stage_and_apply_records() {
    let mut machine = Machine::new();
    for record in [
        Record::Initialized(InvocationMode::Foreground),
        Record::Prepared,
        Record::ReleaseIntent,
        Record::GoSentOrAmbiguous,
        Record::CommandExited,
        Record::WorkloadReapIntent,
        Record::WorkloadReaped,
        Record::ScanIntent,
        Record::ScanTransferred,
        Record::StageIntent(0),
        Record::Staged(0),
        Record::StageIntent(1),
        Record::Staged(1),
        Record::PublicationStaged,
        Record::ScopeReapIntent,
        Record::ScopeReapProved,
        Record::ApplyIntent(0),
        Record::Applied(0),
        Record::ApplyIntent(1),
        Record::Applied(1),
        Record::Committed,
    ] {
        machine.push(record).expect("positive lifecycle history");
    }
    assert!(machine.is_terminal());
    assert!(machine.push(Record::AbortObserved).is_err());
}

#[test]
fn rollback_requires_reverse_apply_and_stage_resolution() {
    let mut machine = Machine::new();
    for record in [
        Record::Initialized(InvocationMode::Foreground),
        Record::Prepared,
        Record::ReleaseIntent,
        Record::GoSentOrAmbiguous,
        Record::CommandExited,
        Record::WorkloadReapIntent,
        Record::WorkloadReaped,
        Record::ScanIntent,
        Record::ScanTransferred,
        Record::StageIntent(0),
        Record::Staged(0),
        Record::PublicationStaged,
        Record::ScopeReapIntent,
        Record::ScopeReapProved,
        Record::ApplyIntent(0),
        Record::AbortObserved,
    ] {
        machine.push(record).expect("abort prefix");
    }
    assert!(machine.push(Record::RolledBack).is_err());
    for record in [
        Record::RollbackIntent(0),
        Record::RollbackApplied(0),
        Record::DiscardIntent(0),
        Record::Discarded(0),
        Record::RolledBack,
    ] {
        machine.push(record).expect("resolved rollback");
    }
    assert!(machine.is_terminal());
}

#[test]
fn proved_pre_release_cleanup_can_refuse_but_unproved_cleanup_quarantines() {
    let mut proved = Machine::new();
    for record in [
        Record::Initialized(InvocationMode::Foreground),
        Record::Prepared,
        Record::RefusalObserved,
        Record::PreparationCleanupIntent,
        Record::PreparationCleanupProved,
        Record::Refused,
    ] {
        proved.push(record).expect("proved refusal history");
    }
    assert!(proved.is_terminal());

    let mut unproved = Machine::new();
    for record in [
        Record::Initialized(InvocationMode::Foreground),
        Record::Prepared,
        Record::RefusalObserved,
        Record::PreparationCleanupIntent,
        Record::PreparationCleanupUnproved,
    ] {
        unproved.push(record).expect("unproved cleanup prefix");
    }
    assert!(unproved.push(Record::Refused).is_err());
    unproved
        .push(Record::Quarantined)
        .expect("unproved cleanup quarantines");
}

#[test]
fn publication_lease_is_exclusive_and_released_with_its_descriptor() {
    let sample = crate::sample::Sample::new("sandbox-transaction-lock");
    let state = sample.root().join("state");
    let first = Lease::try_acquire_in(&state)
        .expect("lock state")
        .expect("first lease");
    assert!(Lease::try_acquire_in(&state).expect("lock state").is_none());
    drop(first);
    lease_after_transient_holders(&state, "lease after descriptor close");
}

#[test]
fn durable_frames_replay_through_the_same_closed_validator() {
    let (sample, journal) = journal_with(&[
        Record::Prepared,
        Record::RefusalObserved,
        Record::PreparationCleanupIntent,
        Record::PreparationCleanupProved,
        Record::Refused,
    ]);
    let recovered = recover_wal(&journal).expect("valid journal");
    assert_eq!(recovered.records.len(), 6);
    assert!(recovered.machine.is_terminal());
    assert!(!recovered.torn_tail);
    drop(sample);
}

#[test]
fn durable_background_frames_bind_the_result_key_and_acceptance_receipt() {
    use std::os::unix::fs::DirBuilderExt as _;

    let sample = crate::sample::Sample::new("sandbox-background-transaction-journal");
    let projection_root = sample.root().join("stage");
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(&projection_root).expect("stage directory");
    let key = CallResultKey::from_digest([0x3c; 32]);
    let receipt = [0x5a; 32];
    let mut transaction = Transaction::start(
        &projection_root,
        SandboxId::new(),
        InvocationMode::Background,
        Some(key),
    )
    .expect("background transaction journal");
    for record in [
        Record::Prepared,
        Record::ReleaseIntent,
        Record::OwnerTransferred,
        Record::GoSentOrAmbiguous,
        Record::CallAcceptIntent,
        Record::CallAccepted(receipt),
    ] {
        transaction.append(record).expect("journal record");
    }
    drop(transaction);

    let recovered =
        recover_wal(&projection_root.join("transaction.wal")).expect("valid background journal");
    assert_eq!(recovered.frame.call_result_key, key.bytes());
    assert_eq!(
        recovered.records.last(),
        Some(&Record::CallAccepted(receipt))
    );
}

#[test]
fn invocation_mode_and_durable_result_identity_cannot_disagree() {
    assert!(Invocation::new(InvocationMode::Foreground, None).is_ok());
    assert!(
        Invocation::new(
            InvocationMode::Background,
            Some(CallResultKey::from_digest([1; 32]))
        )
        .is_ok()
    );
    assert!(
        Invocation::new(
            InvocationMode::Detachable,
            Some(CallResultKey::from_digest([3; 32]))
        )
        .is_ok()
    );
    assert!(
        Invocation::new(
            InvocationMode::Foreground,
            Some(CallResultKey::from_digest([2; 32]))
        )
        .is_err()
    );
    assert!(Invocation::new(InvocationMode::Background, None).is_err());
    assert!(Invocation::new(InvocationMode::Detachable, None).is_err());
}

#[test]
fn a_truncated_tail_is_removed_to_the_last_verified_frame() {
    let (sample, journal) = journal_with(&[Record::Prepared]);
    let complete = fs::metadata(&journal).expect("journal metadata").len();
    OpenOptions::new()
        .append(true)
        .open(&journal)
        .expect("append journal")
        .write_all(b"partial")
        .expect("torn tail fixture");

    let recovered = recover_wal(&journal).expect("recover torn tail");
    assert!(recovered.torn_tail);
    assert_eq!(
        fs::metadata(&journal).expect("recovered metadata").len(),
        complete
    );
    drop(sample);
}

#[test]
fn recovery_appends_after_the_verified_boundary_of_a_torn_tail() {
    let (sample, journal) = journal_with(&[]);
    OpenOptions::new()
        .append(true)
        .open(&journal)
        .expect("append journal")
        .write_all(b"partial")
        .expect("torn tail fixture");

    let mut recovered = recover_wal(&journal).expect("recover torn tail");
    recovered.append(Record::Prepared).expect("recovery frame");
    drop(recovered);
    let replayed = recover_wal(&journal).expect("replay recovered WAL");
    assert_eq!(
        replayed.records,
        vec![
            Record::Initialized(InvocationMode::Foreground),
            Record::Prepared
        ]
    );
    drop(sample);
}

#[test]
fn boot_pid_and_start_time_distinguish_live_and_dead_owners() {
    assert!(
        !OwnerIdentity::current()
            .expect("current identity")
            .owner_is_dead()
            .expect("live owner check")
    );
    assert!(dead_owner().owner_is_dead().expect("dead owner check"));
}

#[test]
fn checksum_corruption_is_never_treated_as_a_torn_tail() {
    let (sample, journal) = journal_with(&[Record::Prepared]);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&journal)
        .expect("open journal");
    let end = file.seek(SeekFrom::End(-1)).expect("last checksum byte");
    let mut byte = [0_u8; 1];
    file.read_exact(&mut byte).expect("checksum byte");
    file.seek(SeekFrom::Start(end)).expect("rewind checksum");
    byte[0] ^= 0xff;
    file.write_all(&byte).expect("corrupt checksum");
    file.sync_all().expect("sync corruption");

    assert!(recover_wal(&journal).is_err());
    drop(sample);
}

#[test]
fn terminal_stale_transactions_are_cleaned_idempotently() {
    let sample = crate::sample::Sample::new("sandbox-terminal-recovery");
    let base = sample.root().join("recovery");
    create_private_test_directory(&base);
    let stage = stale_journal(&base, true);

    // A journal held by somebody else means its owner is finishing that stage,
    // which is a state of the machine rather than of this rule. Waiting it out
    // keeps the assertions below about recovery.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let settled = loop {
        let settled = reconcile_stale_transactions(&base).expect("terminal cleanup");
        if settled.busy == 0 {
            break settled;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the journal stayed held by something else"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };

    assert_eq!(settled.removed, 1);
    assert!(!stage.exists());
    let again = reconcile_stale_transactions(&base).expect("idempotent terminal cleanup");
    assert_eq!(
        again,
        Reconciled::default(),
        "a second pass found work to do"
    );
}

#[test]
fn a_stage_whose_journal_is_held_is_left_alone_and_said_to_be_busy() {
    let sample = crate::sample::Sample::new("sandbox-busy-recovery");
    let base = sample.root().join("recovery");
    create_private_test_directory(&base);
    let stage = stale_journal(&base, true);
    // How a stage looks while its own owner is finishing it: the journal open and
    // locked, everything else already gone. Removing it here would race that
    // owner, so the pass leaves it — and must not call that nothing to do.
    let held = File::open(stage.join("transaction.wal")).expect("the journal");
    rustix::fs::flock(&held, FlockOperation::LockExclusive).expect("hold the journal");

    let settled = reconcile_stale_transactions(&base).expect("cleanup with a held journal");

    assert_eq!(settled.busy, 1, "a held journal read as nothing to do");
    assert_eq!(settled.removed, 0);
    assert!(
        stage.exists(),
        "a stage its owner is still finishing was taken from it"
    );
}

#[test]
fn live_nonterminal_transactions_are_skipped_and_retain_their_evidence() {
    let sample = crate::sample::Sample::new("sandbox-nonterminal-recovery");
    let base = sample.root().join("recovery");
    create_private_test_directory(&base);
    let stage = stale_journal(&base, false);

    reconcile_stale_transactions(&base).expect("live transaction is not stale");
    assert!(stage.join("transaction.wal").exists());
}

#[test]
fn live_transaction_wal_is_never_repaired_while_its_owner_may_append() {
    let sample = crate::sample::Sample::new("sandbox-live-wal-lock");
    let base = sample.root().join("recovery");
    create_private_test_directory(&base);
    let sandbox = SandboxId::new();
    let stage = base.join(format!("crucible-projection-{sandbox}"));
    create_private_test_directory(&stage);
    let mut transaction = Transaction::start_owned(
        &stage,
        sandbox,
        Invocation::new(InvocationMode::Foreground, None).expect("foreground identity"),
        OwnerIdentity::current().expect("live owner"),
    )
    .expect("transaction journal");
    transaction
        .append(Record::Prepared)
        .expect("prepared record");
    let journal = stage.join("transaction.wal");
    let mut concurrent = OpenOptions::new()
        .append(true)
        .open(&journal)
        .expect("second journal descriptor");
    concurrent
        .write_all(b"CRSB")
        .expect("partial in-flight frame");
    concurrent.sync_all().expect("partial frame durability");
    let length = concurrent.metadata().expect("journal metadata").len();

    reconcile_stale_transactions(&base).expect("live transaction is skipped");

    assert_eq!(
        fs::metadata(&journal).expect("retained journal").len(),
        length,
        "reconciliation truncated a WAL its live owner may still extend"
    );
    drop(transaction);
}

#[test]
fn abandoned_prejournal_stage_is_removed() {
    let sample = crate::sample::Sample::new("sandbox-abandoned-prejournal-stage");
    let base = sample.root().join("recovery");
    create_private_test_directory(&base);
    let stage = base.join(format!("crucible-projection-{}", SandboxId::new()));
    create_private_test_directory(&stage);

    reconcile_stale_transactions(&base).expect("abandoned initialization cleanup");

    assert!(!stage.exists());
}

#[test]
fn read_only_lifecycles_have_a_durable_transaction_without_a_writer_lease() {
    use std::os::unix::fs::DirBuilderExt as _;

    let sample = crate::sample::Sample::new("sandbox-read-only-transaction");
    let stage = sample.root().join("stage");
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(&stage).expect("stage directory");
    let mut transaction =
        Transaction::start(&stage, SandboxId::new(), InvocationMode::Foreground, None)
            .expect("read-only lifecycle journal");
    for record in [
        Record::Prepared,
        Record::RefusalObserved,
        Record::PreparationCleanupIntent,
        Record::PreparationCleanupProved,
        Record::Refused,
    ] {
        transaction.append(record).expect("lifecycle record");
    }
    drop(transaction);

    let recovered = recover_wal(&stage.join("transaction.wal")).expect("read-only WAL");
    assert_eq!(recovered.machine.terminal(), Some(Record::Refused));
}

#[test]
fn dead_pre_release_owners_are_refused_and_cleaned() {
    let sample = crate::sample::Sample::new("sandbox-dead-pre-release-recovery");
    let base = sample.root().join("recovery");
    create_private_test_directory(&base);
    let stage = stale_journal_with(&base, dead_owner(), &[Record::Prepared]);

    reconcile_stale_transactions(&base).expect("pre-release recovery");
    assert!(!stage.exists());
}

#[test]
fn dead_post_release_owners_roll_back_when_no_apply_was_authorized() {
    let sample = crate::sample::Sample::new("sandbox-dead-post-release-recovery");
    let base = sample.root().join("recovery");
    create_private_test_directory(&base);
    let stage = stale_journal_with(
        &base,
        dead_owner(),
        &[
            Record::Prepared,
            Record::ReleaseIntent,
            Record::GoSentOrAmbiguous,
        ],
    );

    reconcile_stale_transactions(&base).expect("post-release rollback");
    assert!(!stage.exists());
}

#[test]
fn dead_background_owner_between_go_and_acceptance_rolls_back() {
    let sample = crate::sample::Sample::new("sandbox-dead-background-acceptance-gap");
    let base = sample.root().join("recovery");
    create_private_test_directory(&base);
    let stage = stale_journal_with_invocation(
        &base,
        dead_owner(),
        Invocation::new(
            InvocationMode::Background,
            Some(CallResultKey::from_digest([0x4a; 32])),
        )
        .expect("background identity"),
        &[
            Record::Prepared,
            Record::ReleaseIntent,
            Record::OwnerTransferred,
            Record::GoSentOrAmbiguous,
        ],
    );

    reconcile_stale_transactions(&base).expect("background acceptance-gap recovery");
    assert!(!stage.exists());
}

#[test]
fn dead_owners_discard_durable_staging_in_reverse_before_rollback() {
    let sample = crate::sample::Sample::new("sandbox-dead-staging-recovery");
    let base = sample.root().join("recovery");
    create_private_test_directory(&base);
    let stage = stale_journal_with(
        &base,
        dead_owner(),
        &[
            Record::Prepared,
            Record::ReleaseIntent,
            Record::GoSentOrAmbiguous,
            Record::CommandExited,
            Record::WorkloadReapIntent,
            Record::WorkloadReaped,
            Record::ScanIntent,
            Record::ScanTransferred,
            Record::StageIntent(0),
            Record::Staged(0),
            Record::StageIntent(1),
            Record::Staged(1),
            Record::PublicationStaged,
            Record::ScopeReapIntent,
            Record::ScopeReapProved,
        ],
    );
    create_private_test_directory(&stage.join("publication"));
    create_private_test_directory(&stage.join("publication/0"));
    create_private_test_directory(&stage.join("publication/1"));

    reconcile_stale_transactions(&base).expect("staging recovery");
    assert!(!stage.exists());
}

#[test]
fn dead_owners_with_an_ambiguous_apply_are_quarantined() {
    let sample = crate::sample::Sample::new("sandbox-dead-apply-recovery");
    let base = sample.root().join("recovery");
    create_private_test_directory(&base);
    let stage = stale_journal_with(
        &base,
        dead_owner(),
        &[
            Record::Prepared,
            Record::ReleaseIntent,
            Record::GoSentOrAmbiguous,
            Record::CommandExited,
            Record::WorkloadReapIntent,
            Record::WorkloadReaped,
            Record::ScanIntent,
            Record::ScanTransferred,
            Record::StageIntent(0),
            Record::Staged(0),
            Record::PublicationStaged,
            Record::ScopeReapIntent,
            Record::ScopeReapProved,
            Record::ApplyIntent(0),
        ],
    );
    create_private_test_directory(&stage.join("publication"));
    create_private_test_directory(&stage.join("publication/0"));

    assert!(reconcile_stale_transactions(&base).is_err());
    let recovered = recover_wal(&stage.join("transaction.wal")).expect("quarantine WAL");
    assert_eq!(recovered.machine.terminal(), Some(Record::Quarantined));
    assert!(stage.join("publication/0").exists());
}

fn journal_with(records: &[Record]) -> (crate::sample::Sample, PathBuf) {
    use std::os::unix::fs::DirBuilderExt as _;

    let sample = crate::sample::Sample::new("sandbox-transaction-journal");
    let projection_root = sample.root().join("stage");
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(&projection_root).expect("stage directory");
    let mut transaction = Transaction::start(
        &projection_root,
        SandboxId::new(),
        InvocationMode::Foreground,
        None,
    )
    .expect("transaction journal");
    for record in records {
        transaction.append(*record).expect("journal record");
    }
    drop(transaction);
    let journal = projection_root.join("transaction.wal");
    (sample, journal)
}

fn stale_journal(base: &Path, terminal: bool) -> PathBuf {
    let mut records = vec![Record::Prepared];
    if terminal {
        records.extend([
            Record::RefusalObserved,
            Record::PreparationCleanupIntent,
            Record::PreparationCleanupProved,
            Record::Refused,
        ]);
    }
    stale_journal_with(
        base,
        OwnerIdentity::current().expect("current owner"),
        &records,
    )
}

fn stale_journal_with(base: &Path, owner: OwnerIdentity, records: &[Record]) -> PathBuf {
    stale_journal_with_invocation(
        base,
        owner,
        Invocation::new(InvocationMode::Foreground, None).expect("foreground identity"),
        records,
    )
}

fn stale_journal_with_invocation(
    base: &Path,
    owner: OwnerIdentity,
    invocation: Invocation,
    records: &[Record],
) -> PathBuf {
    let sandbox = SandboxId::new();
    let stage = base.join(format!("crucible-projection-{sandbox}"));
    create_private_test_directory(&stage);
    let mut transaction =
        Transaction::start_owned(&stage, sandbox, invocation, owner).expect("transaction journal");
    for record in records {
        transaction.append(*record).expect("transaction record");
    }
    drop(transaction);
    stage
}

fn dead_owner() -> OwnerIdentity {
    OwnerIdentity {
        pid: u32::MAX,
        start: 1,
        boot: boot_identity().expect("boot identity"),
    }
}

fn create_private_test_directory(path: &Path) {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path).expect("private directory");
}

#[test]
fn a_stale_journal_lock_lent_to_a_departing_child_is_recovered_not_skipped() {
    let sample = crate::sample::Sample::new("sandbox-lent-journal-lock");
    let base = sample.root().join("recovery");
    create_private_test_directory(&base);
    let stage = stale_journal_with(&base, dead_owner(), &[Record::Prepared]);
    let journal = File::options()
        .read(true)
        .write(true)
        .open(stage.join("transaction.wal"))
        .expect("stale journal");
    // A child another test is spawning may still hold, between fork and exec,
    // a copy of the descriptor the stale transaction just closed, so the lock
    // is taken across the same transient-holder budget the recovery allows.
    assert!(
        lock_after_transient_holder(&journal).expect("journal lock"),
        "the stale journal lock outlived the transient-holder budget"
    );
    let mut child = lend_to_departing_child(&journal, "0.05");
    drop(journal);

    reconcile_stale_transactions(&base).expect("pre-release recovery");
    assert!(
        !stage.exists(),
        "a stale journal whose lock only a departing child still holds was skipped"
    );
    child.wait().expect("departing child");
}

#[test]
fn a_publication_lease_whose_lock_was_replaced_is_no_longer_the_lock() {
    // An age-based cleaner of the temporary directory can remove a lock file
    // that is never written, and the next process to ask creates a fresh one.
    // Two publications would then hold what each believes is the only lock.
    let sample = crate::sample::Sample::new("sandbox-replaced-writable-lock");
    let state = sample.root().join("state");
    let held = Lease::try_acquire_in(&state)
        .expect("lock state")
        .expect("a lease");
    held.confirm().expect("the lock it was taken on");

    std::fs::remove_file(state.join("writable.lock")).expect("the lock is removed");
    let replacement = Lease::try_acquire_in(&state)
        .expect("lock state")
        .expect("a lease on the lock that replaced it");

    assert!(
        held.confirm().is_err(),
        "a replaced lock still read as the lock the lease was taken on"
    );
    drop(replacement);
}

#[test]
fn taking_the_publication_lock_keeps_its_file_from_ageing() {
    use std::fs::FileTimes;
    use std::time::{Duration, SystemTime};

    let sample = crate::sample::Sample::new("sandbox-ageing-writable-lock");
    let state = sample.root().join("state");
    drop(
        Lease::try_acquire_in(&state)
            .expect("lock state")
            .expect("a lease that creates the lock"),
    );
    let lock = state.join("writable.lock");
    let long_ago = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    std::fs::File::options()
        .write(true)
        .open(&lock)
        .expect("the lock")
        .set_times(FileTimes::new().set_modified(long_ago))
        .expect("an old lock");

    drop(lease_after_transient_holders(
        &state,
        "a lease that takes the lock again",
    ));

    let modified = std::fs::metadata(&lock)
        .expect("the lock")
        .modified()
        .expect("its modification time");
    assert!(
        modified > long_ago,
        "a lock nothing ever writes reads as old enough to remove"
    );
}

#[test]
fn a_publication_lease_lent_to_a_departing_child_is_free_once_the_child_is_gone() {
    let sample = crate::sample::Sample::new("sandbox-lent-writable-lease");
    let state = sample.root().join("state");
    let first = Lease::try_acquire_in(&state)
        .expect("lock state")
        .expect("first lease");
    // Kept past the transient-holder budget, so the lease taken below is one
    // the child's exit freed and not one a short wait would have found anyway.
    let mut child = lend_to_departing_child(first.lock(), "0.5");
    drop(first);
    assert!(
        Lease::try_acquire_in(&state).expect("lock state").is_none(),
        "the lease was free while a child still held its descriptor"
    );

    child.wait().expect("departing child");
    lease_after_transient_holders(&state, "lease once the departing child is gone");
}

/// Takes the lock in `state` again once this test has let it go.
///
/// A child another test is spawning may still hold, between fork and exec, a
/// copy of the descriptor this test just closed, so the lock is taken across
/// the same transient-holder budget the recovery allows.
fn lease_after_transient_holders(state: &Path, expected: &str) -> Lease {
    Lease::acquire_in(state, TRANSIENT_LOCK_PAUSE * TRANSIENT_LOCK_RETRIES)
        .expect("lock state")
        .expect(expected)
}

/// Hands a copy of `held` to a child that keeps it open for `seconds`, as a
/// forked child does between `fork` and `exec` while it still carries the
/// parent's descriptor table.
fn lend_to_departing_child(held: &File, seconds: &str) -> std::process::Child {
    let copy = held.try_clone().expect("descriptor copy");
    std::process::Command::new("sleep")
        .arg(seconds)
        .stdin(std::process::Stdio::from(copy))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("departing child")
}

#[test]
fn clearing_a_stage_before_its_journal_leaves_only_the_journal() {
    let sample = crate::sample::Sample::new("sandbox-clear-stage-before-journal");
    let stage = sample.root().join("stage");
    fs::create_dir_all(stage.join("roots/0/nested")).expect("stage roots");
    fs::write(stage.join("roots/0/nested/file"), b"written").expect("stage file");
    fs::write(stage.join("payloads"), b"loose").expect("stage payload");
    fs::write(stage.join("transaction.wal"), b"journal").expect("stage journal");

    clear_stage_before_journal(&stage).expect("stage cleared");

    let remaining: Vec<_> = fs::read_dir(&stage)
        .expect("stage remains")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(remaining, ["transaction.wal"]);

    clear_stage_before_journal(&sample.root().join("absent"))
        .expect("an absent stage is already clear");
}

#[test]
fn a_panic_under_the_state_coordination_leaves_it_usable() {
    // A test that panics while the coordination's lock is held would otherwise
    // leave every later lease refused, which reads exactly as the refusal the
    // coordination exists to stop.
    let poisoning = std::thread::spawn(|| {
        let _held = TEST_STATE_USE.0.lock();
        panic!("a test panicking under the state coordination's lock");
    });
    assert!(poisoning.join().is_err(), "the fixture did not panic");

    // Asked from a thread of its own: a hold left held after the panic would
    // keep the change waiting for ever, and the test says so rather than hang
    // the run.
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(TestStateChange::read());
        drop(TestStateChange::change());
        done.send(())
            .expect("the test is told the coordination answered");
    });
    finished
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("a reading and then a change, after the panic");
}

#[test]
fn a_change_waits_for_a_reading_under_way() {
    // A lease part-way through reading this user's state directory would be
    // refused by a change made now, for a change that is not its own.
    let reading = TestStateChange::read();
    let (asking, asked) = std::sync::mpsc::channel();
    let (holding, held) = std::sync::mpsc::channel();
    let changer = std::thread::spawn(move || {
        asking
            .send(())
            .expect("the test is told the change is asked for");
        let change = TestStateChange::change();
        holding
            .send(())
            .expect("the test is told the change is held");
        drop(change);
    });
    asked.recv().expect("the change is asked for");
    assert!(
        held.recv_timeout(std::time::Duration::from_millis(200))
            .is_err(),
        "a change was taken while a reading was under way"
    );
    drop(reading);
    held.recv_timeout(std::time::Duration::from_secs(30))
        .expect("the change, once the reading ended");
    changer.join().expect("the changing thread");
}

#[test]
fn a_change_waits_for_another_change() {
    // Two tests changing this user's state directory at once would each put back
    // what the other changed while the other still relied on it.
    let first = TestStateChange::change();
    let (asking, asked) = std::sync::mpsc::channel();
    let (holding, held) = std::sync::mpsc::channel();
    let changer = std::thread::spawn(move || {
        asking
            .send(())
            .expect("the test is told the second change is asked for");
        let second = TestStateChange::change();
        holding
            .send(())
            .expect("the test is told the second change is held");
        drop(second);
    });
    asked.recv().expect("the second change is asked for");
    assert!(
        held.recv_timeout(std::time::Duration::from_millis(200))
            .is_err(),
        "a second change was taken while the first still stood"
    );
    drop(first);
    held.recv_timeout(std::time::Duration::from_secs(30))
        .expect("the second change, once the first ended");
    changer.join().expect("the changing thread");
}

#[test]
fn a_change_asked_for_again_by_its_holder_is_taken_at_once() {
    // A test holding the change that reaches code asking for it again would
    // otherwise wait on itself for ever, and letting go of that second hold must
    // not let go of the change the first still holds.
    let (taken, told) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let change = TestStateChange::change();
        let again = TestStateChange::change();
        drop(again);
        taken
            .send(())
            .expect("the test is told the change was taken twice");
        released.recv().expect("the test lets the first hold go");
        drop(change);
    });
    told.recv_timeout(std::time::Duration::from_secs(30))
        .expect("the change asked for again by the thread holding it");

    let (asking, asked) = std::sync::mpsc::channel();
    let (holding, held) = std::sync::mpsc::channel();
    let contender = std::thread::spawn(move || {
        asking
            .send(())
            .expect("the test is told the change is asked for");
        let change = TestStateChange::change();
        holding
            .send(())
            .expect("the test is told the change is held");
        drop(change);
    });
    asked.recv().expect("the change is asked for");
    assert!(
        held.recv_timeout(std::time::Duration::from_millis(200))
            .is_err(),
        "letting go of the second hold let go of the change"
    );
    release.send(()).expect("the first hold is let go");
    held.recv_timeout(std::time::Duration::from_secs(30))
        .expect("the change, once its holder let it go");
    holder.join().expect("the holding thread");
    contender.join().expect("the contending thread");
}

/// Another checkout's state directory, removed however a test ends — unless it
/// is this build's own, which every other test of the process shares.
struct AnotherCheckout(PathBuf);

impl Drop for AnotherCheckout {
    fn drop(&mut self) {
        if state_base().ok().as_deref() != Some(self.0.as_path()) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

/// A state directory's mode, put back however a test ends.
struct ModeRestored(PathBuf, fs::Permissions);

impl Drop for ModeRestored {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, self.1.clone());
    }
}

#[test]
fn a_checkout_neither_refuses_nor_recovers_another_checkouts_sandbox_state() {
    // Two checkouts testing at once: this build, and one compiled from another
    // directory. The other is named under this checkout, so the same test
    // running from a third checkout names a directory of its own.
    let own = state_base().expect("this build's state directory");
    let other = AnotherCheckout(
        state_base_named(&checkout_state_name(
            &format!(
                "crucible-code-sandbox-{}-v1",
                rustix::process::getuid().as_raw()
            ),
            concat!(env!("CARGO_MANIFEST_DIR"), "/another-checkout"),
        ))
        .expect("another checkout's state directory"),
    );
    create_state_directory(&own).expect("this build's state directory");
    create_state_directory(&other.0).expect("another checkout's state directory");

    // What a publication test does to watch a refusal: this build's directory
    // is left in a mode no command accepts.
    let changing = TestStateChange::change();
    let restore = ModeRestored(
        own.clone(),
        fs::metadata(&own)
            .expect("this build's state directory")
            .permissions(),
    );
    fs::set_permissions(&own, fs::Permissions::from_mode(0o750))
        .expect("a state directory that is not private");
    let refused = Lease::try_acquire_in(&own);
    let granted = Lease::try_acquire_in(&other.0);
    drop(restore);
    drop(changing);
    assert!(refused.is_err(), "the changed mode refused nothing");
    assert!(
        matches!(granted, Ok(Some(_))),
        "another checkout's state was refused for a mode this one's was left in: {granted:?}"
    );
    drop(granted);

    // What every preparation does first: recover the stale stages it finds,
    // under the registry lease it holds while it makes a stage of its own, so
    // no stage another test of this process is still making is taken for one.
    let stage = other.0.join(stage_name(SandboxId::new()));
    create_private_test_directory(&stage);
    let registry = RegistryLease::acquire_at(&own).expect("this build's registry lease");
    RegistryLease::reconcile(&registry).expect("this build's recovery");
    drop(registry);
    assert!(
        stage.exists(),
        "this build's recovery removed a stage of another checkout's"
    );

    // Both sit under the same `/var/tmp` the shipped path does, and each is the
    // shipped name with a fixed-width token of its checkout.
    let shipped = format!(
        "crucible-code-sandbox-{}-v1-",
        rustix::process::getuid().as_raw()
    );
    for state in [&own, &other.0] {
        assert_eq!(state.parent(), Some(Path::new("/var/tmp")));
        let token = state
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(&shipped))
            .unwrap_or_default();
        assert!(
            token.len() == 16
                && token
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
            "{} is not the shipped name with a checkout's token",
            state.display()
        );
    }
    assert_ne!(own, other.0);
}

#[test]
fn private_directory_problem_names_each_failing_property() {
    let owner = StateDirectoryOwner {
        uid: 1000,
        gid: 1000,
    };
    let found = |is_dir, uid, gid, mode| StateDirectoryFound {
        is_dir,
        uid,
        gid,
        mode,
    };
    // Full `st_mode` values, file-type bits included (`S_IFDIR` is `0o040000`,
    // `S_IFREG` is `0o100000`), so a mask dropped from `private_directory_problem`
    // fails this test: without `& 0o7777`, `0o040700` would never equal `0o700`
    // and even a correctly-moded directory would report `WrongMode`.
    assert!(matches!(
        private_directory_problem(found(false, 1000, 1000, 0o100_644), owner),
        Some(StateDirectoryProblem::NotADirectory)
    ));
    assert!(matches!(
        private_directory_problem(found(true, 1001, 1000, 0o040_700), owner),
        Some(StateDirectoryProblem::WrongOwner(1001))
    ));
    assert!(matches!(
        private_directory_problem(found(true, 1000, 1001, 0o040_700), owner),
        Some(StateDirectoryProblem::WrongGroup(1001))
    ));
    assert!(matches!(
        private_directory_problem(found(true, 1000, 1000, 0o040_750), owner),
        Some(StateDirectoryProblem::WrongMode(0o750))
    ));
    assert!(private_directory_problem(found(true, 1000, 1000, 0o040_700), owner).is_none());
}

#[test]
fn state_directory_problem_message_names_the_reason() {
    let path = Path::new("/var/tmp/crucible-code-sandbox-1000-v1");
    let display = path.display();
    assert_eq!(
        StateDirectoryProblem::NotADirectory.message(path),
        format!(
            "sandbox state directory {display} is not a directory; remove it (an administrator \
             may need to) before the sandbox can be used"
        )
    );
    assert_eq!(
        StateDirectoryProblem::WrongOwner(1234).message(path),
        format!(
            "sandbox state directory {display} is owned by uid 1234, not this user; remove it \
             (an administrator may need to) before the sandbox can be used"
        )
    );
    assert_eq!(
        StateDirectoryProblem::WrongGroup(1234).message(path),
        format!("sandbox state directory {display} is owned by group 1234, not this user's group")
    );
    assert_eq!(
        StateDirectoryProblem::WrongMode(0o750).message(path),
        format!("sandbox state directory {display} has mode 0750, not 0700")
    );
    assert_eq!(
        StateDirectoryProblem::Symlink.message(path),
        format!(
            "sandbox state directory {display} is a symlink; remove it (an administrator may \
             need to) before the sandbox can be used"
        )
    );
}

#[test]
fn state_directory_problem_classification_carries_no_path_or_uid() {
    let path = Path::new("/var/tmp/crucible-code-sandbox-1000-v1");
    let path_text = path.display().to_string();
    let foreign_ids = ["824601", "824602"];
    // `private_directory_problem` checks the owner first, so `WrongGroup` and
    // `WrongMode` are reached only on this user's own directory: their
    // classification must say how to fix it in place, never to remove it or
    // that an administrator may be needed, unlike a squat by another user, a
    // symlink or a plain file.
    let removal_remedy = [
        StateDirectoryProblem::Symlink,
        StateDirectoryProblem::NotADirectory,
        StateDirectoryProblem::WrongOwner(824_601),
    ];
    let in_place_remedy = [
        StateDirectoryProblem::WrongGroup(824_602),
        StateDirectoryProblem::WrongMode(0o750),
    ];
    for problem in removal_remedy.iter().chain(&in_place_remedy) {
        let classification = problem.classification();
        assert!(
            !classification.contains(&path_text),
            "{problem:?}'s classification carries the path: {classification}"
        );
        assert!(
            !foreign_ids.iter().any(|id| classification.contains(id)),
            "{problem:?}'s classification carries a numeric id: {classification}"
        );
    }
    for problem in removal_remedy {
        let classification = problem.classification();
        assert!(
            classification.contains("must be removed"),
            "{problem:?}'s classification names no removal remedy: {classification}"
        );
    }
    for problem in in_place_remedy {
        let classification = problem.classification();
        assert!(
            !classification.contains("must be removed")
                && !classification.contains("administrator"),
            "{problem:?}'s classification tells the user to remove their own directory: \
             {classification}"
        );
        assert!(
            classification.contains("restoring"),
            "{problem:?}'s classification names no in-place remedy: {classification}"
        );
    }
    // `WrongGroup` and `WrongMode` cannot be told apart without the id or mode
    // the classification withholds, so they share one reason.
    assert_eq!(
        StateDirectoryProblem::WrongGroup(824_602).classification(),
        StateDirectoryProblem::WrongMode(0o750).classification()
    );
    // Every other pair is its own reason.
    assert_ne!(
        StateDirectoryProblem::Symlink.classification(),
        StateDirectoryProblem::NotADirectory.classification()
    );
    assert_ne!(
        StateDirectoryProblem::WrongOwner(824_601).classification(),
        StateDirectoryProblem::WrongGroup(824_602).classification()
    );
}

#[test]
fn open_state_directory_tells_a_symlink_from_a_plain_file_at_that_name() {
    let sample = crate::sample::Sample::new("sandbox-state-directory-open");
    let base = sample.root();

    let target = base.join("elsewhere");
    create_private_test_directory(&target);
    let link = base.join("link");
    crate::sample::symlink(&target, &link);
    let refused = open_state_directory(&link).expect_err("a symlink is refused");
    assert!(
        refused.to_string().contains("is a symlink"),
        "a symlink at the state path was not named as one: {refused}"
    );

    let file = base.join("plain-file");
    fs::write(&file, b"").expect("a plain file");
    let refused = open_state_directory(&file).expect_err("a plain file is refused");
    assert!(
        refused.to_string().contains("is not a directory"),
        "a plain file at the state path was not named as one: {refused}"
    );
}

/// `RegistryLease::acquire` used to discard `acquire_at`'s error and always
/// report the fixed "sandbox lifecycle registry admission is unavailable"
/// reason, so none of the refusal text this module builds ever reached a
/// user — every confined run's first touch of the state directory is this
/// lease (`linux/mod.rs`, `RegistryLease::acquire`). The symlink case below
/// goes through `acquire_at_classified`, the exact function both `acquire`
/// and this test call, so reverting its mapping back to that fixed reason
/// turns this test red; it passes once the classified, path-free, uid-free
/// reason reaches the caller instead.
#[test]
fn registry_admission_reason_classifies_the_state_directory_problem_without_the_path_or_a_uid() {
    // Wrong owner: synthetic, no second user needed. A real mismatched-uid
    // directory cannot be made without root, so the tagged error is built the
    // same way `open_state_directory` builds it.
    let path = Path::new("/var/tmp/crucible-code-sandbox-1000-v1");
    let wrong_owner = refuse_state_directory(StateDirectoryProblem::WrongOwner(824_601), path);
    let reason = registry_admission_reason(&wrong_owner);
    assert_eq!(
        &*reason,
        StateDirectoryProblem::WrongOwner(824_601).classification()
    );
    assert_ne!(
        &*reason, "sandbox lifecycle registry admission is unavailable",
        "the wrong-owner case fell back to the fixed reason instead of being classified"
    );
    assert!(
        !reason.contains("824601") && !reason.contains(&path.display().to_string()),
        "the reason a user sees carried another user's uid or the path: {reason}"
    );

    // Symlink: live, no second user needed either — this user's own symlink
    // squats the name, taken through `acquire_at_classified`, the exact
    // function `RegistryLease::acquire` calls, so this proves the reason
    // reaches a caller and not merely that `registry_admission_reason`
    // computes it.
    let sample = crate::sample::Sample::new("sandbox-registry-admission-reason");
    let base = sample.root();
    let target = base.join("elsewhere");
    create_private_test_directory(&target);
    let link = base.join("link");
    crate::sample::symlink(&target, &link);
    let Err(SandboxError::BackendUnavailable { reason }) =
        RegistryLease::acquire_at_classified(&link)
    else {
        panic!("a symlink at the state path was granted a registry lease");
    };
    assert_eq!(&*reason, StateDirectoryProblem::Symlink.classification());
    assert_ne!(
        &*reason, "sandbox lifecycle registry admission is unavailable",
        "the symlink case fell back to the fixed reason instead of being classified"
    );
    assert!(
        !reason.contains(&link.display().to_string()),
        "the reason a user sees carried the state path: {reason}"
    );

    // Every other admission failure keeps the fixed reason.
    let unrelated = io::Error::other("some other cause entirely");
    assert_eq!(
        &*registry_admission_reason(&unrelated),
        "sandbox lifecycle registry admission is unavailable"
    );
}

/// Under test, the records of the journal in `stage`, read without the lock
/// its owner holds while it lives.
pub(in crate::linux) fn journaled(stage: &Path) -> io::Result<Vec<Record>> {
    let descriptor = rustix::fs::open(
        stage.join("transaction.wal"),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Ok(recover_wal_file(File::from(descriptor))?.machine.records)
}

/// Under test, how recovery settles a command whose owner died with `records`
/// journaled, `Initialized` first, in a stage of its own under `base`: the
/// terminal its journal ends on, and whether a reconcile then removed the
/// stage.
pub(in crate::linux) fn after_a_crash(
    base: &Path,
    records: &[Record],
) -> io::Result<(Option<Record>, bool)> {
    let Some((Record::Initialized(mode), rest)) = records.split_first() else {
        return Err(invalid("a journal begins with its initialization"));
    };
    let sandbox = SandboxId::new();
    let stage = base.join(stage_name(sandbox));
    create_state_directory(&stage)?;
    let dead = OwnerIdentity {
        pid: u32::MAX,
        start: 1,
        boot: boot_identity()?,
    };
    let mut transaction =
        Transaction::start_owned(&stage, sandbox, Invocation::new(*mode, None)?, dead)?;
    for record in rest {
        transaction.append(*record)?;
    }
    drop(transaction);
    let mut recovered = recover_wal(&stage.join("transaction.wal"))?;
    if !recovered.machine.is_terminal() {
        // Settled or refused, the journal says which: a refusal still
        // journals the quarantine it leaves.
        let _ = recover_stale_transaction(&stage, &mut recovered);
    }
    let terminal = recovered.machine.terminal();
    drop(recovered);
    let _ = reconcile_stale_transactions(base);
    Ok((terminal, !stage.exists()))
}
