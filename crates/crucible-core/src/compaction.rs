//! What a compaction was and what it took.
//!
//! Two small values in core because [`crate::Event`] carries them: the thread
//! that draws is told room is being made and then what it came to, and neither
//! of those may name the loop that did it.

/// What asked for room to be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compacting {
    /// The user asked, between turns.
    Asked,
    /// The load reached the bound while a turn was running.
    Full,
    /// The provider refused a request for want of room.
    Refused,
}

/// What compacting did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compacted {
    /// Why it happened.
    pub why: Compacting,
    /// How many messages the recap stands in place of.
    pub replaced: usize,
    /// What the next request would have carried before it.
    pub before: u64,
    /// And what it would carry now.
    pub after: u64,
    /// How many turns were kept word for word.
    pub kept: usize,
}
