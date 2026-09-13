//! Durable interruption, resolution and invocation recovery records.
//!
//! Conversation history says what a provider may read. This module says what
//! an unfinished execution needs in order to resume safely. The two records
//! deliberately share identifiers and ancestry, not storage or projection.
//!
//! What an unfinished call durably holds is the recorded result, never the
//! live wrapper: a checkpoint that could hand back an approval-bound value
//! would let a resume replay authority the user granted to a call that is
//! already gone.

use std::fmt;
use std::str::FromStr;

use uuid::Uuid;

use crucible_types::{
    Ancestry, RecordedToolOutput, TOOL_ARGUMENT_BYTES, TOOL_CALL_ID_BYTES, TOOL_NAME_BYTES,
    TOOL_RESULT_BYTES, ToolCall, ToolId, ToolOutcome, ToolResult,
};

/// Most pending actions retained in one execution checkpoint.
pub const MAX_PENDING_ACTIONS: usize = 128;
/// Most invocation records retained in one execution checkpoint.
pub const MAX_CHECKPOINT_INVOCATIONS: usize = 512;
/// Most sandbox identities retained in one execution checkpoint.
pub const MAX_CHECKPOINT_SANDBOXES: usize = 128;
/// Most bytes retained in a checkpoint metadata word or idempotency key.
pub const MAX_CHECKPOINT_WORD_BYTES: usize = 256;
/// Most bytes retained in a human question or answer.
pub const MAX_HUMAN_INPUT_BYTES: usize = 4_096;

/// Why interruption or recovery state was invalid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InterruptionError {
    /// A local identity was not canonical UUID-v7 text.
    #[error("invalid interruption identity")]
    InvalidId,
    /// One provider call appeared twice in the same pending/invocation set.
    #[error("tool call {0} already has durable interruption state")]
    DuplicateCall(ToolId),
    /// One stable invocation identity appeared twice in pending state.
    #[error("tool invocation already has durable interruption state")]
    DuplicateInvocation,
    /// A retained word was empty, too large, or control-bearing.
    #[error("invalid bounded checkpoint field {0}")]
    InvalidField(&'static str),
    /// An expiry was not later than creation.
    #[error("checkpoint expiry must be later than creation")]
    InvalidExpiry,
    /// The pending or invocation collection reached its ceiling.
    #[error("checkpoint reached its {kind} limit of {maximum}")]
    TooMany {
        /// Which bounded collection filled.
        kind: &'static str,
        /// Its fixed ceiling.
        maximum: usize,
    },
    /// An action identity was not present.
    #[error("pending action was not found")]
    UnknownAction,
    /// A resolution did not match the kind of action it named.
    #[error("resolution does not match the pending action")]
    WrongResolution,
    /// A completed action cannot be rewritten to another outcome.
    #[error("a completed action cannot be resolved differently")]
    AlreadyCompleted,
    /// An unresolved action cannot resume.
    #[error("pending action has not been resolved")]
    Unresolved,
    /// An invocation transition would contradict what was durably recorded.
    #[error("invalid invocation state transition")]
    InvocationState,
    /// Resume authority or cache semantics no longer match the checkpoint.
    #[error("checkpoint resume scope or version changed")]
    ResumeMismatch,
    /// The checkpoint or its cache reference expired.
    #[error("checkpoint expired before resume")]
    Expired,
}

impl InterruptionError {
    /// Accepts one bounded, control-free word retained in a checkpoint.
    ///
    /// The checkpoint owner lives above this crate while the records it holds
    /// live here, so the rule is stated once by the error that reports it.
    ///
    /// # Errors
    ///
    /// [`InterruptionError::InvalidField`] for an empty, oversized, or
    /// control-bearing word.
    pub fn check_word(field: &'static str, value: &str) -> Result<(), Self> {
        if value.is_empty()
            || value.len() > MAX_CHECKPOINT_WORD_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(Self::InvalidField(field));
        }
        Ok(())
    }
}

