//! The speed a turn is asked at: the one second send a refusal of fast earns,
//! and the speed each answer said it was served at.

use crucible_models::{Served, Speed};

use super::*;

/// The speed each request was asked at, in order.
fn speeds(scripted: &Scripted) -> Vec<Speed> {
    scripted
        .sent
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.speed)
        .collect()
}

/// Every refusal of fast the turn reported, as the provider and its reason.
fn refusals(events: &[Event]) -> Vec<(&'static str, &str)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::FastRefused {
                provider, reason, ..
            } => Some((*provider, &**reason)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_refusal_of_fast_is_sent_once_more_at_standard_and_turns_fast_off() {
    let script = Script::new(vec![saying("done")]).refusing_fast();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.runner.hasten(Speed::Fast);

    let stop = scripted.turn("go").expect("a turn");

    assert_eq!(stop, StopReason::Yielded);
    assert_eq!(speeds(&scripted), [Speed::Fast, Speed::Standard]);
    assert_eq!(scripted.runner.speed(), Speed::Standard);
    let events = scripted.events();
    assert_eq!(
        refusals(&events),
        [("script", "your plan does not include fast")]
    );
    // Said once, and not as a retry: the second send is not one of the
    // retries the run allows, and it spends none of them.
    assert!(!events.iter().any(|event| matches!(event, Event::Retrying)));
}

#[test]
fn a_turn_after_the_refusal_is_asked_at_standard_from_the_start() {
    let script = Script::new(vec![saying("first"), saying("second")]).refusing_fast();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.runner.hasten(Speed::Fast);

    scripted.turn("one").expect("a turn");
    scripted.turn("two").expect("a turn");

    assert_eq!(
        speeds(&scripted),
        [Speed::Fast, Speed::Standard, Speed::Standard]
    );
    assert_eq!(refusals(&scripted.events()).len(), 1);
}

#[test]
fn a_tool_the_resent_request_asks_for_runs_once() {
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]).refusing_fast();
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);
    scripted.runner.hasten(Speed::Fast);

    scripted.turn("go").expect("a turn");

    assert_eq!(
        speeds(&scripted),
        [Speed::Fast, Speed::Standard, Speed::Standard]
    );
    let finished = scripted
        .events()
        .iter()
        .filter(|event| matches!(event, Event::ToolFinished { .. }))
        .count();
    assert_eq!(finished, 1);
}

#[test]
fn any_other_refusal_of_a_fast_request_is_reported_as_it_is_and_nothing_is_sent_again() {
    // A model that is itself a fast id is refused the way any model is, for
    // the key or the plan: that is not a refusal of a fast form, and asking
    // again at standard would send the same request to the same answer.
    let mut scripted = Scripted::new(Script::refusing(401), Tools::new(), Verdict::Allow);
    scripted.runner.hasten(Speed::Fast);

    scripted.turn("go").unwrap_err();

    assert_eq!(speeds(&scripted), [Speed::Fast]);
    assert_eq!(scripted.runner.speed(), Speed::Fast);
    assert!(refusals(&scripted.events()).is_empty());
}

#[test]
fn a_turn_is_asked_at_standard_until_somebody_chooses_otherwise() {
    let mut scripted = Scripted::new(
        Script::new(vec![saying("done")]),
        Tools::new(),
        Verdict::Allow,
    );

    scripted.turn("go").expect("a turn");

    assert_eq!(speeds(&scripted), [Speed::Standard]);
    assert_eq!(scripted.runner.served(), Served::Unsaid);
}

#[test]
fn the_speed_the_last_answer_was_served_at_is_kept() {
    for served in [Served::Fast, Served::Standard] {
        let script = Script::new(vec![saying("done")]).serving(served);
        let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
        scripted.runner.hasten(Speed::Fast);

        scripted.turn("go").expect("a turn");

        assert_eq!(scripted.runner.served(), served);
    }
}

#[test]
fn another_speed_forgets_what_the_last_answer_was_served_at() {
    // A label that said `fast` after the speed was turned off would be
    // describing an answer the next request is not going to get.
    let script = Script::new(vec![saying("done")]).serving(Served::Fast);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.runner.hasten(Speed::Fast);
    scripted.turn("go").expect("a turn");

    scripted.runner.hasten(Speed::Fast);
    assert_eq!(scripted.runner.served(), Served::Fast);

    scripted.runner.hasten(Speed::Standard);
    assert_eq!(scripted.runner.served(), Served::Unsaid);
}

#[test]
fn a_refusal_forgets_the_speed_the_last_answer_was_served_at_whatever_the_second_send_does() {
    // The label reads what was served: after a line saying fast is off, a
    // second send that fails must not leave it saying `fast`.
    let script = Script::new(vec![saying("first")])
        .serving(Served::Fast)
        .refusing_fast_after(1)
        .cancelling_when_exhausted();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.runner.hasten(Speed::Fast);
    scripted.turn("one").expect("a turn");
    assert_eq!(scripted.runner.served(), Served::Fast);

    let _ = scripted.turn("two");

    assert_eq!(scripted.runner.speed(), Speed::Standard);
    assert_eq!(scripted.runner.served(), Served::Unsaid);
}

#[test]
fn a_second_send_that_was_stopped_is_said_to_have_turned_fast_off_and_nothing_more() {
    // The file loses its fast as well, so the line is owed; what it may not
    // say is that the message went out again.
    let script = Script::new(Vec::new())
        .refusing_fast()
        .cancelling_when_exhausted();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.runner.hasten(Speed::Fast);

    let _ = scripted.turn("go");

    let resent: Vec<bool> = scripted
        .events()
        .iter()
        .filter_map(|event| match event {
            Event::FastRefused { resent, .. } => Some(*resent),
            _ => None,
        })
        .collect();
    assert_eq!(resent, [false]);
    assert_eq!(scripted.runner.speed(), Speed::Standard);
}

#[test]
fn a_compaction_is_asked_at_the_speed_the_turns_are_and_sent_again_when_refused() {
    let script = Script::new(vec![
        saying("first"),
        saying("second"),
        recap("notes to self"),
    ])
    .refusing_fast_after(2);
    let compacting = Compaction {
        keep_tokens: 1,
        ..Compaction::default()
    };
    let mut scripted = Scripted::within(script, 200_000, compacting);
    scripted.runner.hasten(Speed::Fast);
    scripted.turn("first").expect("a turn to compact from");
    scripted.turn("second").expect("a middle to replace");

    scripted.compacting().expect("a structured recap");

    assert_eq!(
        speeds(&scripted),
        [Speed::Fast, Speed::Fast, Speed::Fast, Speed::Standard]
    );
    assert_eq!(scripted.runner.speed(), Speed::Standard);
    assert_eq!(refusals(&scripted.events()).len(), 1);
}
