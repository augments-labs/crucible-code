//! Which run an event came from.
//!
//! There is one run per turn today, so nothing on screen depends on these
//! answers yet. They are asserted now because attribution is the one thing a
//! later child execution cannot be given retroactively: an event already drawn
//! was drawn without saying whose it was.

use super::*;

/// A destination that keeps the attribution, unlike the harness's own.
struct Attributed(Sender<EventEnvelope>);

impl Post for Attributed {
    fn post(&self, reported: EventEnvelope) {
        drop(self.0.send(reported));
    }
}

/// Takes one turn, reporting to a destination that keeps the envelopes.
fn attributed(scripted: &mut Scripted, prompt: &str) -> Vec<EventEnvelope> {
    let (events, seen) = channel();
    let events = Attributed(events);

    let run = scripted
        .runner
        .starting(&events, &scripted.cancel, &scripted.steer, &scripted.aside);

    scripted
        .runner
        .turn(prompt, Box::new([]), &mut scripted.says, &run)
        .expect("a finished turn");

    drop(events);
    seen.into_iter().collect()
}

#[test]
fn every_event_of_one_turn_names_the_same_run() {
    // Two passes with a tool between them, so the events come from three
    // different places in the loop rather than one.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("found it")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);

    let reported = attributed(&mut scripted, "go");

    let (first, rest) = reported.split_first().expect("a turn reports something");
    assert!(
        rest.len() > 3,
        "a turn this shape reports more than 4 events, and reported {}",
        reported.len()
    );
    for one in rest {
        assert_eq!(
            one.run(),
            first.run(),
            "{:?} came from a different run than the turn started",
            one.event()
        );
    }
}

#[test]
fn a_second_turn_is_a_second_run() {
    let script = Script::new(vec![saying("first"), saying("second")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    let first = attributed(&mut scripted, "one");
    let second = attributed(&mut scripted, "two");

    let (first, _) = first.split_first().expect("the first turn reported");
    let (second, _) = second.split_first().expect("the second turn reported");

    assert_ne!(
        first.run(),
        second.run(),
        "two turns were reported as one run"
    );
}

#[test]
fn a_turn_stopped_before_it_began_still_says_which_run_was_refused() {
    // The pair of events a refused turn posts is still something that
    // happened, and an event with nothing to attribute it to is the one shape
    // this path is not allowed to carry.
    let script = Script::new(vec![saying("never asked")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.cancel.request();

    let reported = attributed(&mut scripted, "go");

    // Balanced, and both halves the same run: a start with no finish leaves the
    // turn looking as though it is still running, and a finish with no start is
    // a shape nothing else here produces.
    let [started, finished] = &reported[..] else {
        panic!("a refused turn starts and finishes, but reported {reported:?}");
    };

    assert!(
        matches!(started.event(), Event::TurnStarted { .. }),
        "the pair did not open with a start: {:?}",
        started.event()
    );
    assert!(
        matches!(
            finished.event(),
            Event::TurnFinished {
                stop: StopReason::Cancelled,
                ..
            }
        ),
        "the pair did not close with a stopped finish: {:?}",
        finished.event()
    );
    assert_eq!(started.run(), finished.run());
    assert_eq!(started.ancestry().depth(), 0, "nothing started it");
}

/// Takes one turn the way the binary does, keeping every envelope.
///
/// A [`TurnError`] is not something the turn can both report and hand back, so
/// whoever asked for the turn is what says it failed. Which run it says that
/// under is this test's whole subject.
///
/// The shape here is the caller's, not the binary's: this crate cannot reach
/// `src/cli/converse.rs`, and the destination the binary hands over drops the
/// envelope on purpose the moment it arrives, so no assertion made downstream
/// of it could see a run at all. What this pins is that a failure reported
/// beside a turn — through the turn's own [`RunContext`], which is the only
/// shape available to a caller holding one — carries that turn's run.
fn refused(scripted: &mut Scripted, prompt: &str) -> Vec<EventEnvelope> {
    let (events, seen) = channel();
    let events = Attributed(events);

    let run = scripted
        .runner
        .starting(&events, &scripted.cancel, &scripted.steer, &scripted.aside);

    if let Err(problem) = scripted
        .runner
        .turn(prompt, Box::new([]), &mut scripted.says, &run)
    {
        run.reporting().post(Event::Failed { error: problem });
    }

    drop(events);
    seen.into_iter().collect()
}

#[test]
fn a_failure_reported_beside_a_turn_carries_that_turns_run() {
    // A provider that refuses with a status nothing recovers from, so the turn
    // starts, reports, and then hands back an error rather than a stop.
    let mut scripted = Scripted::new(Script::failing(), Tools::new(), Verdict::Allow);

    let reported = refused(&mut scripted, "go");

    let (started, rest) = reported.split_first().expect("a refused turn reports");
    assert!(
        matches!(started.event(), Event::TurnStarted { .. }),
        "the turn did not start: {:?}",
        started.event()
    );

    let failed = rest.last().expect("a refused turn says it failed");
    assert!(
        matches!(failed.event(), Event::Failed { .. }),
        "the turn did not end in a failure: {:?}",
        failed.event()
    );
    assert_eq!(
        failed.run(),
        started.run(),
        "the failure was reported under a run that never took a turn"
    );
}
