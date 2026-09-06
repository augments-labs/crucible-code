//! Transport-facing fixtures exercise the real body and streamed response.

use super::*;
use crate::transport::Replay;
use crucible_core::{
    ApiKey, Delta, Effort, Header, HeaderKey, Message, RequestPurpose, StopReason, Transcript,
};
use serde_json::Value;
use std::sync::Arc;

const ANSWER: &str = "event: step.start\ndata: {\"event_type\":\"step.start\",\"index\":0,\"step\":{\"type\":\"model_output\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n\nevent: step.stop\ndata: {\"event_type\":\"step.stop\",\"index\":0}\n\nevent: interaction.completed\ndata: {\"event_type\":\"interaction.completed\",\"interaction\":{\"status\":\"completed\"}}\n\n";
const SECRET: &str = "google-test-key-do-not-log";

fn provider(status: u16, body: &str) -> (Google, Arc<Replay>) {
    let replay = Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bare("x-goog-api-key"));
    (
        Google::at(VENDOR, Box::new(credential), Box::new(Arc::clone(&replay))),
        replay,
    )
}

fn request(transcript: &Transcript) -> Request<'_> {
    Request {
        purpose: RequestPurpose::Turn,
        model: "gemini-3.8-flash",
        transcript,
        tools: &[],
        max_tokens: 8192,
        system: None,
        effort: None,
        attached: &[],
        prompt_cache: None,
    }
}

#[test]
fn google_accepts_the_documented_trailing_done_marker() {
    let (provider, _) = provider(200, &format!("{ANSWER}event: done\ndata: [DONE]\n\n"));
    let transcript = Transcript::new();
    let mut stream = provider
        .stream(request(&transcript), &Cancel::new())
        .unwrap();
    let mut continuation = 0;
    let mut stopped = 0;
    while let Some(delta) = stream.next() {
        match delta.unwrap() {
            Delta::Continuation(_) => continuation += 1,
            Delta::Stopped(StopReason::Yielded) => stopped += 1,
            _ => {}
        }
    }
    assert_eq!(continuation, 1);
    assert_eq!(stopped, 1);
}

#[test]
fn google_done_markers_cannot_hide_incomplete_or_contradictory_streams() {
    for body in [
        "event: done\ndata: [DONE]\n\n".to_owned(),
        format!("{ANSWER}event: done\ndata: [DONE]\n\nevent: done\ndata: [DONE]\n\n"),
        format!(
            "{ANSWER}event: done\ndata: [DONE]\n\nevent: error\ndata: {{\"event_type\":\"error\",\"error\":{{\"message\":\"private-canary\"}}}}\n\n"
        ),
    ] {
        let (provider, _) = provider(200, &body);
        let transcript = Transcript::new();
        let mut stream = provider
            .stream(request(&transcript), &Cancel::new())
            .unwrap();
        let mut failed = false;
        while let Some(delta) = stream.next() {
            if let Err(error) = delta {
                assert!(!format!("{error:?}").contains("private-canary"));
                failed = true;
            }
        }
        assert!(failed);
    }
}

#[test]
fn google_refusals_never_echo_private_request_or_response_payloads() {
    let transcript = Transcript::new();
    for status in [400, 401, 403, 404, 408, 429, 503] {
        let (provider, _) = provider(
            status,
            &format!(
                r#"{{"error":{{"code":"invalid_request","message":"private-signature-canary {SECRET}"}}}}"#
            ),
        );
        let Err(error) = provider.stream(request(&transcript), &Cancel::new()) else {
            panic!("refusal returned a stream");
        };
        assert!(!format!("{error:?} {error}").contains("private-signature-canary"));
        assert!(!format!("{error:?} {error}").contains(SECRET));
        assert!(matches!(error, ProviderError::Refused { status: actual, .. } if actual == status));
        assert_eq!(error.transient(), matches!(status, 408 | 429 | 503));
    }
    let (provider, _) = provider(
        400,
        r#"{"error":{"code":"context_length_exceeded","message":"private-signature-canary"}}"#,
    );
    assert!(matches!(
        provider.stream(request(&transcript), &Cancel::new()),
        Err(ProviderError::WindowExceeded { .. })
    ));
}

