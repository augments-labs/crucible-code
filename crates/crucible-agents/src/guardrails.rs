//! What a definition will accept, and what it will stand behind.
//!
//! Two checks with the same shape at either end of an invocation. An input
//! guardrail reads what the caller asked before any of it reaches a provider;
//! an output guardrail reads the final candidate answer before it is accepted
//! and written down. Both answer with a [`Decision`], and both may fail to
//! reach one, which is a third answer rather than a refusal.
//!
//! What a guardrail is handed is [`AgentContext`] and nothing else: the run it
//! belongs to, the agent it is checking for, and the words in question. There
//! is no runner on it, no way to publish an event, no approval to mint, no
//! configuration and no parent session. That is the whole of the authority a
//! check has, and it is why a guardrail can only narrow what happens — refusing
//! an invocation or an answer — and never widen it.
//!
//! Tool input and output guards are a different thing under a similar word.
//! They stay where they are, at the tool level, judging one call's arguments
//! and one call's result.

use std::fmt;
use std::sync::Arc;

use crucible_types::{AgentId, RunId};

/// The most bytes of a guardrail's name that are kept.
///
/// A name is whatever a check answers to, and a refusal, a check that could
/// not decide and [`NameTaken`] all carry it to a reader.
pub const GUARDRAIL_NAME_BYTES: usize = 256;

/// The most bytes of a guardrail's reason that are kept, for a refusal and for
/// a check that could not decide alike.
pub const GUARDRAIL_REASON_BYTES: usize = 4 * 1024;

/// What a name or a reason that was cut ends with, inside its ceiling.
const CUT: &str = " [cut]";

/// Words kept under a ceiling, and whether that cost any of them.
///
/// Both a name and a reason are a check's own words and a check is code
/// somebody wrote, so there is no length they can be relied on to have. They
/// are drawn for a reader and sent to a client, and what is kept of them is
/// settled here, before either holds a copy.
///
/// A cut is told twice. [`CUT`] is written after the words, so that a reader
/// of the words alone is told; and the fact is kept beside them, because a
/// check may end its own words with the mark and the words cannot then say
/// which it was. Whoever carries them on reports the fact, not the mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Kept {
    words: Box<str>,
    cut: bool,
}

impl Kept {
    /// `words`, whole where they fit under `ceiling` and otherwise cut on a
    /// character boundary with [`CUT`] written after them, still under it.
    pub(crate) fn of(words: &str, ceiling: usize) -> Self {
        if words.len() <= ceiling {
            return Self {
                words: words.into(),
                cut: false,
            };
        }

        let mut end = ceiling.saturating_sub(CUT.len());
        while !words.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        let mut cut = String::with_capacity(end.saturating_add(CUT.len()));
        cut.push_str(words.get(..end).unwrap_or_default());
        cut.push_str(CUT);
        Self {
            words: cut.into(),
            cut: true,
        }
    }

    /// What was kept, mark and all.
    pub(crate) fn as_str(&self) -> &str {
        &self.words
    }

    /// Whether there were more words than these.
    pub(crate) const fn was_cut(&self) -> bool {
        self.cut
    }
}

/// What one check is told about the invocation it is judging.
///
/// Read-only, borrowed for the length of the check, and narrow on purpose. A
/// guardrail that could reach the session would be a second route to
/// everything a run may do, and the point of asking one is that it cannot.
///
/// The error code below is what a guard reaching for a session fails with
/// today, rather than something the harness checks: it pins the absence of the
/// name, which is the property this type is for.
///
/// ```compile_fail,E0599
/// fn checking(context: &crucible_agents::AgentContext<'_>) {
///     let _ = context.session();
/// }
/// ```
///
/// `Debug` names the run and the agent and redacts the words, which are the
/// reader's prompt.
#[derive(Clone, Copy)]
pub struct AgentContext<'a> {
    run: RunId,
    agent: &'a AgentId,
    said: &'a str,
}

impl fmt::Debug for AgentContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentContext")
            .field("run", &self.run)
            .field("agent", &self.agent)
            .field("said", &"[redacted]")
            .finish()
    }
}

impl<'a> AgentContext<'a> {
    /// The view one check of one invocation gets.
    ///
    /// Public because the runner is what builds it and lives in another crate.
    /// It confers nothing: every argument is an identity or the text already
    /// being judged, so there is no authority here to withhold.
    #[must_use]
    pub const fn new(run: RunId, agent: &'a AgentId, said: &'a str) -> Self {
        Self { run, agent, said }
    }

