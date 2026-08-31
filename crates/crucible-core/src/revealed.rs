//! Which deferred tools the model has asked for.
//!
//! Most sessions never search the web and never write a plan, and a schema the
//! model is shown is a schema it pays for on **every** request of every turn. So
//! a tool may be registered without being advertised: it is held back until the
//! model looks it up, and from then on it is offered like any other.
//!
//! One set, shared between the tool that reveals a name and the roster that
//! decides what to advertise. Cloning shares it rather than copying, the same
//! bargain [`crate::Cancel`] makes and for the same reason — the thing doing the
//! revealing and the thing reading the answer are not the same object and must
//! not be able to disagree.
//!
//! It lives in core because the two crates that hold it must not depend on each
//! other: the tool is in `crucible-tools` and the roster is in
//! `crucible-runner`, and the arrow between them is one core exists to avoid.
//!
//! Bound to the session rather than to a turn. Having looked a tool up once,
//! the model keeps it — a set that emptied between turns would make it look the
//! same tool up again and again, spending the round trip the mechanism exists
//! to save.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The deferred tools that have been asked for.
#[derive(Debug, Clone, Default)]
pub struct Revealed(Arc<State>);

#[derive(Debug, Default)]
struct State {
    held: Mutex<BTreeSet<Box<str>>>,
    revision: AtomicU64,
}

impl Revealed {
    /// Nothing asked for yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Offers `name` from now on.
    ///
    /// Silent about whether it was already there. Looking a tool up twice is
    /// something a model does, and it is not a mistake worth an answer.
    pub fn reveal(&self, name: &str) {
        if let Ok(mut held) = self.0.held.lock()
            && held.insert(name.into())
        {
            self.0.revision.fetch_add(1, Ordering::Release);
        }
    }

    /// Whether `name` has been asked for.
    ///
    /// A poisoned lock answers no, which advertises less rather than more: the
    /// failure that hides a tool is one the model can recover from by looking it
    /// up again, and the one that reveals everything is a bound silently gone.
    #[must_use]
    pub fn holds(&self, name: &str) -> bool {
        self.0.held.lock().is_ok_and(|held| held.contains(name))
    }

    /// Which material reveal state this is.
    ///
    /// Equality is the contract: the number has no public ordering meaning.
    /// It moves only when membership changes, so looking up the same name
    /// twice does not churn a tool snapshot generation.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.0.revision.load(Ordering::Acquire)
    }

    /// Forgets everything asked for.
    ///
    /// What `/clear` does to it. A fresh session has not looked anything up,
    /// and carrying the last one's answers into it would advertise tools this
    /// conversation has never heard of.
    pub fn forget(&self) {
        if let Ok(mut held) = self.0.held.lock()
            && !held.is_empty()
        {
            held.clear();
            self.0.revision.fetch_add(1, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_material_membership_change_moves_the_revision() {
        let revealed = Revealed::new();
        let empty = revealed.revision();

        revealed.reveal("web_fetch");
        let holding = revealed.revision();
        revealed.reveal("web_fetch");
        assert_eq!(revealed.revision(), holding);

        revealed.forget();
        let forgotten = revealed.revision();
        revealed.forget();

        assert_ne!(holding, empty);
        assert_ne!(forgotten, holding);
        assert_eq!(revealed.revision(), forgotten);
    }
}
