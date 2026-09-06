//! Real request/stream boundaries preserve the ordered assistant content array.

use super::*;
use crate::transport::Replay;
use crucible_core::{
    ApiKey, Continuation, Delta, Header, HeaderKey, Message, RequestPurpose, ToolArgs, ToolCall,
    ToolId, ToolOutput, ToolResult, Transcript,
};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::sync::Arc;

fn blocks() -> Vec<Value> {
    vec![
        json!({"type":"thinking","thinking":"","signature":"first-private-signature"}),
        json!({"type":"text","text":"before"}),
        json!({"type":"tool_use","id":"call-1","name":"read","input":{"path":"a"}}),
        json!({"type":"thinking","thinking":"private-summary","signature":"second-private-signature"}),
        json!({"type":"text","text":"after"}),
    ]
}

fn response(blocks: &[Value]) -> String {
    let mut events = vec![json!({"type":"message_start","message":{"id":"msg-fixture"}})];
    for (index, block) in blocks.iter().enumerate() {
        events.push(json!({"type":"content_block_start","index":index,"content_block":block}));
        events.push(json!({"type":"content_block_stop","index":index}));
    }
    events.push(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}));
    events.push(json!({"type":"message_stop"}));
    sse(&events)
}

fn sse(events: &[Value]) -> String {
    let mut payload = String::new();
    for event in events {
        write!(
            payload,
            "event: {}\ndata: {event}\n\n",
            event.get("type").unwrap().as_str().unwrap()
        )
        .unwrap();
    }
    payload
}

fn provider(body: &str) -> (Anthropic, Arc<Replay>) {
    let replay = Arc::new(Replay::new(200, body));
    let credential = HeaderKey::new(
        ApiKey::new("synthetic-fable-key"),
        Header::bare("x-api-key"),
    );
    (
        Anthropic::at(VENDOR, Box::new(credential), Box::new(Arc::clone(&replay))),
        replay,
    )
}

fn request(transcript: &Transcript) -> Request<'_> {
    Request {
        model: FABLE_51,
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

fn answer(provider: &Anthropic, transcript: &Transcript) -> Message {
    answer_with(provider, request(transcript))
}

fn answer_with(provider: &Anthropic, request: Request<'_>) -> Message {
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
                let last = calls.last_mut().unwrap();
                last.args = ToolArgs::new(format!("{}{more}", last.args.as_str()));
            }
            Delta::Continuation(state) => assert!(pending.replace(state).is_none()),
            Delta::Stopped(reason) => stop = Some(reason),
            _ => {}
        }
    }
    assert_eq!(text, "beforeafter");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls.first().unwrap().args.as_str(), r#"{"path":"a"}"#);
    let continuation = pending
        .expect("complete Fable response must retain its private blocks")
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
fn fable_51_does_not_assume_a_future_claude_model_has_compatible_signatures() {
    let (provider, replay) = provider(&response(&blocks()));
    let mut history = Transcript::new();
    history.push(Message::said("read a")).unwrap();
    let Message::Agent {
        text,
        calls,
        stop,
        continuation: Some(original),
    } = answer(&provider, &history)
    else {
        panic!("missing native state")
    };
    let mut state =
        Continuation::new(original.protocol(), "claude-fable-99", original.scope()).unwrap();
    for part in original.parts() {
        state.push(part.clone()).unwrap();
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
    provider.stream(request(&history), &Cancel::new()).unwrap();
    let sent = replay.sent();
    assert!(
        !sent.body.contains("private"),
        "unknown model signatures crossed the compatibility boundary"
    );
    assert!(sent.body.contains("Historical tool request"));
}

#[test]
fn fable_51_thinking_cannot_change_after_its_signature_has_started() {
    for started in ["already-signed", ""] {
        let payload = sse(&[
            json!({"type":"message_start","message":{}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":started}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"private-signature"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"late-private-thinking"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
            json!({"type":"message_stop"}),
        ]);
        let (provider, _) = provider(&payload);
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
                    panic!("invalid signed thinking must not be retained")
                }
                _ => {}
            }
        }
        let error = error.expect("thinking after a signature must fail");
        assert!(!format!("{error:?} {error}").contains("private"));
    }
}

