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

    scripted
        .runner
        .turn(
            prompt,
            Box::new([]),
            &mut scripted.says,
            &events,
            &scripted.cancel,
            &scripted.steer,
            &scripted.aside,
        )
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
        "a turn this shape reports more than {} events",
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

    let [started, finished] = &reported[..] else {
        panic!("a refused turn starts and finishes, but reported {reported:?}");
    };

    assert_eq!(started.run(), finished.run());
    assert_eq!(started.ancestry().depth(), 0, "nothing started it");
}
