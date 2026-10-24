//! Putting a pending action to a front end, and settling it on its answer.
//!
//! A turn stops twice over: when the permission engine wants to know whether a
//! call may run, and when a model puts questions to the person. A turn an
//! installed agent runs stops a third way, when the agent asks leave to act,
//! and is put through the same front as a permission question is, by the
//! [`Asker`] a turn is handed. Either way the
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
//! declined without being put; a call whose tool, subject, or line or address
//! as sent would be cut is denied without being put, unless the front end says
//! it draws from the whole value it is lent ([`Front::draws_whole`]). Either
//! way no identity is spent on an action nobody was shown.
//!
//! An installed agent's leave is answered to the agent, which acts on it inside
//! its own sandbox: what comes back is the contract's decision, never anything
//! the permission engine reads.
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
use crucible_client_api::inspection::Claim;
use crucible_client_api::pending::Operation;
use crucible_client_api::{
    Asked, Capabilities, Capability, Choice, Decision, Effect, ErrorCode, Lasting, Pending,
    PendingId, Picked, Refusal, Ruling, Said, Text,
};
use crucible_execution::{Confinement, Detail};
use crucible_runtime::BoxFuture;
use crucible_tools::{Ask, Remember, Sensitivity, Verdict};
use crucible_types::{Answered, Question, ToolCall};

