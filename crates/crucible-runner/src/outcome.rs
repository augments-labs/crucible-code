//! How a run ended, and what it cost.
//!
//! One word — the reason the model stopped — is not enough to answer for a
//! turn. Which run it was, whether it ended because it finished or because
//! somebody stopped it, and what it spent getting there are all facts the run
//! itself holds, and holding them on one value is what stops a caller reading
//! the spend off an event, where it is the screen's copy rather than the run's.
//!
//! Nobody outside this crate is handed one yet, and the type is published
//! ahead of that. [`Runner::turn`] answers with a [`StopReason`], which is all
//! the wiring above it asks for; a run's own record of itself is kept on this
//! shape meanwhile, so that the caller which does want the rest reads it off
//! one value rather than reassembling it from events.
//!
//! What is *not* here: failures. A provider that would not answer, a tool the
//! user refused, a turn that spent past its ceiling — those stay [`TurnError`],
//! because a caller has to be made to tell them from an ending, and a status
//! field is a thing you can forget to read. A store that would not keep what it
//! was told is not one of them: it does not stop a turn, and whoever opened the
//! store reports it when they close it.
//!
//! [`TurnError`]: crate::TurnError
//! [`Runner::turn`]: crate::Runner::turn

use crucible_agents::{GuardrailError, Rejection};
use crucible_core::{RunId, Spend, StopReason};

/// How a run ended, in the words the harness uses rather than the model's.
///
/// Three, because there are three things that can *decide* a run is over: the
/// person, a ceiling, and the exchange itself running out of things to do.
/// Which ceiling, and what the model called it, is [`RunResult::stop`] — this
/// is the answer to "who ended this", which is the question a caller asks
/// first and the one a `StopReason` makes them work out for themselves.
///
/// Note what this is not: it is not whether the answer is any good. A run can
/// end under [`RunStatus::Completed`] having produced a truncated answer or
/// none at all — see the variant. A caller that needs to know whether there is
/// a usable answer reads [`RunResult::stop`], which is kept beside the status
/// for exactly that reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// Nobody and nothing ended it: the exchange itself stopped.
    ///
    /// The wide bucket, and deliberately so. The model yielded, or asked for
    /// tools and the pass ended the discussion — but also: the provider
    /// filtered the answer, paused mid-sentence, or stopped without saying
    /// why. Those three are *not* finished answers, and
    /// [`StopReason::Filtered`], [`StopReason::Paused`] and
    /// [`StopReason::Unknown`] each say so at length in their own
    /// documentation.
    ///
    /// They are here because this enum answers a narrower question than it
    /// might look like it does: which of the three deciders ended the run. No
    /// person cancelled those and no ceiling this run was under stopped them,
    /// so `Cancelled` and `LimitReached` would both be lies. A fourth variant
    /// for "ended without finishing" is the honest answer and is not this
    /// phase's to add — the harness result was fixed at three values, and the
    /// first caller that has to branch on a half-finished run is the one that
    /// should pay for widening it.
    ///
    /// So: read the `stop` beside this before telling anybody the answer is
    /// complete.
    Completed,
    /// Somebody stopped it.
    Cancelled,
    /// A ceiling did: the response ran out of room, or the request would not
    /// fit the window and this session does not make room by itself.
    ///
    /// Only the ceilings a run can *end* on. The ones this program sets over a
    /// whole turn — what it may spend, how much tool output it may carry — are
    /// [`TurnError`] and stay there: they are the turn failing to fit inside
    /// what it was given rather than an answer that arrived.
    ///
    /// [`TurnError`]: crate::TurnError
    LimitReached,
}

impl RunStatus {
    /// Which of the three deciders ended a run that stopped for this reason.
    ///
    /// Everything that is neither a cancellation nor a ceiling this run ended
    /// on lands in [`RunStatus::Completed`], including the three endings that
    /// are not finished answers. [`RunStatus::Completed`] says why.
    #[must_use]
    pub const fn of(stop: StopReason) -> Self {
        match stop {
            StopReason::Cancelled => Self::Cancelled,
            StopReason::OutOfTokens | StopReason::WindowExceeded => Self::LimitReached,
            StopReason::Yielded
            | StopReason::WantsTools
            | StopReason::Filtered
            | StopReason::Paused
            | StopReason::Unknown => Self::Completed,
        }
    }
}

/// What one run ended as.
///
/// Published ahead of the callers that will read one, and stated only here:
/// there is no way in from outside, because a run that nobody ran has no
/// ending to report.
///
/// The error code is what this fails with today, not something the harness
/// checks — `compile_fail` accepts any compile error — so the snippet names
/// nothing it does not need, and the private call is the only thing in it that
/// can fail.
///
/// ```compile_fail,E0624
/// use crucible_runner::RunResult;
///
/// let invented = RunResult::new(unimplemented!(), unimplemented!(), unimplemented!());
/// ```
#[derive(Debug, Clone, Copy)]
pub struct RunResult {
    /// Which run this was — the same identity its events carried.
    run: RunId,
    /// What decided it was over.
    status: RunStatus,
    /// And what the model called it. Kept beside the status rather than
    /// replaced by it: the status is what a caller branches on, and this is
    /// what a reader is told.
    stop: StopReason,
    /// What the run produced, in tokens, across every request it made —
    /// including the ones it spent making room.
    spent: Spend,
}

