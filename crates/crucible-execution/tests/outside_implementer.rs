//! A backend written outside the crate, with nothing but its public contract.
//!
//! This is the fixture a new harness starts from: it claims one capability,
//! refuses every operation it does not claim before looking at anything else,
//! reports a turn through the sink it is handed, and passes the crate's own
//! conformance check. If this stops compiling, the contract changed under every
//! backend written against it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use crucible_execution::{
    Backend, Capabilities, Capability, Decision, Description, Error, Event, Lasting, Outcome,
    PendingRef, SessionRef, Sink, Stale, Text, TurnRequest, Unsupported, conformance,
};
use crucible_runtime::{BoxFuture, answered};

/// An agent that echoes the prompt back and can be stopped.
struct Echo {
    /// Whether the running turn was asked to stop.
    stopping: AtomicBool,
}

impl Echo {
    /// What it claims: cancelling, learned from the driver. Nothing else.
    fn claims() -> Capabilities {
        Capabilities::unknown().declared(Capability::Cancel, true)
    }

    /// Refuses `capability` unless it is claimed.
    fn gate(capability: Capability) -> Result<(), Error> {
        if Self::claims().supports(capability) {
            Ok(())
        } else {
            Err(Unsupported(capability).into())
        }
    }
}

impl Backend for Echo {
    fn describe(&self) -> Description {
        Description::new("echo", "echo", Self::claims()).reporting("1.0.0")
    }

    fn open(&self, session: Option<SessionRef>) -> BoxFuture<'_, Result<SessionRef, Error>> {
        Box::pin(async move {
            if session.is_some() {
                Self::gate(Capability::Resume)?;
            }
            SessionRef::new("echo-session").ok_or_else(|| Error::Failed(failure("no name")))
        })
    }

    fn prompt<'a>(
        &'a self,
        turn: TurnRequest,
        events: &'a dyn Sink,
    ) -> BoxFuture<'a, Result<Outcome, Error>> {
        Box::pin(async move {
            events.post(Event::Started { turn: turn.turn() });
            if self.stopping.swap(false, Ordering::Relaxed) {
                events.post(Event::Finished(Outcome::Cancelled));
                return Ok(Outcome::Cancelled);
            }
            events.post(Event::Delta(Text::new(turn.prompt())));
            events.post(Event::Finished(Outcome::Answered));
            Ok(Outcome::Answered)
        })
    }

    fn decide(&self, _pending: &PendingRef, decision: Decision) -> Result<(), Error> {
        match decision {
            Decision::Allow(Lasting::Once) => Self::gate(Capability::PermissionOnce)?,
            Decision::Allow(Lasting::Session) => Self::gate(Capability::PermissionSession)?,
            Decision::Answer(_) => Self::gate(Capability::Questions)?,
            Decision::Deny => {}
        }
        Err(Stale::Settled.into())
    }

    fn interrupt(&self) -> Result<(), Error> {
        Self::gate(Capability::Cancel)?;
        self.stopping.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn close(&self) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
}

fn failure(reason: &str) -> crucible_execution::Failure {
    crucible_execution::Failure::new(reason)
}

/// Every event a turn posted, in order.
#[derive(Default)]
struct Kept(Mutex<Vec<Event>>);

impl Sink for Kept {
    fn post(&self, event: Event) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

/// The fixture keeps to its claims, as the crate's own check judges them.
#[test]
fn an_outside_backend_passes_conformance() {
    let echo = Echo {
        stopping: AtomicBool::new(false),
    };

    assert_eq!(answered!(conformance(&echo)), Ok(()));
}

/// The fixture runs a turn through the contract alone, and the operation it
/// claims does what it says.
#[test]
fn an_outside_backend_runs_and_stops_a_turn() {
    let echo = Echo {
        stopping: AtomicBool::new(false),
    };
    let kept = Kept::default();
    let first = crucible_types::TurnId::FIRST;

    let session = answered!(echo.open(None)).expect("a new session");
    assert_eq!(session.as_str(), "echo-session");
    let outcome = answered!(echo.prompt(TurnRequest::new(first, "hello"), &kept));
    assert_eq!(outcome, Ok(Outcome::Answered));

    echo.interrupt().expect("cancelling is claimed");
    let outcome = answered!(echo.prompt(TurnRequest::new(first.next(), "again"), &kept));
    assert_eq!(outcome, Ok(Outcome::Cancelled));

    let events = kept.0.into_inner().expect("an unpoisoned lock");
    assert_eq!(
        events.get(1),
        Some(&Event::Delta(Text::new("hello"))),
        "{events:?}"
    );
    assert_eq!(answered!(echo.close()), Ok(()));
}
