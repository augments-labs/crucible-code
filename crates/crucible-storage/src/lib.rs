//! Session, history, checkpoint and cache contracts a durable store is written
//! against.
//!
//! Persistence has two halves that are easy to confuse. One is what a record
//! *means* — the identity a stored call result is keyed by, the receipt a sink
//! returns, the pending action a resume must still settle. The other is what
//! produced it: a sandbox lifecycle, a prompt-cache projection, a live tool
//! wrapper still bound to an approval.
//!
//! This crate owns the first half only. Everything here validates and bounds
//! itself from values in `crucible-types`, so an external store implementation
//! compiles against the contract without linking the runtime that fills it —
//! including the redacted sandbox records a journal and a checkpoint keep,
//! which is why nothing in this crate names `crucible-sandbox` and a store can
//! read a session back without a sandbox in the build. Which half a record
//! belongs to is a ruling rather than a preference of this crate:
//!
//! > the records a journal or checkpoint stores move down into
//! > `crucible-storage` as storage-owned redacted records, which
//! > `crucible-sandbox` — already allowed to depend on storage — converts its
//! > values into
//!
//! ```
//! use std::future::{self, Future};
//! use std::pin::Pin;
//! use std::sync::Mutex;
//!
//! use crucible_storage::{
//!     CallResultKey, CallResultReceipt, CallResultStoreError, CustomEntry, CustomProjector,
//!     SessionOwner, SessionStore,
//! };
//! use crucible_types::{
//!     Calibration, Compacted, ContextError, ContextPatch, ContextSnapshot, Message, SessionId,
//!     ToolId, ToolResult,
//! };
//!
//! /// What a write hands back: any name for the one boxed `Send` future type.
//! type Written<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
//!
//! /// Everything one session was told, in the order it was told.
//! #[derive(Debug)]
//! enum Kept {
//!     Said(Message),
//!     Compacted { replaced: usize, recap: String },
//!     Cleared { results: Vec<ToolId>, notice: Option<String> },
//!     Measured(Calibration),
//! }
//!
//! #[derive(Default)]
//! struct Everything {
//!     kept: Mutex<Vec<Kept>>,
//!     context: Mutex<Option<ContextSnapshot>>,
//! }
//!
//! impl SessionStore for Everything {
//!     fn session_id(&self) -> Option<SessionId> {
//!         None
//!     }
//!
//!     // Held in memory for one run: nobody's records, so nobody to name.
//!     fn owner(&self) -> Option<SessionOwner> {
//!         None
//!     }
//!
//!     fn append_message<'a>(&'a self, message: &'a Message) -> Written<'a, ()> {
//!         self.push(Kept::Said(message.clone()))
//!     }
//!
//!     fn context_snapshot(&self) -> Option<ContextSnapshot> {
//!         self.context.lock().ok().and_then(|held| held.clone())
//!     }
//!
//!     fn contextual<'a>(&'a self, patch: &'a ContextPatch) -> Written<'a, Result<(), ContextError>> {
//!         Box::pin(async move {
//!             let Ok(mut held) = self.context.lock() else {
//!                 return Ok(());
//!             };
//!             *held = Some(patch.apply(&held.clone().unwrap_or_default())?);
//!             Ok(())
//!         })
//!     }
//!
//!     fn compacted<'a>(&'a self, replaced: usize, recap: &'a str) -> Written<'a, ()> {
//!         self.push(Kept::Compacted { replaced, recap: recap.to_owned() })
//!     }
//!
//!     fn display_compacted(&self, _compacted: Compacted, _pruned: bool) -> Written<'_, ()> {
//!         Box::pin(future::ready(()))
//!     }
//!
//!     fn pruned<'a>(&'a self, _freed: usize, results: &'a [ToolId]) -> Written<'a, ()> {
//!         self.push(Kept::Cleared { results: results.to_vec(), notice: None })
//!     }
//!
//!     fn restricted<'a>(
//!         &'a self,
//!         _freed: usize,
//!         results: &'a [ToolId],
//!         notice: &'a str,
//!     ) -> Written<'a, ()> {
//!         let notice = Some(notice.to_owned());
//!         self.push(Kept::Cleared { results: results.to_vec(), notice })
//!     }
//!
//!     fn measured<'a>(&'a self, calibration: &'a Calibration) -> Written<'a, ()> {
//!         self.push(Kept::Measured(*calibration))
//!     }
//!
//!     fn calibrated(&self) -> Option<Calibration> {
//!         None
//!     }
//! }
//!
//! impl Everything {
//!     /// Kept in memory, so written by the time anybody asks.
//!     fn push(&self, one: Kept) -> Written<'_, ()> {
//!         if let Ok(mut held) = self.kept.lock() {
//!             held.push(one);
//!         }
//!         Box::pin(future::ready(()))
//!     }
//! }
//!
//! impl CustomProjector for Everything {
//!     fn project(&self, entry: &CustomEntry) -> Option<Message> {
//!         (entry.namespace() == "ledger").then(|| Message::said(entry.data()))
//!     }
//! }
//!
//! /// A key is derived, never invented, and a receipt says what was written.
//! fn accept(
//!     key: CallResultKey,
//!     result: &ToolResult,
//! ) -> Result<CallResultReceipt, CallResultStoreError> {
//!     let _ = (key, result);
//!     Err(CallResultStoreError::Unavailable)
//! }
//! ```

