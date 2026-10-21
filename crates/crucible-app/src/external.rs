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
//! program's frames, so nothing is translated here. The one event added here
//! is [`Event::Opened`]: once a turn has ended, the program's own name for its
//! session is posted where it is new, so that whoever records the session can
//! keep the name a later launch resumes it by. A turn whose stream could
//! not be read to its end has already posted that it is uncertain and failed;
//! it answers the failure, and the run it was on is spent.
//!
//! A turn holds the run while it is under way, and gives it back however it
//! ends. A turn whose caller stopped waiting may have left the program
//! mid-answer, and one whose run ended left nothing to answer, so neither run
//! takes a further turn: the next prompt stops it and is refused. An
//! interrupt reaches only a turn under way, and one rung as a turn ends is
//! forgotten when the run is given back. Closing asks a turn under way to stop
//! and waits, for at most [`CLOSING`], until it gives the run back, then holds
//! the run while it stops it, so that every close returns once nothing the run
//! started is still running, or fails saying so; once a stop could not be
//! confirmed, every later close and prompt fails saying so too.
//!
//! Confinement is claimed from what the sandbox reported when the run was
//! started and from nothing else.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crucible_execution::{
    Backend, Capabilities, Capability, Confined, Decision, Description, Error, Event, Failure,
    Lasting, Outcome, PendingRef, Scope, SessionRef, Sink, TurnRequest, Unsupported,
};
use crucible_external_harnesses::{
    CANCEL_GRACE, Control, Dialect, Host, STOP_GRACE, Undecided, Unstopped,
};
use crucible_runtime::BoxFuture;
use tokio::sync::Notify;

/// The longest closing waits for a turn under way to give its run back: the
/// grace a turn asked to stop is given, the grace its process is then given,
/// and room for the turn to return.
pub const CLOSING: Duration = CANCEL_GRACE
    .saturating_add(STOP_GRACE)
    .saturating_add(Duration::from_secs(5));

/// A run and the wire it is read through, while no turn holds them.
struct Running {
    /// The run.
    host: Host,
    /// Its wire.
    dialect: Box<dyn Dialect>,
}

/// Where the run is.
enum Slot {
    /// Waiting for a turn.
    Idle(Running),
    /// Held by a turn, or by a prompt stopping a spent run; `closing` once
    /// closing waits for it.
    Away {
        /// Whether closing waits for it.
        closing: bool,
    },
    /// Given back by a turn that did not finish or whose run ended, or while
    /// closing; it takes no further turn and is stopped by whatever reaches
    /// it next.
    Spent(Running),
    /// Stopped.
    Closed,
    /// Stopped as far as it could be, without its end confirmed, or left
    /// while it was being stopped; and why.
    Unstopped(Unstopped),
}

/// An installed agent's run, answering as a [`Backend`].
pub struct External {
    /// Where the run is.
    slot: Mutex<Slot>,
    /// Rung whenever the run comes back from being away.
    returned: Notify,
    /// What decisions and interrupts reach the run through.
    control: Control,
    /// The session the run records into, as the contract names it.
    session: SessionRef,
    /// The program's own name for its session, as last posted.
    opened: Mutex<Option<SessionRef>>,
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
    /// `host`, read through `dialect`, recording into `session` as the entry
    /// `instance`, claiming what `capabilities` declares and confined as the
    /// run was.
    #[must_use]
    pub fn new(
        host: Host,
        dialect: Box<dyn Dialect>,
        session: SessionRef,
        instance: &str,
        capabilities: Capabilities,
    ) -> Self {
        let confined = Confined::new(host.claim(), Scope::Instance);
        Self {
            driver: dialect.program(),
            control: host.control().clone(),
            slot: Mutex::new(Slot::Idle(Running { host, dialect })),
            returned: Notify::new(),
            session,
            opened: Mutex::new(None),
            instance: instance.into(),
            capabilities: capabilities.confined(confined),
        }
    }