macro_rules! durable_id {
    ($name:ident, $debug:literal) => {
        #[doc = concat!("Stable UUID-v7 identity for one ", $debug, ".")]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Uuid);

        impl $name {
            #[doc = concat!("Mints one ", $debug, " identity.")]
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            #[doc = concat!("Canonical UUID text for this ", $debug, ".")]
            #[must_use]
            pub fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0.as_hyphenated())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    f,
                    concat!(stringify!($name), "({})"),
                    self.0.as_hyphenated()
                )
            }
        }

        impl FromStr for $name {
            type Err = InterruptionError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let uuid = Uuid::try_parse(value).map_err(|_| InterruptionError::InvalidId)?;
                if uuid.get_version_num() != 7 || uuid.to_string() != value {
                    return Err(InterruptionError::InvalidId);
                }
                Ok(Self(uuid))
            }
        }
    };
}

durable_id!(ActionId, "pending action");
durable_id!(CheckpointId, "execution checkpoint");
durable_id!(InvocationId, "tool invocation");
durable_id!(JournalEntryId, "custom journal entry");

/// One digest used to revalidate a resume boundary without persisting its text.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResumeDigest([u8; 32]);

impl ResumeDigest {
    /// Takes one domain-separated digest.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Private bytes for a protected checkpoint codec.
    #[must_use]
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for ResumeDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ResumeDigest([redacted])")
    }
}

/// Exact authority-bearing scope a resumed execution must still match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumeScope {
    endpoint: ResumeDigest,
    model: ResumeDigest,
    credential: ResumeDigest,
    authority: ResumeDigest,
}

impl ResumeScope {
    /// Joins independently domain-separated route, model, credential, and
    /// authority fingerprints.
    #[must_use]
    pub const fn new(
        endpoint: ResumeDigest,
        model: ResumeDigest,
        credential: ResumeDigest,
        authority: ResumeDigest,
    ) -> Self {
        Self {
            endpoint,
            model,
            credential,
            authority,
        }
    }

    /// Endpoint/deployment fingerprint.
    #[must_use]
    pub const fn endpoint(self) -> ResumeDigest {
        self.endpoint
    }

    /// Exact model/revision fingerprint.
    #[must_use]
    pub const fn model(self) -> ResumeDigest {
        self.model
    }

    /// Credential-owned scope fingerprint.
    #[must_use]
    pub const fn credential(self) -> ResumeDigest {
        self.credential
    }

    /// Effective permission/policy fingerprint.
    #[must_use]
    pub const fn authority(self) -> ResumeDigest {
        self.authority
    }
}

/// A decision about one pending approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Execute under freshly revalidated authority.
    Approved,
    /// Finalize the call without its effect.
    Rejected,
}

/// A durable result supplied to one pending action.
#[derive(Clone, PartialEq, Eq)]
pub enum ActionResolution {
    /// Human/policy decision for an approval.
    Approval(ApprovalDecision),
    /// One final result supplied by an external executor.
    ExternalTool(RecordedToolOutput),
    /// Bounded answer to a human-input request.
    HumanInput(Box<str>),
    /// The pending action was cancelled.
    Cancelled,
}

impl fmt::Debug for ActionResolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Approval(decision) => f.debug_tuple("Approval").field(decision).finish(),
            Self::ExternalTool(_) => f.write_str("ExternalTool([redacted])"),
            Self::HumanInput(_) => f.write_str("HumanInput([redacted])"),
            Self::Cancelled => f.write_str("Cancelled"),
        }
    }
}

/// One tool call waiting for authority.
#[derive(Clone)]
pub struct PendingApproval {
    id: ActionId,
    invocation: InvocationId,
    call: ToolCall,
    ancestry: Ancestry,
    expires_at: u64,
}

impl PendingApproval {
    /// Creates one pending approval.
    #[must_use]
    pub fn new(call: ToolCall, ancestry: Ancestry, expires_at: u64) -> Self {
        Self {
            id: ActionId::new(),
            invocation: InvocationId::new(),
            call,
            ancestry,
            expires_at,
        }
    }

