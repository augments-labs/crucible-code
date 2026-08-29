//! How a run ended, and what it cost.
//!
//! A turn used to hand back one word — the reason the model stopped — and
//! everything else it knew about itself died with the stack frame: which run it
//! was, whether it ended because it finished or because somebody stopped it,
//! and what it spent getting there. A caller that wanted the spend read it off
//! an event, which is the screen's copy rather than the run's.
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
/// Three, because there are three things that can decide a run is over: the
/// model, the person, and a ceiling. Which ceiling, and what the model called
/// it, is [`RunResult::stop`] — this is the answer to "did it work", which is
/// the question a caller asks first and the one a `StopReason` makes them
/// work out for themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// The model finished and yielded, or a tool pass ended the discussion.
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
    /// What a run that stopped for this reason did.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunResult {
    /// Which run this was — the same identity its events carried.
    pub run: RunId,
    /// What decided it was over.
    pub status: RunStatus,
    /// And what the model called it. Kept beside the status rather than
    /// replaced by it: the status is what a caller branches on, and this is
    /// what a reader is told.
    pub stop: StopReason,
    /// What the run produced, in tokens, across every request it made —
    /// including the ones it spent making room.
    pub spent: Spend,
}

impl RunResult {
    /// A run that ended for this reason, having spent this much.
    ///
    /// The status is worked out here rather than passed in, so it cannot come
    /// to disagree with the reason beside it.
    #[must_use]
    pub const fn new(run: RunId, stop: StopReason, spent: Spend) -> Self {
        Self {
            run,
            status: RunStatus::of(stop),
            stop,
            spent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_that_yielded_completed_the_run() {
        let result = RunResult::new(RunId::new(), StopReason::Yielded, Spend::new(12));

        assert_eq!(result.status, RunStatus::Completed);
        assert_eq!(result.stop, StopReason::Yielded);
        assert_eq!(result.spent.tokens(), 12);
    }

    #[test]
    fn a_run_somebody_stopped_says_so_rather_than_reading_as_finished() {
        let result = RunResult::new(RunId::new(), StopReason::Cancelled, Spend::NONE);

        assert_eq!(result.status, RunStatus::Cancelled);
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
