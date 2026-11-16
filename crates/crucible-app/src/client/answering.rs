//! The command that answers questions the model asked without waiting.
//!
//! The model's questions wait in a registry held by [`Questions`], a handle
//! the front end supplied at startup and keeps a clone of, as it keeps the
//! turn's `Cancel`. The front end reads what waits through it and answers
//! through the doors here; it can register, settle, take, clear or restore
//! nothing itself, and wires it to nothing: startup gives the registry to
//! the question tool and the conversation, and nothing outside this crate can
//! but a test built with the `proof` feature. One conversation holds a handle
//! at a time: a second start handed one already held is refused, since two
//! holders would share one era and one registry, and the claim ends when the
//! holding conversation is dropped. [`answer`] settles a batch there and
//! nothing else, so it needs no conversation: it is the door for an answer
//! while a turn has the conversation, and the running turn delivers it. While
//! no turn runs, [`turn`](super::turn) takes the same command and starts the
//! turn that delivers it. A handle no conversation holds settles nothing:
//! nothing is left to deliver an answer, and the next holder empties it.
//!
//! A batch's id is unique within its session, not across sessions, so the
//! handle keeps beside the registry the [`Era`] the batches belong to: the
//! session's identity where it records, otherwise a stint number minted here.
//! Each time the conversation replaces its session it sets the era to the
//! incoming session's and empties the registry, under the handle's one lock,
//! before the swap; the session picked up brings its own waiting batches back
//! at its first turn's first assembly. An answer is settled only under that
//! same lock and only where its era is the handle's, so an answer shown in
//! one session never settles a batch of another, though both may be `q0`.
//! The same session resumed is the same era: it never hands one id to two
//! batches, so the batch its log brings back is the one the answer was shown.
//!
//! What waits is read the same way, the era and the set together under the
//! lock, so a report converted after the conversation has moved on shows a
//! pair that held at one moment and never one session's batches under
//! another's era. The lock is taken before the registry's own, which the
//! registry takes per call and never holds while the runner posts.
//!
//! An answer settles questions. It is no ruling on a pending action and no yes
//! to a route whose vendor uses what is sent: nothing here reaches the
//! permission engine or the content-use hold.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crucible_builtins::AskUser;
use crucible_client_api::{
    AnswerOutcome, Capabilities, Capability, Command, Era, ErrorCode, Outcome, Picked, Refusal,
    Replied, Request, Said, Text,
};
use crucible_tools::{Deferred, Unsettled};
use crucible_types::{Answered, Batch, QuestionId, Reply, SessionId};

/// The front end's hold on the model's questions asked without waiting, and
/// on which session they belong to, as the module says.
///
/// Cloned to share: every clone is the same registry and the same era.
#[derive(Clone)]
pub struct Questions(Arc<Shared>);

/// What every clone of one [`Questions`] shares.
struct Shared {
    /// The era and the claim. Locked around every change of either, every
    /// settling and every read of what waits, so each sees them and the
    /// registry as they stood together.
    state: Mutex<State>,
    /// The batches. Its own lock is taken per call and inside `state`'s.
    held: Deferred,
}

/// What [`Shared::state`] guards.
struct State {
    /// The session the registry's batches belong to.
    era: Era,
    /// Whether a conversation holds the handle: taken by
    /// [`Questions::claim`] and given up when the [`Hold`] it built is
    /// dropped, so a settling that saw it taken finished before the holder
    /// let go.
    claimed: bool,
}

