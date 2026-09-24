//! Putting a pending action to a front end, and settling it on its answer.
//!
//! A turn stops twice over: when the permission engine wants to know whether a
//! call may run, and when a model puts questions to the person. Either way the
//! stop is given an identity minted here, put to the [`Front`] as a
//! [`Pending`], and held until a [`Decision`] naming that identity and
//! answering that kind of question comes back. The count identities come from
//! is this module's and the process's: no caller holds it, makes one or hands
//! one in, so there is no second count to start again at one, and a decision
//! composed for an earlier turn's action names nothing in a later turn, in
//! this conversation or another. A decision naming any other identity is
//! stale; one answering the other kind of question is wrong; both are refused
//! by name and the action stays exactly as pending as it was — for [`TRIES`]
//! such decisions. After that the front end is told the action is abandoned
//! and it settles as it does where nobody answers, so a front end that is
//! wrong for ever cannot hold a turn for ever.
//!
//! An action is put only where it can be put whole. Questions with more
//! answers than a list holds, or an answer whose name would be cut, are
//! declined without being put; a call whose tool or subject would be cut is
//! denied without being put, unless the front end says it draws from the
//! whole value it is lent ([`Front::draws_whole`]). Either way no identity is
//! spent on an action nobody was shown.
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

use std::sync::atomic::{AtomicU64, Ordering};

use crucible_client_api::bounds::ITEMS;
use crucible_client_api::{
    Asked, Capabilities, Capability, Choice, Decision, Effect, ErrorCode, Lasting, Pending,
    PendingId, Picked, Refusal, Ruling, Said, Text,
};
use crucible_runtime::BoxFuture;
use crucible_tools::{Ask, Remember, Sensitivity, Verdict};
use crucible_types::{Answered, Question, ToolCall};

/// How many pending identities this process has given out.
///
/// One count for as long as the host runs, private to this module, so an
/// identity is never given out twice whoever asks for the turn and however
/// many conversations the host holds. That is what makes the identity of an
/// action from an earlier turn, or of one already settled, name nothing
/// afterwards.
///
/// True of the first `u64::MAX` identities, which is every one a host gives
/// out: at a million a second that is more than half a million years.
/// Nothing stops the count there. The addition wraps, so the identity after
/// `u64::MAX` is `u64::MAX` once more — the saturating step in [`mint`] keeps
/// it off zero — and the one after that is one again.
static MINTED: AtomicU64 = AtomicU64::new(0);

