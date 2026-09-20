//! Putting a pending action to a front end, and settling it on its answer.
//!
//! A turn stops twice over: when the permission engine wants to know whether a
//! call may run, and when a model puts questions to the person. Either way the
//! stop is given an identity minted here, put to the [`Front`] as a
//! [`Pending`], and held until a [`Decision`] naming that identity and
//! answering that kind of question comes back. A decision naming any other
//! identity is stale; one answering the other kind of question is wrong; both
//! are refused by name and the action stays exactly as pending as it was — for
//! [`TRIES`] such decisions. After that the front end is told the action is
//! abandoned and it settles as it does where nobody answers, so a front end
//! that is wrong for ever cannot hold a turn for ever.
//!
//! A decision is never a permission. It is read here, against the action this
//! module itself put, and what the engine is handed is a
//! [`Verdict`] made from the engine's own type inside the engine's own call —
//! the one frame in which the call, its arguments and the roster generation it
//! was admitted under are all still the ones the question was about. The proof
//! a tool runs under is minted by the engine from that verdict and by nothing
//! else. Minting one from a contract value does not compile — the error code
//! is what this fails with today and not a gate, since `compile_fail` accepts
//! any compile error:
//!
//! ```compile_fail,E0624
//! use crucible_client_api::Ruling;
//! use crucible_tools::{Grant, Verdict};
//!
//! let said = Ruling::Allow;
//! let forged = Grant::issue(match said {
//!     Ruling::Allow => Verdict::Allow,
//!     Ruling::Deny => Verdict::Deny,
//! });
//! ```
//!
//! Nor does a contract value convert into the engine's verdict on its own:
//!
//! ```compile_fail,E0277
//! use crucible_client_api::Ruling;
//! use crucible_tools::Verdict;
//!
//! let verdict: Verdict = Ruling::Allow.into();
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crucible_client_api::bounds::ITEMS;
use crucible_client_api::{
    Asked, Capabilities, Capability, Choice, Decision, Effect, ErrorCode, Lasting, Pending,
    PendingId, Picked, Refusal, Ruling, Said, Text,
};
use crucible_tools::{Ask, Remember, Sensitivity, Verdict};
use crucible_types::{Answered, Question, ToolCall};

/// Where pending identities come from: one counter for as long as the host
/// runs, so an identity is never given out twice.
///
/// Cloned freely; every clone counts on the same counter. That is what makes
/// the identity of an action from an earlier turn, or of one already settled,
/// name nothing afterwards.
#[derive(Debug, Clone, Default)]
pub struct Minting(Arc<AtomicU64>);

impl Minting {
    /// A counter nothing has been minted from.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The next identity.
    fn next(&self) -> PendingId {
        PendingId::new(self.0.fetch_add(1, Ordering::Relaxed).saturating_add(1))
    }
}

/// What a pending action is about, as the host holds it.
///
/// A [`Pending`] is what may leave the process, and its words are cut to the
/// contract's ceilings. A front end standing on the host draws from the whole
/// value instead, which is why it is lent beside the pending action and is in
/// no serialized form.
#[derive(Debug, Clone, Copy)]
pub enum Shown<'a> {
    /// The call a permission question is about.
    Call {
        /// The call.
        call: &'a ToolCall,
        /// What it would do.
        sensitivity: &'a Sensitivity,
    },
    /// The questions a model asked.
    Questions(&'a [Question]),
}

/// Whoever answers pending actions: a terminal, or a consumer with none.
pub trait Front {
    /// Puts `pending` and waits for a word about it.
    ///
    /// `None` is "nobody is going to answer", which settles a permission as a
    /// denial and questions as declined. A decision that does not fit is
    /// refused and the same action is put again, [`TRIES`] times at most; the
    /// last of them is followed by [`ErrorCode::Abandoned`] and the action
    /// settles as it does for `None`.
    fn put(&mut self, pending: &Pending, shown: Shown<'_>) -> Option<Decision>;

    /// Says that the decision just given settled nothing, and why.
    fn refused(&mut self, refusal: Refusal);
}

/// What a decision that fits its pending action comes to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Settled {
    /// A yes or a no, and how long it lasts.
    Ruled(Ruling, Lasting),
    /// One answer per question.
    Answered(Vec<Picked>),
    /// Nobody will answer the questions.
    Declined,
}

/// Holds `decision` against `pending`, the one action it may settle.
pub(crate) fn settle(pending: &Pending, decision: Decision) -> Result<Settled, Refusal> {
    if decision.id() != pending.id() {
        return Err(ErrorCode::StaleDecision.into());
    }

    match (pending, decision) {
        (
            Pending::Permission { .. },
            Decision::Ruled {
                ruling, lasting, ..
            },
        ) => Ok(Settled::Ruled(ruling, lasting)),
        (Pending::Questions { questions, .. }, Decision::Answered { answers, .. }) => {
            if answers.len() == questions.len() {
                Ok(Settled::Answered(answers))
            } else {
                Err(ErrorCode::InvalidArgument.into())
            }
        }
        (Pending::Questions { .. }, Decision::Declined { .. }) => Ok(Settled::Declined),
        (Pending::Permission { .. }, Decision::Answered { .. } | Decision::Declined { .. })
        | (Pending::Questions { .. }, Decision::Ruled { .. }) => {
            Err(ErrorCode::WrongDecision.into())
        }
    }
}

/// How many decisions that fit nothing one pending action takes before it is
/// settled as though nobody answered.
///
/// A person at a panel cannot give one, so this is never met at a terminal. It
/// is what stops a client that answers wrongly for ever from holding the turn
/// that asked for ever.
pub const TRIES: usize = 3;