impl Questions {
    /// A handle on an empty registry, in a stint of its own until the
    /// conversation it is given to names its session.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Shared {
            state: Mutex::new(State {
                era: stint(),
                claimed: false,
            }),
            held: Deferred::default(),
        }))
    }

    /// The question tool `tool`, holding the questions it is asked not to
    /// wait on here. Given to the conversation too, by
    /// [`Conversation::holding`](crate::Conversation::holding), whose runner
    /// delivers what is settled. Crate-private: a tool holding the registry
    /// registers into it, so only startup builds one.
    pub(crate) fn asking(&self, tool: AskUser) -> AskUser {
        tool.deferring(self.0.held.clone())
    }

    /// `Self::asking`, for a test outside this crate that wires a handle
    /// as startup does. Compiled only with the `proof` feature, which
    /// nothing that ships turns on.
    #[cfg(feature = "proof")]
    #[must_use]
    pub fn asking_for_proof(&self, tool: AskUser) -> AskUser {
        self.asking(tool)
    }

    /// The claim of the one conversation holding the handle, recording into
    /// the session `incoming` names, or `None` while another holds it. A claim
    /// taken moves the questions there as [`Self::moved`] does, under the
    /// same lock, so no answer settles between the claim and the move that
    /// would empty it. The claim lasts as long as the [`Hold`], so a refused
    /// claim builds none: dropping one would end the holder's.
    pub(crate) fn claim(self, incoming: Option<&SessionId>) -> Option<Hold> {
        let mut state = self.state();
        if state.claimed {
            return None;
        }
        state.claimed = true;
        self.moving(&mut state.era, incoming);
        drop(state);
        Some(Hold(self))
    }

    /// The registry itself, for what the application wires to it: the
    /// runner that restores and delivers, and the conversation's look at
    /// whether anything settled waits to be delivered.
    pub(crate) fn held(&self) -> &Deferred {
        &self.0.held
    }

    /// The era and every batch waiting, oldest first, as they stood together.
    /// While no conversation holds the handle, what is reported is what the
    /// last holder left, and none of it can be answered.
    #[must_use]
    pub fn waiting(&self) -> (Era, Vec<Batch>) {
        let state = self.state();
        (Era::clone(&state.era), self.0.held.waiting())
    }

    /// Moves the questions to the session `incoming` names, or to a stint of
    /// its own where it records nothing: the era becomes the incoming
    /// session's and every batch, waiting or settled, is forgotten, together.
    pub(crate) fn moved(&self, incoming: Option<&SessionId>) {
        self.moving(&mut self.state().era, incoming);
    }

    /// [`Self::moved`], with the state's lock already taken.
    fn moving(&self, era: &mut Era, incoming: Option<&SessionId>) {
        *era = incoming.map_or_else(stint, |session| Era::Session(session.clone()));
        self.0.held.clear();
    }

    /// Whether a batch of `era` numbered `id` waits.
    pub(crate) fn waits(&self, era: &Era, id: QuestionId) -> bool {
        let state = self.state();
        state.era == *era && self.0.held.waiting().iter().any(|batch| batch.id() == id)
    }

    /// Settles the batch of `era` numbered `id` with `replies`, as the
    /// registry settles one; a batch of another era, or any batch while no
    /// conversation holds the handle, is [`Unsettled::Stale`], and nothing
    /// changes.
    fn settle(&self, era: &Era, id: QuestionId, replies: Vec<Reply>) -> Result<(), Unsettled> {
        let state = self.state();
        if !state.claimed || state.era != *era {
            return Err(Unsettled::Stale(id));
        }
        self.0.held.settle(id, replies)
    }

    /// The era and the claim, whether or not a thread came apart while
    /// holding them: each is set whole, so there is no half of one to find.
    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Default for Questions {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Questions {
    /// The era only: what waits is the model's words and the person's.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Questions")
            .field("era", &self.state().era)
            .finish_non_exhaustive()
    }
}

/// A conversation's claim on a [`Questions`] handle, which it keeps while it
/// holds the handle; dropping it leaves the handle free to be held again.
#[derive(Debug)]
pub(crate) struct Hold(Questions);

impl Hold {
    /// The handle held.
    pub(crate) fn questions(&self) -> &Questions {
        &self.0
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.0.state().claimed = false;
    }
}

/// A stint number no other stint has had while this host runs, counted from
/// one.
fn stint() -> Era {
    static MINTED: AtomicU64 = AtomicU64::new(0);
    Era::Stint(MINTED.fetch_add(1, Ordering::Relaxed).saturating_add(1))
}

