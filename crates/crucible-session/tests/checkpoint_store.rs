//! File-backed execution-checkpoint contract.

use std::fs;

use crucible_core::{
    Ancestry, CheckpointId, CheckpointStore, ExecutionCheckpoint, InvocationRecord, Message,
    PendingAction, PendingApproval, PendingExternalTool, RecoveryAction, ResumeDigest, ResumeScope,
    RunHistory, RunItem, StopReason, TOOL_ARGUMENT_BYTES, ToolArgs, ToolCall, ToolEffect, ToolId,
    ToolOutcome, ToolOutput, ToolResult,
};
#[cfg(unix)]
use crucible_core::{
    ResumeEvidence, SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance,
    SandboxCapabilities, SandboxCapability, SandboxCheckpoint, SandboxCleanup, SandboxFeature,
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxId,
    SandboxInspection, SandboxManifest, SandboxMode, SandboxNetworkPolicy, SandboxPolicy,
    SandboxResourceLimits,
};
use crucible_session::{CHECKPOINT_FORMAT, CheckpointError, FileCheckpointStore};

fn directory(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "crucible-phase4-{name}-{}-{}",
        std::process::id(),
        CheckpointId::new()
    ))
}

fn scope() -> ResumeScope {
    ResumeScope::new(
        ResumeDigest::new([1; 32]),
        ResumeDigest::new([2; 32]),
        ResumeDigest::new([3; 32]),
        ResumeDigest::new([4; 32]),
    )
}

