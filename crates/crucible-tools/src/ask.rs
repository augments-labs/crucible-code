//! Where a tool puts the questions only a person can answer.

use crucible_runtime::BoxFuture;
use crucible_types::{Answered, Question};

/// Where a tool puts its questions to whoever can answer them.
///
/// A trait for the reason `Post` and [`crate::Watch`] are: the thing
/// that answers is a front end, and a second one must need no edit here.
///
/// It is the neighbour of [`crate::Ask`] and answers a different question.
/// That one asks whether a call may run and is owed a verdict, so its silence
/// has to be a refusal — running a tool nobody agreed to is worse than
/// stopping. This one asks a person to decide something, and nothing runs
/// either way, so its silence is nobody answering.
pub trait Put: Send + Sync {
    /// Puts `questions` and awaits the answer.
    ///
    /// A question's wait is human-length, so this hands back a future rather
    /// than blocking inside a ready one: an implementation answers `Pending`
    /// the moment it is asked and wakes its waker once somebody has decided,
    /// so the thread polling it is never held for the wait.
    ///
    /// One [`Answered`] per question, in the order they were asked. `None` is
    /// nobody answered — the ask was left, or there was never anybody there
    /// — and it is not a failure: the tool turns it into a result the turn
    /// survives.
    fn put<'a>(&'a self, questions: &'a [Question]) -> BoxFuture<'a, Option<Vec<Answered>>>;
}

#[cfg(test)]
mod tests;