    /// Which run is being checked.
    #[must_use]
    pub const fn run(&self) -> RunId {
        self.run
    }

    /// Which agent it is being checked for.
    #[must_use]
    pub const fn agent(&self) -> &AgentId {
        self.agent
    }

    /// The words in question: what the caller asked, for an input check, and
    /// nothing for an output one, whose candidate is handed in separately.
    #[must_use]
    pub const fn said(&self) -> &str {
        self.said
    }
}

/// Which guardrail refused, and why.
///
/// Kept apart from a failure to decide because the two mean opposite things to
/// a reader: this one is the check working.
///
/// A check does not make one. It answers [`Decision::Rejected`] with its reason
/// alone, and whoever asked it writes the refusal under the name the check was
/// [`Declared`] with. That name was read once, before the check was asked
/// anything, and no other check on the definition has it, so a refusal cannot
/// be put under another check's name. The
/// error code below is what handing a decision a refusal fails with today,
/// rather than something the harness checks: it pins that a decision has no
/// room for a name.
///
/// ```compile_fail,E0308
/// use crucible_agents::{Decision, Rejection};
///
/// let _ = Decision::Rejected(Rejection::new("somebody-else", "no"));
/// ```
///
/// The same names with the refusal kept out of the decision, which compiles:
/// were one of them to move, this is the example that would say so, where the
/// one above would go on failing for a reason nobody meant.
///
/// ```
/// use crucible_agents::{Decision, Rejection};
///
/// let _ = Decision::Rejected("no".into());
/// let _ = Rejection::new("somebody-else", "no");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    guard: Kept,
    why: Kept,
}

impl Rejection {
    /// A refusal from `guard`, for the reason a reader is shown, each kept to
    /// its ceiling ([`GUARDRAIL_NAME_BYTES`], [`GUARDRAIL_REASON_BYTES`]) and
    /// marked where it was cut.
    ///
    /// For a caller that has the check's name and not its declaration: no
    /// check can hand one back, so the name is whatever that caller knows the
    /// check to be called. A caller holding the [`Declared`] check asks it
    /// with [`Declared::refused`] instead, as the runner does, because only
    /// that carries over whether the name was cut when it was declared.
    #[must_use]
    pub fn new(guard: &str, why: &str) -> Self {
        Self {
            guard: Kept::of(guard, GUARDRAIL_NAME_BYTES),
            why: Kept::of(why, GUARDRAIL_REASON_BYTES),
        }
    }

    /// Which guardrail refused.
    #[must_use]
    pub fn guard(&self) -> &str {
        self.guard.as_str()
    }

    /// Whether the guardrail's name was longer than what [`Self::guard`]
    /// holds. The mark the name ends with says so to a reader; this is what
    /// says so to whoever sends the name on.
    #[must_use]
    pub const fn guard_was_cut(&self) -> bool {
        self.guard.was_cut()
    }

    /// What it said about the refusal.
    #[must_use]
    pub fn why(&self) -> &str {
        self.why.as_str()
    }

    /// Whether it said more than [`Self::why`] holds.
    #[must_use]
    pub const fn why_was_cut(&self) -> bool {
        self.why.was_cut()
    }
}

/// What a check decided.
///
/// Two answers, and neither of them widens anything. There is no variant that
/// grants a tool, raises a ceiling or approves a call: a guardrail narrows or
/// refuses, and everything it could otherwise permit was settled before it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Carry on.
    Allowed,
    /// Do not, for the reason a reader is shown. Which check said so is not
    /// the check's to say: it is read off the check that was asked.
    Rejected(Box<str>),
}

impl Decision {
    /// A refusal, for the reason a reader is shown.
    #[must_use]
    pub fn rejected(why: &str) -> Self {
        Self::Rejected(why.into())
    }
}

/// What a check says when it ran and could not decide: its reason, and nothing
/// else.
///
/// There is no name on it, for the reason a [`Decision`] has none: which check
/// could not say is read off the check that was asked, so one check cannot put
/// its non-answer under another's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Undecided(Box<str>);

impl Undecided {
    /// Could not decide, for the reason a reader is shown.
    #[must_use]
    pub fn because(problem: &str) -> Self {
        Self(problem.into())
    }

