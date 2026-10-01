//! Google's fast form: `service_tier: "priority"` on the models the vendor
//! lists for it, at its own address, and the tier each answer says it was
//! served at. No refusal of it is documented, so none is taken for one.

use super::*;
use crate::transport::Replay;
use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::{FastForm, RequestPurpose, Served, Speed};
use crucible_types::Transcript;
use serde_json::Value;
use std::sync::Arc;

/// An answer whose completed interaction says it was served at `tier`, or
/// says nothing where `tier` is empty.
fn answered(tier: &str) -> String {
    let said = if tier.is_empty() {
        String::new()
    } else {
        format!(",\"service_tier\":\"{tier}\"")
    };
    format!(
        "event: step.start\ndata: {{\"event_type\":\"step.start\",\"index\":0,\"step\":{{\"type\":\"model_output\",\"content\":[{{\"type\":\"text\",\"text\":\"hello\"}}]}}}}\n\n\
         event: step.stop\ndata: {{\"event_type\":\"step.stop\",\"index\":0}}\n\n\
         event: interaction.completed\ndata: {{\"event_type\":\"interaction.completed\",\"interaction\":{{\"status\":\"completed\"{said}}}}}\n\n"
    )
}

fn google(endpoint: Endpoint, status: u16, body: &str) -> (Google, Arc<Replay>) {
    let replay = Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(
        ApiKey::new("synthetic-fast-key"),
        Header::bare("x-goog-api-key"),
    );
    (
        Google::at(
            endpoint,
            Box::new(credential),
            Box::new(Arc::clone(&replay)),
        ),
        replay,
    )
}

fn asking(model: &'static str) -> Request<'static> {
    Request {
        purpose: RequestPurpose::Turn,
        model,
        transcript: Box::leak(Box::new(Transcript::new())),
        tools: &[],
        max_tokens: 8192,
        system: None,
        effort: None,
        attached: &[],
        prompt_cache: None,
    }
}

/// The tier the request asked for, or `None` where it named none.
fn tier_sent(endpoint: Endpoint, model: &'static str, speed: Speed) -> Option<String> {
    let (provider, replay) = google(endpoint, 200, &answered("standard"));
    let cancel = Cancel::new();
    let _ = crucible_runtime::answered!(provider.stream_at(asking(model), speed, &cancel));
    let body: Value = serde_json::from_str(&replay.sent().body).unwrap();
    body.get("service_tier")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[test]
fn a_fast_request_asks_for_the_priority_tier_and_a_standard_one_names_none() {
    for model in [
        "gemini-3.8-flash",
        "gemini-3.7-flash",
        "gemini-3.6-flash",
        "gemini-3.1-pro-preview",
    ] {
        assert_eq!(
            tier_sent(VENDOR, model, Speed::Fast).as_deref(),
            Some("priority"),
            "{model}"
        );
        assert_eq!(tier_sent(VENDOR, model, Speed::Standard), None, "{model}");
    }
}

#[test]
fn a_model_the_vendor_does_not_list_and_a_configured_address_ask_for_nothing() {
    assert_eq!(tier_sent(VENDOR, "gemini-unlisted", Speed::Fast), None);
    assert_eq!(
        tier_sent(
            Endpoint::fixed("https://gateway.example/v1beta/interactions"),
            "gemini-3.8-flash",
            Speed::Fast
        ),
        None
    );
}

#[test]
fn only_the_models_the_vendor_lists_have_a_fast_form() {
    let (vendor, _) = google(VENDOR, 200, "");
    let (gateway, _) = google(
        Endpoint::fixed("https://gateway.example/v1beta/interactions"),
        200,
        "",
    );
    assert!(matches!(
        vendor.fast("gemini-3.8-flash"),
        FastForm::Field(_)
    ));
    assert_eq!(vendor.fast("gemini-unlisted"), FastForm::None);
    assert_eq!(gateway.fast("gemini-3.8-flash"), FastForm::None);
}

#[test]
fn the_tier_served_is_the_one_the_response_header_says() {
    // The vendor says to watch the header for a request served at standard;
    // the body says the other tier here, so only the header gives the answer.
    for (header, body, served) in [
        (Some("priority"), "standard", Served::Fast),
        (Some("standard"), "priority", Served::Standard),
        (None, "priority", Served::Unsaid),
    ] {
        let replay = Replay::new(200, answered(body));
        let replay = match header {
            Some(tier) => replay.tiered(tier),
            None => replay,
        };
        let provider = Google::at(
            VENDOR,
            Box::new(HeaderKey::new(
                ApiKey::new("synthetic-fast-key"),
                Header::bare("x-goog-api-key"),
            )),
            Box::new(Arc::new(replay)),
        );
        let cancel = Cancel::new();
        let mut stream = crucible_runtime::answered!(provider.stream_at(
            asking("gemini-3.8-flash"),
            Speed::Fast,
            &cancel
        ))
        .unwrap();
        while let Some(delta) = crucible_runtime::answered!(stream.next()) {
            delta.unwrap();
        }
        assert_eq!(stream.served(), served, "{header:?}");
    }
}

#[test]
fn no_refusal_of_a_fast_request_is_taken_for_a_refusal_of_fast() {
    let (provider, _) = google(
        VENDOR,
        400,
        r#"{"error":{"code":400,"message":"priority is not available for this project"}}"#,
    );
    let cancel = Cancel::new();
    let answer = crucible_runtime::answered!(provider.stream_at(
        asking("gemini-3.8-flash"),
        Speed::Fast,
        &cancel
    ));
    assert!(
        matches!(answer, Err(ProviderError::Refused { .. })),
        "{:?}",
        answer.err()
    );
}