use crate::content_use::{Warned, full_sentence};

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
pub(crate) fn mint() -> PendingId {
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
    /// The route a send would go on, whose vendor uses what is sent.
    Warning(&'a Warned),
    /// What an installed agent asked leave to do, and who would do it.
    External(Leave<'a>),
    /// Which way an account operation on an installed agent goes on.
    Account(&'a Confirmation),
    /// Which way an install, update, rollback or removal of a catalog agent
    /// goes on, put by the agents door.
    Install(&'a Confirmation),
}

/// What an installed agent asked leave to do, as the host holds it, whole,
/// and who would do it: the agent's own words for the action, read as a
/// client is told them, beside who runs it and how far it is held in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leave<'a> {
    /// The agent's name for the tool that would act.
    pub tool: &'a str,
    /// The spelling its rules are matched against.
    pub summary: &'a str,
    /// The kind of thing it would do.
    pub effect: Effect,
    /// The line that would run, or what would be sent, as the call carried
    /// it: nothing for a read or a change to a file.
    pub sent: Option<&'a str>,
    /// Whether a command would be left running after its turn has ended.
    pub left_running: bool,
    /// Who acts on it: the agent, by the name its configured instance goes by.
    pub acting: Acting<'a>,
}

/// Who runs an installed agent's actions, and how they are held in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Acting<'a> {
    /// The configured instance that runs them.
    pub instance: &'a str,
    /// How the actions it takes on this machine are held in, in the sandbox's
    /// own word: nothing where nothing has said.
    pub confinement: Option<Claim>,
}

impl<'a> Acting<'a> {
    /// The actions of `instance`, held in as `claims` says.
    #[must_use]
    pub fn new(instance: &'a str, claims: &crucible_execution::Capabilities) -> Self {
        Self {
            instance,
            confinement: claims
                .confinement()
                .map(|confined| claimed(confined.level())),
        }
    }
}

/// The sandbox's word for confinement `level`, as `crucible sandbox inspect`
/// says it.
const fn claimed(level: Confinement) -> Claim {
    match level {
        Confinement::HardEnforced => Claim::Enforced,
        Confinement::Observed => Claim::Observed,
        Confinement::Unsupported => Claim::Unsupported,
    }
}

/// An account operation's question, as the host holds it, whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    /// What is being asked.
    pub asks: String,
    /// What the operation would affect, a sentence each.
    pub facts: Vec<String>,
    /// The ways it may go, each a name and what it does. The first is the one
    /// recommended; the last leaves everything as it is.
    pub choices: Vec<(String, String)>,
}

/// Whoever answers pending actions: a terminal, or a consumer with none.
///
/// `Sync` for the reason [`Ask`] is: the asker a turn lends by shared reference
/// holds its front end, and the turn may be carried between workers.
pub trait Front: Send + Sync {
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
    /// No, unless a front end says otherwise, and then a call whose tool,
    /// subject, or line or address as sent would be cut is denied without
    /// being put: a yes to half a command line is a yes to a line nobody
    /// read. A front end standing on the host that draws the call itself says
    /// yes here and is put every call.
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
    /// Nobody will answer the questions, or the send goes back.
    Declined,
    /// Send anyway.
    Accepted,
    /// The choice of this name.
    Picked(Said),
}

/// Holds `decision` against `pending`, the one action it may settle.
///
/// An install confirmation is put by the agents door and never by a
/// conversation, so no decision held here fits one: each is refused as
/// [`ErrorCode::WrongDecision`], once it names the action at all.
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
        (
            Pending::Questions { .. } | Pending::Warning { .. } | Pending::Account { .. },
            Decision::Declined { .. },
        ) => Ok(Settled::Declined),
        (Pending::Warning { .. }, Decision::Accepted { .. }) => Ok(Settled::Accepted),
        (Pending::Account { choices, .. }, Decision::Picked { choice, .. }) => {
            if choices
                .iter()
                .any(|one| one.name.as_str() == choice.as_str())
            {
                Ok(Settled::Picked(choice))
            } else {
                Err(ErrorCode::InvalidArgument.into())
            }
        }
        (
            Pending::Permission { .. },
            Decision::Answered { .. }
            | Decision::Declined { .. }
            | Decision::Accepted { .. }
            | Decision::Picked { .. },
        )
        | (
            Pending::Questions { .. },
            Decision::Ruled { .. } | Decision::Accepted { .. } | Decision::Picked { .. },
        )
        | (
            Pending::Warning { .. },
            Decision::Ruled { .. } | Decision::Answered { .. } | Decision::Picked { .. },
        )
        | (
            Pending::Account { .. },
            Decision::Ruled { .. } | Decision::Answered { .. } | Decision::Accepted { .. },
        )
        // A conversation never puts an install confirmation: the agents door
        // does, and settles its own. So no decision fits one held here.
        | (
            Pending::Install { .. },
            Decision::Ruled { .. }
            | Decision::Answered { .. }
            | Decision::Declined { .. }
            | Decision::Accepted { .. }
            | Decision::Picked { .. },
        ) => Err(ErrorCode::WrongDecision.into()),
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
pub(crate) fn turned_away(front: &mut dyn Front, refusal: Refusal, tries: &mut usize) -> bool {
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

/// Whoever answers the pending actions of a turn: the permission engine's
/// questions, and those an installed agent raises while it runs a turn.
pub trait Asker: Ask {
    /// Puts `pending`, which an installed agent raised and `acting` would act
    /// on, and answers what was decided. No, unless the asker says otherwise:
    /// an asker that puts nothing has nobody to say yes.
    fn pending<'a>(
        &'a mut self,
        _pending: &'a crucible_execution::Pending,
        _acting: Acting<'a>,
    ) -> BoxFuture<'a, crucible_execution::Decision> {
        Box::pin(std::future::ready(crucible_execution::Decision::Deny))
    }
}

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
            // where nobody answers. That holds for the line or address as
            // sent as much as for the subject: a yes to the start of a line
            // is a yes to a line nobody read. Decided before an identity is
            // minted for it.
            let tool = Text::cut(&call.name);
            let subject = Text::cut(&sensitivity.to_string());
            let asked = operation(call, sensitivity);
            let cut = match &asked {
                Operation::Command { sent, .. } | Operation::Network { sent } => sent.truncated(),
                Operation::Other => false,
            };
            if (tool.truncated() || subject.truncated() || cut) && !self.front.draws_whole() {
                return DENIED;
            }

            let pending = Pending::Permission {
                id: mint(),
                tool,
                effect: effect(sensitivity),
                subject,
                asked,
            };
            let shown = Shown::Call { call, sensitivity };

            ruled(self.front, &pending, shown)
                .await
                .map_or(DENIED, |(ruling, lasting)| {
                    (verdict(ruling), remember(lasting))
                })
        })
    }
}

