//! How a run ended, and what it cost.
//!
//! One word — the reason the model stopped — is not enough to answer for a
//! turn. Which run it was, whether it ended because it finished or because
//! somebody stopped it, and what it spent getting there are all facts the run
//! itself holds, so they are handed back together rather than left on the
//! stack frame. A caller reading the spend off an event would be reading the
//! screen's copy rather than the run's.
//!
//! What is *not* here: failures. A provider that would not answer, a log that
//! would not write, a tool the user refused — those stay [`TurnError`], because
//! a caller has to be made to tell them from an ending, and a status field is
//! a thing you can forget to read.
//!
//! [`TurnError`]: crucible_core::TurnError

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
    /// [`TurnError`]: crucible_core::TurnError
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
    #[must_use]
    pub const fn new(run: RunId, stop: StopReason, spent: Spend) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

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