    /// Restores an exact pending identity after its protected wire was checked.
    #[must_use]
    pub const fn restore(
        id: ActionId,
        invocation: InvocationId,
        call: ToolCall,
        ancestry: Ancestry,
        expires_at: u64,
    ) -> Self {
        Self {
            id,
            invocation,
            call,
            ancestry,
            expires_at,
        }
    }

    /// Stable action identity.
    #[must_use]
    pub const fn id(&self) -> ActionId {
        self.id
    }

    /// Stable identity retained before approval can begin an effect.
    #[must_use]
    pub const fn invocation(&self) -> InvocationId {
        self.invocation
    }

    /// Provider call waiting for authority.
    #[must_use]
    pub const fn call(&self) -> &ToolCall {
        &self.call
    }
}

impl fmt::Debug for PendingApproval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingApproval")
            .field("id", &self.id)
            .field("invocation", &self.invocation)
            .field("call", &self.call)
            .field("ancestry", &self.ancestry)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// One call waiting for an external executor to supply its final result.
#[derive(Clone)]
pub struct PendingExternalTool {
    id: ActionId,
    invocation: InvocationId,
    call: ToolCall,
    ancestry: Ancestry,
    expires_at: u64,
}

impl PendingExternalTool {
    /// Creates one pending external call and its stable invocation identity.
    #[must_use]
    pub fn new(call: ToolCall, ancestry: Ancestry, expires_at: u64) -> Self {
        Self {
            id: ActionId::new(),
            invocation: InvocationId::new(),
            call,
            ancestry,
            expires_at,
        }
    }

    /// Restores an exact pending and invocation identity.
    #[must_use]
    pub const fn restore(
        id: ActionId,
        invocation: InvocationId,
        call: ToolCall,
        ancestry: Ancestry,
        expires_at: u64,
    ) -> Self {
        Self {
            id,
            invocation,
            call,
            ancestry,
            expires_at,
        }
    }

    /// Stable action identity.
    #[must_use]
    pub const fn id(&self) -> ActionId {
        self.id
    }

    /// Stable invocation identity.
    #[must_use]
    pub const fn invocation(&self) -> InvocationId {
        self.invocation
    }

    /// Provider call waiting for an external result.
    #[must_use]
    pub const fn call(&self) -> &ToolCall {
        &self.call
    }
}

impl fmt::Debug for PendingExternalTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingExternalTool")
            .field("id", &self.id)
            .field("invocation", &self.invocation)
            .field("call", &self.call)
            .field("ancestry", &self.ancestry)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// One bounded question waiting for a human answer.
#[derive(Clone)]
pub struct PendingHumanInput {
    id: ActionId,
    question: Box<str>,
    ancestry: Ancestry,
    expires_at: u64,
}

impl PendingHumanInput {
    /// Creates one pending human question.
    ///
    /// # Errors
    ///
    /// Empty and oversized questions are rejected.
    pub fn new(
        question: impl Into<Box<str>>,
        ancestry: Ancestry,
        expires_at: u64,
    ) -> Result<Self, InterruptionError> {
        Self::restore(ActionId::new(), question, ancestry, expires_at)
    }

    /// Restores one exact question identity.
    ///
    /// # Errors
    ///
    /// The question is bounded before retention.
    pub fn restore(
        id: ActionId,
        question: impl Into<Box<str>>,
        ancestry: Ancestry,
        expires_at: u64,
    ) -> Result<Self, InterruptionError> {
        let question = bounded_human("human question", question.into())?;
        Ok(Self {
            id,
            question,
            ancestry,
            expires_at,
        })
    }

    /// Stable action identity.
    #[must_use]
    pub const fn id(&self) -> ActionId {
        self.id
    }

    /// The bounded question for the UI that owns it.
    #[must_use]
    pub fn question(&self) -> &str {
        &self.question
    }
}

impl fmt::Debug for PendingHumanInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingHumanInput")
            .field("id", &self.id)
            .field("question", &"[redacted]")
            .field("ancestry", &self.ancestry)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// The three serializable interruption kinds.
#[derive(Debug, Clone)]
pub enum PendingAction {
    /// Tool permission is outstanding.
    Approval(PendingApproval),
    /// Another executor owes a tool result.
    ExternalTool(PendingExternalTool),
    /// A human answer is outstanding.
    HumanInput(PendingHumanInput),
}

