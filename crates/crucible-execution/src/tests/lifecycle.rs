//! An instance's state moves only by steps it admits, and a signed-out
//! instance is never ready.

use std::collections::HashSet;

use crate::{
    Admission, Authentication, Ending, Lifecycle, Operation, Recheck, Refused, Running, Step,
};

/// Every step there is.
fn steps() -> Vec<Step> {
    let mut steps = vec![Step::Enable, Step::Disable];
    steps.extend(Operation::ALL.map(Step::Begin));
    steps.extend(Ending::ALL.map(Step::End));
    steps.extend(
        [
            Authentication::Unknown,
            Authentication::SignedOut,
            Authentication::SignedIn,
            Authentication::Expired,
            Authentication::Unverifiable,
        ]
        .map(Step::Observe),
    );
    steps
}

/// Every state reachable from a new instance, either admission.
fn reachable() -> HashSet<Lifecycle> {
    let mut seen = HashSet::new();
    let mut waiting = vec![
        Lifecycle::new(Admission::Enabled),
        Lifecycle::new(Admission::Disabled),
    ];
    while let Some(state) = waiting.pop() {
        if !seen.insert(state) {
            continue;
        }
        for step in steps() {
            let mut next = state;
            if next.apply(step).is_ok() {
                waiting.push(next);
            } else {
                assert_eq!(next, state, "a refused {step:?} changed {state:?}");
            }
        }
    }
    seen
}

/// No state any sequence of steps reaches is both signed out and ready, and
/// every ready state was last seen signed in, enabled, with nothing under way.
#[test]
fn no_reachable_state_is_signed_out_and_ready() {
    let states = reachable();
    assert!(states.len() > 10, "the walk reached only {}", states.len());
    for state in states {
        if state.ready() {
            assert_eq!(
                state.authentication(),
                Authentication::SignedIn,
                "{state:?}"
            );
            assert_eq!(state.admission(), Admission::Enabled, "{state:?}");
            assert_eq!(state.running(), Running::Idle, "{state:?}");
        }
        assert!(
            !(state.authentication() == Authentication::SignedOut && state.ready()),
            "{state:?} is signed out and ready"
        );
    }
}

/// A sign-out that succeeds leaves the instance signed out and not ready.
#[test]
fn a_sign_out_that_succeeds_leaves_it_not_ready() {
    let mut state = Lifecycle::new(Admission::Enabled);
    state
        .apply(Step::Observe(Authentication::SignedIn))
        .unwrap();
    assert!(state.ready());

    state.apply(Step::Begin(Operation::SigningOut)).unwrap();
    assert!(!state.ready(), "not ready while an operation is under way");
    state.apply(Step::End(Ending::Succeeded)).unwrap();

    assert_eq!(state.authentication(), Authentication::SignedOut);
    assert!(!state.ready());
}

/// An operation nobody can confirm the end of needs recovery and forgets the
/// account; one that failed to sign out forgets it too.
#[test]
fn an_unconfirmed_ending_needs_recovery() {
    let mut state = Lifecycle::new(Admission::Enabled);
    state
        .apply(Step::Observe(Authentication::SignedIn))
        .unwrap();
    state.apply(Step::Begin(Operation::Verifying)).unwrap();
    state.apply(Step::End(Ending::Unknown)).unwrap();
    assert_eq!(state.running(), Running::RecoveryRequired);
    assert_eq!(state.authentication(), Authentication::Unknown);
    assert!(!state.ready());

    let mut state = Lifecycle::new(Admission::Enabled);
    state
        .apply(Step::Observe(Authentication::SignedIn))
        .unwrap();
    state.apply(Step::Begin(Operation::SigningOut)).unwrap();
    state.apply(Step::End(Ending::Failed)).unwrap();
    assert_eq!(state.authentication(), Authentication::Unknown);
}

/// Recovery ends only where a check or a verification succeeds: one that was
/// cancelled or failed established nothing, and nothing else may start
/// meanwhile.
#[test]
fn only_a_successful_check_recovers() {
    let mut state = Lifecycle::new(Admission::Enabled);
    state
        .apply(Step::Observe(Authentication::SignedIn))
        .unwrap();
    state.apply(Step::Begin(Operation::Disconnecting)).unwrap();
    state.apply(Step::End(Ending::Unknown)).unwrap();
    state
        .apply(Step::Observe(Authentication::SignedIn))
        .unwrap();
    assert_eq!(
        state.apply(Step::Begin(Operation::SigningOut)),
        Err(Refused::Recovering)
    );

    state.apply(Step::Begin(Operation::Checking)).unwrap();
    assert_eq!(state.running(), Running::Recovering(Recheck::Checking));
    assert_eq!(state.apply(Step::Disable), Err(Refused::Busy));
    assert_eq!(
        state.apply(Step::Begin(Operation::Verifying)),
        Err(Refused::Busy)
    );
    state.apply(Step::End(Ending::Cancelled)).unwrap();
    assert_eq!(state.running(), Running::RecoveryRequired);
    assert!(!state.ready());

    state.apply(Step::Begin(Operation::Checking)).unwrap();
    state.apply(Step::End(Ending::Succeeded)).unwrap();
    assert_eq!(state.running(), Running::Idle);
    assert!(state.ready());
}

/// A disabled instance admits management operations and nothing that takes it
/// out of use again; one operation runs at a time, and only a running one ends.
#[test]
fn steps_the_state_does_not_admit_are_refused() {
    let mut disabled = Lifecycle::new(Admission::Disabled);
    assert_eq!(
        disabled.apply(Step::Begin(Operation::Disconnecting)),
        Err(Refused::Disabled)
    );
    assert_eq!(
        disabled.apply(Step::End(Ending::Succeeded)),
        Err(Refused::Idle)
    );
    disabled
        .apply(Step::Begin(Operation::AwaitingLogin))
        .unwrap();
    assert_eq!(
        disabled.apply(Step::Begin(Operation::Checking)),
        Err(Refused::Busy)
    );
    assert_eq!(disabled.apply(Step::Enable), Err(Refused::Busy));

    let mut enabled = Lifecycle::new(Admission::Enabled);
    enabled
        .apply(Step::Begin(Operation::Disconnecting))
        .unwrap();
    enabled.apply(Step::End(Ending::Succeeded)).unwrap();
    assert_eq!(enabled.admission(), Admission::Disabled);
}
