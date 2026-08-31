//! Cross-module contracts for framework history and durable interruption.

use crucible_core::{
    ActionResolution, Ancestry, ApprovalDecision, CacheCheckpoint, CheckpointId, CompactionRecord,
    CustomEntry, CustomProjector, ExecutionCheckpoint, IdempotencyKey, InputTokenUsage,
    InterruptionError, InvocationId, InvocationRecord, InvocationState, JournalError,
    MAX_RUN_ITEM_RETAINED_BYTES, Message, PendingAction, PendingActions, PendingApproval,
    PendingExternalTool, PendingHumanInput, PromptCacheFingerprint, PromptCachePolicyVersion,
    PromptCacheResourceId, PromptCacheScopeDigest, RecoveryAction, ResumeDigest, ResumeEvidence,
    ResumeScope, RunHistory, RunItem, StopReason, TOOL_RESULT_BYTES, ToolArgs, ToolCall,
    ToolEffect, ToolId, ToolOutcome, ToolOutput, ToolResult,
};

fn call(id: &str) -> ToolCall {
    ToolCall {
        id: ToolId::new(id),
        name: "write".into(),
        args: ToolArgs::new(r#"{"path":"notes.txt"}"#),
    }
}

struct ProjectCustomEntry;

impl CustomProjector for ProjectCustomEntry {
    fn project(&self, _entry: &CustomEntry) -> Option<Message> {
        Some(Message::said("explicitly projected"))
    }
}

#[test]
fn provider_projection_refuses_an_unanswered_or_twice_answered_call() {
    let ancestry = Ancestry::new();
    let mut history = RunHistory::new();
    history
        .push(
            RunItem::message(
                ancestry,
                Message::Agent {
                    text: "".into(),
                    calls: vec![call("call-1")],
                    stop: Some(StopReason::WantsTools),
                },
            )
            .expect("a bounded assistant item"),
        )
        .expect("one bounded item");

    assert!(matches!(
        history.project(),
        Err(JournalError::UnansweredCall(_))
    ));

    let answered = RunItem::message(
        ancestry,
        Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call-1"),
            output: ToolOutput::ok("done"),
        }]),
    )
    .expect("a bounded result");
    history.push(answered.clone()).expect("room for result");
    assert_eq!(
        history
            .project()
            .expect("a settled transcript")
            .messages()
            .len(),
        2
    );

    history.push(answered).expect("room for duplicate");
    assert!(matches!(
        history.project(),
        Err(JournalError::DuplicateResult(_))
    ));
}

#[test]
fn provider_projection_does_not_let_an_unanswered_call_cross_a_later_message() {
    let ancestry = Ancestry::new();
    let mut history = RunHistory::new();
    history
        .push(
            RunItem::message(
                ancestry,
                Message::Agent {
                    text: "".into(),
                    calls: vec![call("call-1")],
                    stop: Some(StopReason::WantsTools),
                },
            )
            .unwrap(),
        )
        .unwrap();
    history
        .push(RunItem::message(ancestry, Message::said("too early")).unwrap())
        .unwrap();
    history
        .push(
            RunItem::message(
                ancestry,
                Message::ToolResults(vec![ToolResult {
                    id: ToolId::new("call-1"),
                    output: ToolOutput::ok("late"),
                }]),
            )
            .unwrap(),
        )
        .unwrap();

    assert!(matches!(
        history.project(),
        Err(JournalError::UnansweredCall(_))
    ));
}

#[test]
fn custom_entries_are_bounded_and_invisible_until_a_projector_opts_in() {
    let custom = CustomEntry::new(
        "example.notes",
        1,
        r#"{"state":"private-canary"}"#,
        "fixture",
    )
    .expect("bounded custom entry");
    let mut history = RunHistory::new();
    history.push(RunItem::Custom(custom)).expect("one entry");

    assert!(history.project().expect("custom is skipped").is_empty());

    let transcript = history
        .project_with(&ProjectCustomEntry)
        .expect("explicit projection");
    assert_eq!(
        transcript.messages(),
        &[Message::said("explicitly projected")]
    );
    assert!(format!("{:?}", history.items()).contains("[redacted]"));
    assert!(!format!("{:?}", history.items()).contains("private-canary"));
}