impl PendingAction {
    /// Stable action identity.
    #[must_use]
    pub const fn id(&self) -> ActionId {
        match self {
            Self::Approval(action) => action.id,
            Self::ExternalTool(action) => action.id,
            Self::HumanInput(action) => action.id,
        }
    }

    /// Execution tree attribution, including nested deferrals.
    #[must_use]
    pub const fn ancestry(&self) -> Ancestry {
        match self {
            Self::Approval(action) => action.ancestry,
            Self::ExternalTool(action) => action.ancestry,
            Self::HumanInput(action) => action.ancestry,
        }
    }

    /// Expiry in Unix seconds.
    #[must_use]
    pub const fn expires_at(&self) -> u64 {
        match self {
            Self::Approval(action) => action.expires_at,
            Self::ExternalTool(action) => action.expires_at,
            Self::HumanInput(action) => action.expires_at,
        }
    }

    /// Tool call, where this action owes the provider one eventual result.
    #[must_use]
    pub const fn call(&self) -> Option<&ToolCall> {
        match self {
            Self::Approval(action) => Some(&action.call),
            Self::ExternalTool(action) => Some(&action.call),
            Self::HumanInput(_) => None,
        }
    }

    /// External invocation identity, where one exists.
    #[must_use]
    pub const fn invocation(&self) -> Option<InvocationId> {
        match self {
            Self::Approval(action) => Some(action.invocation),
            Self::ExternalTool(action) => Some(action.invocation),
            Self::HumanInput(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
struct PendingRecord {
    action: PendingAction,
    resolution: Option<ActionResolution>,
    completed: bool,
}

/// Idempotent resolution state for one checkpoint's pending actions.
#[derive(Debug, Clone, Default)]
pub struct PendingActions {
    records: Vec<PendingRecord>,
}

/// Whether a durable resolution changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionChange(bool);

impl ResolutionChange {
    /// `true` only when this operation changed the durable state.
    #[must_use]
    pub const fn changed(self) -> bool {
        self.0
    }
}

impl PendingActions {
    /// Empty pending-action set.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// Inserts one unique pending action.
    ///
    /// # Errors
    ///
    /// Duplicate ids and a full collection are refused.
    pub fn insert(&mut self, action: PendingAction) -> Result<(), InterruptionError> {
        validate_action(&action)?;
        if self.records.len() >= MAX_PENDING_ACTIONS {
            return Err(InterruptionError::TooMany {
                kind: "pending-action",
                maximum: MAX_PENDING_ACTIONS,
            });
        }
        if self
            .records
            .iter()
            .any(|record| record.action.id() == action.id())
        {
            return Err(InterruptionError::InvalidId);
        }
        if let Some(call) = action.call()
            && self
                .records
                .iter()
                .any(|record| record.action.call().is_some_and(|held| held.id == call.id))
        {
            return Err(InterruptionError::DuplicateCall(call.id.clone()));
        }
        if let Some(invocation) = action.invocation()
            && self
                .records
                .iter()
                .any(|record| record.action.invocation() == Some(invocation))
        {
            return Err(InterruptionError::DuplicateInvocation);
        }
        self.records.push(PendingRecord {
            action,
            resolution: None,
            completed: false,
        });
        Ok(())
    }

    /// Applies an approval.
    ///
    /// # Errors
    ///
    /// The action must exist, be an approval, and not be completed under a
    /// different resolution.
    pub fn approve(&mut self, id: ActionId) -> Result<ResolutionChange, InterruptionError> {
        self.resolve(id, ActionResolution::Approval(ApprovalDecision::Approved))
    }

    /// Applies a rejection. Before resume, this replaces an earlier approval
    /// as the single latest decision rather than accumulating both.
    ///
    /// # Errors
    ///
    /// The action must exist, be an approval, and not be completed under a
    /// different resolution.
    pub fn reject(&mut self, id: ActionId) -> Result<ResolutionChange, InterruptionError> {
        self.resolve(id, ActionResolution::Approval(ApprovalDecision::Rejected))
    }

    /// Supplies one external final result.
    ///
    /// # Errors
    ///
    /// The action must exist, await external execution, and not already be
    /// completed under another resolution.
    pub fn resolve_external(
        &mut self,
        id: ActionId,
        mut output: RecordedToolOutput,
    ) -> Result<ResolutionChange, InterruptionError> {
        let _ = output.limit_encoded(TOOL_RESULT_BYTES);
        self.resolve(id, ActionResolution::ExternalTool(output))
    }

    /// Supplies one bounded human answer.
    ///
    /// # Errors
    ///
    /// The answer must fit its retained bound and name an unfinished human
    /// input action.
    pub fn answer(
        &mut self,
        id: ActionId,
        answer: impl Into<Box<str>>,
    ) -> Result<ResolutionChange, InterruptionError> {
        let answer = bounded_human("human answer", answer.into())?;
        self.resolve(id, ActionResolution::HumanInput(answer))
    }

    /// Cancels any still-pending kind.
    ///
    /// # Errors
    ///
    /// The action must exist and not already be completed under another
    /// resolution.
    pub fn cancel(&mut self, id: ActionId) -> Result<ResolutionChange, InterruptionError> {
        self.resolve(id, ActionResolution::Cancelled)
    }

    fn resolve(
        &mut self,
        id: ActionId,
        resolution: ActionResolution,
    ) -> Result<ResolutionChange, InterruptionError> {
        let record = self
            .records
            .iter_mut()
            .find(|record| record.action.id() == id)
            .ok_or(InterruptionError::UnknownAction)?;
        if !resolution_matches(&record.action, &resolution) {
            return Err(InterruptionError::WrongResolution);
        }
        validate_resolution(&resolution)?;
        if record.resolution.as_ref() == Some(&resolution) {
            return Ok(ResolutionChange(false));
        }
        if record.completed {
            return Err(InterruptionError::AlreadyCompleted);
        }
        record.resolution = Some(resolution);
        Ok(ResolutionChange(true))
    }

    /// Current single resolution, if one has arrived.
    #[must_use]
    pub fn resolution(&self, id: ActionId) -> Option<&ActionResolution> {
        self.records
            .iter()
            .find(|record| record.action.id() == id)
            .and_then(|record| record.resolution.as_ref())
    }

    /// Resolves one stop point before provider projection and marks it complete.
    ///
    /// # Errors
    ///
    /// Unknown and still-unresolved actions cannot resume.
    pub fn resume(&mut self, id: ActionId) -> Result<ResumedAction, InterruptionError> {
        let record = self
            .records
            .iter_mut()
            .find(|record| record.action.id() == id)
            .ok_or(InterruptionError::UnknownAction)?;
        let resolution = record
            .resolution
            .clone()
            .ok_or(InterruptionError::Unresolved)?;
        record.completed = true;
        Ok(ResumedAction {
            action: record.action.clone(),
            resolution,
        })
    }

    /// Records restored state after the checkpoint codec validates it.
    ///
    /// # Errors
    ///
    /// The action/resolution relationship and collection bound are checked.
    pub fn restore(
        &mut self,
        action: PendingAction,
        resolution: Option<ActionResolution>,
        completed: bool,
    ) -> Result<(), InterruptionError> {
        validate_action(&action)?;
        if let Some(resolution) = &resolution {
            if !resolution_matches(&action, resolution) {
                return Err(InterruptionError::WrongResolution);
            }
            validate_resolution(resolution)?;
        }
        if completed && resolution.is_none() {
            return Err(InterruptionError::Unresolved);
        }
        self.insert(action)?;
        let Some(record) = self.records.last_mut() else {
            return Err(InterruptionError::UnknownAction);
        };
        record.resolution = resolution;
        record.completed = completed;
        Ok(())
    }

    /// Restored/persisted records for a checkpoint codec.
    pub fn entries(
        &self,
    ) -> impl Iterator<Item = (&PendingAction, Option<&ActionResolution>, bool)> {
        self.records
            .iter()
            .map(|record| (&record.action, record.resolution.as_ref(), record.completed))
    }
}

/// One resolved interruption ready for execution or transcript completion.
#[derive(Debug, Clone)]
pub struct ResumedAction {
    action: PendingAction,
    resolution: ActionResolution,
}

impl ResumedAction {
    /// Original nested execution attribution.
    #[must_use]
    pub const fn ancestry(&self) -> Ancestry {
        self.action.ancestry()
    }

    /// Approved call to execute under revalidated authority.
    #[must_use]
    pub const fn approved_call(&self) -> Option<&ToolCall> {
        match (&self.action, &self.resolution) {
            (
                PendingAction::Approval(action),
                ActionResolution::Approval(ApprovalDecision::Approved),
            ) => Some(&action.call),
            _ => None,
        }
    }

    /// Stable invocation identity carried across an approval or external stop.
    #[must_use]
    pub const fn invocation(&self) -> Option<InvocationId> {
        self.action.invocation()
    }

    /// Builds the prepared invocation that an approved call must execute.
    /// Repeated resume returns the same invocation identity, so a caller can
    /// consult its durable state instead of minting a second effect.
    #[must_use]
    pub fn approved_invocation(
        &self,
        effect: ToolEffect,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Option<InvocationRecord> {
        let call = self.approved_call()?.clone();
        Some(InvocationRecord::restore(
            self.invocation()?,
            call,
            self.ancestry(),
            effect,
            idempotency_key,
            InvocationState::Prepared,
        ))
    }

    /// Bounded human answer, which remains framework input until an explicit
    /// caller chooses how to use it.
    #[must_use]
    pub fn human_input(&self) -> Option<&str> {
        match &self.resolution {
            ActionResolution::HumanInput(answer) => Some(answer),
            _ => None,
        }
    }

    /// Exactly one final result for rejected, cancelled, or externally
    /// completed provider calls. Approval returns `None` because its effect
    /// must run first; human input is not a provider tool call.
    #[must_use]
    pub fn into_tool_result(self) -> Option<ToolResult> {
        let call = self.action.call()?.id.clone();
        let output = match self.resolution {
            ActionResolution::Approval(ApprovalDecision::Rejected) => {
                RecordedToolOutput::failed("not run: approval was rejected")
            }
            ActionResolution::ExternalTool(output) => output,
            ActionResolution::Cancelled => {
                RecordedToolOutput::failed("not run: action was cancelled")
            }
            ActionResolution::Approval(ApprovalDecision::Approved)
            | ActionResolution::HumanInput(_) => return None,
        };
        Some(ToolResult { id: call, output })
    }
}

/// Whether an invocation may safely be repeated after an ambiguous crash.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolEffect {
    /// Reads state but does not intentionally change it.
    ReadOnly,
    /// May change state, but the same idempotency key makes repetition one effect.
    Idempotent,
    /// May cause an effect that blind repetition can duplicate.
    #[default]
    NonIdempotent,
}

/// Optional bounded key supplied to an idempotent executor.
#[derive(Clone, PartialEq, Eq)]
pub struct IdempotencyKey(Box<str>);

impl IdempotencyKey {
    /// Validates one non-secret key before retention.
    ///
    /// # Errors
    ///
    /// Empty, oversized, or control-bearing keys are refused.
    pub fn new(value: impl Into<Box<str>>) -> Result<Self, InterruptionError> {
        bounded("idempotency key", value.into()).map(Self)
    }

    /// Exact executor-owned key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IdempotencyKey([redacted])")
    }
}

/// Durable point reached by one tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationState {
    /// Admitted and checkpointed before the effect begins.
    Prepared,
    /// The executor may have caused its effect.
    Started,
    /// One bounded final result is durably available.
    Finished {
        /// Closed invocation outcome.
        outcome: ToolOutcome,
        /// Exact provider result to reuse.
        output: RecordedToolOutput,
    },
}

/// One stable tool invocation and its crash-recovery evidence.
#[derive(Clone)]
pub struct InvocationRecord {
    id: InvocationId,
    call: ToolCall,
    ancestry: Ancestry,
    effect: ToolEffect,
    idempotency_key: Option<IdempotencyKey>,
    state: InvocationState,
}

impl InvocationRecord {
    /// Records admission before any effect begins.
    #[must_use]
    pub fn new(
        call: ToolCall,
        ancestry: Ancestry,
        effect: ToolEffect,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Self {
        Self::restore(
            InvocationId::new(),
            call,
            ancestry,
            effect,
            idempotency_key,
            InvocationState::Prepared,
        )
    }

    /// Restores an exact invocation state from a checked checkpoint/journal.
    #[must_use]
    // A wire record has six independent, already-typed fields. A carrier made
    // only to satisfy the argument count would hide rather than enforce them.
    #[allow(clippy::too_many_arguments)]
    pub const fn restore(
        id: InvocationId,
        call: ToolCall,
        ancestry: Ancestry,
        effect: ToolEffect,
        idempotency_key: Option<IdempotencyKey>,
        state: InvocationState,
    ) -> Self {
        Self {
            id,
            call,
            ancestry,
            effect,
            idempotency_key,
            state,
        }
    }

    /// Marks the last durable point before calling the executor.
    ///
    /// # Errors
    ///
    /// A finished invocation cannot return to the started state.
    pub fn start(&mut self) -> Result<ResolutionChange, InterruptionError> {
        match self.state {
            InvocationState::Prepared => {
                self.state = InvocationState::Started;
                Ok(ResolutionChange(true))
            }
            InvocationState::Started => Ok(ResolutionChange(false)),
            InvocationState::Finished { .. } => Err(InterruptionError::InvocationState),
        }
    }

    /// Persists one final bounded result. Repeating the same completion is
    /// idempotent; a different completion is refused.
    ///
    /// # Errors
    ///
    /// A different completion cannot replace a finished invocation.
    pub fn finish(
        &mut self,
        outcome: ToolOutcome,
        mut output: RecordedToolOutput,
    ) -> Result<ResolutionChange, InterruptionError> {
        let _ = output.limit_encoded(TOOL_RESULT_BYTES);
        let finished = InvocationState::Finished { outcome, output };
        match &self.state {
            InvocationState::Finished { .. } if self.state == finished => {
                Ok(ResolutionChange(false))
            }
            InvocationState::Finished { .. } => Err(InterruptionError::InvocationState),
            InvocationState::Prepared | InvocationState::Started => {
                self.state = finished;
                Ok(ResolutionChange(true))
            }
        }
    }

    /// Safe action after restart at the currently durable point.
    #[must_use]
    pub const fn recovery(&self) -> RecoveryAction {
        match (&self.state, self.effect, self.idempotency_key.is_some()) {
            (InvocationState::Prepared, _, _)
            | (InvocationState::Started, ToolEffect::ReadOnly, _) => RecoveryAction::Retry,
            (InvocationState::Started, ToolEffect::Idempotent, true) => {
                RecoveryAction::RetryWithIdempotencyKey
            }
            (InvocationState::Started, ToolEffect::Idempotent, false)
            | (InvocationState::Started, ToolEffect::NonIdempotent, _) => RecoveryAction::Reconcile,
            (InvocationState::Finished { .. }, _, _) => RecoveryAction::UseRecordedResult,
        }
    }

    /// Checks the retained call and any final result against their ceilings.
    ///
    /// The checkpoint that admits invocations lives above this crate; the
    /// bounds it must apply belong to the record it admits.
    ///
    /// # Errors
    ///
    /// [`InterruptionError::InvalidField`] when the call or its recorded
    /// result crosses a retention ceiling.
    pub fn validate(&self) -> Result<(), InterruptionError> {
        validate_call(self.call())?;
        if let InvocationState::Finished { output, .. } = self.state() {
            validate_output(output)?;
        }
        Ok(())
    }

    /// Stable invocation identity.
    #[must_use]
    pub const fn id(&self) -> InvocationId {
        self.id
    }

    /// Provider call this invocation answers.
    #[must_use]
    pub const fn call(&self) -> &ToolCall {
        &self.call
    }

    /// Execution attribution.
    #[must_use]
    pub const fn ancestry(&self) -> Ancestry {
        self.ancestry
    }

    /// Side-effect class fixed at admission.
    #[must_use]
    pub const fn effect(&self) -> ToolEffect {
        self.effect
    }

    /// Optional retry key fixed at admission.
    #[must_use]
    pub const fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        self.idempotency_key.as_ref()
    }

    /// Current durable state.
    #[must_use]
    pub const fn state(&self) -> &InvocationState {
        &self.state
    }
}

impl fmt::Debug for InvocationRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("InvocationRecord");
        debug
            .field("id", &self.id)
            .field("call", &self.call)
            .field("ancestry", &self.ancestry)
            .field("effect", &self.effect)
            .field(
                "idempotency_key",
                &self.idempotency_key.as_ref().map(|_| "[redacted]"),
            );
        match &self.state {
            InvocationState::Prepared => debug.field("state", &"Prepared"),
            InvocationState::Started => debug.field("state", &"Started"),
            InvocationState::Finished { outcome, .. } => debug
                .field("state", &"Finished")
                .field("outcome", outcome)
                .field("output", &"[redacted]"),
        };
        debug.finish()
    }
}

