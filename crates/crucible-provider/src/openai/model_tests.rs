//! Exact Astra request and metered-cache facts across the two Responses routes.

use super::*;
use crate::transport::Replay;
use crucible_core::{
    ApiKey, Delta, Effort, Header, HeaderKey, Message, PromptCacheEncoding, PromptCacheMechanism,
    PromptCacheSupport, RequestPurpose, Transcript,
};
use serde_json::{Value, json};
use std::sync::Arc;

const MODEL: &str = "gpt-6-astra";

fn provider(endpoint: Endpoint, response: &str) -> (OpenAi, Arc<Replay>) {
    let replay = Arc::new(Replay::new(200, response));
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

fn request() -> Request<'static> {
    let mut transcript = Transcript::new();
    transcript.push(Message::said("hello")).unwrap();
    Request {
        model: MODEL,
        purpose: RequestPurpose::Turn,
        transcript: Box::leak(Box::new(transcript)),
        tools: &[],
        system: None,
        max_tokens: 8192,
        effort: None,
        attached: &[],
        prompt_cache: None,
    }
}

#[test]
fn astra_exact_rates_and_capabilities_respect_route_revision_retention_and_long_input() {
    let date = PricingDate::new(2026, 9, 6);
    for endpoint in [
        VENDOR,
        SUBSCRIPTION,
        Endpoint::fixed("https://proxy.invalid/responses"),
    ] {
        let (provider, _) = provider(endpoint.clone(), "");
        let capability = provider.prompt_cache_capabilities(MODEL);
        if endpoint != VENDOR && endpoint != SUBSCRIPTION {
            assert_eq!(capability.support(), PromptCacheSupport::Unknown);
        } else {
            assert_eq!(capability.support(), PromptCacheSupport::Supported);
            assert_eq!(
                capability.mechanisms().len(),
                if endpoint == VENDOR { 2 } else { 1 }
            );
            assert!(
                capability
                    .mechanisms()
                    .iter()
                    .all(|item| item.minimum_prefix_tokens() == 1024)
            );
            assert_eq!(
                capability.usage(),
                PromptCacheUsageReporting::ReadAndWriteTokens
            );
        }
        for (tokens, multiplier, output) in
            [(272_000, 1, 50_000_000_000), (272_001, 2, 75_000_000_000)]
        {
            let price = provider
                .prompt_cache_pricing(
                    MODEL,
                    Some(MODEL),
                    Some(tokens),
                    PromptCacheRetentionClass::ProviderDefault,
                    date,
                )
                .unwrap();
            if endpoint != VENDOR {
                assert!(price.is_none());
                continue;
            }
            let price = price.unwrap();
            assert_eq!(
                price.rates().uncached_input,
                rate(multiplier * 10_000_000_000)
            );
            assert_eq!(price.rates().cache_read, rate(multiplier * 1_000_000_000));
            assert_eq!(
                price.rates().cache_write_or_creation,
                rate(multiplier * 12_500_000_000)
            );
            assert_eq!(price.rates().output, rate(output));
        }
        for (revision, retention, at) in [
            (
                Some("gpt-5.6-sol"),
                PromptCacheRetentionClass::ProviderDefault,
                date,
            ),
            (Some(MODEL), PromptCacheRetentionClass::Extended, date),
            (
                Some(MODEL),
                PromptCacheRetentionClass::ProviderDefault,
                PricingDate::new(2026, 8, 31),
            ),
        ] {
            assert!(
                provider
                    .prompt_cache_pricing(MODEL, revision, Some(1000), retention, at)
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(
            provider
                .prompt_cache_capabilities("gpt-6-astra-custom")
                .support(),
            PromptCacheSupport::Unknown
        );
    }
}

#[test]
fn astra_uses_current_cache_options_and_preserves_every_effort_and_route_output_budget() {
    for endpoint in [VENDOR, SUBSCRIPTION] {
        let (provider, replay) = provider(endpoint.clone(), "");
        for effort in [
            None,
            Some(Effort::Low),
            Some(Effort::Medium),
            Some(Effort::High),
            Some(Effort::Xhigh),
            Some(Effort::Max),
        ] {
            let request = crate::fake::cached(
                Request {
                    effort,
                    ..request()
                },
                PromptCacheMechanism::AutomaticPrefix,
                PromptCacheRetentionClass::Ephemeral,
                false,
            );
            assert_eq!(
                provider.prompt_cache_encoding(&request),
                PromptCacheEncoding::AutomaticHintEncoded
            );
            provider.stream(request, &Cancel::new()).unwrap();
            let sent = replay.sent();
            assert_eq!(sent.url, endpoint.as_str());
            let body: Value = serde_json::from_str(&sent.body).unwrap();
            assert_eq!(body.get("model"), Some(&json!(MODEL)));
            assert_eq!(
                body.get("prompt_cache_options"),
                Some(&json!({"mode":"implicit","ttl":"30m"}))
            );
            assert!(body.get("prompt_cache_retention").is_none());
            assert_eq!(
                body.pointer("/reasoning/effort").and_then(Value::as_str),
                effort.map(Effort::as_str)
            );
            assert_eq!(body.get("store"), Some(&Value::Bool(false)));
            assert_eq!(body.get("stream"), Some(&Value::Bool(true)));
            assert_eq!(
                body.get("max_output_tokens").and_then(Value::as_u64),
                (endpoint == VENDOR).then_some(8192)
            );
            for field in [
                "temperature",
                "top_p",
                "top_logprobs",
                "logprobs",
                "previous_response_id",
                "context_management",
            ] {
                assert!(body.get(field).is_none());
            }
            assert!(!sent.body.contains("synthetic-astra-key"));
        }
    }
}

#[test]
fn astra_unreported_cache_writes_are_unknown_not_assumed_free() {
    let response = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-test\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-test\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":100,\"input_tokens_details\":{\"cached_tokens\":20},\"output_tokens\":0,\"total_tokens\":100}}}\n\n",
    );
    let (provider, _) = provider(VENDOR, response);
    let mut stream = provider.stream(request(), &Cancel::new()).unwrap();
    let mut usage = None;
    while let Some(delta) = stream.next() {
        if let Delta::Usage(report) = delta.unwrap() {
            usage = Some(report);
        }
    }
    let usage = usage.unwrap();
    assert_eq!(usage.input.total, Some(100));
    assert_eq!(usage.input.cache_read, Some(20));
    assert_eq!(usage.input.cache_write_or_creation, None);
    assert_eq!(usage.input.uncached, None);
}
