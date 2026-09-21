//! The conversation seam a turn is recorded through and replayed from.
//!
//! A turn produces two kinds of durable fact, and they are not the same kind.
//! One is what was *said* — the closed [`Message`] vocabulary a provider
//! receives. The other is what was *done to what was said*: room made, results
//! cleared, a vendor's results withdrawn, the exact load a request carried, the
//! typed state the words were assembled from. A store that took only the first
//! could be appended to but never replayed, because a session continued later
//! would send the model a transcript nobody ever sent it.
//!
//! So the whole of it is here, and none of it is optional. There is no default
//! body on a recording method: a store that cannot keep one of these facts has
//! to say so in its own code rather than silently drop a line that a continue
//! reads. What is *not* here is how any of it is spelled — file names, line
//! formats, locks and browsing belong to whatever implements this.

use crucible_types::{
    Calibration, Compacted, ContextError, ContextPatch, ContextSnapshot, Message, SessionId, ToolId,
};

use crate::BoxFuture;

/// Whose records a store keeps, as an opaque separator.
///
/// Two stores that answer differently belong to different principals, and
/// nothing one of them kept may be offered under the other's identity. What the
/// words spell is the store's own business — a directory, an account, a tenant
/// — and a reader above compares them, or folds their bytes into a digest,
/// rather than parsing them.
///
/// It always names somebody. A store that keeps nothing for anybody has no
/// owner to answer with, which is a different answer from an owner whose name
/// happens to be empty, and the only one of the two that can be written.
///
/// ```
/// use crucible_storage::SessionOwner;
///
/// assert_eq!(SessionOwner::new(""), None);
/// assert_ne!(SessionOwner::new("/home/one/sessions"), SessionOwner::new("/home/two/sessions"));
/// ```
///
/// Bytes rather than text, because what names an owner need not be text: a
/// directory's name is whatever the filesystem kept, and two names that are not
/// text read as the same replacement characters once they are made into it.
/// An owner named in text is the same owner as one named by that text's bytes.
///
/// ```
/// use crucible_storage::SessionOwner;
///
/// assert_eq!(SessionOwner::new("one"), SessionOwner::of_bytes(b"one"));
/// assert_ne!(SessionOwner::of_bytes(b"\xff"), SessionOwner::of_bytes(b"\xfe"));
/// ```
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SessionOwner(Box<[u8]>);

impl SessionOwner {
    /// The owner `named` spells, or `None` where it spells nobody.
    #[must_use]
    pub fn new(named: &str) -> Option<Self> {
        Self::of_bytes(named.as_bytes())
    }

    /// The owner `named` spells where that is not known to be text, or `None`
    /// where it spells nobody.
    #[must_use]
    pub fn of_bytes(named: &[u8]) -> Option<Self> {
        (!named.is_empty()).then(|| Self(named.into()))
    }

    /// The bytes that tell this owner from another, for a digest to take in.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for SessionOwner {
    /// As text, for a person reading a failure; what is not text is replaced,
    /// so two owners that differ can read alike here and nowhere else.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SessionOwner")
            .field(&String::from_utf8_lossy(&self.0))
            .finish()
    }
}

/// The conversation-writing seam used by a runner.
///
/// Every method takes `&self`: a turn records from the thread it runs on while
/// the application reads the same session to draw it, so the implementation
/// owns whatever it needs to make that safe.
///
/// Every write waits on whatever keeps the record, so each hands back a boxed
/// `Send` future borrowing no more than it was given; what it answers is what
/// the write always answered. What only reads what the store already holds —
/// its name, its owner, the state it reconstructed — answers at once.
pub trait SessionStore: Send + Sync {
    /// Which session this is, or `None` where nothing is being recorded.
    ///
    /// A store that keeps nothing has no name to answer with. What reads this
    /// is a list somebody is picking from, and a session with no log is not on
    /// it.
    fn session_id(&self) -> Option<SessionId>;

    /// Whose records these are, or `None` where they are nobody's.
    ///
    /// A store that keeps nothing, or keeps it only for as long as it lives,
    /// holds no principal's records and says so; one that does answers with the
    /// [`SessionOwner`] every other store of the same principal answers with.
    fn owner(&self) -> Option<SessionOwner>;

    /// Appends one closed conversation message.
    fn append_message<'a>(&'a self, message: &'a Message) -> BoxFuture<'a, ()>;

    /// The typed model-visible state reconstructed for this session.
    ///
    /// `None` means unknown vintage — a session recorded before typed context
    /// existed — and must never be read as a known empty snapshot.
    fn context_snapshot(&self) -> Option<ContextSnapshot>;

    /// Records one per-pass merge patch and advances the reconstructed state.
    ///
    /// # Errors
    ///
    /// [`ContextError`] if the patch cannot produce a valid snapshot from the
    /// state this store holds.
    fn contextual<'a>(&'a self, patch: &'a ContextPatch)
    -> BoxFuture<'a, Result<(), ContextError>>;

    /// Records that room was made, and what the notes stand in place of.
    ///
    /// The messages it replaced are still in the record. This says what
    /// happened; a session continued later reads it and leaves those messages
    /// out of the transcript without losing them.
    fn compacted<'a>(&'a self, replaced: usize, recap: &'a str) -> BoxFuture<'a, ()>;

    /// Records the completed compaction notice as a reader is shown it.
    ///
    /// `pruned` says a clearing record belongs to this same operation, so a
    /// replay for the screen joins the two instead of merging older ones.
    fn display_compacted(&self, compacted: Compacted, pruned: bool) -> BoxFuture<'_, ()>;

    /// Records that old tool results were cleared to make room, and which.
    fn pruned<'a>(&'a self, freed: usize, results: &'a [ToolId]) -> BoxFuture<'a, ()>;

    /// Records that results were cleared because the vendor that produced them
    /// restricts where they may be sent, and the sentence left in their place.
    ///
    /// Beside [`SessionStore::pruned`] and read back the same way, with one
    /// difference that decides how a store may treat it: a pruning a later
    /// reader missed costs a little context, and a restriction a later reader
    /// missed sends one vendor's results to another.
    fn restricted<'a>(
        &'a self,
        freed: usize,
        results: &'a [ToolId],
        notice: &'a str,
    ) -> BoxFuture<'a, ()>;

    /// Records what the request behind the message just recorded carried.
    ///
    /// Written after that message and never beside it: the record is read
    /// forwards, so everything above this line is what was sent to get the
    /// answer above it.
    fn measured<'a>(&'a self, calibration: &'a Calibration) -> BoxFuture<'a, ()>;

    /// What the record this session was picked up from last said it carried.
    ///
    /// `None` for a session started here, and for one whose record does not end
    /// with that line — a resumed run that has to estimate is the behaviour
    /// this exists to replace.
    fn calibrated(&self) -> Option<Calibration>;
}