    /// What the check said about why not.
    #[must_use]
    pub fn problem(&self) -> &str {
        &self.0
    }
}

/// Why a check could not reach a decision.
///
/// Distinct from a refusal in the type, because they are distinct outcomes for
/// the run: a refusal is an answer, and this is the absence of one.
///
/// A check does not make one. It answers with an [`Undecided`], and whoever
/// asked it writes this under the name of the check that was asked. The error
/// code below is what a check handing one back fails with today, rather than
/// something the harness checks: it pins that what a check fails with has no
/// room for a name.
///
/// ```compile_fail,E0308
/// use crucible_agents::{AgentContext, Decision, GuardrailError, Undecided};
///
/// fn checking(_context: &AgentContext<'_>) -> Result<Decision, Undecided> {
///     Err(GuardrailError::undecided("somebody-else", "its list is missing"))
/// }
/// ```
///
/// The same check answering with what it may, which compiles, so that a name
/// that moved fails here rather than leaving the example above failing for a
/// reason nobody meant:
///
/// ```
/// use crucible_agents::{AgentContext, Decision, GuardrailError, Undecided};
///
/// fn checking(_context: &AgentContext<'_>) -> Result<Decision, Undecided> {
///     let _ = GuardrailError::undecided("somebody-else", "its list is missing");
///     Err(Undecided::because("its list is missing"))
/// }
/// ```
///
/// What the variant holds is an [`Unanswered`], whose fields nobody outside
/// this module can write, so the only way to one is [`Self::undecided`] or the
/// check's own [`Declared`], and the ceilings hold for every one there is. The
/// error code below is the one for writing a field that is not the writer's:
///
/// ```compile_fail,E0451
/// use crucible_agents::{GuardrailError, Unanswered};
///
/// let _ = GuardrailError::Undecided(Unanswered {
///     guard: todo!(),
///     problem: todo!(),
/// });
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GuardrailError {
    /// The guardrail ran and could not say.
    #[error("the guardrail `{}` could not decide: {}", .0.guard(), .0.problem())]
    Undecided(Unanswered),
}

/// Which check could not decide, and what it said about why not, each kept to
/// the ceiling a refusal's are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unanswered {
    guard: Kept,
    problem: Kept,
}

impl Unanswered {
    /// Which guardrail.
    #[must_use]
    pub fn guard(&self) -> &str {
        self.guard.as_str()
    }

    /// Whether the guardrail's name was longer than what [`Self::guard`]
    /// holds.
    #[must_use]
    pub const fn guard_was_cut(&self) -> bool {
        self.guard.was_cut()
    }

    /// What it said about why not.
    #[must_use]
    pub fn problem(&self) -> &str {
        self.problem.as_str()
    }

    /// Whether it said more than [`Self::problem`] holds.
    #[must_use]
    pub const fn problem_was_cut(&self) -> bool {
        self.problem.was_cut()
    }
}

impl GuardrailError {
    /// `guard` ran and could not say, for the reason given, each kept to the
    /// ceiling a refusal's are.
    ///
    /// For a caller that has the check's name and not its declaration: no
    /// check can hand one back, so the name is whatever that caller knows the
    /// check to be called. A caller holding the [`Declared`] check asks it
    /// with [`Declared::unanswered`] instead, as the runner does, because only
    /// that carries over whether the name was cut when it was declared.
    #[must_use]
    pub fn undecided(guard: &str, problem: &str) -> Self {
        Self::Undecided(Unanswered {
            guard: Kept::of(guard, GUARDRAIL_NAME_BYTES),
            problem: Kept::of(problem, GUARDRAIL_REASON_BYTES),
        })
    }

    /// Whether the sentence this reads as is short of words the check or its
    /// name had: either was cut to its ceiling.
    #[must_use]
    pub const fn was_cut(&self) -> bool {
        match self {
            Self::Undecided(unanswered) => {
                unanswered.guard_was_cut() || unanswered.problem_was_cut()
            }
        }
    }
}

/// A check, and the name it was declared under.
///
/// The name is read off the check once, when a definition takes it, and kept
/// here. What the check answers to afterwards is not asked again, so a check
/// cannot be one name while it is declared and another once it has refused.
/// Only [`AgentBuilder`](crate::AgentBuilder) makes one, and it makes no two
/// with one name on the same definition.
#[derive(Debug)]
pub struct Declared<G: ?Sized> {
    name: Kept,
    check: Arc<G>,
}

