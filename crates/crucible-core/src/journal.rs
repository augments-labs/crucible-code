//! The framework-history writing seam.
//!
//! A provider receives the deliberately closed `Message` vocabulary. The
//! framework needs a wider history for attempts, interruptions, invocation
//! recovery and extension state; those records are [`RunItem`]s, they live in
//! `crucible-storage` beside the ports that write them, and they cross the
//! provider boundary only through an explicit projection.
//!
//! So this module holds the seam and nothing else: the trait a runner and an
//! invocation worker append through. The values it names are re-exported by
//! this crate from their owner, so a consumer keeps one import while ownership
//! sits with the crate that validates the values. A store implementation is
//! written against the same names.

use std::fmt;

use crucible_storage::{
    CallResultKey, CallResultReceipt, CallResultStoreError, RunItem, SessionStore,
};
use crucible_types::ToolResult;

/// The framework-history writing seam used by runners and invocation workers.
///
/// Above [`SessionStore`] rather than beside it, because framework history and
/// conversation are two readings of one turn: a journal record that named a
/// message the conversation never kept, or a conversation that went on past the
/// journal, would be a session whose two halves disagree about what happened. A
/// runner therefore holds one store and writes both through it.
pub trait JournalStore: SessionStore + Send + Sync {
    /// Appends one already bounded framework record.
    fn append_run_item(&self, item: &RunItem);

    /// Durably inserts one source-qualified result exactly once.
    ///
    /// Implementations must return the same receipt when the same key and
    /// logical result are repeated, and [`CallResultStoreError::Conflict`]
    /// when the key is already bound to different content. The default keeps
    /// in-memory and test journals fail closed at a background-acceptance
    /// boundary instead of pretending they are durable.
    ///
    /// # Errors
    ///
    /// Storage is unavailable, the key conflicts with different content, the
    /// result is invalid, or the protected write could not complete durably.
    fn put_call_result(
        &self,
        _key: CallResultKey,
        _result: &ToolResult,
    ) -> Result<CallResultReceipt, CallResultStoreError> {
        Err(CallResultStoreError::Unavailable)
    }

    /// Removes accepted sidecars only after their ordinary result message and
    /// companion journal metadata have crossed the sink's durability barrier.
    ///
    /// The default is for in-memory journals, which cannot own sidecars.
    fn settle_call_results(&self) {}
}

impl fmt::Debug for dyn JournalStore {
    /// Names the session and nothing else.
    ///
    /// A store's contents are the conversation; printing them here would put a
    /// transcript into any structure that derives `Debug` over one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.session_id() {
            Some(id) => write!(f, "JournalStore({})", id.as_str()),
            None => f.write_str("JournalStore(unrecorded)"),
        }
    }
}
