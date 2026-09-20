//! What a compaction was, and how its notes are marked.
//!
//! Shared values rather than the compactor's own, because three readers that
//! may not name the compactor need them: the session log records what a
//! compaction took and reads it back for display, the event a drawing thread is
//! told carries it, and the screen recognises the notes a recap left in the
//! transcript. What a compaction asks the model for, and what asking for room
//! came back with, belong to `crucible-context`.

/// What a recap is marked with where it stands in a transcript.
///
/// Shared because two owners need the same string for opposite reasons: the
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
    /// How many conversation messages the recap stands in place of.
    ///
    /// Typed harness context is omitted: it is reassembled after compaction
    /// and was never a user or agent message shown in the conversation.
    pub replaced: usize,
    /// What the next request would have carried before it.
    pub before: u64,
    /// And what it would carry now.
    pub after: u64,
    /// How many turns were kept word for word.
    pub kept: usize,
}