impl<G: ?Sized> Declared<G> {
    /// `check`, under the name it answered to when it was declared.
    pub(crate) fn under(name: Kept, check: Arc<G>) -> Self {
        Self { name, check }
    }

    /// What the check was called when it was declared, kept to
    /// [`GUARDRAIL_NAME_BYTES`].
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// The check.
    #[must_use]
    pub fn check(&self) -> &G {
        &self.check
    }

    /// This check's refusal, for the reason it gave.
    ///
    /// Written from here rather than from [`Self::name`], because a name cut
    /// when it was declared fits its ceiling from then on, and a refusal made
    /// from the name alone would say it was whole.
    #[must_use]
    pub fn refused(&self, why: &str) -> Rejection {
        Rejection {
            guard: self.name.clone(),
            why: Kept::of(why, GUARDRAIL_REASON_BYTES),
        }
    }

    /// This check having run and not decided, for the reason it gave.
    #[must_use]
    pub fn unanswered(&self, problem: &str) -> GuardrailError {
        GuardrailError::Undecided(Unanswered {
            guard: self.name.clone(),
            problem: Kept::of(problem, GUARDRAIL_REASON_BYTES),
        })
    }
}

impl<G: ?Sized> Clone for Declared<G> {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            check: Arc::clone(&self.check),
        }
    }
}

/// A check declared under a name another check on the definition already has.
///
/// Refused rather than kept, because a refusal is written under its check's
/// name and two checks with one name would each read as the other.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a guardrail called `{}` is already declared", .0.as_str())]
pub struct NameTaken(Kept);

impl NameTaken {
    /// The name that was already declared, as it was kept.
    pub(crate) const fn of(name: Kept) -> Self {
        Self(name)
    }

    /// The name both checks answer to.
    #[must_use]
    pub fn name(&self) -> &str {
        self.0.as_str()
    }

    /// Whether the second check's name was longer than what [`Self::name`]
    /// holds.
    #[must_use]
    pub const fn was_cut(&self) -> bool {
        self.0.was_cut()
    }
}

/// A check on what the caller asked, run before any of it reaches a provider.
///
/// Implemented outside this crate as well as in it: the whole surface a custom
/// check needs is its own name and this one method, and neither requires
/// anything a caller cannot already build.
///
/// ```
/// use crucible_agents::{AgentContext, Decision, InputGuardrail, Undecided};
///
/// /// Refuses anything that reads like a credential being pasted in.
/// #[derive(Debug)]
/// struct NoSecrets;
///
/// impl InputGuardrail for NoSecrets {
///     fn name(&self) -> &str {
///         "no-secrets"
///     }
///
///     fn checking(&self, context: &AgentContext<'_>) -> Result<Decision, Undecided> {
///         if context.said().contains("api-key:") {
///             return Ok(Decision::rejected("the prompt carries a credential"));
///         }
///         Ok(Decision::Allowed)
///     }
/// }
/// ```
pub trait InputGuardrail: std::fmt::Debug + Send + Sync {
    /// What this guardrail is called, in a refusal a reader sees. Read once,
    /// when the check is declared on a definition: a refusal it makes, or a
    /// decision it could not reach, is written under what this answered then.
    fn name(&self) -> &str;

    /// Judges what the caller asked.
    ///
    /// Asked once per invocation: a turn retried after it failed is answered
    /// from the decision this reached the first time, while the same words said
    /// again after a turn has ended are a new invocation and are put to this
    /// afresh.
    ///
    /// # Errors
    ///
    /// [`Undecided`] where the check ran and could not decide. That is not a
    /// refusal: a run ends differently for each.
    fn checking(&self, context: &AgentContext<'_>) -> Result<Decision, Undecided>;
}

/// A check on the final candidate answer, run before it is accepted.
///
/// The candidate is handed in rather than read off the context, because what is
/// being judged here is the answer and not the question.
///
/// Already streamed words cannot be recalled: a reader has seen them by the
/// time this runs. What a refusal here buys is that the answer is not accepted
/// and not written down as one — a guard that must decide before anybody reads
/// a word has to buffer behind the output ceilings instead.
pub trait OutputGuardrail: std::fmt::Debug + Send + Sync {
    /// What this guardrail is called, in a refusal a reader sees. Read once,
    /// when the check is declared on a definition.
    fn name(&self) -> &str;