impl Asker for Deciding<'_> {
    /// Puts an installed agent's leave to act as a permission question is put,
    /// under the same capability and the same rule about words that would be
    /// cut, and answers the agent with the ruling. An agent's question is not
    /// put: no front holds a free answer to one yet, so it is declined.
    fn pending<'a>(
        &'a mut self,
        pending: &'a crucible_execution::Pending,
        acting: Acting<'a>,
    ) -> BoxFuture<'a, crucible_execution::Decision> {
        Box::pin(async move {
            const DENY: crucible_execution::Decision = crucible_execution::Decision::Deny;
            let crucible_execution::Pending::Permission {
                tool,
                summary,
                detail,
            } = pending
            else {
                return DENY;
            };
            if !self.capabilities.has(Capability::Permissions) {
                return DENY;
            }

            // Words the agent's reader already cut are not the question, even
            // to a front that draws the whole value: it is that value.
            let (effect, asked) = outside(detail);
            let clipped = tool.incomplete()
                || summary.incomplete()
                || match detail {
                    Detail::Command { sent, .. } | Detail::Network { sent } => sent.incomplete(),
                    Detail::Reads | Detail::ReadsOutside | Detail::MutatesFile => false,
                };
            let named = tool.as_str();
            let tool = Text::cut(named);
            let subject = Text::cut(summary.as_str());
            let cut = match &asked {
                Operation::Command { sent, .. } | Operation::Network { sent } => sent.truncated(),
                Operation::Other => false,
            };
            if clipped
                || ((tool.truncated() || subject.truncated() || cut) && !self.front.draws_whole())
            {
                return DENY;
            }

            let put = Pending::Permission {
                id: mint(),
                tool,
                effect,
                subject,
                asked,
            };
            let (sent, left_running) = match detail {
                Detail::Command { sent, left_running } => (Some(sent.as_str()), *left_running),
                Detail::Network { sent } => (Some(sent.as_str()), false),
                Detail::Reads | Detail::ReadsOutside | Detail::MutatesFile => (None, false),
            };
            let shown = Shown::External(Leave {
                tool: named,
                summary: summary.as_str(),
                effect,
                sent,
                left_running,
                acting,
            });
            match ruled(self.front, &put, shown).await {
                Some((Ruling::Allow, Lasting::Once)) => {
                    crucible_execution::Decision::Allow(crucible_execution::Lasting::Once)
                }
                Some((Ruling::Allow, Lasting::Session)) => {
                    crucible_execution::Decision::Allow(crucible_execution::Lasting::Session)
                }
                Some((Ruling::Deny, _)) | None => DENY,
            }
        })
    }
}

/// Puts the permission question `pending` through `front` until a ruling fits
/// it, or `None` where nobody answered or too many decisions fit nothing.
async fn ruled(
    front: &mut dyn Front,
    pending: &Pending,
    shown: Shown<'_>,
) -> Option<(Ruling, Lasting)> {
    let mut tries = 0;
    loop {
        let decision = front.put(pending, shown).await?;
        let refusal = match settle(pending, decision) {
            Ok(Settled::Ruled(ruling, lasting)) => return Some((ruling, lasting)),
            // `settle` gives a permission nothing else; refused the same way
            // all the same, so that a new arm cannot become a yes.
            Ok(
                Settled::Answered(_) | Settled::Declined | Settled::Accepted | Settled::Picked(_),
            ) => ErrorCode::WrongDecision.into(),
            Err(refusal) => refusal,
        };
        if turned_away(front, refusal, &mut tries) {
            return None;
        }
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
            Ok(Settled::Ruled(..) | Settled::Accepted | Settled::Picked(_)) => {
                ErrorCode::WrongDecision.into()
            }
            Err(refusal) => refusal,
        };
        if turned_away(front, refusal, &mut tries) {
            return None;
        }
    }
}

/// Puts the question a route whose vendor uses what is sent asks through
/// `front`: `true` where the answer was to send anyway.
///
/// Put only to a client that said it answers permission questions, which this
/// is one of: a yes lets something happen that nothing else would. Put whole
/// or not at all, as a permission question is: a sentence cut short is not
/// the vendor's, and one nobody was shown is answered as no.
pub async fn warned(capabilities: Capabilities, front: &mut dyn Front, warned: &Warned) -> bool {
    if !capabilities.has(Capability::Permissions) {
        return false;
    }
    // An installed agent's route is asked about as a launch, so the client is
    // told what starting it runs, in the words the terminal shows.
    let sentence = full_sentence(warned);
    let words = [
        Text::cut(&warned.route),
        Text::cut(&warned.shown),
        Text::cut(&sentence),
        Text::cut(&warned.warning.cited()),
    ];
    if words.iter().any(Text::truncated) && !front.draws_whole() {
        return false;
    }
    let [route, shown, sentence, source] = words;
    let pending = Pending::Warning {
        id: mint(),
        route,
        shown,
        sentence,
        source,
    };

    let mut tries = 0;
    loop {
        let Some(decision) = front.put(&pending, Shown::Warning(warned)).await else {
            return false;
        };
        let refusal = match settle(&pending, decision) {
            Ok(Settled::Accepted) => return true,
            Ok(Settled::Declined) => return false,
            Ok(Settled::Ruled(..) | Settled::Answered(_) | Settled::Picked(_)) => {
                ErrorCode::WrongDecision.into()
            }
            Err(refusal) => refusal,
        };
        if turned_away(front, refusal, &mut tries) {
            return false;
        }
    }
}

