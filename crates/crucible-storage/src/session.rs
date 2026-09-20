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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionOwner(Box<str>);

impl SessionOwner {
    /// The owner `named` spells, or `None` where it spells nobody.
    #[must_use]
    pub fn new(named: &str) -> Option<Self> {
        (!named.is_empty()).then(|| Self(named.into()))
    }

    /// The bytes that tell this owner from another, for a digest to take in.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// The conversation-writing seam used by a runner.
///
/// Every method takes `&self`: a turn records from the thread it runs on while
/// the application reads the same session to draw it, so the implementation
/// owns whatever it needs to make that safe.
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
    fn append_message(&self, message: &Message);

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
    fn contextual(&self, patch: &ContextPatch) -> Result<(), ContextError>;

    /// Records that room was made, and what the notes stand in place of.
    ///
    /// The messages it replaced are still in the record. This says what
    /// happened; a session continued later reads it and leaves those messages
    /// out of the transcript without losing them.
    fn compacted(&self, replaced: usize, recap: &str);

    /// Records the completed compaction notice as a reader is shown it.
    ///
    /// `pruned` says a clearing record belongs to this same operation, so a
    /// replay for the screen joins the two instead of merging older ones.
    fn display_compacted(&self, compacted: Compacted, pruned: bool);

    /// Records that old tool results were cleared to make room, and which.
    fn pruned(&self, freed: usize, results: &[ToolId]);

    /// Records that results were cleared because the vendor that produced them
    /// restricts where they may be sent, and the sentence left in their place.
    ///
    /// Beside [`SessionStore::pruned`] and read back the same way, with one
    /// difference that decides how a store may treat it: a pruning a later
    /// reader missed costs a little context, and a restriction a later reader
    /// missed sends one vendor's results to another.
    fn restricted(&self, freed: usize, results: &[ToolId], notice: &str);

    /// Records what the request behind the message just recorded carried.
    ///
    /// Written after that message and never beside it: the record is read
    /// forwards, so everything above this line is what was sent to get the
    /// answer above it.
    fn measured(&self, calibration: &Calibration);

    /// What the record this session was picked up from last said it carried.
    ///
    /// `None` for a session started here, and for one whose record does not end
    /// with that line — a resumed run that has to estimate is the behaviour
    /// this exists to replace.
    fn calibrated(&self) -> Option<Calibration>;
}
