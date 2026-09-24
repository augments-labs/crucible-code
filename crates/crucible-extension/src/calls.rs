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
//!
//! A call is only a call within one generation of an extension. The number on
//! the wire is chosen by whichever end starts the call, and a replacement
//! process starts counting wherever it likes, so the host holds a [`Call`] —
//! the number and the generation it was made in — rather than the number
//! alone.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::spoken::CallId;

/// The most calls that may be in flight in one direction at once.
///
/// Both directions, for different reasons. Crucible reaching this is crucible
/// leaking calls it never collects, and it would rather say so than grow a
/// table for the life of a run. An extension reaching it is an extension
/// asking faster than crucible answers, which is bounded here so that a program
/// somebody else wrote cannot decide how much work this process holds.
pub const EXTENSION_CALLS: usize = 64;

/// Which extension process, of every one crucible has spoken to, a call
/// belongs to.
///
/// Every process a host speaks to is a generation of its own, the first one
/// and each replacement alike. A replacement is a program crucible has not
/// spoken to before — restarted, or rebuilt by whoever can write to where it
/// lives — and it may have a call open under a number an earlier one also used.
/// So a call carries its generation, and only the generation being spoken to
/// settles it: an answer composed for the old process's call must not reach
/// the new one's, carrying whatever the old one was being answered with.
///
/// Drawn from one count that every host in this process shares, so no two
/// processes crucible has hosted share a generation, whichever host started
/// them and however: a call from a host that was stopped and started again is
/// refused by the new one as a replaced one's is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Generation(u64);

/// The next generation any host in this process is given.
static GENERATIONS: AtomicU64 = AtomicU64::new(0);

impl Generation {
    /// A generation no process crucible has hosted has had, or nothing once
    /// they run out.
    pub(crate) fn fresh() -> Option<Self> {
        Self::drawn_from(&GENERATIONS)
    }

    /// The next generation `count` holds, or nothing once it has none left.
    ///
    /// Never wraps: a number handed out a second time would refuse nothing.
    pub(crate) fn drawn_from(count: &AtomicU64) -> Option<Self> {
        count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .ok()
            .map(Self)
    }

    /// The generation numbered `number`, for a test that builds a conversation
    /// by hand.
    #[cfg(test)]
    pub(crate) const fn numbered(number: u64) -> Self {
        Self(number)
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, form: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(form, "{}", self.0)
    }
}

/// One call, as the host holds it: its number on the wire, and the generation
/// it was made in.
///
/// Only a conversation hands these out, stamped with its own generation, so a
/// number cannot be moved from one generation into another by rewrapping it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Call {
    /// Which process the call belongs to.
    generation: Generation,
    /// Its number there.
    id: CallId,
}

impl Call {
    /// Call `id` of `generation`.
    pub(crate) const fn new(generation: Generation, id: CallId) -> Self {
        Self { generation, id }
    }

    /// Its number on the wire.
    #[cfg(test)]
    pub(crate) const fn id(self) -> CallId {
        self.id
    }

    /// Its number on the wire, where it was made in `generation`.
    ///
    /// # Errors
    ///
    /// [`CallError::Elsewhere`] where it was made in another.
    pub(crate) fn of(self, generation: Generation) -> Result<CallId, CallError> {
        if self.generation == generation {
            Ok(self.id)
        } else {
            Err(CallError::Elsewhere { call: self })
        }
    }
}

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

    /// The call belongs to an extension process other than the one being
    /// spoken to.
    ///
    /// Refused rather than passed on by its number: the process it was made
    /// with has been replaced or its host stopped, and the one there now may
    /// have a call open under the same number that this was never meant for.
    #[error(
        "call {} belongs to extension generation {}, not the one being spoken to",
        .call.id,
        .call.generation
    )]
    Elsewhere {
        /// The call, as the host held it.
        call: Call,
    },
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