/// Recovery decision made from durable state, never from a cache fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// No effect began, or a read-only effect may safely repeat.
    Retry,
    /// Repeat while supplying the exact retained idempotency key.
    RetryWithIdempotencyKey,
    /// Establish remote/effect state before doing anything again.
    Reconcile,
    /// Reuse the already persisted final result.
    UseRecordedResult,
}

fn validate_action(action: &PendingAction) -> Result<(), InterruptionError> {
    if action.expires_at() == 0 {
        return Err(InterruptionError::InvalidExpiry);
    }
    if let Some(call) = action.call() {
        validate_call(call)?;
    }
    Ok(())
}

fn validate_resolution(resolution: &ActionResolution) -> Result<(), InterruptionError> {
    match resolution {
        ActionResolution::ExternalTool(output) => validate_output(output),
        ActionResolution::HumanInput(answer) => {
            if answer.is_empty() || answer.len() > MAX_HUMAN_INPUT_BYTES {
                return Err(InterruptionError::InvalidField("human answer"));
            }
            Ok(())
        }
        ActionResolution::Approval(_) | ActionResolution::Cancelled => Ok(()),
    }
}

fn validate_output(output: &RecordedToolOutput) -> Result<(), InterruptionError> {
    if output.text().len() > TOOL_RESULT_BYTES
        || serde_json::to_string(output.text())
            .map_or(true, |encoded| encoded.len() > TOOL_RESULT_BYTES)
    {
        return Err(InterruptionError::InvalidField("tool result"));
    }
    Ok(())
}

