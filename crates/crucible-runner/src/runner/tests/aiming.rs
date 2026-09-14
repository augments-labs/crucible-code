//! What a session can be re-aimed at without ending it.
//!
//! `/model`, `/login` and `/effort` all change what the next request goes out
//! as, mid-session. Each of these asks the same question of a different field:
//! that the change reaches the wire, and that it reaches the *next* request
//! rather than the one already sent.

use crucible_types::{RecordedToolOutput, ResultProvenance, ToolCall};

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
fn switching_away_from_a_vendor_that_restricts_its_results_takes_them_out_of_the_next_request() {
    let first = Script::new(vec![
        calling("call_search", "web_search", r#"{"query":"rust"}"#),
        saying("an answer from the vendor that restricts"),
    ])
    .with_name("restricting")
    .restricting(RESTRICTED);

    let mut scripted = Scripted::new(
        first,
        tools([Fixed::new("web_search")
            .answering("restricted search results canary")
            .answered_by(
                ResultProvenance::answered("restricting", Some(RESTRICTED))
                    .expect("a bounded term"),
            )]),
        Verdict::Allow,
    );

    scripted
        .turn("search for rust")
        .expect("the turn to finish");

    let second = Script::new(vec![saying("an answer from elsewhere")]).with_name("elsewhere");
    let after = second.sent();
    scripted.runner.serve(Box::new(second));
    scripted.turn("summarize").expect("the turn to finish");

    drop(after);
    let result = only_result(&scripted);
    assert!(
        !result
            .output
            .text()
            .contains("restricted search results canary"),
        "a restricted result went out to the vendor it was taken away from: {}",
        result.output.text()
    );
    assert_eq!(
        result.output.text(),
        RESTRICTED,
        "the result stands without the sentence saying why it is empty"
    );
}

#[test]
fn leaving_a_vendor_that_restricts_its_results_keeps_the_ones_another_vendor_answered() {
    // A session that searched under one vendor and then moved to one that
    // restricts its own results holds results the restricting vendor never
    // produced. Its term covers what it produced; taking the others away as
    // well empties the conversation of answers nobody restricted.
    let first = Script::new(vec![
        calling("call_search", "web_search", r#"{"query":"rust"}"#),
        saying("an answer from a vendor that restricts nothing"),
    ])
    .with_name("unrestricting");

    let mut scripted = Scripted::new(
        first,
        tools([Fixed::new("web_search")
            .answering("search results the first vendor answered")
            .answered_by(
                ResultProvenance::answered("unrestricting", None).expect("a bounded vendor"),
            )]),
        Verdict::Allow,
    );
    scripted
        .turn("search for rust")
        .expect("the turn to finish");

    scripted.runner.serve(Box::new(
        Script::new(vec![saying("an answer from the vendor that restricts")])
            .with_name("restricting")
            .restricting(RESTRICTED),
    ));
    scripted.turn("and now").expect("the turn to finish");

    scripted.runner.serve(Box::new(
        Script::new(vec![saying("from elsewhere")]).with_name("elsewhere"),
    ));
    scripted.turn("summarize").expect("the turn to finish");

    assert_eq!(
        only_result(&scripted).output.text(),
        "search results the first vendor answered",
        "leaving a vendor took away a result that vendor never produced"
    );
}

#[test]
fn a_search_a_restricting_vendor_answers_after_the_session_left_it_is_not_sent_on() {
    // The search source is chosen when the run starts, so a session that moved
    // away from the vendor whose search it uses still searches through that
    // vendor. What such a search answers has to be kept from the provider the
    // session now talks to from the moment it is recorded, not from the next
    // switch: the next request of the same turn is already on its way there.
    let first = Script::new(vec![saying("from the vendor that restricts")])
        .with_name("restricting")
        .restricting(RESTRICTED);
    let mut scripted = Scripted::new(
        first,
        tools([Fixed::new("web_search")
            .answering("grounded after the switch canary")
            .answered_by(
                ResultProvenance::answered("restricting", Some(RESTRICTED))
                    .expect("a bounded term"),
            )]),
        Verdict::Allow,
    );
    scripted.turn("hello").expect("the turn to finish");

    let elsewhere = Script::new(vec![
        calling("call_search", "web_search", r#"{"query":"rust"}"#),
        saying("an answer from elsewhere"),
    ])
    .with_name("elsewhere");
    let sent = elsewhere.sent();
    scripted.runner.serve(Box::new(elsewhere));
    scripted.turn("search now").expect("the turn to finish");

    assert_eq!(
        only_result(&scripted).output.text(),
        RESTRICTED,
        "a result the restricting vendor answered stayed in the transcript the next request was built from"
    );
    let requests = sent.lock().expect("the requests the vendor was sent");
    assert_eq!(requests.len(), 2, "the search, then the answer after it");
    assert!(
        requests
            .last()
            .is_some_and(|request| !request.carried("grounded after the switch canary")),
        "the request after the search carried what the restricting vendor answered"
    );
}

#[test]
fn a_search_result_an_older_build_recorded_is_taken_away_when_leaving_a_vendor_that_restricts() {
    // A log written before results said who answered them cannot say whether a
    // search came from the vendor being left. The build that wrote it took every
    // search result away in that case, and a session continued here keeps that
    // promise rather than sending them on.
    let first = Script::new(vec![saying("an answer from the vendor that restricts")])
        .with_name("restricting")
        .restricting(RESTRICTED);

    let mut transcript = Transcript::new();
    transcript
        .push(Message::said("search for rust"))
        .expect("a prompt");
    transcript
        .push(Message::Agent {
            continuation: None,
            text: "".into(),
            calls: vec![ToolCall {
                id: ToolId::new("call_search"),
                name: "web_search".into(),
                args: ToolArgs::new(r#"{"query":"rust"}"#),
            }],
            stop: Some(StopReason::WantsTools),
        })
        .expect("a call");
    transcript
        .push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call_search"),
            output: RecordedToolOutput::ok("search results from an older log")
                .answered_by(ResultProvenance::Unrecorded),
        }]))
        .expect("its result");

    let scripted = Scripted::new(first, tools([Fixed::new("web_search")]), Verdict::Allow);
    let mut scripted = Scripted {
        runner: scripted.runner.resuming(transcript),
        ..scripted
    };

    scripted.runner.serve(Box::new(
        Script::new(vec![saying("from elsewhere")]).with_name("elsewhere"),
    ));

    assert_eq!(
        only_result(&scripted).output.text(),
        RESTRICTED,
        "a search result nobody could attribute went on to another vendor"
    );
}