/// Tells `front` its decision was refused, and whether that was one too many.
fn turned_away(front: &mut dyn Front, refusal: Refusal, tries: &mut usize) -> bool {
    front.refused(refusal);
    *tries = tries.saturating_add(1);
    if *tries < TRIES {
        return false;
    }
    front.refused(ErrorCode::Abandoned.into());
    true
}

/// What is answered where nobody will: no, and only this once.
const DENIED: (Verdict, Remember) = (Verdict::Deny, Remember::Never);

/// The permission engine's asking, put through a [`Front`].
pub struct Deciding<'a> {
    front: &'a mut dyn Front,
    minting: &'a Minting,
    capabilities: Capabilities,
}

impl std::fmt::Debug for Deciding<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Deciding")
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

impl<'a> Deciding<'a> {
    /// Asks `front`, for a client that said it has `capabilities`.
    pub fn new(front: &'a mut dyn Front, minting: &'a Minting, capabilities: Capabilities) -> Self {
        Self {
            front,
            minting,
            capabilities,
        }
    }
}

impl Ask for Deciding<'_> {
    fn ask(&mut self, call: &ToolCall, sensitivity: &Sensitivity) -> (Verdict, Remember) {
        // A client that never said it answers permission questions is not put
        // one: the call is refused rather than left waiting on nobody.
        if !self.capabilities.has(Capability::Permissions) {
            return DENIED;
        }

        let pending = Pending::Permission {
            id: self.minting.next(),
            tool: Text::cut(&call.name),
            effect: effect(sensitivity),
            subject: Text::cut(&sensitivity.to_string()),
        };
        let shown = Shown::Call { call, sensitivity };

        let mut tries = 0;
        loop {
            let Some(decision) = self.front.put(&pending, shown) else {
                return DENIED;
            };
            let refusal = match settle(&pending, decision) {
                Ok(Settled::Ruled(ruling, lasting)) => return (verdict(ruling), remember(lasting)),
                // `settle` gives a permission nothing else; refused the same
                // way all the same, so that a new arm cannot become a yes.
                Ok(Settled::Answered(_) | Settled::Declined) => ErrorCode::WrongDecision.into(),
                Err(refusal) => refusal,
            };
            if turned_away(self.front, refusal, &mut tries) {
                return DENIED;
            }
        }
    }
}

/// Puts a model's `asked` questions through `front`, and answers with what was
/// chosen — or `None` where nobody answered, which is what a tool asking them
/// already takes for "nobody is there".
///
/// What was chosen and the note beside it come back whole: a decision carries
/// them as [`Said`], which refuses what it cannot hold, so nothing a tool acts
/// on was shortened on the way. The questions themselves go out cut for a
/// reader, and the answers offered beyond [`ITEMS`] are not put.
///
/// More than [`ITEMS`] questions are declined without being put, and the asker
/// hears "nobody is there": one pending action holds no more, and half of them
/// answered is not an answer. The question tool refuses a call that long before
/// it asks anything, so this is the floor under an asker that does not.
pub fn questions(
    minting: &Minting,
    capabilities: Capabilities,
    front: &mut dyn Front,
    asked: &[Question],
) -> Option<Vec<Answered>> {
    // More questions than one pending action holds cannot be put whole, and
    // half of them answered is not an answer.
    if !capabilities.has(Capability::Questions) || asked.len() > ITEMS {
        return None;
    }

    let pending = Pending::Questions {
        id: minting.next(),
        questions: asked.iter().map(put).collect(),
    };

    let mut tries = 0;
    loop {
        let decision = front.put(&pending, Shown::Questions(asked))?;
        let refusal = match settle(&pending, decision) {
            Ok(Settled::Answered(answers)) => {
                return Some(
                    answers
                        .into_iter()
                        .map(|picked| {
                            Answered::new(picked.chosen.iter().map(Said::as_str))
                                .noting(picked.note.as_str())
                        })
                        .collect(),
                );
            }
            Ok(Settled::Declined) => return None,
            Ok(Settled::Ruled(..)) => ErrorCode::WrongDecision.into(),
            Err(refusal) => refusal,
        };
        if turned_away(front, refusal, &mut tries) {
            return None;
        }
    }
}

/// One question, as it is put to a client.
fn put(question: &Question) -> Asked {
    Asked {
        heading: Text::cut(question.heading()),
        asks: Text::cut(question.question()),
        several: question.takes_several(),
        choices: question
            .answers()
            .take(ITEMS)
            .map(|answer| Choice {
                name: Text::cut(answer.answer()),
                says: Text::cut(answer.says()),
            })
            .collect(),
    }
}

/// The kind of thing a call would do, as a client is told it.
const fn effect(sensitivity: &Sensitivity) -> Effect {
    match sensitivity {
        Sensitivity::ReadOnly { .. } => Effect::Reads,
        Sensitivity::ReadsOutside { .. } => Effect::ReadsOutside,
        Sensitivity::MutatesFile { .. } => Effect::MutatesFile,
        Sensitivity::SpawnsProcess { .. } => Effect::SpawnsProcess,
        Sensitivity::ReachesNetwork { .. } => Effect::ReachesNetwork,
    }
}

/// The engine's verdict for a ruling that fit the action it was about.
const fn verdict(ruling: Ruling) -> Verdict {
    match ruling {
        Ruling::Allow => Verdict::Allow,
        Ruling::Deny => Verdict::Deny,
    }
}

/// How long the engine keeps it. Nothing a client says is kept past the host:
/// a standing rule is written by the person at the host.
const fn remember(lasting: Lasting) -> Remember {
    match lasting {
        Lasting::Once => Remember::Never,
        Lasting::Session => Remember::Session,
    }
}
