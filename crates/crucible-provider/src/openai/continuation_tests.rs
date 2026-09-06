//! Native Responses items survive a full stream and the next stateless request.

use super::*;
use crate::transport::Replay;
use crucible_core::{
    ApiKey, Continuation, Delta, Effort, Header, HeaderKey, Message, RequestPurpose, ToolArgs,
    ToolCall, ToolId, ToolOutput, ToolResult, Transcript,
};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::sync::Arc;

fn output() -> Vec<Value> {
    vec![
        json!({"type":"reasoning","id":"rs-first","summary":[],"encrypted_content":"private-first"}),
        json!({"type":"message","id":"msg-before","status":"completed","role":"assistant","phase":"commentary","content":[
            {"type":"output_text","text":"beforeé","annotations":[]},
            {"type":"output_text","text":"!","annotations":[{"type":"url_citation","start_index":0,"end_index":1,"url":"https://example.com","title":"Example"}]}
        ]}),
        json!({"type":"function_call","id":"fc-first","call_id":"call-1","name":"read","arguments":"{\"path\":\"a\"}","status":"completed"}),
        json!({"type":"reasoning","id":"rs-next","summary":[{"type":"summary_text","text":"private-summary"}],"encrypted_content":"private-second"}),
        json!({"type":"web_search_call","id":"ws-first","status":"completed","action":{"type":"search","queries":["fixture"]}}),
        json!({"type":"message","id":"msg-after","status":"completed","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"after","annotations":[]}]}),
    ]
}

fn sse(events: &[Value]) -> String {
    let mut body = String::new();
    for event in events {
        writeln!(body, "data: {event}\n").unwrap();
    }
    body
}

fn events(output: &[Value], repeat: bool) -> Vec<Value> {
    let mut events = vec![
        json!({"type":"response.created","response":{"id":"resp-fixture","status":"in_progress","output":[]}}),
    ];
    for (index, item) in output.iter().enumerate() {
        let mut started = item.clone();
        if item.get("type").and_then(Value::as_str) == Some("message") {
            started
                .as_object_mut()
                .unwrap()
                .insert("content".into(), json!([]));
        }
        events
            .push(json!({"type":"response.output_item.added","output_index":index,"item":started}));
        events.push(json!({"type":"response.output_item.done","output_index":index,"item":item}));
    }
    events.push(json!({"type":"response.completed","response":{"id":"resp-fixture","status":"completed","output":if repeat {output.to_vec()} else {vec![]}}}));
    events
}

fn provider(endpoint: Endpoint, body: &str) -> (OpenAi, Arc<Replay>) {
    let replay = Arc::new(Replay::new(200, body));
    let credential = HeaderKey::new(ApiKey::new("synthetic-astra-key"), Header::bearer());
    (
        OpenAi::at(
            endpoint,
            Box::new(credential),
            Box::new(Arc::clone(&replay)),
        ),
        replay,
    )
}

fn request(transcript: &Transcript) -> Request<'_> {
    Request {
        model: ASTRA,
        purpose: RequestPurpose::Turn,
        transcript,
        tools: &[],
        system: None,
        max_tokens: 8192,
        effort: None,
        attached: &[],
        prompt_cache: None,
    }
}

fn answer(provider: &OpenAi, request: Request<'_>) -> Message {
    let mut stream = provider.stream(request, &Cancel::new()).unwrap();
    let mut text = String::new();
    let mut calls: Vec<ToolCall> = Vec::new();
    let mut pending: Option<Continuation> = None;
    let mut stop = None;
    while let Some(delta) = stream.next() {
        match delta.unwrap() {
            Delta::Text(more) => text.push_str(&more),
            Delta::ToolStarted { id, name } => calls.push(ToolCall {
                id,
                name,
                args: ToolArgs::new(""),
            }),
            Delta::ToolArgs(more) => {
                let call = calls.last_mut().unwrap();
                call.args = ToolArgs::new(format!("{}{more}", call.args.as_str()));
            }
            Delta::Continuation(state) => assert!(pending.replace(state).is_none()),
            Delta::Stopped(reason) => stop = Some(reason),
            _ => {}
        }
    }
    assert_eq!(text, "beforeé!after");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls.first().unwrap().args.as_str(), r#"{"path":"a"}"#);
    let continuation = pending
        .expect("complete Astra response must retain ordered native items")
        .finish(&text, calls.len(), stop)
        .unwrap();
    assert!(!format!("{continuation:?}").contains("private"));
    Message::Agent {
        text: text.into(),
        calls,
        stop,
        continuation: Some(continuation),
    }
}

fn answered() -> Message {
    Message::ToolResults(vec![ToolResult {
        id: ToolId::new("call-1"),
        output: ToolOutput::ok("contents"),
    }])
}

#[test]
fn native_history_is_descriptive_when_switching_to_a_legacy_protocol_reader() {
    let (origin, _) = provider(VENDOR, &sse(&events(&output(), true)));
    let mut history = Transcript::new();
    history.push(Message::said("first")).unwrap();
    history.push(answer(&origin, request(&history))).unwrap();
    history.push(answered()).unwrap();
    for model in ["gpt-5.6", "claude-opus-5", "k3"] {
        let replay = Arc::new(Replay::new(200, ""));
        let credential = Box::new(HeaderKey::new(
            ApiKey::new("synthetic-foreign-key"),
            Header::bearer(),
        ));
        let transport = Box::new(Arc::clone(&replay));
        let target: Box<dyn Provider> = match model {
            "gpt-5.6" => Box::new(OpenAi::at(VENDOR, credential, transport)),
            "claude-opus-5" => Box::new(crate::Anthropic::at(
                crate::Anthropic::VENDOR,
                credential,
                transport,
            )),
            _ => Box::new(crate::Moonshot::at(
                crate::Moonshot::PLATFORM,
                credential,
                transport,
            )),
        };
        target
            .stream(
                Request {
                    model,
                    ..request(&history)
                },
                &Cancel::new(),
            )
            .unwrap();
        let sent = replay.sent();
        assert!(!sent.body.contains("private"));
        let body: Value = serde_json::from_str(&sent.body).unwrap();
        let messages = body
            .get("input")
            .or_else(|| body.get("messages"))
            .unwrap()
            .as_array()
            .unwrap();
        assert!(
            messages
                .iter()
                .all(|message| message.get("role") == Some(&json!("user"))),
            "foreign history was framed as native calls for {model}"
        );
        assert!(sent.body.contains("Historical tool request"));
        assert!(sent.body.contains("Historical tool results"));
    }
}

#[test]
fn astra_known_usage_fields_reject_malformed_shapes_before_retaining_state() {
    for usage in [
        json!("private-not-usage"),
        json!({"input_tokens":"private-not-a-count"}),
        json!({"output_tokens":-1}),
        json!({"total_tokens":1.5}),
        json!({"input_tokens_details":[]}),
        json!({"input_tokens_details":{"cached_tokens":true}}),
        json!({"input_tokens_details":{"cache_write_tokens":"private-not-a-count"}}),
        json!({"output_tokens_details":{"reasoning_tokens":{}}}),
    ] {
        let mut fixture = events(&output(), true);
        fixture
            .last_mut()
            .unwrap()
            .get_mut("response")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("usage".into(), usage);
        let (provider, _) = provider(VENDOR, &sse(&fixture));
        let transcript = Transcript::new();
        let mut stream = provider
            .stream(request(&transcript), &Cancel::new())
            .unwrap();
        let mut error = None;
        while let Some(delta) = stream.next() {
            match delta {
                Err(problem) => {
                    error = Some(problem);
                    break;
                }
                Ok(Delta::Continuation(_)) => {
                    panic!("malformed usage must not retain continuation")
                }
                _ => {}
            }
        }
        let error = error.expect("malformed known usage field must fail");
        assert!(!format!("{error:?} {error}").contains("private-not"));
    }
}

#[test]
fn astra_effort_changes_preserve_the_request_prefix_and_replay_update_positions() {
    for initial in [None, Some(Effort::Low)] {
        let (provider, replay) = provider(VENDOR, &sse(&events(&output(), true)));
        let mut transcript = Transcript::new();
        transcript.push(Message::said("first")).unwrap();
        transcript
            .push(answer(
                &provider,
                Request {
                    effort: initial,
                    ..request(&transcript)
                },
            ))
            .unwrap();
        transcript.push(answered()).unwrap();
        transcript.push(Message::said("harder")).unwrap();
        transcript
            .push(answer(
                &provider,
                Request {
                    effort: Some(Effort::High),
                    ..request(&transcript)
                },
            ))
            .unwrap();
        let first: Value = serde_json::from_str(&replay.sent().body).unwrap();
        assert_eq!(
            first.pointer("/reasoning/effort"),
            initial.map(|rung| json!(rung.as_str())).as_ref()
        );
        let items = first.get("input").unwrap().as_array().unwrap();
        assert_eq!(
            items.get(8),
            Some(&json!({"type":"configuration_update","reasoning":{"effort":"high"}}))
        );
        assert_eq!(
            items.get(9),
            Some(&json!({"role":"user","content":"harder"}))
        );
        transcript.push(answered()).unwrap();
        provider
            .stream(
                Request {
                    effort: Some(Effort::High),
                    ..request(&transcript)
                },
                &Cancel::new(),
            )
            .unwrap();
        let second: Value = serde_json::from_str(&replay.sent().body).unwrap();
        assert_eq!(
            second.pointer("/reasoning/effort"),
            first.pointer("/reasoning/effort")
        );
        assert_eq!(
            second
                .get("input")
                .unwrap()
                .as_array()
                .unwrap()
                .get(..items.len()),
            Some(items.as_slice())
        );
        assert_eq!(
            second
                .get("input")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item.get("type") == Some(&json!("configuration_update")))
                .count(),
            1
        );
    }
}