/// The next identity: one more than however many were given out before it.
fn mint() -> PendingId {
    PendingId::new(MINTED.fetch_add(1, Ordering::Relaxed).saturating_add(1))
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
pub trait Front: Send {
    /// Puts `pending` and awaits a word about it.
    ///
    /// The wait is human-length, so this hands back a future rather than
    /// blocking inside a ready one: an implementation answers `Pending` the
    /// moment it is asked and wakes its waker once somebody has decided, so
    /// the thread polling it is never held for the wait. A dropped turn drops
    /// the future with it, and an implementation must leave no waiter behind
    /// when that happens.
    ///
    /// `None` is "nobody is going to answer", which settles a permission as a
    /// denial and questions as declined. A decision that does not fit is
    /// refused and the same action is put again, [`TRIES`] times at most; the
    /// last of them is followed by [`ErrorCode::Abandoned`] and the action
    /// settles as it does for `None`.
    ///
    /// The decision is the one given in answer to this put. A front end that
    /// carries decisions in from outside the process answers with one that
    /// arrived after it put this action and discards any that arrived before:
    /// an identity is the next number from a count and can be guessed, so a
    /// decision sent ahead of the action it names was composed by somebody who
    /// had not been shown it.
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Decision>>;

    /// Says that the decision just given settled nothing, and why.
    fn refused(&mut self, refusal: Refusal);

    /// Whether whoever answers is shown the whole [`Shown`] value, rather than
    /// the [`Pending`] whose words are cut to the contract's ceilings.
    ///
    /// No, unless a front end says otherwise, and then a call whose tool or
    /// subject would be cut is denied without being put: a yes to half a
    /// command line is a yes to a line nobody read. A front end standing on
    /// the host that draws the call itself says yes here and is put every
    /// call.
    fn draws_whole(&self) -> bool {
        false
    }
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
    pub fn new(front: &'a mut dyn Front, capabilities: Capabilities) -> Self {
        Self {
            front,
            capabilities,
        }
    }
}

impl Ask for Deciding<'_> {
    fn ask<'a>(
        &'a mut self,
        call: &'a ToolCall,
        sensitivity: &'a Sensitivity,
    ) -> BoxFuture<'a, (Verdict, Remember)> {
        Box::pin(async move {
            // A client that never said it answers permission questions is not
            // put one: the call is refused rather than left waiting on nobody.
            if !self.capabilities.has(Capability::Permissions) {
                return DENIED;
            }

            // Words cut for a reader who has nothing else to read are not the
            // question, so it is not asked, and the call is refused as it is
            // where nobody answers. Decided before an identity is minted for
            // it.
            let tool = Text::cut(&call.name);
            let subject = Text::cut(&sensitivity.to_string());
            if (tool.truncated() || subject.truncated()) && !self.front.draws_whole() {
                return DENIED;
            }

            let pending = Pending::Permission {
                id: mint(),
                tool,
                effect: effect(sensitivity),
                subject,
            };
            let shown = Shown::Call { call, sensitivity };

            let mut tries = 0;
            loop {
                let Some(decision) = self.front.put(&pending, shown).await else {
                    return DENIED;
                };
                let refusal = match settle(&pending, decision) {
                    Ok(Settled::Ruled(ruling, lasting)) => {
                        return (verdict(ruling), remember(lasting));
                    }
                    // `settle` gives a permission nothing else; refused the
                    // same way all the same, so that a new arm cannot become a
                    // yes.
                    Ok(Settled::Answered(_) | Settled::Declined) => ErrorCode::WrongDecision.into(),
                    Err(refusal) => refusal,
                };
                if turned_away(self.front, refusal, &mut tries) {
                    return DENIED;
                }
            }
        })
    }
}

/// Puts a model's `asked` questions through `front`, and answers with what was
/// chosen — or `None` where nobody answered, which is what a tool asking them
/// already takes for "nobody is there".
///
/// What was chosen and the note beside it come back whole: a decision carries
/// them as [`Said`], which refuses what it cannot hold, so nothing a tool acts
/// on was shortened on the way. What a question asks and what an answer means
/// go out cut for a reader; the answers themselves go out whole or not at all.
///
/// More than [`ITEMS`] questions are declined without being put, and the asker
/// hears "nobody is there": one pending action holds no more, and half of them
/// answered is not an answer. So is a question offering more than [`ITEMS`]
/// answers, or an answer whose name would be cut: an answer left out cannot be
/// chosen, and a name that was cut is not the one the asker reads back. The
/// question tool refuses a call that long before it asks anything, so this is
/// the floor under an asker that does not.
pub async fn questions(
    capabilities: Capabilities,
    front: &mut dyn Front,
    asked: &[Question],
) -> Option<Vec<Answered>> {
    // More questions than one pending action holds cannot be put whole, and
    // half of them answered is not an answer.
    if !capabilities.has(Capability::Questions) || asked.len() > ITEMS {
        return None;
    }

    // Read before an identity is minted, so that questions declined here,
    // which nobody is put, spend none.
    let questions = asked.iter().map(put).collect::<Option<_>>()?;
    let pending = Pending::Questions {
        id: mint(),
        questions,
    };

    let mut tries = 0;
    loop {
        let decision = front.put(&pending, Shown::Questions(asked)).await?;
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

/// One question, as it is put to a client, or `None` where it cannot be put
/// whole: more answers than a list here holds, or an answer's name longer than
/// the words here carry.
fn put(question: &Question) -> Option<Asked> {
    if question.answers().len() > ITEMS {
        return None;
    }

    Some(Asked {
        heading: Text::cut(question.heading()),
        asks: Text::cut(question.question()),
        several: question.takes_several(),
        choices: question
            .answers()
            .map(|answer| {
                let name = Text::cut(answer.answer());
                (!name.truncated()).then(|| Choice {
                    name,
                    says: Text::cut(answer.says()),
                })
            })
            .collect::<Option<_>>()?,
    })
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
