//! What `/usage` reads off the runner: the session's totals, and the plan
//! windows the last response reported.
//!
//! Totals are the whole session's, added up across responses and turns, and
//! each response is counted once however many times its provider reports it.
//! Plan windows are one reading, the latest, held for the credential in force
//! and let go of when the provider or the model changes.

use std::time::UNIX_EPOCH;

use crucible_types::{PlanWindows, Window, WindowReading};

use super::*;
use crate::SessionCost;

/// One response that reports 100 input tokens, 20 of them read from a
/// cache, and 10 output, and stops: under the fixture's prices that is
/// 132 000 000 000 femtodollars.
fn reporting(stop: StopReason) -> Vec<Delta> {
    let usage = ProviderUsage::new(
        InputTokenUsage::inclusive_read(Some(100), Some(20)).unwrap(),
        Some(10),
        None,
        None,
        &[],
    )
    .unwrap();
    vec![Delta::Usage(usage), Delta::Stopped(stop)]
}

/// The same response asking for a tool first.
fn reporting_a_call(id: &str) -> Vec<Delta> {
    let mut deltas = vec![
        Delta::ToolStarted {
            id: ToolId::new(id),
            name: "read".into(),
        },
        Delta::ToolArgs("{}".into()),
    ];
    deltas.extend(reporting(StopReason::WantsTools));
    deltas
}

/// Windows a sign-in might report: 42% of a weekly window.
fn weekly(percent: u8) -> PlanWindows {
    PlanWindows::new(UNIX_EPOCH + Duration::from_secs(1_700_000_000)).with(
        Window::Weekly,
        WindowReading::new(
            percent,
            Some(UNIX_EPOCH + Duration::from_secs(1_700_600_000)),
        ),
    )
}

/// How long the script keeps each request out before answering.
const WAITS: Duration = Duration::from_millis(5);

/// The last totals a turn posted.
fn posted(scripted: &Scripted) -> Option<Totals> {
    scripted
        .events()
        .into_iter()
        .rev()
        .find_map(|event| match event {
            Event::Used { totals } => Some(totals),
            _ => None,
        })
}

#[test]
fn usage_totals_add_up_every_response_of_two_turns() {
    // Three responses across two turns: a call and its answer, then one more
    // answer. Each is counted once, whatever the provider repeated in it.
    let script = Script::new(vec![
        reporting_a_call("a"),
        reporting(StopReason::Yielded),
        reporting(StopReason::Yielded),
    ])
    .priced()
    .waiting(WAITS);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);
    assert_eq!(scripted.runner.totals().cost(), SessionCost::Unspent);

    scripted.turn("first").expect("the first turn to finish");
    scripted.turn("second").expect("the second turn to finish");

    let totals = scripted.runner.totals();
    assert_eq!(totals.input(), 300);
    assert_eq!(totals.output(), 30);
    assert_eq!(totals.cache_read(), 60);
    assert_eq!(totals.cache_write(), 0);
    let SessionCost::Priced(cost) = totals.cost() else {
        panic!(
            "every response was priced, so the sum is: {:?}",
            totals.cost()
        );
    };
    assert_eq!(cost.femtocurrency(), 3 * 132_000_000_000);
    assert_eq!(cost.currency().as_str(), "USD");
    // Each of the three requests was out for at least what the script kept
    // it, and that time is counted.
    assert!(totals.api() > Duration::ZERO, "{:?}", totals.api());
    assert!(totals.api() >= 3 * WAITS, "{:?}", totals.api());
    assert!(
        totals.api() <= totals.started().elapsed(),
        "time waiting on requests cannot exceed the time the session has run"
    );
    assert_eq!(
        posted(&scripted),
        Some(totals),
        "a turn posts the same totals the runner reads between turns"
    );
}

#[test]
fn usage_an_unpriced_model_reads_as_not_priced() {
    // The fixture without `.priced()` has no price for its model. Tokens are
    // still counted; the cost is said not to be known rather than to be zero.
    let script = Script::new(vec![reporting(StopReason::Yielded)]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);

    scripted.turn("go").expect("the turn to finish");

    let totals = scripted.runner.totals();
    assert_eq!(totals.cost(), SessionCost::NotPriced);
    assert_eq!(totals.input(), 100);
    assert_eq!(totals.output(), 10);
}

