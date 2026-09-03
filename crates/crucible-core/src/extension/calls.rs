//! The calls in flight with one extension.
//!
//! Two tables, because the two directions go wrong differently. Crucible asks
//! and has to match an answer back to what it was waiting on; an extension asks
//! and crucible has to bound how much of that it will carry at once. Neither
//! table knows what a call is about — what is remembered against one is the
//! host's, and this only keeps it straight.
//!
//! Nothing here reads a clock. A call that is never answered is a call still
//! waiting as far as this is concerned, and how long to wait belongs where
//! there is something to wait with.

use std::collections::{BTreeMap, BTreeSet};

use super::spoken::CallId;

/// The most calls that may be in flight in one direction at once.
///
/// Both directions, for different reasons. Crucible reaching this is crucible
/// leaking calls it never collects, and it would rather say so than grow a
/// table for the life of a run. An extension reaching it is an extension
/// asking faster than crucible answers, which is bounded here so that a program
/// somebody else wrote cannot decide how much work this process holds.
pub const EXTENSION_CALLS: usize = 64;

/// Why a call could not be started or settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CallError {
    /// The direction is already carrying as many as it may.
    #[error("more than {maximum} calls would be in flight at once")]
    TooMany {
        /// The ceiling.
        maximum: usize,
    },

    /// Nothing was waiting under that identifier.
    #[error("call {id} is not one that is in flight")]
    Unknown {
        /// The identifier that arrived.
        id: CallId,
    },

    /// A second call arrived under an identifier already in flight.
    #[error("call {id} is already in flight")]
    Repeated {
        /// The identifier both claim.
        id: CallId,
    },

    /// There are no identifiers left to hand out.
    #[error("there are no call identifiers left")]
    Exhausted,
}

/// The calls crucible has made and is waiting on.
///
/// `T` is whatever the host needs back when the answer comes — the method that
/// was asked for, somewhere to put the result, whatever it is. Held here rather
/// than named here, so this stays about keeping calls straight.
///
/// A call crucible has given up on stays in the table with nothing against it.
/// It has to: the extension was never told, so it may still answer, and an
/// answer this could not place would read as the far end inventing a call.
/// Keeping the identifier is also what bounds giving up — a call crucible
/// abandoned holds its place among [`EXTENSION_CALLS`] until the extension
/// says something about it, so a host that gives up in a loop runs out of room
/// rather than out of memory.
#[derive(Debug)]
pub struct Asked<T> {
    /// What is being waited on, by call, or nothing where it was given up on.
    waiting: BTreeMap<CallId, Option<T>>,
    /// The next identifier to hand out, or nothing once they run out.
    next: Option<u64>,
}

impl<T> Default for Asked<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Asked<T> {
    /// A table waiting on nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            waiting: BTreeMap::new(),
            next: Some(0),
        }
    }

    /// A table whose next identifier is `next`.
    #[cfg(test)]
    pub(crate) const fn counting_from(next: u64) -> Self {
        Self {
            waiting: BTreeMap::new(),
            next: Some(next),
        }
    }

    /// Starts a call, remembering `about` until it is answered.
    ///
    /// # Errors
    ///
    /// [`CallError`] where this direction is already carrying
    /// [`EXTENSION_CALLS`], or where there are no identifiers left.
    pub fn ask(&mut self, about: T) -> Result<CallId, CallError> {
        if self.waiting.len() >= EXTENSION_CALLS {
            return Err(CallError::TooMany {
                maximum: EXTENSION_CALLS,
            });
        }
        let number = self.next.ok_or(CallError::Exhausted)?;
        let id = CallId::new(number);
        self.next = number.checked_add(1);
        self.waiting.insert(id, Some(about));
        Ok(id)
    }

    /// Takes back what was remembered against one call.
    ///
    /// `Ok(None)` is an answer to a call crucible gave up on, which is a
    /// perfectly ordinary thing for the far end to send and nothing for the
    /// host to do anything about; it is told apart here rather than upstairs
    /// so that only this table has to know a given-up call still exists.
    ///
    /// # Errors
    ///
    /// [`CallError::Unknown`] where nothing was waiting under it, which covers
    /// both an identifier crucible never handed out and one it already
    /// collected an answer for.
    pub fn answered(&mut self, id: CallId) -> Result<Option<T>, CallError> {
        self.waiting.remove(&id).ok_or(CallError::Unknown { id })
    }

    /// Stops waiting on one call, taking back what was remembered against it.
    ///
    /// The call is not over — the extension was not told and may still be
    /// working — but crucible owes its own caller an answer now rather than
    /// whenever the far end gets there.
    ///
    /// # Errors
    ///
    /// [`CallError::Unknown`] where nothing was waiting under it, which covers
    /// giving up twice on the same call.
    pub fn given_up(&mut self, id: CallId) -> Result<T, CallError> {
        self.waiting
            .get_mut(&id)
            .and_then(Option::take)
            .ok_or(CallError::Unknown { id })
    }

    /// How many calls are in flight, including ones crucible gave up on.
    ///
    /// Given-up calls are counted because they still hold their place: the
    /// extension has not answered them, and crucible is still obliged to
    /// recognise the identifier when it does.
    #[must_use]
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// Everything still waiting, in call order, leaving nothing behind.
    ///
    /// For the far end going away. A call nobody will ever answer has to become
    /// a failure the host reports, because the alternative is whatever was
    /// waiting on it waiting for the length of the run. A call crucible gave up
    /// on is not among them: the host was handed what it was waiting on at the
    /// moment it gave up, and one call owes one final answer, not two.
    pub fn abandoned(&mut self) -> Vec<(CallId, T)> {
        std::mem::take(&mut self.waiting)
            .into_iter()
            .filter_map(|(id, about)| about.map(|about| (id, about)))
            .collect()
    }
}

/// The calls an extension has made that crucible has not answered yet.
///
/// Only the identifiers. What crucible is doing about one is the host's to
/// hold; what this settles is whether it should be doing it at all.
#[derive(Debug, Default)]
pub struct Serving {
    /// What is being worked on.
    open: BTreeSet<CallId>,
}

impl Serving {
    /// Nothing taken on.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            open: BTreeSet::new(),
        }
    }

    /// Takes on one call.
    ///
    /// # Errors
    ///
    /// [`CallError`] where [`EXTENSION_CALLS`] are already open, or where that
    /// identifier is one of them — two live calls under one identifier are two
    /// calls one answer would settle, and the far end chose the number.
    pub fn take(&mut self, id: CallId) -> Result<(), CallError> {
        if self.open.contains(&id) {
            return Err(CallError::Repeated { id });
        }
        if self.open.len() >= EXTENSION_CALLS {
            return Err(CallError::TooMany {
                maximum: EXTENSION_CALLS,
            });
        }
        self.open.insert(id);
        Ok(())
    }

    /// Marks one answered.
    ///
    /// # Errors
    ///
    /// [`CallError::Unknown`] where it is not one crucible took on.
    pub fn answered(&mut self, id: CallId) -> Result<(), CallError> {
        if self.open.remove(&id) {
            Ok(())
        } else {
            Err(CallError::Unknown { id })
        }
    }

    /// How many are open.
    #[must_use]
    pub fn open(&self) -> usize {
        self.open.len()
    }
}

#[cfg(test)]
mod tests;