#[test]
fn compaction_metadata_correlates_without_copying_recap_plaintext() {
    let ancestry = Ancestry::new();
    let first = CompactionRecord::new(ancestry, 4, "recap-plaintext-canary");
    let same = CompactionRecord::new(ancestry, 4, "recap-plaintext-canary");
    let changed = CompactionRecord::new(ancestry, 4, "different recap");
    let mut history = RunHistory::new();
    history.push(RunItem::Compaction(first)).unwrap();

    assert_eq!(first.recap_digest(), same.recap_digest());
    assert_ne!(first.recap_digest(), changed.recap_digest());
    assert_eq!(first.recap_bytes(), "recap-plaintext-canary".len());
    assert!(history.project().unwrap().is_empty());
    assert!(!format!("{first:?}").contains("recap-plaintext-canary"));
}

#[test]
fn approval_resolution_is_idempotent_and_has_only_one_latest_decision() {
    let ancestry = Ancestry::new();
    let approval = PendingApproval::new(call("approval-1"), ancestry, 4_000);
    let id = approval.id();
    let mut pending = PendingActions::new();
    pending
        .insert(PendingAction::Approval(approval))
        .expect("one pending action");

    assert!(pending.approve(id).expect("first decision").changed());
    assert!(!pending.approve(id).expect("same decision").changed());
    assert!(
        pending
            .reject(id)
            .expect("latest decision replaces")
            .changed()
    );
    assert_eq!(
        pending.resolution(id),
        Some(&ActionResolution::Approval(ApprovalDecision::Rejected))
    );
    assert!(!pending.reject(id).expect("same rejection").changed());
}

#[test]
fn pending_and_invocation_snapshots_reject_duplicate_provider_call_ids() {
    let ancestry = Ancestry::new();
    let mut pending = PendingActions::new();
    pending
        .insert(PendingAction::Approval(PendingApproval::new(
            call("same-call"),
            ancestry,
            4_000,
        )))
        .unwrap();
    assert!(matches!(
        pending.insert(PendingAction::ExternalTool(PendingExternalTool::new(
            call("same-call"),
            ancestry,
            4_000,
        ))),
        Err(InterruptionError::DuplicateCall(_))
    ));
    assert_eq!(pending.entries().count(), 1);

    let mut checkpoint =
        ExecutionCheckpoint::new(CheckpointId::new(), ancestry, scope(1), None, 1_000, 5_000)
            .unwrap();
    checkpoint
        .add_invocation(InvocationRecord::new(
            call("same-invocation-call"),
            ancestry,
            ToolEffect::NonIdempotent,
            None,
        ))
        .unwrap();
    assert!(matches!(
        checkpoint.add_invocation(InvocationRecord::new(
            call("same-invocation-call"),
            ancestry,
            ToolEffect::NonIdempotent,
            None,
        )),
        Err(InterruptionError::DuplicateCall(_))
    ));
    assert_eq!(checkpoint.invocations().len(), 1);
}

#[test]
fn approved_resume_keeps_one_invocation_identity_across_repetition() {
    let ancestry = Ancestry::new();
    let approval = PendingApproval::new(call("approval-1"), ancestry, 4_000);
    let action = approval.id();
    let invocation = approval.invocation();
    let mut pending = PendingActions::new();
    pending.insert(PendingAction::Approval(approval)).unwrap();
    pending.approve(action).unwrap();

    let first = pending.resume(action).unwrap();
    let second = pending.resume(action).unwrap();
    assert_eq!(first.invocation(), Some(invocation));
    assert_eq!(second.invocation(), Some(invocation));
    assert_eq!(
        first
            .approved_invocation(ToolEffect::NonIdempotent, None)
            .unwrap()
            .id(),
        invocation
    );
    assert_eq!(
        second
            .approved_invocation(ToolEffect::NonIdempotent, None)
            .unwrap()
            .id(),
        invocation
    );
}