impl RunResult {
    /// A run that ended for this reason, having spent this much.
    ///
    /// The status is worked out here rather than passed in, so it cannot come
    /// to disagree with the reason beside it. The fields are private and this
    /// is the only constructor, which is what makes that a property of the
    /// type: a literal or a later assignment would both be ways to build a run
    /// that says it completed and that a person cancelled it.
    ///
    /// A run is something this crate ends, so this crate is what states one —
    /// which is the visibility rather than only the sentence. [`RunResult`]
    /// carries what happens to a caller that tries to state one anyway.
    #[must_use]
    pub(crate) const fn new(run: RunId, stop: StopReason, spent: Spend) -> Self {
        Self {
            run,
            status: RunStatus::of(stop),
            stop,
            spent,
        }
    }

    /// Which run this was.
    #[must_use]
    pub const fn run(&self) -> RunId {
        self.run
    }

    /// Which of the three deciders ended it.
    #[must_use]
    pub const fn status(&self) -> RunStatus {
        self.status
    }

    /// What the model called the ending. Read this before calling an answer
    /// complete — [`RunStatus::Completed`] says why.
    #[must_use]
    pub const fn stop(&self) -> StopReason {
        self.stop
    }

    /// What it produced, in tokens, across every request it made.
    #[must_use]
    pub const fn spent(&self) -> Spend {
        self.spent
    }
}

/// How one invocation of an agent ended.
///
/// Three, and they are three because the caller has to be able to tell them
/// apart. A run that ended is an answer. A run a guardrail refused produced no
/// answer this agent will stand behind, and saying so is not the same as saying
/// the model stopped. A guardrail that could not decide produced no verdict at
/// all — the check itself is what went wrong, and treating that as a refusal
/// would let a check that cannot run refuse everything.
///
/// Cancellation is not a fourth: somebody stopping a run is an ending the model
/// and the loop already have a word for, and it arrives as
/// [`StopReason::Cancelled`] inside [`Turned::Ran`].
///
/// A guardrail never widens anything, so there is no variant here for one
/// having allowed something. Allowing is the run carrying on.
#[derive(Debug)]
pub enum Turned {
    /// The exchange ran to an ending, and the answer was accepted.
    Ran(RunResult),

    /// A guardrail refused: the invocation on the way in, or the final
    /// candidate answer on the way out.
    Rejected {
        /// Which check refused, and what it said about why.
        rejection: Rejection,
        /// How the model's own answer ended, where there was one to refuse.
        ///
        /// `None` for an input check, which runs before the first request of
        /// the invocation: nothing was asked, so nothing stopped.
        stop: Option<StopReason>,
    },

    /// A guardrail ran and could not reach a decision.
    Undecided {
        /// What the check said about why not.
        problem: GuardrailError,
        /// How the model's own answer ended, where there was one to judge.
        stop: Option<StopReason>,
    },
}

impl Turned {
    /// How the model's own answer ended, where a request went out at all.
    ///
    /// `None` is an invocation that never reached a provider, which is the one
    /// shape that has no ending to report and the reason this is an option
    /// rather than a reason invented for it.
    #[must_use]
    pub const fn stop(&self) -> Option<StopReason> {
        match self {
            Self::Ran(result) => Some(result.stop()),
            Self::Rejected { stop, .. } | Self::Undecided { stop, .. } => *stop,
        }
    }

    /// What the run ended as, where it ran to an ending.
    #[must_use]
    pub const fn result(&self) -> Option<&RunResult> {
        match self {
            Self::Ran(result) => Some(result),
            Self::Rejected { .. } | Self::Undecided { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_before_the_first_request_has_no_ending_to_report() {
        let refused = Turned::Rejected {
            rejection: Rejection::new("no-secrets", "the prompt carries a private key"),
            stop: None,
        };

        assert_eq!(refused.stop(), None);
        assert!(refused.result().is_none());
    }

    #[test]
    fn a_refused_answer_still_says_how_the_model_stopped() {
        let refused = Turned::Rejected {
            rejection: Rejection::new("house-style", "the answer names a competitor"),
            stop: Some(StopReason::Yielded),
        };

        assert_eq!(
            refused.stop(),
            Some(StopReason::Yielded),
            "a refused answer lost the ending the model gave it"
        );
    }

    #[test]
    fn a_check_that_could_not_decide_is_not_a_refusal() {
        let undecided = Turned::Undecided {
            problem: GuardrailError::undecided("no-secrets", "the scanner was unreachable"),
            stop: None,
        };

        assert!(
            matches!(undecided, Turned::Undecided { .. }),
            "a check that could not decide reads as one that refused"
        );
    }

    #[test]
    fn a_model_that_yielded_completed_the_run() {
        let result = RunResult::new(RunId::new(), StopReason::Yielded, Spend::new(12));

        assert_eq!(result.status(), RunStatus::Completed);
        assert_eq!(result.stop(), StopReason::Yielded);
        assert_eq!(result.spent().tokens(), 12);
    }

    #[test]
    fn a_run_somebody_stopped_says_so_rather_than_reading_as_finished() {
        let result = RunResult::new(RunId::new(), StopReason::Cancelled, Spend::NONE);

        assert_eq!(result.status(), RunStatus::Cancelled);
    }

    #[test]
    fn the_two_endings_a_ceiling_decides_are_told_apart_from_the_ones_it_did_not() {
        assert_eq!(
            RunStatus::of(StopReason::OutOfTokens),
            RunStatus::LimitReached
        );
        assert_eq!(
            RunStatus::of(StopReason::WindowExceeded),
            RunStatus::LimitReached
        );
        assert_eq!(
            RunStatus::of(StopReason::Filtered),
            RunStatus::Completed,
            "a filter is not a ceiling this run was under"
        );
    }
}