/// Puts an account operation's `confirmation` through `front`, and answers
/// with the place in its choices of the one picked, or `None` where nobody
/// picked one: the client did not say it answers account questions, it
/// declined, it answered nothing, or it was turned away [`TRIES`] times.
///
/// Put whole or not at all, as a permission question is: a sentence about
/// what would change, cut short, is not what the person agreed to.
pub async fn confirmed(
    capabilities: Capabilities,
    front: &mut dyn Front,
    confirmation: &Confirmation,
) -> Option<usize> {
    if !capabilities.has(Capability::Account) || confirmation.choices.len() > ITEMS {
        return None;
    }
    let asks = Text::cut(&confirmation.asks);
    let facts: Vec<Text> = confirmation
        .facts
        .iter()
        .map(|fact| Text::cut(fact))
        .collect();
    let choices: Vec<Choice> = confirmation
        .choices
        .iter()
        .enumerate()
        .map(|(at, (name, says))| Choice {
            name: Text::cut(name),
            says: Text::cut(says),
            recommended: at == 0,
        })
        .collect();
    let cut = asks.truncated()
        || facts.iter().any(Text::truncated)
        || choices
            .iter()
            .any(|choice| choice.name.truncated() || choice.says.truncated());
    if facts.len() > ITEMS || (cut && !front.draws_whole()) {
        return None;
    }
    let pending = Pending::Account {
        id: mint(),
        asks,
        facts,
        choices,
    };

    let mut tries = 0;
    loop {
        let decision = front.put(&pending, Shown::Account(confirmation)).await?;
        let refusal = match settle(&pending, decision) {
            Ok(Settled::Picked(choice)) => {
                return confirmation
                    .choices
                    .iter()
                    .position(|(name, _)| name == choice.as_str());
            }
            Ok(Settled::Declined) => return None,
            Ok(Settled::Ruled(..) | Settled::Answered(_) | Settled::Accepted) => {
                ErrorCode::WrongDecision.into()
            }
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
                    recommended: answer.is_recommended(),
                })
            })
            .collect::<Option<_>>()?,
    })
}

/// The kind of thing a call would do, as a client is told it.
pub(crate) const fn effect(sensitivity: &Sensitivity) -> Effect {
    match sensitivity {
        Sensitivity::ReadOnly { .. } => Effect::Reads,
        Sensitivity::ReadsOutside { .. } => Effect::ReadsOutside,
        Sensitivity::MutatesFile { .. } => Effect::MutatesFile,
        Sensitivity::SpawnsProcess { .. } => Effect::SpawnsProcess,
        Sensitivity::ReachesNetwork { .. } => Effect::ReachesNetwork,
    }
}

/// What exactly a call would run or send, as the call carried it.
///
/// The subject is the engine's spelling, which is the one a rule is matched
/// against and drops what tells two such calls apart; this is what the
/// terminal's panel quotes instead. A read or a change of a file is said whole
/// by its subject.
pub(crate) fn operation(call: &ToolCall, sensitivity: &Sensitivity) -> Operation {
    match sensitivity {
        Sensitivity::SpawnsProcess { command } => Operation::Command {
            sent: Text::cut(command.sent()),
            left_running: crucible_builtins::backgrounded(&call.args),
        },
        Sensitivity::ReachesNetwork { host } => Operation::Network {
            sent: Text::cut(host.sent()),
        },
        Sensitivity::ReadOnly { .. }
        | Sensitivity::ReadsOutside { .. }
        | Sensitivity::MutatesFile { .. } => Operation::Other,
    }
}

/// What an installed agent's action would do, and exactly what it would run or
/// send, as a client is told them: the one reading of a [`Detail`], which
/// says for the runner's own calls what [`effect`] and [`operation`] say, so a
/// client is told the same of the same action whichever backend asked.
pub(crate) fn outside(detail: &Detail) -> (Effect, Operation) {
    match detail {
        Detail::Command { sent, left_running } => (
            Effect::SpawnsProcess,
            Operation::Command {
                sent: Text::cut(sent.as_str()),
                left_running: *left_running,
            },
        ),
        Detail::Network { sent } => (
            Effect::ReachesNetwork,
            Operation::Network {
                sent: Text::cut(sent.as_str()),
            },
        ),
        Detail::Reads => (Effect::Reads, Operation::Other),
        Detail::ReadsOutside => (Effect::ReadsOutside, Operation::Other),
        Detail::MutatesFile => (Effect::MutatesFile, Operation::Other),
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