    /// Judges the answer the model finished on.
    ///
    /// # Errors
    ///
    /// [`Undecided`] where the check ran and could not decide.
    fn checking(&self, context: &AgentContext<'_>, candidate: &str) -> Result<Decision, Undecided>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_check_is_told_never_shows_the_words_it_is_judging() {
        // The words are the reader's prompt, which the transcript redacts once
        // they are a message. A check that logs what it was handed is not where
        // they stop being theirs.
        let agent = AgentId::new("coding");
        let context = AgentContext::new(RunId::new(), &agent, "said-debug-canary");

        let shown = format!("{context:?}");
        assert!(!shown.contains("said-debug-canary"), "{shown}");
        assert!(shown.contains("redacted"), "{shown}");
        assert!(shown.contains("coding"), "{shown}");
    }

    /// What `kept` holds of `whole`, which must be as much of it as fits before
    /// the mark: never a character split, and never fewer characters than the
    /// ceiling had room for.
    fn cut_from(kept: &str, whole: &str, ceiling: usize) {
        assert!(kept.len() <= ceiling, "{} bytes were kept", kept.len());
        let said = kept.strip_suffix(CUT).expect("a cut read as whole");
        assert!(
            whole.starts_with(said),
            "what was kept is not what was said"
        );
        let room = ceiling - CUT.len();
        assert!(
            said.len() + '€'.len_utf8() > room,
            "{} bytes were kept where {room} fitted",
            said.len()
        );
    }

    #[test]
    fn a_reason_longer_than_the_ceiling_is_cut_and_says_so() {
        // A check's reason is drawn for a reader and sent to a client, so what
        // is kept of it is decided where it is first kept. Three bytes to the
        // character, so that the ceiling less the mark falls inside one.
        let long = "€".repeat(GUARDRAIL_REASON_BYTES);
        assert!(!long.is_char_boundary(GUARDRAIL_REASON_BYTES - CUT.len()));

        let refused = Rejection::new("no-secrets", &long);
        cut_from(refused.why(), &long, GUARDRAIL_REASON_BYTES);

        assert!(refused.why_was_cut() && !refused.guard_was_cut());

        let GuardrailError::Undecided(unanswered) = GuardrailError::undecided("no-secrets", &long);
        cut_from(unanswered.problem(), &long, GUARDRAIL_REASON_BYTES);
        assert!(unanswered.problem_was_cut() && !unanswered.guard_was_cut());
    }

    #[test]
    fn words_that_end_in_the_mark_are_not_taken_for_cut_ones() {
        // The mark is a check's to write as well, so it is not what says a
        // reason was cut to anybody who has to report it.
        let forged = format!("the list is short{CUT}");

        let refused = Rejection::new("no-secrets", &forged);
        assert_eq!(refused.why(), forged);
        assert!(!refused.why_was_cut());
        assert!(!GuardrailError::undecided("no-secrets", &forged).was_cut());
    }

    #[test]
    fn a_reason_that_fits_is_kept_as_it_was_said() {
        let fits = "r".repeat(GUARDRAIL_REASON_BYTES);
        assert_eq!(Rejection::new("no-secrets", &fits).why(), fits);
    }

    #[test]
    fn a_name_longer_than_the_ceiling_is_cut_wherever_it_is_kept() {
        let long = "€".repeat(GUARDRAIL_NAME_BYTES);
        assert!(!long.is_char_boundary(GUARDRAIL_NAME_BYTES - CUT.len()));

        let refused = Rejection::new(&long, "no");
        cut_from(refused.guard(), &long, GUARDRAIL_NAME_BYTES);

        assert!(refused.guard_was_cut() && !refused.why_was_cut());

        let undecided = GuardrailError::undecided(&long, "no");
        assert!(undecided.was_cut());
        let GuardrailError::Undecided(unanswered) = undecided;
        cut_from(unanswered.guard(), &long, GUARDRAIL_NAME_BYTES);

        let taken = NameTaken::of(Kept::of(&long, GUARDRAIL_NAME_BYTES));
        cut_from(taken.name(), &long, GUARDRAIL_NAME_BYTES);
        assert!(taken.was_cut());
    }
}