use std::future::Future;
use std::pin::Pin;

pub mod cache;
pub mod interruption;
pub mod journal;
pub mod sandbox;
pub mod session;

pub use cache::PromptCacheResourceStore;
pub use interruption::{
    ActionId, ActionResolution, ApprovalDecision, CheckpointId, ExecutionCheckpoint,
    IdempotencyKey, InterruptionError, InvocationId, InvocationRecord, InvocationState,
    JournalEntryId, MAX_CHECKPOINT_INVOCATIONS, MAX_CHECKPOINT_SANDBOXES,
    MAX_CHECKPOINT_WORD_BYTES, MAX_HUMAN_INPUT_BYTES, MAX_PENDING_ACTIONS, PendingAction,
    PendingActions, PendingApproval, PendingExternalTool, PendingHumanInput, RecoveryAction,
    ResolutionChange, ResumeDigest, ResumeEvidence, ResumeScope, ResumedAction, ToolEffect,
    ValidatedResume,
};
pub use journal::{
    CallResultKey, CallResultReceipt, CallResultStoreError, CompactionRecord, CustomEntry,
    CustomProjector, JournalError, MAX_CUSTOM_DATA_BYTES, MAX_JOURNAL_WORD_BYTES,
    MAX_RUN_HISTORY_BYTES, MAX_RUN_ITEM_BYTES, MAX_RUN_ITEM_RETAINED_BYTES, MAX_RUN_ITEMS,
    RunHistory, RunItem,
};
pub use sandbox::{
    MAX_SANDBOX_BACKEND_ID_BYTES, MAX_SANDBOX_BACKEND_WORD_BYTES,
    MAX_SANDBOX_CONFINING_CPU_SECONDS, MAX_SANDBOX_NETWORK_RULES, SandboxBackendId,
    SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities, SandboxCapability,
    SandboxCapabilityError, SandboxCheckpoint, SandboxCheckpointError, SandboxCleanup,
    SandboxCommandStage, SandboxFact, SandboxFactKind, SandboxFailureKind, SandboxFailurePhase,
    SandboxFeature, SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxGuardrailDecision,
    SandboxInspection, SandboxLifecycle, SandboxNetworkInspection, SandboxPlanInspection,
    SandboxResourceLimits, SandboxRootInspection, SandboxUsage, SandboxViolation,
};
// A Windows Job Object has no per-process handle-count ceiling, so the POSIX
// descriptor limit this constant states has no meaning there and the constant
// itself is compiled out. The gate is the one its definition carries, so the
// name and the value it promises cannot disagree about where it exists.
#[cfg(not(target_os = "windows"))]
pub use sandbox::MAX_SANDBOX_CONFINING_OPEN_FILES;
pub use session::{SessionOwner, SessionStore};

/// What a store's waiting methods hand back.
///
/// The same type as `crucible_runtime::BoxFuture`, spelled out here because
/// this crate names no workspace crate but `crucible-types`. The two agree
/// because they are one type, not two alike, so an implementation may name it
/// either way; a test that implements these traits with the runtime's name
/// holds them to it.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
