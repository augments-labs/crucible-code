//! Where persistent provider prompt-cache resources are remembered.
//!
//! A resource a provider holds on this program's behalf outlives the run that
//! created it, so its record has to be kept somewhere a later run can find it
//! and clean it up. This is the contract that keeping answers to; the values it
//! keeps are `crucible-types`, and the file that keeps them is a session
//! store's.

use std::fmt;

use crucible_types::{
    PromptCacheResourceBinding, PromptCacheResourceError, PromptCacheResourceId,
    PromptCacheResourceRecord,
};

/// Private bounded metadata store, implemented above core using a resolved user-home path.
pub trait PromptCacheResourceStore: Send + fmt::Debug {
    /// Finds the newest exact binding, including non-ready records for reconciliation.
    ///
    /// An older orphan may be retained for explicit cleanup after a replacement
    /// is created. Selecting the newest record prevents that cleanup evidence
    /// from shadowing the usable replacement after restart.
    ///
    /// # Errors
    ///
    /// Returns a typed store error when private metadata cannot be read or
    /// validated.
    fn matching(
        &mut self,
        binding: &PromptCacheResourceBinding,
    ) -> Result<Option<PromptCacheResourceRecord>, PromptCacheResourceError>;

    /// Inserts or replaces one record by local id.
    ///
    /// # Errors
    ///
    /// Returns a typed store error when the bound is reached or private
    /// metadata cannot be persisted.
    fn put(&mut self, record: &PromptCacheResourceRecord) -> Result<(), PromptCacheResourceError>;

    /// Removes one confirmed-deleted local record.
    ///
    /// # Errors
    ///
    /// Returns a typed store error when private metadata cannot be updated.
    fn remove(&mut self, id: &PromptCacheResourceId) -> Result<(), PromptCacheResourceError>;

    /// Returns at most `maximum` records for bounded inspection or cleanup.
    ///
    /// # Errors
    ///
    /// Returns a typed store error when private metadata cannot be read or
    /// validated.
    fn inspect(
        &mut self,
        maximum: usize,
    ) -> Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError>;
}