#[test]
fn a_search_result_an_older_build_recorded_stays_when_leaving_a_vendor_that_restricts_nothing() {
    // The other half of the older build's rule: it took search results away
    // only when leaving a vendor that restricts them.
    let first = Script::new(vec![saying("an answer")]).with_name("unrestricting");

    let mut transcript = Transcript::new();
    transcript
        .push(Message::said("search for rust"))
        .expect("a prompt");
    transcript
        .push(Message::Agent {
            continuation: None,
            text: "".into(),
            calls: vec![ToolCall {
                id: ToolId::new("call_search"),
                name: "web_search".into(),
                args: ToolArgs::new(r#"{"query":"rust"}"#),
            }],
            stop: Some(StopReason::WantsTools),
        })
        .expect("a call");
    transcript
        .push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call_search"),
            output: RecordedToolOutput::ok("search results from an older log")
                .answered_by(ResultProvenance::Unrecorded),
        }]))
        .expect("its result");

    let scripted = Scripted::new(first, tools([Fixed::new("web_search")]), Verdict::Allow);
    let mut scripted = Scripted {
        runner: scripted.runner.resuming(transcript),
        ..scripted
    };
    scripted.runner.serve(Box::new(
        Script::new(vec![saying("from elsewhere")]).with_name("elsewhere"),
    ));

    assert_eq!(
        only_result(&scripted).output.text(),
        "search results from an older log",
        "a vendor that restricts nothing took an older search result away"
    );
}

#[test]
fn a_vendor_that_restricts_nothing_leaves_the_results_where_they_are() {
    // The other half of the same rule, and the one that keeps it from being a
    // clearing on every swap: what may be sent on is the producing vendor's to
    // say, and a vendor that says nothing has restricted nothing.
    let first = Script::new(vec![
        calling("call_search", "web_search", r#"{"query":"rust"}"#),
        saying("an answer"),
    ])
    .with_name("unrestricting");

    let mut scripted = Scripted::new(
        first,
        tools([Fixed::new("web_search")
            .answering("ordinary search results canary")
            .answered_by(
                ResultProvenance::answered("unrestricting", None).expect("a bounded vendor"),
            )]),
        Verdict::Allow,
    );

    scripted
        .turn("search for rust")
        .expect("the turn to finish");

    let second = Script::new(vec![saying("an answer from elsewhere")]).with_name("elsewhere");
    let after = second.sent();
    scripted.runner.serve(Box::new(second));
    scripted.turn("summarize").expect("the turn to finish");

    drop(after);
    assert_eq!(
        only_result(&scripted).output.text(),
        "ordinary search results canary",
        "a result nobody restricted was taken away from the conversation"
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
        Some(72),
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
        Some(72),
        "the exact output correction visibly freed uncompacted context"
    );

    scripted.runner.serve(Box::new(Elsewhere::new()));

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

    scripted.runner.serve(Box::new(Elsewhere::new()));

    assert_eq!(scripted.runner.serving(), ELSEWHERE);
}

/// A provider that answers nothing, under a name of its own.
///
/// Every other provider here is called the same thing, and one assertion needs
/// two that can be told apart.
struct Elsewhere {
    credential_scope: crucible_core::CredentialScopeId,
}

impl Elsewhere {
    fn new() -> Self {
        Self {
            credential_scope: crucible_core::CredentialScopeId::new(),
        }
    }
}

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

    fn prompt_cache_capabilities(&self, _model: &str) -> crucible_core::PromptCacheCapabilities {
        crucible_core::PromptCacheCapabilities::unknown("elsewhere-fixture-v1")
    }

    fn prompt_cache_route(&self) -> crucible_core::PromptCacheRoute<'_> {
        crucible_core::PromptCacheRoute {
            protocol: ELSEWHERE,
            endpoint: ELSEWHERE,
            custom_endpoint: true,
            credential_scope: self.credential_scope,
            account: None,
            project: None,
            request_shape_version: "elsewhere-fixture-v1",
        }
    }

    fn prompt_cache_encoding(&self, _request: &Request<'_>) -> crucible_core::PromptCacheEncoding {
        crucible_core::PromptCacheEncoding::NoControlIntended
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
