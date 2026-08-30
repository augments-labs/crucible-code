//! What a session can be re-aimed at without ending it.
//!
//! `/model`, `/login` and `/effort` all change what the next request goes out
//! as, mid-session. Each of these asks the same question of a different field:
//! that the change reaches the wire, and that it reaches the *next* request
//! rather than the one already sent.

use super::*;

#[test]
fn how_hard_the_session_was_told_to_think_is_on_every_request() {
    // Every turn, not the first one. The loop asks again after each tool call,
    // and a rung that reached only the opening request would leave the thinking
    // the user paid for on the turn that did the least work.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);
    scripted.runner.spec.model.effort = Some(Effort::Max);

    scripted.turn("go").expect("the turn to finish");

    let sent = scripted.sent.lock().unwrap();
    assert_eq!(sent.len(), 2, "one request, then one after the tool ran");
    assert!(
        sent.iter()
            .all(|request| request.effort == Some(Effort::Max)),
        "a request went out without it: {sent:?}"
    );
}

#[test]
fn a_provider_handed_over_mid_session_is_the_one_the_next_turn_is_sent_to() {
    // The half a key given to `/login` needs: a run with no credential resolves
    // the provider that answers nothing, and until it can be replaced that run
    // refuses every turn no matter what it is handed afterwards.
    let first = Script::new(vec![saying("from the first")]);
    let mut scripted = Scripted::new(first, tools([]), Verdict::Allow);

    scripted.turn("go").expect("the turn to finish");

    let second = Script::new(vec![saying("from the second")]);
    let after = second.sent();
    scripted.runner.serve(Box::new(second));
    scripted.turn("again").expect("the turn to finish");

    assert_eq!(
        scripted.sent.lock().unwrap().len(),
        1,
        "the one it started on"
    );
    assert_eq!(after.lock().unwrap().len(), 1, "the one it was handed");

    // And what was said before the swap goes with it. A vendor is who a
    // transcript is sent to, not something a transcript belongs to.
    let sent = after.lock().unwrap();
    let carried = sent.first().expect("the request it was just handed");
    assert!(
        carried.carried("from the first"),
        "the first provider's answer was not carried"
    );
}

#[test]
fn changing_model_replaces_its_limits_and_reestimates_the_load() {
    let script = Script::new(vec![vec![
        Delta::Carried(Carried::new(40_000)),
        Delta::Text("done".into()),
        Delta::Spent(Spend::new(10_000)),
        Delta::Stopped(StopReason::Yielded),
    ]]);
    let mut scripted = Scripted::new(script, tools([]), Verdict::Allow);
    scripted.runner.spec.model.window = Some(200_000);
    scripted.turn("go").expect("a measured turn");
    assert_eq!(
        scripted.runner.left(),
        Some(32),
        "the exact output correction visibly freed uncompacted context"
    );

    scripted
        .runner
        .ask("other", 4096, Some(1_000_000), Some(READS));

    assert_eq!(scripted.runner.model(), "other");
    assert_eq!(scripted.runner.spec.model.max_tokens, 4096);
    assert_eq!(scripted.runner.spec.model.window, Some(1_000_000));
    assert_eq!(
        scripted.runner.left(),
        Some(99),
        "the transcript was not re-estimated against the new window"
    );
    assert_eq!(
        scripted.runner.load.calibrated(),
        None,
        "the old model's exact reading survived the model change"
    );
    assert!(
        scripted.runner.load.tokens() > 0,
        "the transcript stopped counting"
    );
}

#[test]
fn changing_to_a_model_with_no_known_window_clears_the_numeric_reading() {
    let script = Script::new(vec![vec![
        Delta::Carried(Carried::new(40_000)),
        Delta::Stopped(StopReason::Yielded),
    ]]);
    let mut scripted = Scripted::new(script, tools([]), Verdict::Allow);
    scripted.runner.spec.model.window = Some(200_000);
    scripted.turn("go").expect("a measured turn");
    assert_eq!(scripted.runner.left(), Some(77));

    scripted.runner.ask("unbounded", 4_096, None, Some(READS));

    assert_eq!(scripted.runner.left(), None);
    assert_eq!(scripted.runner.load.calibrated(), None);
}

#[test]
fn changing_provider_reestimates_usage_reported_by_the_old_one() {
    let script = Script::new(vec![vec![
        Delta::Carried(Carried::new(40_000)),
        Delta::Text("done".into()),
        Delta::Spent(Spend::new(10_000)),
        Delta::Stopped(StopReason::Yielded),
    ]]);
    let mut scripted = Scripted::new(script, tools([]), Verdict::Allow);
    scripted.runner.spec.model.window = Some(200_000);
    scripted.turn("go").expect("a measured turn");
    assert_eq!(
        scripted.runner.left(),
        Some(32),
        "the exact output correction visibly freed uncompacted context"
    );

    scripted.runner.serve(Box::new(Elsewhere));

    assert_eq!(scripted.runner.left(), Some(99));
    assert_eq!(
        scripted.runner.load.calibrated(),
        None,
        "the old provider's exact reading survived the provider change"
    );
    assert!(
        scripted.runner.load.tokens() > 0,
        "the transcript stopped counting"
    );
}

#[test]
fn the_vendor_a_session_names_is_the_one_it_would_write_to_now() {
    // What a status row is drawn from. `/login` hands over a provider mid
    // session, so a name remembered beside the provider rather than read off
    // it would go on naming the vendor the session opened with — and the row
    // saying that is the row somebody checks before sending anything.
    let script = Script::new(vec![saying("answered")]);
    let mut scripted = Scripted::new(script, tools([]), Verdict::Allow);

    assert_eq!(scripted.runner.serving(), "script");

    scripted.runner.serve(Box::new(Elsewhere));

    assert_eq!(scripted.runner.serving(), ELSEWHERE);
}

/// A provider that answers nothing, under a name of its own.
///
/// Every other provider here is called the same thing, and one assertion needs
/// two that can be told apart.
struct Elsewhere;

/// What it calls itself.
const ELSEWHERE: &str = "elsewhere";

impl Provider for Elsewhere {
    fn name(&self) -> &'static str {
        ELSEWHERE
    }

    /// A stand-in spells what every real provider here spells today.
    ///
    /// It is not a wire protocol, so it has nothing of its own to declare; what
    /// it must not do is differ, or a test would be exercising a capability no
    /// provider has.
    fn spells(&self) -> Modalities {
        Modalities::empty().insert(Modality::Text)
    }

    fn stream(
        &self,
        _request: Request<'_>,
        _cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        Err(ProviderError::Transport {
            provider: ELSEWHERE,
            problem: "nothing is there".into(),
        })
    }
}

#[test]
fn a_rung_asked_for_mid_session_is_on_the_next_request_and_not_the_last_one() {
    // The half `/effort` needs: a session opens on whatever the command line
    // and the files settled, and what is chosen afterwards has to reach the
    // wire without ending the session to do it.
    let script = Script::new(vec![saying("first"), saying("second")]);
    let mut scripted = Scripted::new(script, tools([]), Verdict::Allow);

    assert_eq!(scripted.runner.effort(), None, "nothing has said yet");
    scripted.turn("go").expect("the turn to finish");

    scripted.runner.think(Effort::Low);
    assert_eq!(scripted.runner.effort(), Some(Effort::Low));
    scripted.turn("again").expect("the turn to finish");

    let sent = scripted.sent.lock().unwrap();
    let asked: Vec<Option<Effort>> = sent.iter().map(|request| request.effort).collect();
    assert_eq!(asked, [None, Some(Effort::Low)]);
}
