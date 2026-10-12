//! The facts about one backend instance that decide whether it may run.
//!
//! Three facts, kept apart because each changes for its own reasons: whether
//! the instance is admitted at all ([`Admission`]), what was last observed of
//! its account ([`Authentication`]), and what management operation, if any,
//! is under way ([`Running`]). An instance is [`Lifecycle::ready`] to open a
//! session only when it is enabled, nothing is under way and it was last seen
//! signed in, so a signed-out instance is never ready, whatever else holds.
//!
//! Changes arrive as [`Step`]s through [`Lifecycle::apply`], which refuses a
//! step the current state does not admit and leaves the state as it was. A
//! disabled instance cannot be disconnected again. An operation whose end
//! nobody could confirm leaves the instance needing recovery and its account
//! unknown, because a sign-out that may or may not have happened has left
//! nothing anyone can say about it; only a check or a verification may start
//! then, and only its success recovers the instance.

/// Whether an instance may be used at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Admission {
    /// It may be.
    Enabled,
    /// It may not be; nothing opens a session on it.
    Disabled,
}

/// What was last observed of an instance's account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Authentication {
    /// Nothing has been observed, or what was is no longer known to hold.
    Unknown,
    /// Signed out.
    SignedOut,
    /// Signed in.
    SignedIn,
    /// Signed in once, and that has lapsed.
    Expired,
    /// The program cannot say.
    Unverifiable,
}

/// A management operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operation {
    /// Asking the program what state it is in.
    Checking,
    /// Waiting for the user to finish signing in.
    AwaitingLogin,
    /// Confirming a sign-in took.
    Verifying,
    /// Taking the instance out of use.
    Disconnecting,
    /// Signing the account out.
    SigningOut,
}

impl Operation {
    /// Every operation.
    pub const ALL: [Self; 5] = [
        Self::Checking,
        Self::AwaitingLogin,
        Self::Verifying,
        Self::Disconnecting,
        Self::SigningOut,
    ];
}

/// What is under way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Running {
    /// Nothing.
    Idle,
    /// This operation.
    Operation(Operation),
    /// An operation ended and nobody could confirm how. Only an operation
    /// that reads the state again, [`Operation::Checking`] or
    /// [`Operation::Verifying`], may start, and only its success ends this.
    RecoveryRequired,
}

/// How an operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ending {
    /// It did what it set out to.
    Succeeded,
    /// It was stopped before it changed anything.
    Cancelled,
    /// It failed.
    Failed,
    /// Nobody can say whether it changed anything.
    Unknown,
}

impl Ending {
    /// Every ending.
    pub const ALL: [Self; 4] = [
        Self::Succeeded,
        Self::Cancelled,
        Self::Failed,
        Self::Unknown,
    ];
}

/// One change to an instance's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    /// Admit it.
    Enable,
    /// Stop admitting it.
    Disable,
    /// Start an operation.
    Begin(Operation),
    /// The operation under way ended.
    End(Ending),
    /// Its account was observed in this state.
    Observe(Authentication),
}

/// Why a step was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum Refused {
    /// Another operation is under way.
    #[error("another operation is under way")]
    Busy,
    /// Nothing is under way to end.
    #[error("nothing is under way")]
    Idle,
    /// The instance is disabled, so it is already out of use.
    #[error("that instance is disabled")]
    Disabled,
    /// The instance needs recovery, and only an operation that reads its
    /// state again may start.
    #[error("that instance needs its state checked first")]
    Recovering,
}

/// One backend instance's admission, account and operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Lifecycle {
    /// Whether it may be used.
    admission: Admission,
    /// What was last observed of its account.
    authentication: Authentication,
    /// What is under way.
    running: Running,
    /// Whether the operation under way began from
    /// [`Running::RecoveryRequired`], which any ending but success returns to.
    recovering: bool,
}

impl Lifecycle {
    /// A new instance: admitted as `admission`, nothing known, nothing under
    /// way.
    #[must_use]
    pub const fn new(admission: Admission) -> Self {
        Self {
            admission,
            authentication: Authentication::Unknown,
            running: Running::Idle,
            recovering: false,
        }
    }

    /// Whether it may be used.
    #[must_use]
    pub const fn admission(&self) -> Admission {
        self.admission
    }

    /// What was last observed of its account.
    #[must_use]
    pub const fn authentication(&self) -> Authentication {
        self.authentication
    }

    /// What is under way.
    #[must_use]
    pub const fn running(&self) -> Running {
        self.running
    }

    /// Whether a session may be opened on it now.
    #[must_use]
    pub const fn ready(&self) -> bool {
        matches!(
            (self.admission, self.running, self.authentication),
            (Admission::Enabled, Running::Idle, Authentication::SignedIn)
        )
    }

    /// Takes `step`.
    ///
    /// # Errors
    ///
    /// Why the current state does not admit it; the state is unchanged.
    pub fn apply(&mut self, step: Step) -> Result<(), Refused> {
        let next = self.after(step)?;
        *self = next;
        Ok(())
    }

    /// The state `step` leads to.
    const fn after(self, step: Step) -> Result<Self, Refused> {
        let mut next = self;
        match step {
            Step::Enable | Step::Disable => {
                if let Running::Operation(_) = self.running {
                    return Err(Refused::Busy);
                }
                next.admission = match step {
                    Step::Enable => Admission::Enabled,
                    _ => Admission::Disabled,
                };
            }
            Step::Begin(operation) => {
                if let Running::Operation(_) = self.running {
                    return Err(Refused::Busy);
                }
                if let (Admission::Disabled, Operation::Disconnecting) = (self.admission, operation)
                {
                    return Err(Refused::Disabled);
                }
                if let Running::RecoveryRequired = self.running {
                    match operation {
                        Operation::Checking | Operation::Verifying => next.recovering = true,
                        Operation::AwaitingLogin
                        | Operation::Disconnecting
                        | Operation::SigningOut => return Err(Refused::Recovering),
                    }
                }
                next.running = Running::Operation(operation);
            }
            Step::End(ending) => {
                let Running::Operation(operation) = self.running else {
                    return Err(Refused::Idle);
                };
                next.running = Running::Idle;
                match (operation, ending) {
                    (_, Ending::Unknown) => {
                        next.running = Running::RecoveryRequired;
                        next.authentication = Authentication::Unknown;
                    }
                    (Operation::Disconnecting, Ending::Succeeded) => {
                        next.admission = Admission::Disabled;
                    }
                    (Operation::SigningOut, Ending::Succeeded) => {
                        next.authentication = Authentication::SignedOut;
                    }
                    (Operation::SigningOut, Ending::Failed) => {
                        next.authentication = Authentication::Unknown;
                    }
                    (
                        Operation::Checking
                        | Operation::AwaitingLogin
                        | Operation::Verifying
                        | Operation::Disconnecting
                        | Operation::SigningOut,
                        Ending::Succeeded | Ending::Cancelled | Ending::Failed,
                    ) => {}
                }
                // A recovery that did not succeed established nothing.
                if self.recovering && !matches!(ending, Ending::Succeeded) {
                    next.running = Running::RecoveryRequired;
                }
                next.recovering = false;
            }
            Step::Observe(authentication) => next.authentication = authentication,
        }
        Ok(next)
    }
}
