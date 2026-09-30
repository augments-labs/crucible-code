//! OpenAI's fast form: `service_tier: "priority"` on the models that have
//! one, on the vendor's own addresses, and the speed each answer says it was
//! served at.

use super::*;
use crate::transport::Replay;
use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::{FastForm, RequestPurpose, Served, Speed};
use crucible_types::{Message, Transcript};
use serde_json::Value;
use std::sync::Arc;

/// An answer that ends by saying it was served at `tier`, or says nothing
/// where `tier` is empty.
fn answered(tier: &str) -> String {
    let said = if tier.is_empty() {
        String::new()
    } else {
        format!(",\"service_tier\":\"{tier}\"")
    };
    format!(
        "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}}\n\n\
         data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"resp-fast\",\"status\":\"completed\"{said},\"output\":[],\"usage\":{{\"input_tokens\":10,\"output_tokens\":1,\"total_tokens\":11}}}}}}\n\n"
    )
}

/// The vendor's refusal of a tier the project may not use.
const REFUSED: &str = r#"{"error":{"message":"Invalid service_tier argument: The requested service tier is not allowed for this project.","type":"invalid_request_error","param":"service_tier","code":null}}"#;

fn openai(endpoint: Endpoint, status: u16, body: &str) -> (OpenAi, Arc<Replay>) {
    let replay = Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(ApiKey::new("synthetic-fast-key"), Header::bearer());
    (
        OpenAi::at(
            endpoint,
            Box::new(credential),
            Box::new(Arc::clone(&replay)),
        ),
        replay,
    )
}

fn asking(model: &'static str) -> Request<'static> {
    let mut transcript = Transcript::new();
    transcript.push(Message::said("hello")).unwrap();
    Request {
        model,
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

/// The tier the request asked for, or `None` where it named none.
fn tier_sent(endpoint: Endpoint, model: &'static str, speed: Speed) -> Option<String> {
    let (provider, replay) = openai(endpoint, 200, &answered("default"));
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
        "gpt-6-astra",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-5.5",
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
fn the_sign_in_asks_fast_only_for_the_models_it_serves_fast() {
    assert_eq!(
        tier_sent(SUBSCRIPTION, "gpt-5.6-sol", Speed::Fast).as_deref(),
        Some("priority")
    );
    assert_eq!(tier_sent(SUBSCRIPTION, "gpt-5.5", Speed::Fast), None);
}

#[test]
fn a_model_with_no_fast_form_and_a_configured_address_ask_for_nothing() {
    assert_eq!(tier_sent(VENDOR, "gpt-unreviewed", Speed::Fast), None);
    assert_eq!(
        tier_sent(
            Endpoint::fixed("https://gateway.example/v1/responses"),
            "gpt-6-astra",
            Speed::Fast
        ),
        None
    );
}

#[test]
fn each_model_says_whether_its_fast_form_is_a_field_of_the_request() {
    let (api, _) = openai(VENDOR, 200, "");
    let (signed_in, _) = openai(SUBSCRIPTION, 200, "");
    let (gateway, _) = openai(
        Endpoint::fixed("https://gateway.example/v1/responses"),
        200,
        "",
    );
    assert!(matches!(api.fast("gpt-6-astra"), FastForm::Field(_)));
    assert!(matches!(api.fast("gpt-5.5"), FastForm::Field(_)));
    assert!(matches!(signed_in.fast("gpt-5.6-sol"), FastForm::Field(_)));
    assert_eq!(signed_in.fast("gpt-5.5"), FastForm::None);
    assert_eq!(api.fast("gpt-unreviewed"), FastForm::None);
    assert_eq!(gateway.fast("gpt-6-astra"), FastForm::None);
}

#[test]
fn the_answer_says_the_speed_it_was_served_at() {
    for (tier, served) in [
        ("priority", Served::Fast),
        ("fast", Served::Fast),
        ("default", Served::Standard),
        ("", Served::Unsaid),
    ] {
        let (provider, _) = openai(VENDOR, 200, &answered(tier));
        let cancel = Cancel::new();
        let mut stream = crucible_runtime::answered!(provider.stream_at(
            asking("gpt-5.6-sol"),
            Speed::Fast,
            &cancel
        ))
        .unwrap();
        while let Some(delta) = crucible_runtime::answered!(stream.next()) {
            delta.unwrap();
        }
        assert_eq!(stream.served(), served, "{tier:?}");
    }
}

#[test]
fn the_vendors_refusal_of_the_tier_is_a_refusal_of_fast_only_where_fast_was_asked() {
    let cancel = Cancel::new();

    let (provider, _) = openai(VENDOR, 400, REFUSED);
    let fast = crucible_runtime::answered!(provider.stream_at(
        asking("gpt-6-astra"),
        Speed::Fast,
        &cancel
    ));
    assert!(
        matches!(&fast, Err(ProviderError::FastRefused { provider: "openai", message }) if message.contains("not allowed for this project")),
        "{:?}",
        fast.err()
    );

    let (provider, _) = openai(VENDOR, 400, REFUSED);
    let standard = crucible_runtime::answered!(provider.stream_at(
        asking("gpt-6-astra"),
        Speed::Standard,
        &cancel
    ));
    assert!(
        matches!(&standard, Err(ProviderError::Refused { .. })),
        "{:?}",
        standard.err()
    );

    // Under the sign-in no refusal of fast is documented, so nothing is
    // taken for one and the failure is the one it is.
    let (provider, _) = openai(SUBSCRIPTION, 400, REFUSED);
    let signed_in = crucible_runtime::answered!(provider.stream_at(
        asking("gpt-5.6-sol"),
        Speed::Fast,
        &cancel
    ));
    assert!(
        matches!(&signed_in, Err(ProviderError::Refused { .. })),
        "{:?}",
        signed_in.err()
    );
}