#[test]
fn fable_51_replays_signature_only_thinking_text_and_calls_in_original_order() {
    let expected = blocks();
    let (provider, replay) = provider(&response(&expected));
    let mut transcript = Transcript::new();
    transcript.push(Message::said("read a")).unwrap();
    let agent = answer(&provider, &transcript);
    transcript.push(agent).unwrap();
    transcript.push(answered()).unwrap();
    provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    let sent: Value = serde_json::from_str(&replay.sent().body).unwrap();
    assert_eq!(
        sent.pointer("/messages/1/content"),
        Some(&Value::Array(expected))
    );
    assert_eq!(
        sent.pointer("/messages/2/content/0/tool_use_id")
            .and_then(Value::as_str),
        Some("call-1")
    );
}

#[test]
fn fable_51_compaction_removes_old_thinking_but_preserves_new_thinking() {
    let (provider, replay) = provider(&response(&blocks()));
    let mut transcript = Transcript::new();
    transcript.push(Message::said("old context")).unwrap();
    transcript.push(Message::said("read a")).unwrap();
    transcript.push(answer(&provider, &transcript)).unwrap();
    transcript.push(answered()).unwrap();
    transcript.compacted(1, "recap");
    provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    let sent: Value = serde_json::from_str(&replay.sent().body).unwrap();
    let kept: Vec<_> = blocks()
        .into_iter()
        .filter(|block| block.get("type").and_then(Value::as_str) != Some("thinking"))
        .collect();
    assert_eq!(
        sent.pointer("/messages/2/content"),
        Some(&Value::Array(kept))
    );
    transcript.push(answer(&provider, &transcript)).unwrap();
    transcript.push(answered()).unwrap();
    provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    let sent: Value = serde_json::from_str(&replay.sent().body).unwrap();
    assert_eq!(
        sent.pointer("/messages/4/content"),
        Some(&Value::Array(blocks()))
    );
}

#[test]
fn fable_51_effort_changes_append_controls_without_rewriting_the_cached_prefix() {
    use crucible_core::Effort;
    let (provider, replay) = provider(&response(&blocks()));
    let mut transcript = Transcript::new();
    transcript.push(Message::said("read a")).unwrap();
    let first = answer_with(
        &provider,
        Request {
            effort: Some(Effort::High),
            ..request(&transcript)
        },
    );
    transcript.push(first).unwrap();
    transcript.push(answered()).unwrap();
    let mut previous: Option<Vec<Value>> = None;
    for effort in [
        Some(Effort::Low),
        Some(Effort::Medium),
        Some(Effort::Xhigh),
        Some(Effort::Max),
        None,
    ] {
        let agent = answer_with(
            &provider,
            Request {
                effort,
                ..request(&transcript)
            },
        );
        let sent = replay.sent();
        assert!(sent.headers.iter().any(|(key, value)| {
            key == "anthropic-beta"
                && value
                    .split(',')
                    .any(|part| part.trim() == "mid-conversation-output-config-2026-07-01")
        }));
        let body: Value = serde_json::from_str(&sent.body).unwrap();
        assert_eq!(
            body.pointer("/output_config/effort")
                .and_then(Value::as_str),
            Some("high")
        );
        let messages = body.get("messages").unwrap().as_array().unwrap();
        if let Some(previous) = previous {
            assert_eq!(messages.get(..previous.len()), Some(previous.as_slice()));
        }
        let control = messages
            .iter()
            .rev()
            .find(|message| message.get("role").and_then(Value::as_str) == Some("system"))
            .unwrap();
        assert_eq!(
            control,
            &json!({"role":"system","content":[],"output_config":{"effort":effort.unwrap_or(Effort::High).as_str()}})
        );
        previous = Some(messages.clone());
        transcript.push(agent).unwrap();
        transcript.push(answered()).unwrap();
    }
}

#[test]
fn fable_51_transient_errors_are_retryable_without_echoing_private_payloads() {
    let payload = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"private-thinking-signature\"}}\n\n";
    let (provider, _) = provider(payload);
    let transcript = Transcript::new();
    let mut stream = provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    let error = stream.next().unwrap().unwrap_err();
    assert!(error.transient());
    assert!(!format!("{error:?} {error}").contains("private-thinking-signature"));
    assert!(stream.next().is_none());
}

#[test]
fn fable_51_http_refusal_keeps_status_but_not_echoed_signed_history() {
    let credential = HeaderKey::new(
        ApiKey::new("synthetic-fable-key"),
        Header::bare("x-api-key"),
    );
    let replay = Replay::new(
        400,
        r#"{"error":{"type":"invalid_request_error","message":"private-thinking-signature"}}"#,
    );
    let provider = Anthropic::at(VENDOR, Box::new(credential), Box::new(replay));
    let transcript = Transcript::new();
    let Err(error) = provider.stream(request(&transcript), &Cancel::new()) else {
        panic!("HTTP refusal must fail");
    };
    assert!(matches!(error, ProviderError::Refused { status: 400, .. }));
    assert!(!format!("{error:?} {error}").contains("private-thinking-signature"));
}