fn call(id: &str) -> ToolCall {
    ToolCall {
        id: ToolId::new(id),
        name: "write".into(),
        args: ToolArgs::new(r#"{"path":"notes.txt"}"#),
    }
}

// The fixture is a POSIX absolute path, which no Windows path type accepts;
// Windows has no confinement backend to give it a native shape.
#[cfg(unix)]
#[allow(clippy::expect_used)]
fn sandbox_checkpoint() -> SandboxCheckpoint {
    let policy = SandboxPolicy::new(
        SandboxMode::Required,
        [SandboxFilesystemRule::new(
            "/workspace",
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("checkpoint workspace rule")],
        "/workspace",
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("checkpoint sandbox policy");
    let capabilities = [
        SandboxFeature::Filesystem,
        SandboxFeature::NetworkDeny,
        SandboxFeature::DescriptorIsolation,
        SandboxFeature::ProcessIsolation,
        SandboxFeature::KernelSurface,
        SandboxFeature::PrivilegeIsolation,
        SandboxFeature::Audit,
    ]
    .into_iter()
    .fold(SandboxCapabilities::none(), |claims, feature| {
        claims.with(feature, SandboxCapability::Enforced)
    });
    SandboxCheckpoint::from_inspection(
        &SandboxInspection::new(
            SandboxId::new(),
            SandboxBackendIdentity::new(
                SandboxBackendId::new("checkpoint-backend").expect("checkpoint backend id"),
                "1.0",
                SandboxBackendProvenance::System,
                Some([0x44; 32]),
            )
            .expect("checkpoint backend identity"),
            capabilities,
            &policy,
            &SandboxManifest::empty(),
            true,
            None::<Box<str>>,
            SandboxCleanup::Pending,
        )
        .expect("checkpoint sandbox inspection"),
    )
}

#[test]
fn pending_actions_and_finished_invocations_round_trip_in_their_own_versioned_file() {
    let directory = directory("round-trip");
    let mut store = FileCheckpointStore::in_directory(&directory);
    let id = CheckpointId::new();
    let ancestry = Ancestry::new().child();
    let mut checkpoint =
        ExecutionCheckpoint::new(id, ancestry, scope(), None, 1_000, 9_000).unwrap();
    let pending = PendingApproval::new(call("approval"), ancestry, 8_000);
    let pending_id = pending.id();
    checkpoint
        .pending_mut()
        .insert(PendingAction::Approval(pending))
        .unwrap();
    checkpoint.pending_mut().reject(pending_id).unwrap();

    let mut invocation =
        InvocationRecord::new(call("finished"), ancestry, ToolEffect::NonIdempotent, None);
    invocation.start().unwrap();
    invocation
        .finish(ToolOutcome::Succeeded, ToolOutput::ok("result-canary"))
        .unwrap();
    checkpoint.add_invocation(invocation).unwrap();

    store.save(&checkpoint).expect("checkpoint is durable");
    let loaded = store
        .load(id)
        .expect("checkpoint reads")
        .expect("it exists");

    assert_eq!(loaded.id(), id);
    assert_eq!(loaded.ancestry(), ancestry);
    assert_eq!(loaded.pending().entries().count(), 1);
    assert_eq!(loaded.invocations().len(), 1);
    assert_eq!(
        loaded
            .invocations()
            .first()
            .expect("one invocation")
            .recovery(),
        crucible_core::RecoveryAction::UseRecordedResult
    );

    let path = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "checkpoint")
        })
        .expect("one checkpoint file");
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains(&format!(r#""format":{CHECKPOINT_FORMAT}"#)));
    assert!(text.contains("result-canary"));
    assert!(
        !text.contains("session\""),
        "checkpoint is not a session header: {text}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn sandbox_identity_round_trips_and_resume_refuses_a_weaker_live_backend() {
    let directory = directory("sandbox-resume");
    let mut store = FileCheckpointStore::in_directory(&directory);
    let id = CheckpointId::new();
    let saved = sandbox_checkpoint();
    let mut checkpoint =
        ExecutionCheckpoint::new(id, Ancestry::new(), scope(), None, 1_000, 9_000).unwrap();
    checkpoint.add_sandbox(saved.clone()).unwrap();
    store.save(&checkpoint).unwrap();

    let loaded = store.load(id).unwrap().unwrap();
    assert_eq!(loaded.sandboxes(), std::slice::from_ref(&saved));
    let matching = ResumeEvidence::new(scope(), "policy", "capability", None::<Box<str>>)
        .with_sandbox(saved.clone())
        .unwrap();
    assert!(loaded.validate_resume(&matching, 2_000).is_ok());

    let weaker = SandboxCheckpoint::restore(
        SandboxId::new(),
        saved.backend().clone(),
        saved
            .capabilities()
            .clone()
            .with(SandboxFeature::Audit, SandboxCapability::Observed),
        saved.mode(),
        saved.network(),
        saved.policy_digest(),
        saved.manifest_digest(),
        saved.confined(),
    )
    .unwrap();
    let weaker = ResumeEvidence::new(scope(), "policy", "capability", None::<Box<str>>)
        .with_sandbox(weaker)
        .unwrap();
    assert!(matches!(
        loaded.validate_resume(&weaker, 2_000),
        Err(crucible_core::InterruptionError::ResumeMismatch)
    ));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn format_one_checkpoints_remain_readable_without_inventing_sandbox_identity() {
    let directory = directory("format-one");
    let mut store = FileCheckpointStore::in_directory(&directory);
    let id = CheckpointId::new();
    let ancestry = Ancestry::new();
    let checkpoint = ExecutionCheckpoint::new(id, ancestry, scope(), None, 1_000, 9_000).unwrap();
    store.save(&checkpoint).unwrap();

    let path = directory.join(format!("{id}.checkpoint"));
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let object = document.as_object_mut().expect("checkpoint object");
    object.insert("format".to_owned(), serde_json::json!(1));
    object.remove("sandboxes");
    fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();

    let loaded = store.load(id).unwrap().expect("format-one checkpoint");
    assert_eq!(loaded.id(), id);
    assert_eq!(loaded.ancestry(), ancestry);
    assert!(loaded.sandboxes().is_empty());

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn removal_is_idempotent_and_does_not_create_a_conversation_log() {
    let directory = directory("remove");
    let mut store = FileCheckpointStore::in_directory(&directory);
    let id = CheckpointId::new();
    let checkpoint = ExecutionCheckpoint::new(id, Ancestry::new(), scope(), None, 1, 10).unwrap();

    store.save(&checkpoint).unwrap();
    store.remove(id).unwrap();
    store.remove(id).unwrap();
    assert!(store.load(id).unwrap().is_none());
    assert!(fs::read_dir(&directory).unwrap().all(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_none_or(|ext| ext != "jsonl")
    }));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn every_pending_outcome_survives_stop_and_resume_with_one_provider_result() {
    let directory = directory("resume-outcomes");
    let mut store = FileCheckpointStore::in_directory(&directory);
    let checkpoint_id = CheckpointId::new();
    let ancestry = Ancestry::new().child();
    let mut checkpoint =
        ExecutionCheckpoint::new(checkpoint_id, ancestry, scope(), None, 1_000, 9_000).unwrap();

    let approved = PendingApproval::new(call("approved"), ancestry, 8_000);
    let approved_action = approved.id();
    let approved_invocation = approved.invocation();
    let rejected = PendingApproval::new(call("rejected"), ancestry, 8_000);
    let rejected_action = rejected.id();
    let cancelled = PendingApproval::new(call("cancelled"), ancestry, 8_000);
    let cancelled_action = cancelled.id();
    let external = PendingExternalTool::new(call("external"), ancestry, 8_000);
    let external_action = external.id();

    for action in [
        PendingAction::Approval(approved),
        PendingAction::Approval(rejected),
        PendingAction::Approval(cancelled),
        PendingAction::ExternalTool(external),
    ] {
        checkpoint.pending_mut().insert(action).unwrap();
    }
    store.save(&checkpoint).unwrap();

    // A fresh process applies external decisions and checkpoints them before
    // any provider transcript is projected.
    let mut resumed = store.load(checkpoint_id).unwrap().unwrap();
    resumed.pending_mut().approve(approved_action).unwrap();
    resumed.pending_mut().reject(rejected_action).unwrap();
    resumed.pending_mut().cancel(cancelled_action).unwrap();
    resumed
        .pending_mut()
        .resolve_external(external_action, ToolOutput::ok("external result"))
        .unwrap();
    store.save(&resumed).unwrap();

    let mut resumed = store.load(checkpoint_id).unwrap().unwrap();
    let approved = resumed.pending_mut().resume(approved_action).unwrap();
    let mut invocation = approved
        .approved_invocation(ToolEffect::NonIdempotent, None)
        .unwrap();
    assert_eq!(invocation.id(), approved_invocation);
    invocation.start().unwrap();
    invocation
        .finish(ToolOutcome::Succeeded, ToolOutput::ok("approved result"))
        .unwrap();
    resumed.add_invocation(invocation).unwrap();

    let mut results = vec![ToolResult {
        id: ToolId::new("approved"),
        output: ToolOutput::ok("approved result"),
    }];
    for action in [rejected_action, cancelled_action, external_action] {
        results.push(
            resumed
                .pending_mut()
                .resume(action)
                .unwrap()
                .into_tool_result()
                .expect("a tool-bearing resolution"),
        );
    }
    store.save(&resumed).unwrap();

    let completed = store.load(checkpoint_id).unwrap().unwrap();
    assert_eq!(completed.invocations().len(), 1);
    assert_eq!(
        completed
            .invocations()
            .first()
            .expect("one invocation")
            .recovery(),
        RecoveryAction::UseRecordedResult
    );

    let mut history = RunHistory::new();
    history
        .push(
            RunItem::message(
                ancestry,
                Message::Agent {
                    continuation: None,
                    text: "".into(),
                    calls: ["approved", "rejected", "cancelled", "external"]
                        .into_iter()
                        .map(call)
                        .collect(),
                    stop: Some(StopReason::WantsTools),
                },
            )
            .unwrap(),
        )
        .unwrap();
    history
        .push(RunItem::message(ancestry, Message::ToolResults(results)).unwrap())
        .unwrap();
    let projected = history.project().expect("all four calls are settled");
    let Message::ToolResults(results) = projected
        .messages()
        .get(1)
        .expect("the call is followed by its results")
    else {
        panic!("projected result message");
    };
    assert_eq!(results.len(), 4);

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn all_three_crash_windows_round_trip_to_their_explicit_recovery_policy() {
    let directory = directory("crash-windows");
    let mut store = FileCheckpointStore::in_directory(&directory);
    let checkpoint_id = CheckpointId::new();
    let ancestry = Ancestry::new();
    let mut checkpoint =
        ExecutionCheckpoint::new(checkpoint_id, ancestry, scope(), None, 1_000, 9_000).unwrap();

    let prepared = InvocationRecord::new(
        call("before-effect"),
        ancestry,
        ToolEffect::NonIdempotent,
        None,
    );
    let mut ambiguous = InvocationRecord::new(
        call("after-effect"),
        ancestry,
        ToolEffect::NonIdempotent,
        None,
    );
    ambiguous.start().unwrap();
    let mut completed = InvocationRecord::new(
        call("after-result"),
        ancestry,
        ToolEffect::NonIdempotent,
        None,
    );
    completed.start().unwrap();
    completed
        .finish(ToolOutcome::Succeeded, ToolOutput::ok("durable result"))
        .unwrap();

    checkpoint.add_invocation(prepared).unwrap();
    checkpoint.add_invocation(ambiguous).unwrap();
    checkpoint.add_invocation(completed).unwrap();
    store.save(&checkpoint).unwrap();

    let loaded = store.load(checkpoint_id).unwrap().unwrap();
    assert_eq!(
        loaded
            .invocations()
            .iter()
            .map(InvocationRecord::recovery)
            .collect::<Vec<_>>(),
        [
            RecoveryAction::Retry,
            RecoveryAction::Reconcile,
            RecoveryAction::UseRecordedResult,
        ]
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn an_oversized_encoded_checkpoint_is_refused_before_a_file_is_replaced() {
    let directory = directory("encoded-bound");
    let mut store = FileCheckpointStore::in_directory(&directory);
    let checkpoint_id = CheckpointId::new();
    let ancestry = Ancestry::new();
    let mut checkpoint =
        ExecutionCheckpoint::new(checkpoint_id, ancestry, scope(), None, 1_000, 9_000).unwrap();

    for id in ["large-1", "large-2"] {
        checkpoint
            .add_invocation(InvocationRecord::new(
                ToolCall {
                    id: ToolId::new(id),
                    name: "write".into(),
                    args: ToolArgs::new("x".repeat(TOOL_ARGUMENT_BYTES)),
                },
                ancestry,
                ToolEffect::NonIdempotent,
                None,
            ))
            .unwrap();
    }

    assert!(matches!(
        store.save(&checkpoint),
        Err(CheckpointError::TooLarge)
    ));
    assert!(fs::read_dir(&directory).unwrap().all(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_none_or(|ext| ext != "checkpoint")
    }));

    fs::remove_dir_all(directory).unwrap();
}