#[test]
fn rejected_cancelled_and_external_actions_resume_to_exactly_one_tool_result() {
    let ancestry = Ancestry::new();
    let mut pending = PendingActions::new();
    let rejected = PendingApproval::new(call("rejected"), ancestry, 4_000);
    let cancelled = PendingApproval::new(call("cancelled"), ancestry, 4_000);
    let external = PendingExternalTool::new(call("external"), ancestry, 4_000);
    let rejected_id = rejected.id();
    let cancelled_id = cancelled.id();
    let external_id = external.id();
    pending.insert(PendingAction::Approval(rejected)).unwrap();
    pending.insert(PendingAction::Approval(cancelled)).unwrap();
    pending
        .insert(PendingAction::ExternalTool(external))
        .unwrap();

    pending.reject(rejected_id).unwrap();
    pending.cancel(cancelled_id).unwrap();
    pending
        .resolve_external(external_id, ToolOutput::ok("remote result"))
        .unwrap();

    let results = [rejected_id, cancelled_id, external_id]
        .into_iter()
        .map(|id| pending.resume(id).expect("resolved action resumes"))
        .filter_map(crucible_core::ResumedAction::into_tool_result)
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 3);
    assert_eq!(
        results
            .iter()
            .filter(|one| one.id == ToolId::new("rejected"))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|one| one.id == ToolId::new("cancelled"))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|one| one.id == ToolId::new("external"))
            .count(),
        1
    );
}

#[test]
fn human_input_survives_resolution_without_becoming_a_provider_message_by_default() {
    let ancestry = Ancestry::new().child();
    let human = PendingHumanInput::new("Which environment?", ancestry, 4_000).unwrap();
    let id = human.id();
    let mut pending = PendingActions::new();
    pending.insert(PendingAction::HumanInput(human)).unwrap();
    pending.answer(id, "staging").unwrap();

    let resumed = pending.resume(id).expect("answered input resumes");
    assert_eq!(resumed.human_input(), Some("staging"));
    assert_eq!(resumed.ancestry(), ancestry);
    assert!(resumed.into_tool_result().is_none());
}

fn scope(fill: u8) -> ResumeScope {
    ResumeScope::new(
        ResumeDigest::new([fill; 32]),
        ResumeDigest::new([fill.wrapping_add(1); 32]),
        ResumeDigest::new([fill.wrapping_add(2); 32]),
        ResumeDigest::new([fill.wrapping_add(3); 32]),
    )
}

#[test]
fn checkpoint_resume_revalidates_every_scope_and_never_claims_a_cache_hit() {
    let cache_scope = PromptCacheScopeDigest::new([0x31; 32]);
    let cache_prefix = PromptCacheFingerprint::new([0x41; 32]);
    let cache = CacheCheckpoint::new(
        PromptCachePolicyVersion::CURRENT.as_str(),
        "capabilities-v1",
        Some("pricing-v1"),
        cache_scope,
        cache_prefix,
        None,
        Some(PromptCacheResourceId::new()),
        Some(5_000),
        false,
    )
    .unwrap();
    let checkpoint = ExecutionCheckpoint::new(
        CheckpointId::new(),
        Ancestry::new(),
        scope(1),
        Some(cache),
        1_000,
        5_000,
    )
    .unwrap();
    let evidence = ResumeEvidence::new(
        scope(1),
        "prompt-cache-policy-v1",
        "capabilities-v1",
        Some("pricing-v1"),
    )
    .with_cache_identity(cache_scope, cache_prefix);

    let validated = checkpoint
        .validate_resume(&evidence, 2_000)
        .expect("exact scope");
    assert_eq!(validated.recovery(), RecoveryAction::Reconcile);
    assert!(
        !format!("{validated:?}")
            .to_ascii_lowercase()
            .contains("hit")
    );
    assert!(
        checkpoint
            .validate_resume(
                &ResumeEvidence::new(
                    scope(9),
                    "prompt-cache-policy-v1",
                    "capabilities-v1",
                    Some("pricing-v1")
                ),
                2_000
            )
            .is_err()
    );
    assert!(
        checkpoint
            .validate_resume(
                &ResumeEvidence::new(
                    scope(1),
                    "prompt-cache-policy-v1",
                    "capabilities-v1",
                    Some("pricing-v1")
                )
                .with_cache_identity(cache_scope, PromptCacheFingerprint::new([0x99; 32])),
                2_000
            )
            .is_err()
    );
    assert!(
        checkpoint
            .validate_resume(
                &ResumeEvidence::new(
                    scope(1),
                    "prompt-cache-policy-v1",
                    "capabilities-v1",
                    Some("pricing-v1")
                ),
                2_000
            )
            .is_err()
    );
}

