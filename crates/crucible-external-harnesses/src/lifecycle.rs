//! The account operations on one instance, serialized on the credential scope
//! it shares and journaled so that a crash leaves it needing a check.
//!
//! A [`Coordinator`] keeps one instance's [`Lifecycle`] and the [`Journal`] of
//! the scope its credentials live in. It is restored from that journal before
//! anything is admitted: an operation the journal still has open, or a journal
//! that cannot be read, leaves the instance needing recovery, so it is not
//! ready and only a check may start on it, whatever its account was last seen
//! as.
//!
//! An operation is a [`Slot`]. Beginning one takes the scope's lock, refusing
//! while another holder has it, and keeps its record before it returns, so
//! nothing an operation does can come before the record that it began. Each
//! [`Stage`] the work reaches is kept before the step it names is taken, and
//! the lifecycle's operation follows the stage: a connect checks, then waits
//! on a sign-in, then verifies. A slot finished says how its operation ended;
//! a slot dropped without finishing leaves its record open and the instance
//! needing recovery, which is what a crash leaves too.

use std::fmt;

use crucible_execution::{
    Admission, Authentication, Ending, Lifecycle, Operation, Refused, Running, Step,
};

use crate::journal::{self, Hold, Journal, JournalError, Record, Stage, Work};

/// Why an operation did not begin or move on.
#[derive(Debug, thiserror::Error)]
pub enum Contended {
    /// Another holder has the scope.
    #[error("another account operation on this agent's sign-in is under way")]
    Busy,
    /// An operation the journal has open was never finished; only a connect,
    /// which checks first, may begin over it.
    #[error("an earlier account operation on this agent's sign-in did not finish; check it first")]
    Unresolved(Record),
    /// The instance's state does not admit it.
    #[error(transparent)]
    Refused(#[from] Refused),
    /// That stage is not one this work reaches from where it is.
    #[error("that step is out of order for this operation")]
    Order,
    /// The journal could not be read or kept, and nothing is done unrecorded.
    #[error(transparent)]
    Journal(#[from] JournalError),
}

/// One instance's account state, coordinated on its credential scope.
pub struct Coordinator {
    /// The journal of the scope its credentials live in.
    journal: Journal,
    /// Its name, as the journal keeps it.
    instance: Box<str>,
    /// Its admission, account and operation.
    lifecycle: Lifecycle,
}

impl fmt::Debug for Coordinator {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Coordinator")
            .field("instance", &self.instance)
            .field("lifecycle", &self.lifecycle)
            .finish_non_exhaustive()
    }
}

impl Coordinator {
    /// The instance called `instance`, admitted as `admission`, as `journal`
    /// leaves it: needing recovery where an operation is still open in it or
    /// it cannot be read, and otherwise with nothing known of its account.
    ///
    /// # Errors
    ///
    /// [`JournalError::Reference`] when `instance` is empty or too long.
    pub fn restore(
        journal: Journal,
        instance: &str,
        admission: Admission,
    ) -> Result<Self, JournalError> {
        if instance.is_empty() || instance.len() > journal::REFERENCE_BYTES {
            return Err(JournalError::Reference);
        }
        let mut lifecycle = Lifecycle::new(admission);
        let unresolved = !matches!(journal.read(), Ok(entries) if entries.open.is_none());
        if unresolved {
            // A check may begin on any instance, and one nobody saw end is
            // what needing recovery is.
            for step in [Step::Begin(Operation::Checking), Step::End(Ending::Unknown)] {
                let _ = lifecycle.apply(step);
            }
        }
        Ok(Self {
            journal,
            instance: instance.into(),
            lifecycle,
        })
    }

    /// Its admission, account and operation.
    #[must_use]
    pub const fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }

    /// Its account was seen as `authentication`.
    pub fn observe(&mut self, authentication: Authentication) {
        // Observing is admitted in every state.
        let _ = self.lifecycle.apply(Step::Observe(authentication));
    }

    /// Admits it again, or stops admitting it.
    ///
    /// # Errors
    ///
    /// [`Refused::Busy`] while an operation is under way.
    pub fn admit(&mut self, admission: Admission) -> Result<(), Refused> {
        self.lifecycle.apply(match admission {
            Admission::Enabled => Step::Enable,
            Admission::Disabled => Step::Disable,
        })
    }