/// Settles the batch `request` answers in `held`, and says what came of it.
///
/// The batch is settled whole or not at all. A batch not waiting, one of an
/// era that is not the handle's, or any batch while no conversation holds the
/// handle, is refused as
/// [`ErrorCode::StaleQuestions`]; replies the batch cannot take are
/// [`AnswerOutcome::Invalid`], naming the first that does not fit. Either way
/// the batch is as it was, still waiting if it was.
///
/// A client that did not claim [`Capability::Questions`] is refused as
/// [`ErrorCode::NeedsCapability`]: it is sent nothing of what waits, so it
/// cannot be answering it. Any other command is refused as
/// [`ErrorCode::Busy`]; it is not this door's.
#[must_use]
pub fn answer(request: &Request, held: &Questions) -> Outcome {
    match settle(request, held) {
        Ok(settling) => settled(settling.as_ref().map(|&()| ())),
        Err(refusal) => Outcome::Refused(refusal),
    }
}

/// [`answer`], in the application's own values: the registry's word on the
/// batch, or the refusal of a request that never reached it.
pub(super) fn settle(
    request: &Request,
    held: &Questions,
) -> Result<Result<(), Unsettled>, Refusal> {
    let Command::Answer { era, id, replies } = request.command() else {
        return Err(ErrorCode::Busy.into());
    };
    if !request.capabilities().has(Capability::Questions) {
        return Err(ErrorCode::NeedsCapability.into());
    }
    Ok(held.settle(era, *id, replies.iter().map(reply).collect()))
}

/// The handle an answer to the batch of `era` numbered `id`, from a client
/// claiming `claims`, would settle in, where it could settle anything there:
/// the turn door's check before it asks anybody anything.
///
/// Refused as [`answer`] refuses: [`ErrorCode::NeedsCapability`] for a
/// client that did not claim [`Capability::Questions`], then
/// [`ErrorCode::StaleQuestions`] where there is no handle, the era is not
/// its, or no batch by that id waits. Whether the replies fit is the
/// settling's to say, which checks the era again under the same lock.
pub(super) fn taking(
    claims: Capabilities,
    era: &Era,
    id: QuestionId,
    held: Option<&Questions>,
) -> Result<Questions, Refusal> {
    if !claims.has(Capability::Questions) {
        return Err(ErrorCode::NeedsCapability.into());
    }
    held.filter(|held| held.waits(era, id))
        .cloned()
        .ok_or_else(|| ErrorCode::StaleQuestions.into())
}

/// What a client is told of settling a batch, in the contract's words.
///
/// Every refusal is named, so one the registry adds decides here whether it
/// is about the batch or about the replies.
pub(super) fn settled(settling: Result<(), &Unsettled>) -> Outcome {
    match settling {
        Ok(()) => Outcome::Answer(AnswerOutcome::Settled),
        Err(Unsettled::Stale(_)) => Outcome::Refused(ErrorCode::StaleQuestions.into()),
        Err(
            unfit @ (Unsettled::Count { .. }
            | Unsettled::OneAnswer { .. }
            | Unsettled::Repeated { .. }
            | Unsettled::WrittenTwice { .. }
            | Unsettled::TooLong { .. }
            | Unsettled::Control { .. }),
        ) => Outcome::Answer(AnswerOutcome::Invalid {
            // The registry's sentences repeat nothing a reply said, so this
            // carries no word of the person's.
            reason: Text::cut(&unfit.to_string()),
        }),
    }
}

/// One reply, as the registry takes it. The words come across whole: the
/// contract carried them as `Said`, which refuses what it cannot hold.
fn reply(replied: &Replied) -> Reply {
    match replied {
        Replied::Answered(Picked { chosen, note }) => {
            Reply::Answered(Answered::new(chosen.iter().map(Said::as_str)).noting(note.as_str()))
        }
        Replied::Declined => Reply::Declined,
    }
}