#[test]
fn astra_effort_reset_on_resume_keeps_default_absent_and_native_history_intact() {
    let expected = output();
    let (provider, replay) = provider(VENDOR, &sse(&events(&expected, true)));
    let mut transcript = Transcript::new();
    transcript.push(Message::said("first")).unwrap();
    transcript
        .push(answer(
            &provider,
            Request {
                effort: Some(Effort::Max),
                ..request(&transcript)
            },
        ))
        .unwrap();
    transcript.push(answered()).unwrap();
    transcript
        .push(Message::said("resumed with no configured effort"))
        .unwrap();
    provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    let sent: Value = serde_json::from_str(&replay.sent().body).unwrap();
    assert!(sent.get("reasoning").is_none());
    let items = sent.get("input").unwrap().as_array().unwrap();
    assert_eq!(items.get(1..7), Some(expected.as_slice()));
    assert!(
        items
            .iter()
            .all(|item| item.get("type") != Some(&json!("configuration_update")))
    );
}

#[test]
fn astra_replays_complete_reasoning_phase_annotations_and_native_calls_on_both_routes() {
    for endpoint in [VENDOR, SUBSCRIPTION] {
        let expected = output();
        let (provider, replay) = provider(
            endpoint.clone(),
            &sse(&events(&expected, endpoint == VENDOR)),
        );
        let mut transcript = Transcript::new();
        transcript.push(Message::said("read a")).unwrap();
        transcript
            .push(answer(&provider, request(&transcript)))
            .unwrap();
        transcript.push(answered()).unwrap();
        provider
            .stream(request(&transcript), &Cancel::new())
            .unwrap();
        let sent: Value = serde_json::from_str(&replay.sent().body).unwrap();
        let input = sent.get("input").unwrap().as_array().unwrap();
        assert_eq!(input.get(1..1 + expected.len()), Some(expected.as_slice()));
        assert_eq!(input.last().unwrap().get("call_id"), Some(&json!("call-1")));
        assert!(sent.get("previous_response_id").is_none());
    }
}

