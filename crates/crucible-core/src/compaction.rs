//! What a compaction was and what it took.
//!
//! Two small values in core because [`crate::Event`] carries them: the thread
//! that draws is told room is being made and then what it came to, and neither
//! of those may name the loop that did it.

/// What a recap is marked with where it stands in a transcript.
///
/// In core because two crates need the same string for opposite reasons: the
/// loop writes it so the model reads its own notes under a heading saying whose
/// they are, and the screen reads it so it can draw them as notes rather than
/// as something the user typed. Two copies of it would come apart, and the day
/// they did the notes would go back to looking like a prompt.
pub const RECAP: &str = "[everything before this was compacted to make room; \
these are your own notes on it]\n\n";

/// What asked for room to be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compacting {
    /// The user asked, between turns.
    Asked,
    /// The user chose notes over carrying a session whole, picking it up.
    ///
    /// Its own reason rather than sharing [`Self::Asked`], because what the
    /// record says about it is read later by somebody working out where a
    /// session's middle went — and "you asked" says nothing about which of the
    /// two moments they are looking at.
    Resumed,
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
