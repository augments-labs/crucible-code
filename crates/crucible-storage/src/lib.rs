//! History, checkpoint and cache contracts a durable store is written against.
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
//! use crucible_types::{Message, ToolResult};
//!
//! #[derive(Default)]
//! struct Everything(Mutex<Vec<Message>>);
//!
//! impl SessionStore for Everything {
//!     fn append_message(&self, message: &Message) {
//!         if let Ok(mut held) = self.0.lock() {
//!             held.push(message.clone());
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

pub mod interruption;
pub mod journal;

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
    CustomProjector, JournalError, MAX_CUSTOM_DATA_BYTES, MAX_JOURNAL_WORD_BYTES, SessionStore,
};
