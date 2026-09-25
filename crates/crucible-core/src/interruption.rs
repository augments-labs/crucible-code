//! The checkpoint persistence seam.
//!
//! What an unfinished execution needs in order to resume safely is owned by
//! `crucible-storage`: the resume authority fingerprints, the pending actions
//! still to settle, the invocation records still to finish, and the redacted
//! sandbox lifecycles a resumed turn must still find. This module holds the
//! contract a store is written against, so a runner can hold a checkpoint
//! store without reaching for the values it stores.

use crucible_storage::{CheckpointId, ExecutionCheckpoint};

/// Persistence contract for execution checkpoints.
pub trait CheckpointStore {
    /// Store-owned error preserving its concrete boundary.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Durably replaces the checkpoint under its stable identity.
    ///
    /// # Errors
    ///
    /// Returns the implementation's error when validation or durable storage
    /// fails.
    fn save(&mut self, checkpoint: &ExecutionCheckpoint) -> Result<(), Self::Error>;

    /// Loads one typed checkpoint, if present.
    ///
    /// # Errors
    ///
    /// Returns the implementation's error when protected storage cannot be
    /// read or decoded safely.
    fn load(&self, id: CheckpointId) -> Result<Option<ExecutionCheckpoint>, Self::Error>;

    /// Removes one finished checkpoint. Repeating removal is idempotent.
    ///
    /// # Errors
    ///
    /// Returns the implementation's error when the protected file cannot be
    /// validated or removed.
    fn remove(&mut self, id: CheckpointId) -> Result<(), Self::Error>;
}