#[test]
fn fable_51_dropped_thinking_reports_only_fixed_numeric_facts_and_replaces_fallback_counts() {
    let payload = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"input_transformations\":[{\"type\":\"thinking_dropped\",\"reason\":\"prefix_binding_mismatch\",\"path\":\"private-thinking-signature\"}]}}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"input_transformations\":[{\"type\":\"thinking_dropped\",\"reason\":\"model_binding_mismatch\",\"path\":\"private-thinking-signature\"},{\"type\":\"future-check\",\"reason\":\"private-thinking-signature\"}]}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    );
    let (provider, _) = provider(payload);
    let transcript = Transcript::new();
    let mut stream = provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    let mut reports = Vec::new();
    while let Some(delta) = stream.next() {
        let delta = delta.unwrap();
        assert!(!format!("{delta:?}").contains("private-thinking-signature"));
        if let Delta::Usage(usage) = delta {
            reports.push(usage);
        }
    }
    assert_eq!(reports.len(), 2);
    let report = reports
        .first()
        .unwrap()
        .merged(reports.get(1).unwrap())
        .unwrap();
    assert_eq!(report.input, crucible_core::InputTokenUsage::UNKNOWN);
    assert_eq!(report.output, None);
    assert_eq!(
        report.details(),
        &[
            crucible_core::ProviderNumericDetail {
                label: "thinking_dropped_prefix",
                value: 0
            },
            crucible_core::ProviderNumericDetail {
                label: "thinking_dropped_model",
                value: 1
            },
        ]
    );
}

#[test]
fn fable_51_thinking_usage_is_an_output_subset_never_an_extra_charge() {
    for thinking in [0, 312, 348, 349] {
        let payload = format!(
            "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{}}}}\n\nevent: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":348,\"output_tokens_details\":{{\"thinking_tokens\":{thinking}}}}}}}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
        );
        let (provider, _) = provider(&payload);
        let transcript = Transcript::new();
        let mut stream = provider
            .stream(request(&transcript), &Cancel::new())
            .unwrap();
        let mut usage = None;
        let mut failed = false;
        while let Some(delta) = stream.next() {
            match delta {
                Ok(Delta::Usage(report)) => usage = Some(report),
                Err(_) => failed = true,
                _ => {}
            }
        }
        assert_eq!(failed, thinking > 348);
        if thinking <= 348 {
            let usage = usage.unwrap();
            assert_eq!(usage.output, Some(348));
            assert_eq!(usage.reasoning, Some(thinking));
            assert_eq!(usage.total, None);
        }
    }
}

#[test]
fn fable_51_fragmented_thinking_calls_and_document_citations_round_trip_together() {
    let citation = json!({"type":"page_location","document_index":0,"document_title":"guide","start_page_number":1,"end_page_number":2,"cited_text":"quoted source"});
    let mut expected = blocks();
    expected
        .get_mut(1)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("citations".into(), json!([citation]));
    let mut events = vec![
        json!({"type":"message_start","message":{}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"first-private-"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signature"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":"","citations":[]}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"before"}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"citations_delta","citation":citation}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"call-1","name":"read","input":{}}}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"a\"}"}}),
        json!({"type":"content_block_stop","index":2}),
    ];
    for (index, block) in expected.iter().enumerate().skip(3) {
        events.push(json!({"type":"content_block_start","index":index,"content_block":block}));
        events.push(json!({"type":"content_block_stop","index":index}));
    }
    events.push(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}));
    events.push(json!({"type":"message_stop"}));
    let payload = sse(&events);
    let (provider, replay) = provider(&payload);
    let mut transcript = Transcript::new();
    transcript.push(Message::said("read a")).unwrap();
    transcript.push(answer(&provider, &transcript)).unwrap();
    transcript.push(answered()).unwrap();
    provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    let sent: Value = serde_json::from_str(&replay.sent().body).unwrap();
    assert_eq!(
        sent.pointer("/messages/1/content"),
        Some(&Value::Array(expected))
    );
}

