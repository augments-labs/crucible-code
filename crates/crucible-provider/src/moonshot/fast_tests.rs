//! Kimi's fast form: none behind the switch. The highspeed model is a fast
//! model of its own, and a refusal of it is that model's error, sent nowhere
//! else.

use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::{FastForm, RequestPurpose, Speed};
use crucible_types::{Message, Transcript};

use super::*;
use crate::transport::Replay;

fn kimi(endpoint: Endpoint, status: u16, body: &str) -> (Moonshot, std::sync::Arc<Replay>) {
    let replay = std::sync::Arc::new(Replay::new(status, body));
    (
        Moonshot::at(
            endpoint,
            Box::new(HeaderKey::new(
                ApiKey::new("fabricated-kimi-key"),
                Header::bearer(),
            )),
            Box::new(std::sync::Arc::clone(&replay)),
        ),
        replay,
    )
}

fn asking(model: &'static str) -> Request<'static> {
    let mut transcript = Transcript::new();
    transcript.push(Message::said("hello")).unwrap();
    Request {
        purpose: RequestPurpose::Turn,
        model,
        transcript: Box::leak(Box::new(transcript)),
        tools: &[],
        max_tokens: 1024,
        system: None,
        effort: None,
        attached: &[],
        prompt_cache: None,
    }
}

#[test]
fn the_highspeed_model_is_fast_of_its_own_and_no_other_model_has_a_fast_form() {
    let (coding, _) = kimi(Moonshot::CODING, 200, "");
    let (gateway, _) = kimi(
        Endpoint::fixed("https://gateway.example/v1/chat/completions"),
        200,
        "",
    );
    assert!(matches!(
        coding.fast("kimi-for-coding-highspeed"),
        FastForm::Own(_)
    ));
    for model in ["kimi-for-coding", "k3", "k3-256k"] {
        assert_eq!(coding.fast(model), FastForm::None, "{model}");
    }
    assert_eq!(gateway.fast("kimi-for-coding-highspeed"), FastForm::None);
}

#[test]
fn a_refusal_of_the_highspeed_model_is_its_own_error_and_sent_once() {
    let (provider, replay) = kimi(
        Moonshot::CODING,
        401,
        r#"{"error":{"type":"access_terminated_error","message":"your plan does not include kimi-for-coding-highspeed"}}"#,
    );
    let cancel = Cancel::new();

    let answer = crucible_runtime::answered!(provider.stream_at(
        asking("kimi-for-coding-highspeed"),
        Speed::Fast,
        &cancel
    ));

    assert!(
        matches!(answer, Err(ProviderError::Refused { status: 401, .. })),
        "{:?}",
        answer.err()
    );
    assert_eq!(replay.sent_count(), Some(1));
    assert!(!replay.sent().body.contains("service_tier"));
}