#[test]
fn google_posts_the_exact_key_only_sse_route_and_all_valid_efforts() {
    let (provider, replay) = provider(200, ANSWER);
    let mut transcript = Transcript::new();
    transcript.push(Message::said("hi")).unwrap();
    for effort in [
        None,
        Some(Effort::Low),
        Some(Effort::Medium),
        Some(Effort::High),
    ] {
        let mut stream = provider
            .stream(
                Request {
                    effort,
                    ..request(&transcript)
                },
                &Cancel::new(),
            )
            .unwrap();
        let mut deltas = Vec::new();
        while let Some(delta) = stream.next() {
            deltas.push(delta.unwrap());
        }
        assert!(matches!(deltas.first(),Some(Delta::Text(s)) if s.as_ref()=="hello"));
        assert!(matches!(
            deltas.last(),
            Some(Delta::Stopped(StopReason::Yielded))
        ));
        let sent = replay.sent();
        assert_eq!(
            sent.url,
            "https://generativelanguage.googleapis.com/v1beta/interactions?alt=sse"
        );
        assert!(
            sent.headers
                .contains(&("x-goog-api-key".into(), SECRET.into()))
        );
        assert!(
            sent.headers
                .contains(&("accept".into(), "text/event-stream".into()))
        );
        assert!(
            sent.headers
                .iter()
                .all(|(key, _)| !key.eq_ignore_ascii_case("authorization"))
        );
        assert!(!sent.body.contains(SECRET));
        let body: Value = serde_json::from_str(&sent.body).unwrap();
        assert_eq!(body.get("store").and_then(Value::as_bool), Some(false));
        assert_eq!(body.get("stream").and_then(Value::as_bool), Some(true));
        assert_eq!(
            body.pointer("/generation_config/thinking_level")
                .and_then(Value::as_str),
            effort.map(Effort::as_str)
        );
    }
}

#[test]
fn google_cache_and_prices_use_exact_model_date_and_long_context_tiers() {
    use crucible_core::{
        PriceRate, PricingDate, PromptCacheMechanism, PromptCacheRetentionClass, UsageRate,
    };
    let (mut provider, _) = provider(200, ANSWER);
    let retention = PromptCacheRetentionClass::ProviderDefault;
    let now = PricingDate::new(2026, 9, 6);
    for model in [
        "gemini-3.8-flash",
        "gemini-3.7-flash",
        "gemini-3.6-flash",
        "gemini-3.1-pro-preview",
    ] {
        let caps = provider.prompt_cache_capabilities(model);
        assert_eq!(caps.mechanisms().len(), 1);
        assert_eq!(
            caps.mechanisms().first().unwrap().mechanism(),
            PromptCacheMechanism::AutomaticPrefix
        );
        assert_eq!(
            caps.mechanisms().first().unwrap().minimum_prefix_tokens(),
            4096
        );
        for (date, tokens, input, read, output) in if model == "gemini-3.1-pro-preview" {
            vec![
                (now, 200_000, 2_000_000_000, 200_000_000, 12_000_000_000),
                (now, 200_001, 4_000_000_000, 400_000_000, 18_000_000_000),
            ]
        } else {
            vec![
                (
                    PricingDate::new(2026, 12, 31),
                    500_000,
                    750_000_000,
                    75_000_000,
                    3_750_000_000,
                ),
                (
                    PricingDate::new(2027, 1, 1),
                    500_000,
                    1_500_000_000,
                    150_000_000,
                    7_500_000_000,
                ),
            ]
        } {
            let price = provider
                .prompt_cache_pricing(model, Some(model), Some(tokens), retention, date)
                .unwrap()
                .unwrap();
            assert_eq!(
                price.rates().uncached_input,
                UsageRate::priced(PriceRate::per_million(input))
            );
            assert_eq!(
                price.rates().cache_read,
                UsageRate::priced(PriceRate::per_million(read))
            );
            assert_eq!(
                price.rates().output,
                UsageRate::priced(PriceRate::per_million(output))
            );
        }
        assert!(
            provider
                .prompt_cache_pricing(model, Some(model), None, retention, now)
                .unwrap()
                .is_none()
        );
        assert!(
            provider
                .prompt_cache_pricing(
                    model,
                    Some("different-revision"),
                    Some(5000),
                    retention,
                    now
                )
                .unwrap()
                .is_none()
        );
    }
    provider.endpoint = Endpoint::parse("https://gateway.example/interactions").unwrap();
    assert!(
        provider
            .prompt_cache_capabilities("gemini-3.8-flash")
            .mechanisms()
            .is_empty()
    );
    assert!(
        provider
            .prompt_cache_pricing(
                "gemini-3.8-flash",
                Some("gemini-3.8-flash"),
                Some(5000),
                retention,
                now
            )
            .unwrap()
            .is_none()
    );
}
#[test]
fn debug_never_delegates_to_a_transport_holding_private_history() {
    let replay = Arc::new(Replay::new(200, "private-transport-canary"));
    let provider = Google::at(
        Google::VENDOR,
        Box::new(HeaderKey::new(
            ApiKey::new(SECRET),
            Header::bare("x-goog-api-key"),
        )),
        Box::new(replay),
    );
    assert!(!format!("{provider:?}").contains("private-transport-canary"));
}