#[test]
fn astra_retained_tail_keeps_native_items_but_recap_and_foreign_scope_are_only_visible() {
    let expected = output();
    let (provider, replay) = provider(VENDOR, &sse(&events(&expected, true)));
    let mut transcript = Transcript::new();
    transcript.push(Message::said("old context")).unwrap();
    transcript.push(Message::said("read a")).unwrap();
    transcript
        .push(answer(&provider, request(&transcript)))
        .unwrap();
    transcript.push(answered()).unwrap();
    transcript.compacted(1, "recap");
    provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    let sent: Value = serde_json::from_str(&replay.sent().body).unwrap();
    assert_eq!(
        sent.get("input")
            .unwrap()
            .as_array()
            .unwrap()
            .get(2..2 + expected.len()),
        Some(expected.as_slice())
    );
    provider
        .stream(
            Request {
                purpose: RequestPurpose::Recap,
                ..request(&transcript)
            },
            &Cancel::new(),
        )
        .unwrap();
    let sent = replay.sent();
    assert!(!sent.body.contains("private"));
    assert!(!sent.body.contains("encrypted_content"));
    let value: Value = serde_json::from_str(&sent.body).unwrap();
    assert!(
        value
            .get("input")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("role") == Some(&json!("user")))
    );
    let (foreign, foreign_replay) = self::provider(SUBSCRIPTION, "");
    foreign
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    let sent = foreign_replay.sent();
    assert!(!sent.body.contains("private"));
    let value: Value = serde_json::from_str(&sent.body).unwrap();
    assert!(
        value
            .get("input")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("role") == Some(&json!("user")))
    );
}

