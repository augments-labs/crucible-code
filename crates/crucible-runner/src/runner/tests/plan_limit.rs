//! A used-up plan ends the turn at a request boundary: before sending, where
//! the last reading of the plan's windows says one is used up until a reset
//! still to come, and on the vendor's refusal everywhere else. Neither is
//! asked again, and neither leaves anything in the transcript that was not
//! there before.

use std::time::{SystemTime, UNIX_EPOCH};

use crucible_types::{PlanWindows, Window, WindowReading};

use super::*;
use crate::PlanLimitStop;

/// A day from now: a reset still to come for as long as any test runs.
fn tomorrow() -> SystemTime {
    SystemTime::now() + Duration::from_hours(24)
}

/// A reading with the weekly window `percent` used, starting again at `resets_at`.
fn weekly(percent: u8, resets_at: Option<SystemTime>) -> PlanWindows {
    PlanWindows::new(SystemTime::now()).with(Window::Weekly, WindowReading::new(percent, resets_at))
}

/// A session whose first answer reported `reading`, after that first turn.
fn after_reading(reading: PlanWindows, store: Arc<Recording>) -> Scripted {
    let script = Script::new(vec![saying("one"), saying("two")]).limiting([Some(reading)]);
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Deny, store);
    scripted.turn("one").expect("the first turn to finish");
    assert_eq!(scripted.asked().len(), 1);
    scripted
}

#[test]
fn plan_limit_a_used_up_window_ends_the_turn_before_anything_is_sent() {
    let reset = tomorrow();
    let store = Recording::started("plan limit");
    let mut scripted = after_reading(weekly(100, Some(reset)), Arc::clone(&store));

    let problem = scripted.turn("two").unwrap_err();

    assert!(
        matches!(
            problem,
            TurnError::PlanLimit {
                window: Some(Window::Weekly),
                resets_at: Some(at),
                stopped: PlanLimitStop::BeforeSending,
            } if at == reset
        ),
        "{problem:?}"
    );
    assert_eq!(scripted.asked().len(), 1, "a request went out");
    assert!(
        !scripted
            .events()
            .iter()
            .any(|event| matches!(event, Event::Retrying)),
    );
}

#[test]
fn plan_limit_the_transcript_ends_on_the_line_recorded_before_the_stop() {
    let store = Recording::started("plan limit");
    let mut scripted = after_reading(weekly(100, Some(tomorrow())), Arc::clone(&store));

    scripted.turn("two").unwrap_err();

    let said = store.said();
    assert_eq!(said.last(), Some(&Message::said("two")), "{said:?}");
    assert_eq!(
        conversation(scripted.runner.transcript()).last(),
        Some(&Message::said("two"))
    );
}

#[test]
fn plan_limit_short_of_a_hundred_past_its_reset_or_with_none_does_not_stop() {
    let past = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for reading in [
        weekly(99, Some(tomorrow())),
        weekly(100, Some(past)),
        weekly(100, None),
    ] {
        let mut scripted = after_reading(reading, Recording::nowhere());

        scripted
            .turn("two")
            .unwrap_or_else(|problem| panic!("{reading:?} stopped the turn: {problem:?}"));

        assert_eq!(scripted.asked().len(), 2, "{reading:?}");
    }
}

#[test]
fn plan_limit_a_vendors_refusal_is_asked_once_where_a_busy_moment_is_asked_again() {
    let reset = tomorrow();
    let mut used_up = Scripted::new(
        Script::used_up(Some(reset), None),
        Tools::new(),
        Verdict::Allow,
    );

    let problem = used_up.turn("go").unwrap_err();

    assert!(
        matches!(
            problem,
            TurnError::PlanLimit {
                window: None,
                resets_at: Some(at),
                stopped: PlanLimitStop::Refused,
            } if at == reset
        ),
        "{problem:?}"
    );
    assert_eq!(used_up.asked().len(), 1);
    assert!(
        !used_up
            .events()
            .iter()
            .any(|event| matches!(event, Event::Retrying)),
    );

    // The same status about the moment is still asked again.
    let mut busy = Scripted::new(Script::refusing(429), Tools::new(), Verdict::Allow);
    busy.turn("go").unwrap_err();
    assert_eq!(
        busy.asked().len(),
        1 + usize::from(RunPolicy::default().retry.attempts)
    );
}

#[test]
fn plan_limit_a_refusal_adds_only_the_unended_answer_any_failed_request_leaves() {
    let store = Recording::started("plan limit");
    let mut scripted = Scripted::recording(
        Script::used_up(None, None),
        Tools::new(),
        Verdict::Allow,
        Arc::clone(&store),
    );

    scripted.turn("go").unwrap_err();

    let said: Vec<Message> = store
        .said()
        .into_iter()
        .filter(|message| !matches!(message, Message::Context(_)))
        .collect();
    assert!(
        matches!(
            said.as_slice(),
            [.., Message::User { .. }, Message::Agent { text, calls, stop: None, .. }]
                if text.is_empty() && calls.is_empty()
        ),
        "{said:?}"
    );
}

#[test]
fn plan_limit_the_windows_on_a_refusal_are_kept_and_hold_the_next_turn() {
    let reset = tomorrow();
    let reading = weekly(100, Some(reset));
    let mut scripted = Scripted::new(
        Script::used_up(Some(reset), Some(reading)),
        Tools::new(),
        Verdict::Allow,
    );

    let problem = scripted.turn("go").unwrap_err();

    assert!(
        matches!(
            problem,
            TurnError::PlanLimit {
                window: Some(Window::Weekly),
                stopped: PlanLimitStop::Refused,
                ..
            }
        ),
        "{problem:?}"
    );
    assert_eq!(scripted.runner.plan_limits(), Some(reading));
    assert!(
        scripted
            .events()
            .into_iter()
            .any(|event| matches!(event, Event::PlanLimits { windows } if windows == reading)),
    );

    // What the refusal reported is the reading the next turn is held to.
    let problem = scripted.turn("again").unwrap_err();
    assert!(
        matches!(
            problem,
            TurnError::PlanLimit {
                stopped: PlanLimitStop::BeforeSending,
                ..
            }
        ),
        "{problem:?}"
    );
    assert_eq!(scripted.asked().len(), 1);
}

#[test]
fn plan_limit_a_reading_let_go_on_a_model_change_lets_the_next_turn_go_out() {
    let mut scripted = after_reading(weekly(100, Some(tomorrow())), Recording::nowhere());
    let model = scripted.runner.agent.model().clone();

    scripted.runner.ask(
        "another-model",
        model.max_tokens,
        model.window,
        model.accepts,
    );
    scripted.turn("two").expect("the turn to go out");

    assert_eq!(scripted.asked().len(), 2);
}

#[test]
fn plan_limit_says_the_window_and_the_reset_in_crucibles_own_words() {
    let seconds: u64 = 1_791_190_800;
    let reset = UNIX_EPOCH + Duration::from_secs(seconds);
    let before = TurnError::PlanLimit {
        window: Some(Window::Weekly),
        resets_at: Some(reset),
        stopped: PlanLimitStop::BeforeSending,
    };
    let refused = TurnError::from(ProviderError::PlanLimit {
        provider: "openai",
        window: None,
        resets_at: None,
        reading: None,
    });

    assert_eq!(
        before.to_string(),
        "usage limit reached on the weekly window, which resets at \
         2026-10-05T09:00:00Z; the turn stopped before sending"
    );
    assert_eq!(
        refused.to_string(),
        "usage limit reached, with no reset reported; the vendor refused the request"
    );
}