#[test]
fn checkpoint_resume_refuses_an_expired_unresolved_action() {
    let mut checkpoint = ExecutionCheckpoint::new(
        CheckpointId::new(),
        Ancestry::new(),
        scope(1),
        None,
        1_000,
        5_000,
    )
    .unwrap();
    let ancestry = checkpoint.ancestry();
    checkpoint
        .pending_mut()
        .insert(PendingAction::Approval(PendingApproval::new(
            call("expired"),
            ancestry,
            1_500,
        )))
        .unwrap();

    assert!(
        checkpoint
            .validate_resume(
                &ResumeEvidence::new(scope(1), "policy", "capability", None::<Box<str>>),
                2_000,
            )
            .is_err()
    );
}

#[test]
fn checkpoint_resume_refuses_an_expired_approval_that_has_not_executed() {
    let mut checkpoint = ExecutionCheckpoint::new(
        CheckpointId::new(),
        Ancestry::new(),
        scope(1),
        None,
        1_000,
        5_000,
    )
    .unwrap();
    let approval = PendingApproval::new(call("stale-approval"), checkpoint.ancestry(), 1_500);
    let id = approval.id();
    checkpoint
        .pending_mut()
        .insert(PendingAction::Approval(approval))
        .unwrap();
    checkpoint.pending_mut().approve(id).unwrap();

    assert!(matches!(
        checkpoint.validate_resume(
            &ResumeEvidence::new(scope(1), "policy", "capability", None::<Box<str>>),
            2_000,
        ),
        Err(InterruptionError::Expired)
    ));
}

#[test]
fn invocation_recovery_distinguishes_all_three_crash_windows() {
    let ancestry = Ancestry::new();
    let prepared =
        InvocationRecord::new(call("prepared"), ancestry, ToolEffect::NonIdempotent, None);
    assert_eq!(prepared.recovery(), RecoveryAction::Retry);

    let mut ambiguous =
        InvocationRecord::new(call("ambiguous"), ancestry, ToolEffect::NonIdempotent, None);
    ambiguous.start().unwrap();
    assert_eq!(ambiguous.recovery(), RecoveryAction::Reconcile);

    let mut safe = InvocationRecord::new(
        call("idempotent"),
        ancestry,
        ToolEffect::Idempotent,
        Some(IdempotencyKey::new("operation-42").unwrap()),
    );
    safe.start().unwrap();
    assert_eq!(safe.recovery(), RecoveryAction::RetryWithIdempotencyKey);

    let mut completed =
        InvocationRecord::new(call("completed"), ancestry, ToolEffect::NonIdempotent, None);
    completed.start().unwrap();
    completed
        .finish(ToolOutcome::Succeeded, ToolOutput::ok("stored result"))
        .unwrap();
    assert_eq!(completed.recovery(), RecoveryAction::UseRecordedResult);
    assert!(
        !completed
            .finish(ToolOutcome::Succeeded, ToolOutput::ok("stored result"))
            .unwrap()
            .changed()
    );
}

#[test]
fn retained_phase_four_fields_are_rejected_before_storage() {
    assert!(CustomEntry::new("bad namespace", 1, "{}", "fixture").is_err());
    assert!(CustomEntry::new("example.ok", 1, "x".repeat(70_000), "fixture").is_err());
    assert!(IdempotencyKey::new("x".repeat(2_000)).is_err());
    assert!(
        RunItem::message(
            Ancestry::new(),
            Message::said("x".repeat(MAX_RUN_ITEM_RETAINED_BYTES + 1)),
        )
        .is_err()
    );

    let usage = InputTokenUsage::inclusive_read(Some(100), Some(40)).unwrap();
    assert_eq!(usage.uncached, Some(60));
}

