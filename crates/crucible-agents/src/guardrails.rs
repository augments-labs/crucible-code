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

use crucible_types::{AgentId, RunId};

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
#[derive(Debug, Clone, Copy)]
pub struct AgentContext<'a> {
    run: RunId,
    agent: &'a AgentId,
    said: &'a str,
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

/// Why a guardrail refused.
///
/// Kept apart from a failure to decide because the two mean opposite things to
/// a reader: this one is the check working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    guard: Box<str>,
    why: Box<str>,
}

impl Rejection {
    /// A refusal from `guard`, for the reason a reader is shown.
    #[must_use]
    pub fn new(guard: &str, why: &str) -> Self {
        Self {
            guard: guard.into(),
            why: why.into(),
        }
    }

    /// Which guardrail refused.
    #[must_use]
    pub fn guard(&self) -> &str {
        &self.guard
    }

    /// What it said about the refusal.
    #[must_use]
    pub fn why(&self) -> &str {
        &self.why
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
    /// Do not.
    Rejected(Rejection),
}

/// Why a check could not reach a decision.
///
/// Distinct from a refusal in the type, because they are distinct outcomes for
/// the run: a refusal is an answer, and this is the absence of one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GuardrailError {
    /// The guardrail ran and could not say.
    #[error("the guardrail `{guard}` could not decide: {problem}")]
    Undecided {
        /// Which guardrail.
        guard: Box<str>,
        /// What it said about why not.
        problem: Box<str>,
    },
}

impl GuardrailError {
    /// A guardrail that ran and could not say, for the reason given.
    #[must_use]
    pub fn undecided(guard: &str, problem: &str) -> Self {
        Self::Undecided {
            guard: guard.into(),
            problem: problem.into(),
        }
    }
}

/// A check on what the caller asked, run before any of it reaches a provider.
///
/// Implemented outside this crate as well as in it: the whole surface a custom
/// check needs is its own name and this one method, and neither requires
/// anything a caller cannot already build.
///
/// ```
/// use crucible_agents::{AgentContext, Decision, GuardrailError, InputGuardrail, Rejection};
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
///     fn checking(&self, context: &AgentContext<'_>) -> Result<Decision, GuardrailError> {
///         if context.said().contains("api-key:") {
///             return Ok(Decision::Rejected(Rejection::new(
///                 self.name(),
///                 "the prompt carries a credential",
///             )));
///         }
///         Ok(Decision::Allowed)
///     }
/// }
/// ```
pub trait InputGuardrail: std::fmt::Debug + Send + Sync {
    /// What this guardrail is called, in a refusal a reader sees.
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
    /// [`GuardrailError`] where the check ran and could not decide. That is not
    /// a refusal: a run ends differently for each.
    fn checking(&self, context: &AgentContext<'_>) -> Result<Decision, GuardrailError>;
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
    /// What this guardrail is called, in a refusal a reader sees.
    fn name(&self) -> &str;

    /// Judges the answer the model finished on.
    ///
    /// # Errors
    ///
    /// [`GuardrailError`] where the check ran and could not decide.
    fn checking(
        &self,
        context: &AgentContext<'_>,
        candidate: &str,
    ) -> Result<Decision, GuardrailError>;
}