#[test]
fn fable_51_ignores_additive_sse_events_without_parsing_their_payload() {
    let payload = format!(
        "event: proxy-heartbeat\ndata: not-json\n\n{}",
        response(&blocks())
    );
    let (provider, _) = provider(&payload);
    let transcript = Transcript::new();
    answer(&provider, &transcript);
}

#[test]
fn fable_51_cache_prices_and_capabilities_use_exact_reviewed_model_and_route() {
    let (provider, _) = provider("");
    let capabilities = provider.prompt_cache_capabilities(FABLE_51);
    assert_eq!(
        capabilities.support(),
        crucible_core::PromptCacheSupport::Supported
    );
    assert!(
        capabilities
            .mechanisms()
            .iter()
            .all(|capability| capability.minimum_prefix_tokens() == 512)
    );
    for (retention, write) in [
        (PromptCacheRetentionClass::ProviderDefault, 12_500_000_000),
        (PromptCacheRetentionClass::Ephemeral, 12_500_000_000),
        (PromptCacheRetentionClass::Extended, 20_000_000_000),
    ] {
        let price = provider
            .prompt_cache_pricing(
                FABLE_51,
                Some(FABLE_51),
                Some(999_000),
                retention,
                PricingDate::new(2026, 9, 6),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            price.rates().uncached_input,
            UsageRate::priced(PriceRate::per_million(10_000_000_000))
        );
        assert_eq!(
            price.rates().cache_read,
            UsageRate::priced(PriceRate::per_million(250_000_000))
        );
        assert_eq!(
            price.rates().cache_write_or_creation,
            UsageRate::priced(PriceRate::per_million(write))
        );
        assert_eq!(
            price.rates().output,
            UsageRate::priced(PriceRate::per_million(50_000_000_000))
        );
    }
    for (model, revision, at) in [
        (
            FABLE_51,
            Some("claude-fable-5"),
            PricingDate::new(2026, 9, 6),
        ),
        (
            "claude-fable-5-1-custom",
            Some(FABLE_51),
            PricingDate::new(2026, 9, 6),
        ),
        (FABLE_51, Some(FABLE_51), PricingDate::new(2026, 8, 31)),
    ] {
        assert!(
            provider
                .prompt_cache_pricing(
                    model,
                    revision,
                    Some(1000),
                    PromptCacheRetentionClass::ProviderDefault,
                    at
                )
                .unwrap()
                .is_none()
        );
    }
    let custom = Anthropic::at(
        Endpoint::parse("https://proxy.invalid/v1/messages").unwrap(),
        Box::new(HeaderKey::new(
            ApiKey::new("synthetic-fable-key"),
            Header::bare("x-api-key"),
        )),
        Box::new(Replay::new(200, "")),
    );
    assert_eq!(
        custom.prompt_cache_capabilities(FABLE_51).support(),
        crucible_core::PromptCacheSupport::Unknown
    );
    assert!(
        custom
            .prompt_cache_pricing(
                FABLE_51,
                Some(FABLE_51),
                Some(1000),
                PromptCacheRetentionClass::ProviderDefault,
                PricingDate::new(2026, 9, 6)
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn fable_51_explicit_cache_marker_falls_back_before_thinking_only_history() {
    let payload = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"private-thinking-signature\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    );
    let (provider, replay) = provider(payload);
    let mut transcript = Transcript::new();
    transcript.push(Message::said("earlier")).unwrap();
    let mut stream = provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    let mut pending = None;
    while let Some(delta) = stream.next() {
        if let Delta::Continuation(state) = delta.unwrap() {
            pending = Some(state);
        }
    }
    let stop = Some(crucible_core::StopReason::Yielded);
    let continuation = pending.unwrap().finish("", 0, stop).unwrap();
    transcript
        .push(Message::Agent {
            text: "".into(),
            calls: vec![],
            stop,
            continuation: Some(continuation),
        })
        .unwrap();
    transcript.push(Message::said("current")).unwrap();
    let transcript = Box::leak(Box::new(transcript));
    let request = crate::fake::cached(
        request(transcript),
        crucible_core::PromptCacheMechanism::ExplicitBreakpoints,
        PromptCacheRetentionClass::ProviderDefault,
        false,
    );
    assert_eq!(
        provider.prompt_cache_encoding(&request),
        crucible_core::PromptCacheEncoding::BreakpointsEncoded(1)
    );
    provider.stream(request, &Cancel::new()).unwrap();
    let sent: Value = serde_json::from_str(&replay.sent().body).unwrap();
    assert_eq!(
        sent.pointer("/messages/0/content/0/cache_control"),
        Some(&json!({"type":"ephemeral"}))
    );
    assert!(
        sent.pointer("/messages/1/content/0/cache_control")
            .is_none()
    );
}

#[test]
fn fable_51_empty_text_blocks_are_not_replayed_as_invalid_input_or_cache_targets() {
    let mut output = blocks();
    output.push(json!({"type":"text","text":""}));
    let (provider, replay) = provider(&response(&output));
    let mut transcript = Transcript::new();
    transcript.push(Message::said("read a")).unwrap();
    transcript.push(answer(&provider, &transcript)).unwrap();
    transcript.push(answered()).unwrap();
    provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    let sent: Value = serde_json::from_str(&replay.sent().body).unwrap();
    assert_eq!(
        sent.pointer("/messages/1/content"),
        Some(&Value::Array(blocks()))
    );
}

#[test]
fn fable_51_private_admission_depends_on_retained_bytes_not_network_fragment_count() {
    use super::continuation::Blocks;
    use crate::sse::SseEvent;
    let scope = crucible_core::ContinuationScope::from_digest([0; 32]);
    let event = |name: &str, data: Value| SseEvent {
        name: name.into(),
        data: data.to_string(),
    };
    let mut blocks = Blocks::new(FABLE_51, scope, None).unwrap();
    blocks
        .deltas(&event("message_start", json!({"message":{}})))
        .unwrap();
    blocks
        .deltas(&event(
            "content_block_start",
            json!({"index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        ))
        .unwrap();
    let fragment = event(
        "content_block_delta",
        json!({"index":0,"delta":{"type":"thinking_delta","thinking":"x"}}),
    );
    for _ in 0..40_000 {
        blocks.deltas(&fragment).unwrap();
    }
    blocks
        .deltas(&event(
            "content_block_delta",
            json!({"index":0,"delta":{"type":"signature_delta","signature":"signature"}}),
        ))
        .unwrap();
    blocks
        .deltas(&event("content_block_stop", json!({"index":0})))
        .unwrap();
    blocks
        .deltas(&event(
            "message_delta",
            json!({"delta":{"stop_reason":"end_turn"}}),
        ))
        .unwrap();
    let complete = blocks.deltas(&event("message_stop", json!({}))).unwrap();
    let state = complete
        .into_iter()
        .find_map(|delta| match delta {
            Delta::Continuation(state) => Some(state),
            _ => None,
        })
        .unwrap();
    let state = state
        .finish("", 0, Some(crucible_core::StopReason::Yielded))
        .unwrap();
    let crucible_core::ContinuationPart::Opaque(data) = state.parts().get(1).unwrap() else {
        panic!("thinking must remain private");
    };
    assert_eq!(
        serde_json::from_str::<Value>(data.as_str())
            .unwrap()
            .get("thinking"),
        Some(&Value::String("x".repeat(40_000)))
    );
}

#[test]
fn fable_51_aggregate_text_and_argument_limits_accept_exactly_the_boundary() {
    for excess in [0, 1] {
        let arguments = (0..2)
            .map(|index| {
                json!({
                    "type":"tool_use", "id":format!("call-{index}"), "name":"read",
                    "input":{"x":"x".repeat(512 * 1024 - 8 + if index == 0 {excess} else {0})}
                })
            })
            .collect::<Vec<_>>();
        let (provider, _) = provider(&response(&arguments));
        let transcript = Transcript::new();
        let mut stream = provider
            .stream(request(&transcript), &Cancel::new())
            .unwrap();
        let mut error = false;
        while let Some(delta) = stream.next() {
            if delta.is_err() {
                error = true;
                break;
            }
        }
        assert_eq!(error, excess != 0, "aggregate argument admission");

        let mut events = vec![json!({"type":"message_start","message":{}})];
        for index in 0..16 {
            events.push(json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":"x".repeat(512 * 1024 + if index == 0 {excess} else {0})}}));
            events.push(json!({"type":"content_block_stop","index":index}));
        }
        events.push(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}));
        events.push(json!({"type":"message_stop"}));
        let (provider, _) = self::provider(&sse(&events));
        let mut stream = provider
            .stream(request(&transcript), &Cancel::new())
            .unwrap();
        let mut error = false;
        while let Some(delta) = stream.next() {
            if delta.is_err() {
                error = true;
                break;
            }
        }
        assert_eq!(error, excess != 0, "aggregate text admission");
    }
}