#[test]
fn astra_text_waiting_behind_reasoning_is_not_lost_when_its_item_becomes_current() {
    let output = output();
    let mut fixture = events(&output, true);
    let first_done = fixture.remove(2);
    fixture.insert(3,json!({"type":"response.output_text.delta","output_index":1,"content_index":0,"item_id":"msg-before","delta":"before"}));
    fixture.insert(4, first_done);
    fixture.insert(5,json!({"type":"response.output_text.delta","output_index":1,"content_index":0,"item_id":"msg-before","delta":"é"}));
    let (provider, _) = provider(VENDOR, &sse(&fixture));
    let mut transcript = Transcript::new();
    transcript.push(Message::said("read a")).unwrap();
    answer(&provider, request(&transcript));
}

#[test]
fn astra_contradictory_text_done_cannot_be_silently_replaced_by_a_later_item() {
    let mut fixture = events(&output(), true);
    fixture.insert(4,json!({"type":"response.output_text.done","output_index":1,"content_index":0,"item_id":"msg-before","text":"contradictory"}));
    let (provider, _) = provider(VENDOR, &sse(&fixture));
    let transcript = Transcript::new();
    let mut stream = provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    let mut error = None;
    while let Some(delta) = stream.next() {
        if let Err(problem) = delta {
            error = Some(problem);
        }
    }
    assert!(error.is_some(), "contradictory completed text was accepted");
}

#[test]
fn astra_http_refusals_do_not_echo_encrypted_request_state() {
    let replay = Arc::new(Replay::new(
        400,
        r#"{"error":{"message":"private-encrypted-canary","type":"invalid_request_error"}}"#,
    ));
    let credential = HeaderKey::new(ApiKey::new("synthetic-astra-key"), Header::bearer());
    let provider = OpenAi::at(VENDOR, Box::new(credential), Box::new(replay));
    let transcript = Transcript::new();
    let error = provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap_err();
    assert!(!format!("{error:?} {error}").contains("private-encrypted-canary"));
}

fn failed_stream(events: &[Value]) -> Option<ProviderError> {
    let (provider, _) = provider(VENDOR, &sse(events));
    let transcript = Transcript::new();
    let mut stream = provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    while let Some(delta) = stream.next() {
        if let Err(error) = delta {
            return Some(error);
        }
    }
    None
}