    /// Begins `work` on `profile` under `generation`.
    ///
    /// # Errors
    ///
    /// [`Contended`] for each reason it did not begin; nothing was kept or
    /// changed then.
    pub fn begin(
        &mut self,
        work: Work,
        profile: Option<&str>,
        generation: u64,
    ) -> Result<Slot<'_>, Contended> {
        let operation = operation(work, Stage::Begun, self.lifecycle.admission());
        let mut began = self.lifecycle;
        began.apply(Step::Begin(operation))?;
        let hold = self.journal.hold()?.ok_or(Contended::Busy)?;
        let mut entries = match self.journal.read() {
            Ok(entries) => entries,
            // A journal that cannot be read is replaced only by a connect,
            // which checks before it changes anything.
            Err(JournalError::Unreadable(_)) if work == Work::Connect => {
                journal::Entries::default()
            }
            Err(problem) => return Err(problem.into()),
        };
        if let Some(open) = entries.open.take() {
            if work != Work::Connect {
                return Err(Contended::Unresolved(open));
            }
            journal::finished(&mut entries, open, Ending::Unknown);
        }
        let record = Journal::record(&entries, work, &self.instance, profile, generation)?;
        entries.open = Some(record.clone());
        self.journal.write(&hold, &entries)?;
        self.lifecycle = began;
        Ok(Slot {
            coordinator: self,
            hold,
            entries,
            record,
            operation,
            finished: false,
        })
    }
}

/// The lifecycle operation `work` is at `stage`, on an instance admitted as
/// `admission`.
const fn operation(work: Work, stage: Stage, admission: Admission) -> Operation {
    match (work, stage) {
        (Work::Connect, Stage::Mutating) => Operation::AwaitingLogin,
        (Work::Connect, Stage::Verifying | Stage::Committing)
        | (Work::SignOut, Stage::Verifying) => Operation::Verifying,
        (Work::Connect, Stage::Begun | Stage::Closed | Stage::Quiesced) => Operation::Checking,
        (Work::SignOut, _) => Operation::SigningOut,
        (Work::Disconnect, _) => Operation::Disconnecting,
        // Removing what is already out of use takes nothing more out of use.
        (Work::Remove, _) => match admission {
            Admission::Enabled => Operation::Disconnecting,
            Admission::Disabled => Operation::Checking,
        },
    }
}

/// One operation under way, holding its scope.
pub struct Slot<'a> {
    /// The instance it acts for.
    coordinator: &'a mut Coordinator,
    /// The scope's lock.
    hold: Hold,
    /// The journal as this operation last kept it.
    entries: journal::Entries,
    /// Its record.
    record: Record,
    /// The lifecycle operation it is in.
    operation: Operation,
    /// Whether it was finished.
    finished: bool,
}

impl fmt::Debug for Slot<'_> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Slot")
            .field("record", &self.record)
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}

impl Slot<'_> {
    /// Its record as kept.
    #[must_use]
    pub const fn record(&self) -> &Record {
        &self.record
    }

    /// Keeps that it has reached `stage`, before the step it names is taken.
    ///
    /// # Errors
    ///
    /// [`Contended::Order`] for a stage this work does not reach next, and
    /// [`Contended::Journal`] when it could not be kept, in which case the
    /// step must not be taken.
    pub fn advance(&mut self, stage: Stage) -> Result<(), Contended> {
        let stages = self.record.work().stages();
        let at = stages.iter().position(|kept| *kept == self.record.stage());
        let to = stages.iter().position(|kept| *kept == stage);
        match (at, to) {
            (Some(at), Some(to)) if to > at => {}
            _ => return Err(Contended::Order),
        }
        let next = operation(
            self.record.work(),
            stage,
            self.coordinator.lifecycle.admission(),
        );
        let mut lifecycle = self.coordinator.lifecycle;
        if next != self.operation {
            lifecycle.apply(Step::End(Ending::Succeeded))?;
            lifecycle.apply(Step::Begin(next))?;
        }
        let mut record = self.record.clone();
        journal::staged(&mut record, stage);
        self.entries.open = Some(record.clone());
        self.coordinator.journal.write(&self.hold, &self.entries)?;
        self.record = record;
        self.operation = next;
        self.coordinator.lifecycle = lifecycle;
        Ok(())
    }

    /// Finishes it as `ending`, and releases the scope.
    ///
    /// # Errors
    ///
    /// [`Contended::Journal`] when the ending could not be kept. The instance
    /// is left as `ending` says in this process, and its record open for the
    /// next one, which will need a check.
    pub fn finish(mut self, ending: Ending) -> Result<Record, Contended> {
        self.finished = true;
        // The lifecycle takes every ending from any operation under way.
        let _ = self.coordinator.lifecycle.apply(Step::End(ending));
        let record = self.record.clone();
        journal::finished(&mut self.entries, record, ending);
        self.coordinator.journal.write(&self.hold, &self.entries)?;
        Ok(self
            .entries
            .history
            .back()
            .cloned()
            .unwrap_or_else(|| self.record.clone()))
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        if !self.finished {
            // Nobody saw it end. Its record stays open in the journal.
            let _ = self.coordinator.lifecycle.apply(Step::End(Ending::Unknown));
        }
    }
}

/// Whether `lifecycle` needs recovery before anything but a check begins.
#[must_use]
pub const fn needs_recovery(lifecycle: &Lifecycle) -> bool {
    matches!(
        lifecycle.running(),
        Running::RecoveryRequired | Running::Recovering(_)
    )
}
