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
//! compiles against the contract without linking the runtime that fills it:
//!
//! ```
//! use std::sync::Mutex;
//!
//! use crucible_storage::{
//!     CallResultKey, CallResultReceipt, CallResultStoreError, CustomEntry, CustomProjector,
//!     SessionStore,
//! };
//! use crucible_types::{
//!     Calibration, Compacted, ContextError, ContextPatch, ContextSnapshot, Message, SessionId,
//!     ToolId, ToolResult,
//! };
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
//!     fn owner(&self) -> Box<str> {
//!         "in-memory".into()
//!     }
//!
//!     fn append_message(&self, message: &Message) {
//!         self.push(Kept::Said(message.clone()));
//!     }
//!
//!     fn context_snapshot(&self) -> Option<ContextSnapshot> {
//!         self.context.lock().ok().and_then(|held| held.clone())
//!     }
//!
//!     fn contextual(&self, patch: &ContextPatch) -> Result<(), ContextError> {
//!         let Ok(mut held) = self.context.lock() else {
//!             return Ok(());
//!         };
//!         *held = Some(patch.apply(&held.clone().unwrap_or_default())?);
//!         Ok(())
//!     }
//!
//!     fn compacted(&self, replaced: usize, recap: &str) {
//!         self.push(Kept::Compacted { replaced, recap: recap.to_owned() });
//!     }
//!
//!     fn display_compacted(&self, _compacted: Compacted, _pruned: bool) {}
//!
//!     fn pruned(&self, _freed: usize, results: &[ToolId]) {
//!         self.push(Kept::Cleared { results: results.to_vec(), notice: None });
//!     }
//!
//!     fn restricted(&self, _freed: usize, results: &[ToolId], notice: &str) {
//!         let notice = Some(notice.to_owned());
//!         self.push(Kept::Cleared { results: results.to_vec(), notice });
//!     }
//!
//!     fn measured(&self, calibration: &Calibration) {
//!         self.push(Kept::Measured(*calibration));
//!     }
//!
//!     fn calibrated(&self) -> Option<Calibration> {
//!         None
//!     }
//! }
//!
//! impl Everything {
//!     fn push(&self, one: Kept) {
//!         if let Ok(mut held) = self.kept.lock() {
//!             held.push(one);
//!         }
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

pub mod cache;
pub mod interruption;
pub mod journal;
pub mod session;

pub use cache::PromptCacheResourceStore;
pub use interruption::{
    ActionId, ActionResolution, ApprovalDecision, CheckpointId, IdempotencyKey, InterruptionError,
    InvocationId, InvocationRecord, InvocationState, JournalEntryId, MAX_CHECKPOINT_INVOCATIONS,
    MAX_CHECKPOINT_SANDBOXES, MAX_CHECKPOINT_WORD_BYTES, MAX_HUMAN_INPUT_BYTES,
    MAX_PENDING_ACTIONS, PendingAction, PendingActions, PendingApproval, PendingExternalTool,
    PendingHumanInput, RecoveryAction, ResolutionChange, ResumeDigest, ResumeScope, ResumedAction,
    ToolEffect,
};
pub use journal::{
    CallResultKey, CallResultReceipt, CallResultStoreError, CompactionRecord, CustomEntry,
    CustomProjector, JournalError, MAX_CUSTOM_DATA_BYTES, MAX_JOURNAL_WORD_BYTES,
};
pub use session::SessionStore;