#[test]
fn astra_function_identity_cannot_change_between_added_and_done() {
    let mut fixture = events(&output(), true);
    let added = fixture
        .iter_mut()
        .find(|event| {
            event.get("type") == Some(&json!("response.output_item.added"))
                && event.pointer("/item/type") == Some(&json!("function_call"))
        })
        .unwrap();
    added
        .get_mut("item")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("call_id".into(), json!("another-call"));
    assert!(failed_stream(&fixture).is_some());
}

#[test]
fn astra_function_arguments_done_must_agree_with_its_item() {
    let mut fixture = events(&output(), true);
    let position = fixture
        .iter()
        .position(|event| {
            event.get("type") == Some(&json!("response.output_item.done"))
                && event.pointer("/item/type") == Some(&json!("function_call"))
        })
        .unwrap();
    fixture.insert(position,json!({"type":"response.function_call_arguments.done","output_index":2,"item_id":"fc-first","arguments":"{\"path\":\"different\"}"}));
    assert!(failed_stream(&fixture).is_some());
}

#[test]
fn astra_transient_stream_errors_keep_retry_classification_without_private_prose() {
    let error = failed_stream(&[
        json!({"type":"error","code":"server_error","message":"private-retry-canary"}),
    ])
    .unwrap();
    assert!(error.transient());
    assert!(!format!("{error:?} {error}").contains("private-retry-canary"));
}

#[test]
fn astra_replay_rejects_invalid_native_headers_before_sending() {
    use crucible_core::{ContinuationData, ContinuationPart};
    let (provider, replay) = provider(VENDOR, &sse(&events(&output(), true)));
    let mut history = Transcript::new();
    history.push(Message::said("read a")).unwrap();
    let Message::Agent {
        text,
        calls,
        stop,
        continuation: Some(original),
    } = answer(&provider, request(&history))
    else {
        panic!("missing agent");
    };
    let mut state =
        Continuation::new(original.protocol(), original.model(), original.scope()).unwrap();
    for part in original.parts() {
        let mut part = part.clone();
        if let ContinuationPart::Opaque(data) = &part {
            let mut value: Value = serde_json::from_str(data.as_str()).unwrap();
            if let Some(message) = value.get_mut("message") {
                message
                    .as_object_mut()
                    .unwrap()
                    .insert("phase".into(), json!("private-invalid-phase"));
                part = ContinuationPart::Opaque(ContinuationData::new(&value.to_string()).unwrap());
            }
        }
        state.push(part).unwrap();
    }
    let state = state.finish(&text, calls.len(), stop).unwrap();
    history
        .push(Message::Agent {
            text,
            calls,
            stop,
            continuation: Some(state),
        })
        .unwrap();
    history.push(answered()).unwrap();
    let previous = replay.sent().body;
    let error = provider
        .stream(request(&history), &Cancel::new())
        .expect_err("invalid replay headers were accepted");
    assert!(!format!("{error:?} {error}").contains("private-invalid-phase"));
    assert_eq!(
        replay.sent().body,
        previous,
        "invalid request reached transport"
    );
}

#[test]
fn astra_explicit_cache_uses_a_supported_input_block_without_mutating_native_items() {
    let expected = output();
    let (provider, replay) = provider(VENDOR, &sse(&events(&expected, true)));
    let mut history = Transcript::new();
    history.push(Message::said("read a")).unwrap();
    history.push(answer(&provider, request(&history))).unwrap();
    history.push(answered()).unwrap();
    history.push(Message::said("continue")).unwrap();
    let request = crate::fake::cached(
        request(Box::leak(Box::new(history))),
        crucible_core::PromptCacheMechanism::ExplicitBreakpoints,
        PromptCacheRetentionClass::Ephemeral,
        false,
    );
    assert_eq!(
        provider.prompt_cache_encoding(&request),
        crucible_core::PromptCacheEncoding::BreakpointsEncoded(1)
    );
    provider.stream(request, &Cancel::new()).unwrap();
    let body: Value = serde_json::from_str(&replay.sent().body).unwrap();
    assert_eq!(
        body.pointer("/input/0/content/0/prompt_cache_breakpoint"),
        Some(&json!({"mode":"explicit"}))
    );
    assert_eq!(
        body.get("input")
            .unwrap()
            .as_array()
            .unwrap()
            .get(1..1 + expected.len()),
        Some(expected.as_slice())
    );
    assert_eq!(
        replay
            .sent()
            .body
            .matches("prompt_cache_breakpoint")
            .count(),
        1
    );
}

