//! Whether an extension that ended may be started again.
//!
//! Dying once is not a reason to give up on a program. A process that fell over
//! on its first frame is often perfectly well on its second, and an extension
//! that is unavailable for the rest of a run because of one bad start is a
//! worse outcome than trying again. Dying repeatedly is a different thing: at
//! some point the restarts are the failure rather than the recovery, and
//! something has to be counting.
//!
//! Counting is only half of it. The other half is that an extension can die at
//! a moment when starting it again would be unsafe rather than merely futile.
//! If crucible was waiting on a call when the process went away, nothing on
//! this side can see how far the extension got with it — asked for and not
//! begun, begun and not finished, or finished with the answer lost on the way
//! back are the same silence from here. Asking again would be asking for
//! whatever it was a second time, and this cannot know whether that is harmless.
//! So an ending like that ends the extension for the run, however much of the
//! ceiling is left.
//!
//! Nothing here starts anything, and nothing here reads a manifest, a setting
//! or a process. It holds the count and answers the question, and the answer is
//! a value only it can make. A supervisor that takes one of those rather than a
//! number it checked itself cannot be written in a way that forgets to ask.

/// Whether what an ended extension was doing is known to have finished.
///
/// The usual source is [`Ended::waiting`](crate::Ended::waiting): calls still
/// outstanding when the process went away are exactly the ones whose effect
/// cannot be seen from here. A host that knows its own outstanding calls were
/// harmless to repeat may say so; nothing in this module can know that for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ambiguity {
    /// Nothing was outstanding, so nothing can have half-happened.
    Settled,

    /// Something was, and whether it took effect cannot be known from here.
    Unsettled,
}

/// How many more times one extension may be started again, and how many it has
/// already spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Restarts {
    /// The most restarts this extension is allowed in one run.
    ceiling: u32,
    /// How many of them it has used.
    spent: u32,
}

impl Restarts {
    /// A budget of `ceiling` restarts, none of them spent.
    ///
    /// A ceiling of zero is the honest way to say an extension gets one start
    /// and no more, rather than a policy that has to be spelled somewhere else.
    #[must_use]
    pub const fn ceiling(ceiling: u32) -> Self {
        Self { ceiling, spent: 0 }
    }

    /// How many restarts have been permitted so far.
    #[must_use]
    pub const fn spent(&self) -> u32 {
        self.spent
    }

    /// How many are left.
    #[must_use]
    pub const fn left(&self) -> u32 {
        self.ceiling.saturating_sub(self.spent)
    }

    /// Permits one more start, after an ending of the given certainty.
    ///
    /// A permitted restart is spent whether or not the caller manages to start
    /// anything; a budget that only counted successes would let a program that
    /// cannot start be tried forever.
    ///
    /// # Errors
    ///
    /// [`NoRestart::Unsettled`] where the ending left crucible waiting, which
    /// is refused before the budget is even looked at — a call whose effect is
    /// unknown is not made safe to repeat by having restarts to spare.
    /// [`NoRestart::Spent`] where the ceiling is reached.
    pub fn again(&mut self, after: Ambiguity) -> Result<Restarting, NoRestart> {
        if after == Ambiguity::Unsettled {
            return Err(NoRestart::Unsettled);
        }
        if self.left() == 0 {
            return Err(NoRestart::Spent {
                ceiling: self.ceiling,
            });
        }
        self.spent += 1;
        Ok(Restarting { nth: self.spent })
    }
}

/// Permission to start one extension again, which only [`Restarts::again`]
/// gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Restarting {
    /// Which restart this is, counting from one.
    nth: u32,
}

impl Restarting {
    /// Which restart this is, counting from one.
    ///
    /// The number a diagnostic needs to say whether an extension is recovering
    /// or circling.
    #[must_use]
    pub const fn nth(&self) -> u32 {
        self.nth
    }
}

/// Why an extension will not be started again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NoRestart {
    /// It ended while crucible was still waiting on it.
    #[error(
        "the extension ended while crucible was waiting on it, so what it had \
         already done cannot be known"
    )]
    Unsettled,

    /// It has been started again as often as it is allowed to be.
    #[error("the extension has used all {ceiling} of the restarts it is allowed")]
    Spent {
        /// The ceiling it reached.
        ceiling: u32,
    },
}

#[cfg(test)]
mod tests;