    fn slot(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
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

/// A stop that could not be confirmed, as the contract's failure.
fn unstopped(unstopped: &Unstopped) -> Error {
    Failure::new(&unstopped.to_string()).into()
}

/// The run, taken away for one turn or to be stopped, and given back however
/// that ends.
struct Away<'s> {
    /// What it is given back to.
    external: &'s External,
    /// The run, for as long as it is held and not stopped.
    running: Option<Running>,
    /// Whether the turn returned, so that the program is between turns.
    finished: bool,
    /// Whether the run it took was confirmed to have stopped.
    stopped: bool,
    /// Why the run it took was not, once its stop has said.
    unstopped: Option<Unstopped>,
}

impl Drop for Away<'_> {
    fn drop(&mut self) {
        let mut slot = self.external.slot();
        let closing = matches!(*slot, Slot::Away { closing: true });
        *slot = match self.running.take() {
            None if self.stopped => Slot::Closed,
            // Left while it was being stopped, when the stop said nothing.
            None => Slot::Unstopped(self.unstopped.take().unwrap_or(Unstopped::Unfinished)),
            Some(running) if self.finished && !closing && !running.host.ended() => {
                // A stop rung as the turn ended has nothing left to stop.
                running.host.rest();
                Slot::Idle(running)
            }
            Some(running) => Slot::Spent(running),
        };
        drop(slot);
        self.external.returned.notify_waiters();
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
            let (running, spent) = {
                let mut slot = self.slot();
                match std::mem::replace(&mut *slot, Slot::Away { closing: false }) {
                    Slot::Idle(running) => (running, false),
                    Slot::Spent(running) => (running, true),
                    other @ Slot::Away { .. } => {
                        *slot = other;
                        return Err(Failure::new("a turn is already running on this agent").into());
                    }
                    Slot::Closed => {
                        *slot = Slot::Closed;
                        return Err(Failure::new("this agent's run has ended").into());
                    }
                    Slot::Unstopped(left) => {
                        let failure = unstopped(&left);
                        *slot = Slot::Unstopped(left);
                        return Err(failure);
                    }
                }
            };
            let mut away = Away {
                external: self,
                running: Some(running),
                finished: false,
                stopped: false,
                unstopped: None,
            };
            if spent {
                if let Some(running) = away.running.take()
                    && let Err(left) = running.host.stop(STOP_GRACE).await
                {
                    let failure = unstopped(&left);
                    away.unstopped = Some(left);
                    return Err(failure);
                }
                away.stopped = true;
                return Err(Failure::new(
                    "this agent's run cannot take another turn since its last one, so it was stopped",
                )
                .into());
            }
            let Some(running) = away.running.as_mut() else {
                return Err(Failure::new("this agent's run has ended").into());
            };
            let ended = running
                .host
                .turn(running.dialect.as_mut(), &self.session, &turn, events)
                .await;
            away.finished = true;
            if let Some(named) = running.host.session().and_then(SessionRef::new) {
                let mut opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
                if opened.as_ref() != Some(&named) {
                    *opened = Some(named.clone());
                    drop(opened);
                    events.post(Event::Opened(named));
                }
            }
            Ok(ended?)
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
        // Held while ringing, so that a turn cannot give the run back between
        // the look and the ring and leave the stop for the next turn.
        let slot = self.slot();
        if matches!(*slot, Slot::Away { .. }) {
            self.control.interrupt();
        }
        Ok(())
    }

    fn close(&self) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let deadline = tokio::time::Instant::now() + CLOSING;
            loop {
                let returned = self.returned.notified();
                tokio::pin!(returned);
                returned.as_mut().enable();
                let found = {
                    let mut slot = self.slot();
                    match std::mem::replace(&mut *slot, Slot::Away { closing: true }) {
                        Slot::Idle(running) | Slot::Spent(running) => Some(running),
                        Slot::Closed => {
                            *slot = Slot::Closed;
                            return Ok(());
                        }
                        Slot::Unstopped(left) => {
                            let failure = unstopped(&left);
                            *slot = Slot::Unstopped(left);
                            return Err(failure);
                        }
                        Slot::Away { closing } => {
                            if !closing {
                                self.control.interrupt();
                            }
                            None
                        }
                    }
                };
                if let Some(running) = found {
                    // Held while it stops, so that another close waits for it.
                    let mut away = Away {
                        external: self,
                        running: Some(running),
                        finished: false,
                        stopped: false,
                        unstopped: None,
                    };
                    if let Some(running) = away.running.take()
                        && let Err(left) = running.host.stop(STOP_GRACE).await
                    {
                        let failure = unstopped(&left);
                        away.unstopped = Some(left);
                        return Err(failure);
                    }
                    away.stopped = true;
                    return Ok(());
                }
                if tokio::time::timeout_at(deadline, returned).await.is_err() {
                    return Err(Failure::new(
                        "this agent's turn did not stop, so its run may still be running",
                    )
                    .into());
                }
            }
        })
    }
}

#[cfg(test)]
mod tests;