#[test]
fn astra_recap_reports_only_the_explicit_marker_it_actually_writes() {
    let (provider, replay) = provider(VENDOR, "");
    let mut history = Transcript::new();
    history.push(Message::said("earlier")).unwrap();
    history.push(Message::said("recap now")).unwrap();
    let request = crate::fake::cached(
        Request {
            purpose: RequestPurpose::Recap,
            ..request(Box::leak(Box::new(history)))
        },
        crucible_core::PromptCacheMechanism::ExplicitBreakpoints,
        PromptCacheRetentionClass::Ephemeral,
        false,
    );
    assert_eq!(
        provider.prompt_cache_encoding(&request),
        crucible_core::PromptCacheEncoding::BreakpointsEncoded(1)
    );
    provider.stream(request, &Cancel::new()).unwrap();
    let body: Value = serde_json::from_str(&replay.sent().body).unwrap();
    assert_eq!(
        body.pointer("/input/0/content/0/prompt_cache_breakpoint"),
        Some(&json!({"mode":"explicit"}))
    );
}

fn bounded_answer(output: &[Value]) -> Result<(), ProviderError> {
    let (provider, _) = provider(SUBSCRIPTION, &sse(&events(output, false)));
    let history = Transcript::new();
    let mut stream = provider.stream(request(&history), &Cancel::new())?;
    let mut pending = None;
    let mut text = String::new();
    let mut calls = 0;
    let mut stop = None;
    while let Some(delta) = stream.next() {
        match delta? {
            Delta::Text(more) => text.push_str(&more),
            Delta::ToolStarted { .. } => calls += 1,
            Delta::Continuation(state) => pending = Some(state),
            Delta::Stopped(reason) => stop = Some(reason),
            _ => {}
        }
    }
    pending
        .expect("bounded complete output must retain replay")
        .finish(&text, calls, stop)
        .unwrap();
    Ok(())
}

#[test]
fn astra_assembly_limits_text_and_arguments_at_the_exact_aggregate_boundary() {
    for extra in [0, 1] {
        let output:Vec<_>=(0..32).map(|index|json!({"type":"message","id":format!("msg-{index}"),"role":"assistant","status":"completed","content":[{"type":"output_text","text":"x".repeat(256*1024+if index==31 {extra} else {0}),"annotations":[]}]})).collect();
        assert_eq!(
            bounded_answer(&output).is_ok(),
            extra == 0,
            "text ceiling+{extra}"
        );
        let output:Vec<_>=(0..2).map(|index|json!({"type":"function_call","id":format!("fc-{index}"),"call_id":format!("call-{index}"),"name":"read","status":"completed","arguments":format!("{{\"x\":\"{}\"}}","x".repeat(512*1024-8+if index==1 {extra} else {0}))})).collect();
        assert_eq!(
            bounded_answer(&output).is_ok(),
            extra == 0,
            "argument ceiling+{extra}"
        );
    }
}

#[test]
fn astra_assembly_limits_call_count_and_private_state() {
    for count in [128, 129] {
        let output:Vec<_>=(0..count).map(|index|json!({"type":"function_call","id":format!("fc-{index}"),"call_id":format!("call-{index}"),"name":"read","status":"completed","arguments":"{}"})).collect();
        assert_eq!(bounded_answer(&output).is_ok(), count == 128);
    }
    for count in [6, 7] {
        let output:Vec<_>=(0..count).map(|index|json!({"type":"reasoning","id":format!("rs-{index}"),"summary":[],"encrypted_content":"x".repeat(150_000)})).collect();
        assert_eq!(bounded_answer(&output).is_ok(), count == 6);
    }
}