#[test]
fn usage_a_response_that_reports_only_a_spend_is_not_priced() {
    // A provider that says what it generated and nothing a price can apply to
    // leaves a priced session one figure short, so the sum stops being one.
    let script = Script::new(vec![
        reporting(StopReason::Yielded),
        vec![
            Delta::Spent(Spend::new(7)),
            Delta::Stopped(StopReason::Yielded),
        ],
    ])
    .priced();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);

    scripted.turn("priced").expect("the first turn to finish");
    assert!(matches!(
        scripted.runner.totals().cost(),
        SessionCost::Priced(_)
    ));
    scripted
        .turn("unpriced")
        .expect("the second turn to finish");

    let totals = scripted.runner.totals();
    assert_eq!(totals.cost(), SessionCost::NotPriced);
    assert_eq!(totals.output(), 17);
}

/// What [`reporting`] comes to under the fixture's prices.
const ONE_ANSWER: u128 = 132_000_000_000;

/// A response's opening report: what it carried, 100 input tokens with 20 of
/// them read from a cache, and nothing yet about what it generated, the way an
/// Anthropic answer opens.
fn opening() -> Delta {
    Delta::Usage(
        ProviderUsage::new(
            InputTokenUsage::inclusive_read(Some(100), Some(20)).unwrap(),
            None,
            None,
            None,
            &[],
        )
        .unwrap(),
    )
}

/// What the session cost, where it is a lower bound.
fn at_least(totals: &Totals) -> u128 {
    let SessionCost::AtLeast(cost) = totals.cost() else {
        panic!(
            "an answer that never said what it cost leaves a lower bound: {:?}",
            totals.cost()
        );
    };
    cost.femtocurrency()
}

#[test]
fn usage_an_answer_stopped_after_its_opening_report_leaves_a_lower_bound() {
    // Esc pressed after the input was reported and before the output was:
    // what it carried still counts, and what it cost is not known, so the sum
    // of everything else is what the session cost at least. It is not "not
    // priced": the model has a price.
    let cancel = Cancel::new();
    let script = Script::new(vec![vec![opening()], reporting(StopReason::Yielded)])
        .priced()
        .interrupted_by(cancel.clone());
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);
    scripted.cancel = cancel;

    assert_eq!(
        scripted.turn("stopped").expect("the turn to stop"),
        StopReason::Cancelled
    );
    assert_eq!(at_least(&scripted.runner.totals()), 0);
    scripted.cancel.reset();
    scripted
        .turn("answered")
        .expect("the second turn to finish");

    let totals = scripted.runner.totals();
    assert_eq!(at_least(&totals), ONE_ANSWER);
    assert_eq!(totals.input(), 200, "the stopped answer's input counts");
    assert_eq!(totals.cache_read(), 40);
    assert_eq!(totals.output(), 10);
}

#[test]
fn usage_an_answer_broken_off_before_it_reported_anything_leaves_a_lower_bound() {
    // The first request is accepted and goes away before it has said a word,
    // and is asked again: an OpenAI answer reports nothing until it completes,
    // so whatever it was billed is not known, and the sum says it is a floor.
    let script = Script::dropping(1, vec![reporting(StopReason::Yielded)]).priced();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);

    scripted.turn("go").expect("the turn to finish");

    let totals = scripted.runner.totals();
    assert_eq!(at_least(&totals), ONE_ANSWER);
    assert_eq!(totals.input(), 100);
}

#[test]
fn usage_an_answer_that_completed_without_a_report_is_not_priced() {
    // The provider said the answer was done and gave nothing to price it by:
    // no sum is the session's from then on, and "nothing asked yet" is only
    // for a session no response has ended in.
    let script = Script::new(vec![
        reporting(StopReason::Yielded),
        vec![Delta::Stopped(StopReason::Yielded)],
    ])
    .priced();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);

    scripted.turn("priced").expect("the first turn to finish");
    scripted.turn("silent").expect("the second turn to finish");

    assert_eq!(scripted.runner.totals().cost(), SessionCost::NotPriced);
}

#[test]
fn usage_an_answer_that_completed_without_a_report_overrides_a_lower_bound() {
    // A floor says everything but the stopped answer is priced. An answer that
    // then completes and says nothing it used is not priced at all, so the
    // floor no longer holds and the sum is not the session's either.
    let cancel = Cancel::new();
    let script = Script::new(vec![
        vec![opening()],
        vec![Delta::Stopped(StopReason::Yielded)],
    ])
    .priced()
    .interrupted_by(cancel.clone());
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);
    scripted.cancel = cancel;

    assert_eq!(
        scripted.turn("stopped").expect("the turn to stop"),
        StopReason::Cancelled
    );
    assert_eq!(at_least(&scripted.runner.totals()), 0);
    scripted.cancel.reset();
    scripted.turn("silent").expect("the second turn to finish");

    assert_eq!(scripted.runner.totals().cost(), SessionCost::NotPriced);
}

