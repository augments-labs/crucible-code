//! The one thing a runaway turn is actually stopped for.
//!
//! A bound only where somebody asked for one, and on what a turn consumes
//! rather than on how many times round it went: a turn making progress is not
//! a turn to stop for being long. Read at the top of a pass, so a turn that
//! crossed the figure while working still finishes what it started and stops
//! before asking for anything more.

use super::*;

/// A turn under `ceiling` tokens, over a script that spends as it goes.
fn held_to(ceiling: u64, script: Script) -> Scripted {
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);
    scripted.runner.policy.bounds.spend = Some(ceiling);
    scripted
}

/// A pass that asks for a tool and reports what it cost.
fn costing(id: &str, spend: u64) -> Vec<Delta> {
    vec![
        Delta::ToolStarted {
            id: ToolId::new(id),
            name: "read".into(),
        },
        Delta::ToolArgs("{}".into()),
        Delta::Spent(Spend::new(spend)),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

#[test]
fn a_turn_that_has_spent_its_ceiling_is_not_asked_again() {
    // The second pass never goes out: the check is at the top of the loop, so
    // the first response's cost is what it is read against.
    let script = Script::new(vec![costing("a", 90), saying("never asked")]);
    let mut scripted = held_to(50, script);

    let problem = scripted.turn("go").expect_err("a turn over its ceiling");

    assert!(
        matches!(problem, TurnError::Spent { ceiling } if ceiling == 50),
        "stopped for the wrong reason: {problem:?}"
    );
    assert_eq!(
        scripted.asked().len(),
        1,
        "a request went out after the ceiling was reached"
    );
}

#[test]
fn a_turn_under_its_ceiling_runs_to_the_end() {
    let script = Script::new(vec![costing("a", 10), saying("done")]);
    let mut scripted = held_to(50, script);

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Yielded);
}

#[test]
fn a_turn_nobody_bounded_is_not_stopped_for_what_it_spent() {
    // The same script that fails above, under the shipped default: no ceiling
    // at all, so the figure is never compared against anything.
    let script = Script::new(vec![costing("a", 90), saying("done")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);

    assert_eq!(scripted.runner.policy.bounds.spend, None);
    assert_eq!(scripted.turn("go").unwrap(), StopReason::Yielded);
}

#[test]
fn a_turn_stopped_at_its_ceiling_still_answered_every_call_it_recorded() {
    // The ceiling falls between passes, which is after the pass that ran the
    // calls wrote their results. A turn that ended holding a call nothing
    // answered is the shape a replay drops on the way back in.
    let script = Script::new(vec![costing("a", 90), saying("never asked")]);
    let mut scripted = held_to(50, script);

    scripted.turn("go").expect_err("a turn over its ceiling");

    let recorded = scripted.runner.transcript();
    let asked: usize = recorded
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::Agent { calls, .. } => Some(calls.len()),
            _ => None,
        })
        .sum();
    let answered: usize = recorded
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults(results) => Some(results.len()),
            _ => None,
        })
        .sum();

    assert_eq!(asked, 1, "the pass that ran did not record its call");
    assert_eq!(answered, asked, "a recorded call went unanswered");
}

#[test]
fn a_run_asking_for_more_than_the_session_allows_is_still_held_to_it() {
    // The session's policy is a ceiling rather than a starting point. A
    // context minted before the session was narrowed still carries the wider
    // figure, and a caller can write one by hand; neither may buy a turn more
    // than the session it runs in allows.
    let script = Script::new(vec![costing("a", 90), saying("never asked")]);
    let mut scripted = held_to(50, script);

    let asking = RunContext::new(
        RunPolicy::default(),
        &scripted.events,
        &scripted.cancel,
        &scripted.steer,
        &scripted.aside,
    );

    let problem = scripted
        .runner
        .turn("go", Box::new([]), &mut scripted.says, &asking)
        .expect_err("a turn over the session's ceiling");

    assert!(
        matches!(problem, TurnError::Spent { ceiling } if ceiling == 50),
        "a run asking for no ceiling lifted the session's: {problem:?}"
    );
}