#[test]
fn restored_results_must_already_fit_the_encoded_result_ceiling() {
    let ancestry = Ancestry::new();
    let encoded_too_large = "\"".repeat(TOOL_RESULT_BYTES / 2 + 1);
    assert!(encoded_too_large.len() < TOOL_RESULT_BYTES);

    let external = PendingExternalTool::new(call("external-bound"), ancestry, 4_000);
    let mut pending = PendingActions::new();
    assert!(matches!(
        pending.restore(
            PendingAction::ExternalTool(external),
            Some(ActionResolution::ExternalTool(ToolOutput::ok(
                encoded_too_large.clone()
            ))),
            false,
        ),
        Err(InterruptionError::InvalidField("tool result"))
    ));
    assert_eq!(pending.entries().count(), 0);

    let mut checkpoint =
        ExecutionCheckpoint::new(CheckpointId::new(), ancestry, scope(1), None, 1_000, 5_000)
            .unwrap();
    let restored = InvocationRecord::restore(
        InvocationId::new(),
        call("invocation-bound"),
        ancestry,
        ToolEffect::NonIdempotent,
        None,
        InvocationState::Finished {
            outcome: ToolOutcome::Succeeded,
            output: ToolOutput::ok(encoded_too_large),
        },
    );
    assert!(matches!(
        checkpoint.add_invocation(restored),
        Err(InterruptionError::InvalidField("tool result"))
    ));
    assert!(checkpoint.invocations().is_empty());
}

#[test]
fn checkpoint_diagnostics_redact_pending_and_invocation_content() {
    let ancestry = Ancestry::new();
    let cache = CacheCheckpoint::new(
        "policy-v1",
        "capability-v1",
        None::<Box<str>>,
        PromptCacheScopeDigest::new([0x11; 32]),
        PromptCacheFingerprint::new([0x22; 32]),
        None,
        Some(PromptCacheResourceId::new()),
        Some(4_000),
        true,
    )
    .unwrap();
    let mut checkpoint = ExecutionCheckpoint::new(
        CheckpointId::new(),
        ancestry,
        scope(1),
        Some(cache),
        1_000,
        5_000,
    )
    .unwrap();

    let external = PendingExternalTool::new(
        ToolCall {
            id: ToolId::new("secret-call"),
            name: "secret-tool".into(),
            args: ToolArgs::new(r#"{"token":"argument-secret-canary"}"#),
        },
        ancestry,
        4_000,
    );
    let external_id = external.id();
    checkpoint
        .pending_mut()
        .insert(PendingAction::ExternalTool(external))
        .unwrap();
    checkpoint
        .pending_mut()
        .resolve_external(external_id, ToolOutput::ok("external-output-secret-canary"))
        .unwrap();

    let human = PendingHumanInput::new("question-secret-canary", ancestry, 4_000).unwrap();
    let human_id = human.id();
    checkpoint
        .pending_mut()
        .insert(PendingAction::HumanInput(human))
        .unwrap();
    checkpoint
        .pending_mut()
        .answer(human_id, "answer-secret-canary")
        .unwrap();

    let mut invocation = InvocationRecord::new(
        call("debug-invocation"),
        ancestry,
        ToolEffect::Idempotent,
        Some(IdempotencyKey::new("idempotency-secret-canary").unwrap()),
    );
    invocation.start().unwrap();
    invocation
        .finish(
            ToolOutcome::Succeeded,
            ToolOutput::ok("invocation-output-secret-canary"),
        )
        .unwrap();
    checkpoint.add_invocation(invocation).unwrap();

    let debug = format!("{checkpoint:?}");
    for secret in [
        "secret-call",
        "secret-tool",
        "argument-secret-canary",
        "external-output-secret-canary",
        "question-secret-canary",
        "answer-secret-canary",
        "idempotency-secret-canary",
        "invocation-output-secret-canary",
    ] {
        assert!(!debug.contains(secret), "checkpoint debug leaked {secret}");
    }
}
