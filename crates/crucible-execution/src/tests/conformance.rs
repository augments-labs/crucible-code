//! A backend that says yes to what it does not claim is caught.

use std::sync::atomic::{AtomicBool, Ordering};

use crucible_runtime::{BoxFuture, answered};

use crate::{
    Backend, Capabilities, Capability, Decision, Description, Error, Outcome, PendingRef,
    SessionRef, Sink, TurnRequest, Unsupported, conformance,
};

/// A fake that claims `claims` and, where `lies` is set, answers `Ok` to
/// everything whatever it claims.
struct Fake {
    claims: Capabilities,
    lies: bool,
    interrupted: AtomicBool,
}

impl Fake {
    fn new(claims: Capabilities, lies: bool) -> Self {
        Self {
            claims,
            lies,
            interrupted: AtomicBool::new(false),
        }
    }

    fn gate(&self, capability: Capability) -> Result<(), Error> {
        if self.lies || self.claims.supports(capability) {
            Ok(())
        } else {
            Err(Unsupported(capability).into())
        }
    }
}

impl Backend for Fake {
    fn describe(&self) -> Description {
        Description::new("fake", "fake", self.claims)
    }

    fn open(&self, session: Option<SessionRef>) -> BoxFuture<'_, Result<SessionRef, Error>> {
        Box::pin(async move {
            match session {
                Some(named) => self.gate(Capability::Resume).map(|()| named),
                None => Ok(SessionRef::new("new").expect("a short name")),
            }
        })
    }

    fn prompt<'a>(
        &'a self,
        _turn: TurnRequest,
        _events: &'a dyn Sink,
    ) -> BoxFuture<'a, Result<Outcome, Error>> {
        Box::pin(async { Ok(Outcome::Answered) })
    }

    fn decide(&self, _pending: &PendingRef, decision: Decision) -> Result<(), Error> {
        match decision {
            Decision::Allow(crate::Lasting::Once) => self.gate(Capability::PermissionOnce),
            Decision::Allow(crate::Lasting::Session) => self.gate(Capability::PermissionSession),
            Decision::Answer(_) => self.gate(Capability::Questions),
            Decision::Deny => Ok(()),
        }
    }

    fn interrupt(&self) -> Result<(), Error> {
        self.gate(Capability::Cancel)?;
        self.interrupted.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn close(&self) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
}

/// A backend claiming nothing that answers `Ok` to an interrupt has said
/// something its description denies, and conformance names the capability.
#[test]
fn a_backend_answering_ok_for_an_unclaimed_operation_fails_conformance() {
    let lying = Fake::new(Capabilities::unknown(), true);

    let verdict = answered!(conformance(&lying));

    let refused = verdict.expect_err("a lying backend must fail conformance");
    assert_eq!(refused.capability(), Capability::Cancel);
}

/// The same backend, refusing what it does not claim, conforms; and the check
/// never runs an operation it claims.
#[test]
fn a_backend_refusing_what_it_does_not_claim_conforms() {
    let claims = Capabilities::unknown().declared(Capability::Cancel, true);
    let honest = Fake::new(claims, false);

    assert_eq!(answered!(conformance(&honest)), Ok(()));
    assert!(
        !honest.interrupted.load(Ordering::Relaxed),
        "conformance must not run an operation the backend claims"
    );
}

/// Each gated operation is checked, not only the first: a backend honest about
/// cancelling and lying about resuming is caught at resuming.
#[test]
fn every_gated_operation_is_checked() {
    for capability in [
        Capability::Resume,
        Capability::PermissionOnce,
        Capability::PermissionSession,
        Capability::Questions,
    ] {
        let mut claims = Capabilities::unknown();
        for honest in [
            Capability::Cancel,
            Capability::Resume,
            Capability::PermissionOnce,
            Capability::PermissionSession,
            Capability::Questions,
        ] {
            if honest != capability {
                claims = claims.declared(honest, true);
            }
        }
        let lying = Fake::new(claims.declared(capability, false), true);

        let refused = answered!(conformance(&lying)).expect_err("a lie must be caught");
        assert_eq!(refused.capability(), capability);
    }
}