fn validate_call(call: &ToolCall) -> Result<(), InterruptionError> {
    if call.id.as_str().is_empty()
        || call.id.as_str().len() > TOOL_CALL_ID_BYTES
        || call.name.is_empty()
        || call.name.len() > TOOL_NAME_BYTES
        || call.args.as_str().len() > TOOL_ARGUMENT_BYTES
    {
        return Err(InterruptionError::InvalidField("checkpoint tool call"));
    }
    Ok(())
}

fn resolution_matches(action: &PendingAction, resolution: &ActionResolution) -> bool {
    matches!(
        (action, resolution),
        (PendingAction::Approval(_), ActionResolution::Approval(_))
            | (
                PendingAction::ExternalTool(_),
                ActionResolution::ExternalTool(_)
            )
            | (
                PendingAction::HumanInput(_),
                ActionResolution::HumanInput(_)
            )
            | (_, ActionResolution::Cancelled)
    )
}

fn bounded(field: &'static str, value: Box<str>) -> Result<Box<str>, InterruptionError> {
    InterruptionError::check_word(field, &value)?;
    Ok(value)
}

fn bounded_human(field: &'static str, value: Box<str>) -> Result<Box<str>, InterruptionError> {
    if value.is_empty() || value.len() > MAX_HUMAN_INPUT_BYTES {
        return Err(InterruptionError::InvalidField(field));
    }
    Ok(value)
}
