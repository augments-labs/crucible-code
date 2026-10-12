//! An installed agent's run, behind the contract every backend implements.
//!
//! [`External`] holds one run that `launching` started and the dialect its
//! wire is read through, the way `native` holds the runner: a prompt is one of
//! the run's turns, an interrupt asks the run's own way to stop, and a
//! decision a client gives goes to the run's table of open questions and from
//! there to the program. Nothing here is a permission: the program asked, and
//! the answer is the program's to act on inside its sandbox.
//!
//! The run posts the contract's own events, which its dialect made of the
//! program's frames, so nothing is translated here. A turn whose stream could
//! not be read to its end has already posted that it is uncertain and failed;
//! it answers the failure, and the run it was on is spent.
//!
//! Confinement is claimed from what the sandbox reported when the run was
//! started and from nothing else.

use std::sync::{Mutex, PoisonError};

use crucible_execution::{
    Backend, Capabilities, Capability, Confined, Decision, Description, Error, Failure, Lasting,
    Outcome, PendingRef, Scope, SessionRef, Sink, TurnRequest, Unsupported,
};
use crucible_external_harnesses::{Control, Dialect, Host, STOP_GRACE, Undecided};
use crucible_runtime::BoxFuture;

/// A run and the wire it is read through, while no turn holds them.
struct Running {
    /// The run.
    host: Host,
    /// Its wire.
    dialect: Box<dyn Dialect>,
}

/// An installed agent's run, answering as a [`Backend`].
pub struct External {
    /// The run, while no turn holds it.
    running: Mutex<Option<Running>>,
    /// What decisions and interrupts reach the run through.
    control: Control,
    /// The session the run records into, as the contract names it.
    session: SessionRef,
    /// The command its program is looked up as.
    driver: &'static str,
    /// The `externalAgents` entry it is.
    instance: Box<str>,
    /// What it claims.
    capabilities: Capabilities,
}

impl std::fmt::Debug for External {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("External")
            .field("driver", &self.driver)
            .field("instance", &self.instance)
            .field("session", &self.session)
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

impl External {
    /// `host`, read through `dialect` and reached through the `control` it
    /// was launched with, recording into `session` as the entry `instance`,
    /// claiming what `capabilities` declares and confined as the run was.
    // Each is a different thing the caller already holds, and a struct that
    // only carried them here would be one more name for the same call.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        host: Host,
        dialect: Box<dyn Dialect>,
        control: Control,
        session: SessionRef,
        instance: &str,
        capabilities: Capabilities,
    ) -> Self {
        let confined = Confined::new(host.claim(), Scope::Instance);
        Self {
            driver: dialect.program(),
            running: Mutex::new(Some(Running { host, dialect })),
            control,
            session,
            instance: instance.into(),
            capabilities: capabilities.confined(confined),
        }
    }

    /// The run, unless a turn holds it or it has been closed.
    fn take(&self) -> Option<Running> {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

/// Refuses `capability` unless the agent claims it.
fn gate(capabilities: &Capabilities, capability: Capability) -> Result<(), Error> {
    if capabilities.supports(capability) {
        Ok(())
    } else {
        Err(Unsupported(capability).into())
    }
}

/// The run, taken out for one turn and put back however the turn ends.
struct Turning<'s> {
    /// Where it goes back to.
    slot: &'s Mutex<Option<Running>>,
    /// The run, for as long as the turn holds it.
    running: Option<Running>,
}

impl Drop for Turning<'_> {
    fn drop(&mut self) {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        *slot = self.running.take();
    }
}

impl Backend for External {
    fn describe(&self) -> Description {
        Description::new(self.driver, &self.instance, self.capabilities)
    }

    fn open(&self, session: Option<SessionRef>) -> BoxFuture<'_, Result<SessionRef, Error>> {
        Box::pin(async move {
            match session {
                None => Ok(self.session.clone()),
                Some(session) => {
                    gate(&self.capabilities, Capability::Resume)?;
                    // A session is resumed by the launch that starts its run,
                    // so a run answers for its own session and no other.
                    if session == self.session {
                        Ok(session)
                    } else {
                        Err(Failure::new("this run was started for another session").into())
                    }
                }
            }
        })
    }

    fn prompt<'a>(
        &'a self,
        turn: TurnRequest,
        events: &'a dyn Sink,
    ) -> BoxFuture<'a, Result<Outcome, Error>> {
        Box::pin(async move {
            let mut turning = Turning {
                slot: &self.running,
                running: self.take(),
            };
            let Some(running) = turning.running.as_mut() else {
                return Err(Failure::new("a turn is already running on this agent").into());
            };
            Ok(running
                .host
                .turn(running.dialect.as_mut(), &self.session, &turn, events)
                .await?)
        })
    }

    fn decide(&self, pending: &PendingRef, decision: Decision) -> Result<(), Error> {
        match decision {
            Decision::Allow(Lasting::Once) => gate(&self.capabilities, Capability::PermissionOnce)?,
            Decision::Allow(Lasting::Session) => {
                gate(&self.capabilities, Capability::PermissionSession)?;
            }
            Decision::Answer(_) => gate(&self.capabilities, Capability::Questions)?,
            Decision::Deny => {}
        }
        self.control
            .decide(pending, decision)
            .map_err(|undecided| match undecided {
                Undecided::Stale(stale) => Error::Stale(stale),
                Undecided::Full => Failure::new(&undecided.to_string()).into(),
            })
    }

    fn interrupt(&self) -> Result<(), Error> {
        gate(&self.capabilities, Capability::Cancel)?;
        self.control.interrupt();
        Ok(())
    }

    fn close(&self) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            match self.take() {
                Some(running) => {
                    running.host.stop(STOP_GRACE).await;
                }
                // A turn holds the run: it is asked to stop, and the run goes
                // when the turn gives it back and this is dropped.
                None => self.control.interrupt(),
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
