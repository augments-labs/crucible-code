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
fn writable_lease_is_exclusive_and_released_with_its_descriptor() {
    let sample = crate::sample::Sample::new("sandbox-transaction-lock");
    let state = sample.root().join("state");
    let first = Lease::acquire_at(&state).expect("first lease");
    assert!(Lease::acquire_at(&state).is_err());
    drop(first);
    Lease::acquire_at(&state).expect("lease after descriptor close");
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
    let state_root = sample.root().join("state");
    let projection_root = sample.root().join("stage");
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(&projection_root).expect("stage directory");
    let lease = Lease::acquire_at(&state_root).expect("transaction lease");
    let key = CallResultKey::from_digest([0x3c; 32]);
    let receipt = [0x5a; 32];
    let mut transaction = Transaction::start(
        Some(lease),
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
    let stage = stale_journal(&sample, &base, true);

    reconcile_stale_transactions(&base).expect("terminal cleanup");
    assert!(!stage.exists());
    reconcile_stale_transactions(&base).expect("idempotent terminal cleanup");
}

#[test]
fn live_nonterminal_transactions_are_skipped_and_retain_their_evidence() {
    let sample = crate::sample::Sample::new("sandbox-nonterminal-recovery");
    let base = sample.root().join("recovery");
    create_private_test_directory(&base);
    let stage = stale_journal(&sample, &base, false);

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
    let lease = Lease::acquire_at(&sample.root().join("state")).expect("transaction lease");
    let mut transaction = Transaction::start_owned(
        Some(lease),
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
    let mut transaction = Transaction::start(
        None,
        &stage,
        SandboxId::new(),
        InvocationMode::Foreground,
        None,
    )
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
    let stage = stale_journal_with(&sample, &base, dead_owner(), &[Record::Prepared]);

    reconcile_stale_transactions(&base).expect("pre-release recovery");
    assert!(!stage.exists());
}

#[test]
fn dead_post_release_owners_roll_back_when_no_apply_was_authorized() {
    let sample = crate::sample::Sample::new("sandbox-dead-post-release-recovery");
    let base = sample.root().join("recovery");
    create_private_test_directory(&base);
    let stage = stale_journal_with(
        &sample,
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
        &sample,
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
        &sample,
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
        &sample,
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
    let state_root = sample.root().join("state");
    let projection_root = sample.root().join("stage");
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(&projection_root).expect("stage directory");
    let lease = Lease::acquire_at(&state_root).expect("transaction lease");
    let mut transaction = Transaction::start(
        Some(lease),
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

fn stale_journal(sample: &crate::sample::Sample, base: &Path, terminal: bool) -> PathBuf {
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
        sample,
        base,
        OwnerIdentity::current().expect("current owner"),
        &records,
    )
}

fn stale_journal_with(
    sample: &crate::sample::Sample,
    base: &Path,
    owner: OwnerIdentity,
    records: &[Record],
) -> PathBuf {
    stale_journal_with_invocation(
        sample,
        base,
        owner,
        Invocation::new(InvocationMode::Foreground, None).expect("foreground identity"),
        records,
    )
}

fn stale_journal_with_invocation(
    sample: &crate::sample::Sample,
    base: &Path,
    owner: OwnerIdentity,
    invocation: Invocation,
    records: &[Record],
) -> PathBuf {
    let sandbox = SandboxId::new();
    let stage = base.join(format!("crucible-projection-{sandbox}"));
    create_private_test_directory(&stage);
    let lease = Lease::acquire_at(&sample.root().join("state")).expect("transaction lease");
    let mut transaction = Transaction::start_owned(Some(lease), &stage, sandbox, invocation, owner)
        .expect("transaction journal");
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
    let stage = stale_journal_with(&sample, &base, dead_owner(), &[Record::Prepared]);
    let journal = File::options()
        .read(true)
        .write(true)
        .open(stage.join("transaction.wal"))
        .expect("stale journal");
    rustix::fs::flock(&journal, FlockOperation::NonBlockingLockExclusive).expect("journal lock");
    let mut child = lend_to_departing_child(&journal);
    drop(journal);

    reconcile_stale_transactions(&base).expect("pre-release recovery");
    assert!(
        !stage.exists(),
        "a stale journal whose lock only a departing child still holds was skipped"
    );
    child.wait().expect("departing child");
}

#[test]
fn a_writable_lease_lent_to_a_departing_child_does_not_refuse_the_next_writer() {
    let sample = crate::sample::Sample::new("sandbox-lent-writable-lease");
    let state = sample.root().join("state");
    let first = Lease::acquire_at(&state).expect("first lease");
    let mut child = lend_to_departing_child(first.lock());
    drop(first);

    Lease::acquire_at(&state).expect("lease once only a departing child holds the old one");
    child.wait().expect("departing child");
}

/// Hands a copy of `held` to a child that keeps it open briefly, as a forked
/// child does between `fork` and `exec` while it still carries the parent's
/// descriptor table.
fn lend_to_departing_child(held: &File) -> std::process::Child {
    let copy = held.try_clone().expect("descriptor copy");
    std::process::Command::new("sleep")
        .arg("0.05")
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