#[test]
fn usage_an_unpriced_model_is_not_priced_however_its_answers_end() {
    let cancel = Cancel::new();
    let script = Script::dropping(1, vec![vec![opening()], reporting(StopReason::Yielded)])
        .interrupted_by(cancel.clone());
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);
    scripted.cancel = cancel;

    assert_eq!(
        scripted.turn("stopped").expect("the turn to stop"),
        StopReason::Cancelled
    );
    assert_eq!(scripted.runner.totals().cost(), SessionCost::NotPriced);
    scripted.cancel.reset();
    scripted
        .turn("answered")
        .expect("the second turn to finish");

    assert_eq!(scripted.runner.totals().cost(), SessionCost::NotPriced);
}

#[test]
fn usage_a_request_never_sent_leaves_the_sum_as_it_was() {
    // Stopped before it went out: nothing was spent on it, so the sum is still
    // the whole of what the session cost.
    let script = Script::new(vec![reporting(StopReason::Yielded)])
        .priced()
        .cancelling_when_exhausted();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);

    scripted.turn("priced").expect("the first turn to finish");
    let _stopped = scripted.turn("never sent");

    let SessionCost::Priced(cost) = scripted.runner.totals().cost() else {
        panic!(
            "the one priced answer, and nothing else: {:?}",
            scripted.runner.totals().cost()
        );
    };
    assert_eq!(cost.femtocurrency(), ONE_ANSWER);
}

#[test]
fn usage_lines_an_edit_changed_are_added_up() {
    let diff = Diff::new([
        Line::new(1, Change::Added, "one"),
        Line::new(2, Change::Removed, "two"),
        Line::new(3, Change::Added, "three"),
    ]);
    let script = Script::new(vec![
        calling("a", "edit", "{}"),
        calling("b", "edit", "{}"),
        saying("done"),
    ]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("edit").showing(diff)]),
        Verdict::Allow,
    );

    scripted.turn("edit twice").expect("the turn to finish");

    let totals = scripted.runner.totals();
    assert_eq!((totals.added(), totals.removed()), (4, 2));
    assert_eq!(posted(&scripted).map(|one| one.added()), Some(4));
}

#[test]
fn usage_plan_windows_a_response_reports_are_kept_and_posted() {
    let script = Script::new(vec![saying("one"), saying("two")]).limiting([Some(weekly(42)), None]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);
    assert_eq!(scripted.runner.plan_limits(), None);

    scripted.turn("one").expect("the first turn to finish");

    assert_eq!(scripted.runner.plan_limits(), Some(weekly(42)));
    assert!(
        scripted
            .events()
            .into_iter()
            .any(|event| matches!(event, Event::PlanLimits { windows } if windows == weekly(42))),
        "a turn whose response reported windows posts them"
    );

    // A later response that reports none says nothing about the windows, so
    // the reading the last one gave stands.
    scripted.turn("two").expect("the second turn to finish");
    assert_eq!(scripted.runner.plan_limits(), Some(weekly(42)));
}

#[test]
fn usage_plan_windows_are_let_go_when_the_provider_changes() {
    let script = Script::new(vec![saying("one")]).limiting([Some(weekly(42))]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);
    scripted.turn("one").expect("the turn to finish");
    assert!(scripted.runner.plan_limits().is_some());

    scripted
        .runner
        .serve(Box::new(Script::new(vec![]).with_name("anthropic")));

    assert_eq!(scripted.runner.plan_limits(), None);
}

#[test]
fn usage_plan_windows_are_let_go_when_the_model_changes() {
    let script = Script::new(vec![saying("one")]).limiting([Some(weekly(42))]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);
    scripted.turn("one").expect("the turn to finish");
    let model = scripted.runner.agent.model().clone();

    // Asked again for the model already in force: nothing moved.
    scripted
        .runner
        .ask(&model.name, model.max_tokens, model.window, model.accepts);
    assert!(scripted.runner.plan_limits().is_some());

    scripted.runner.ask(
        "another-model",
        model.max_tokens,
        model.window,
        model.accepts,
    );
    assert_eq!(scripted.runner.plan_limits(), None);
}

#[test]
fn usage_totals_start_again_for_a_session_picked_up() {
    let script = Script::new(vec![reporting(StopReason::Yielded)]).priced();
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);
    scripted.turn("go").expect("the turn to finish");
    assert_eq!(scripted.runner.totals().input(), 100);

    scripted
        .runner
        .pick_up(crate::recording::Recording::nowhere(), Transcript::new());

    let totals = scripted.runner.totals();
    assert_eq!(totals.input(), 0);
    assert_eq!(totals.cost(), SessionCost::Unspent);
    assert_eq!(totals.api(), Duration::ZERO);
}
