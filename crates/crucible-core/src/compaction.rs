//! What a compaction was and what it took.
//!
//! Two small values in core because [`crate::Event`] carries them: the thread
//! that draws is told room is being made and then what it came to, and neither
//! of those may name the loop that did it. A third says which of the three
//! things asking for room can come back with happened, for the same reason: the
//! screen has a different line for each.

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

/// What asking for room came back with.
///
/// Three answers rather than an [`Option`], because the two that made no room
/// made none for opposite reasons and owe the reader different sentences: one
/// is a session with nothing behind the turns it keeps whole, and the other is
/// somebody who pressed a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Room {
    /// It was made, and this is what it took.
    Made(Compacted),
    /// There was nothing worth replacing — a session with no middle. Nothing
    /// was asked of the model and nothing changed.
    Nothing,
    /// Somebody stopped the recap while it was being written, so nothing
    /// changed. Half a session's memory is not one, and standing it in place of
    /// the messages it was meant to replace would lose the rest for good: the
    /// log still holds them, and nothing the model is sent ever would again.
    Stopped,
}
