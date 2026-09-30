//! A second vendor on this wire, written here alone: a dialect is all it
//! takes, and nothing in the module it runs against was edited for it.

use crucible_credentials::{ApiKey, Header, HeaderKey, Outgoing};
use crucible_models::{Attached, Content, Delta, Effort, Provider, ProviderError, Request};
use crucible_runtime::Cancel;
use crucible_types::{
    InputTokenUsage, Message, Modalities, Modality, ProviderUsage, StopReason, Transcript,
};
use serde_json::Value;

use super::{Chat, Dialect};
use crate::endpoint::Endpoint;
use crate::transport::Replay;

/// Where the fabricated vendor serves this wire.
const RELAY: Endpoint = Endpoint::fixed("https://chat.example/v1/chat/completions");

/// A fabricated vendor that does everything a dialect may do its own way.
#[derive(Debug)]
struct Relay;

impl Dialect for Relay {
    const NAME: &'static str = "relay";
    const TITLE: &'static str = "Relay";
    const ADDRESSES: &'static [Endpoint] = &[RELAY];
    const SHAPE: &'static str = "relay-chat-completions-v1";

    fn spells() -> Modalities {
        Modalities::empty().insert(Modality::Text)
    }

    fn headers(outgoing: &mut Outgoing) {
        outgoing.set_header("x-client", "relay-test");
    }

    /// Counts that leave the cached part out of the prompt, unlike the
    /// inclusive ones the wire's first vendor sends.
    fn usage(payload: &Value) -> Result<Option<Delta>, ProviderError> {
        let Some(usage) = payload.get("usage") else {
            return Ok(None);
        };
        let number = |field: &str| usage.get(field).and_then(Value::as_u64);
        let input =
            InputTokenUsage::disjoint(number("fresh_tokens"), number("reused_tokens"), None)
                .map_err(|problem| ProviderError::Protocol {
                    provider: Self::NAME,
                    problem: problem.to_string().into(),
                })?;
        let usage = ProviderUsage::new(input, number("answer_tokens"), None, None, &[]).map_err(
            |problem| ProviderError::Protocol {
                provider: Self::NAME,
                problem: problem.to_string().into(),
            },
        )?;
        Ok(Some(Delta::Usage(usage)))
    }

    fn effort(effort: Effort) -> &'static str {
        match effort {
            Effort::Low => "quick",
            Effort::Medium => "normal",
            Effort::High | Effort::Xhigh | Effort::Max => "deep",
        }
    }

    fn prompt_cache(_model: &str) -> crucible_models::PromptCacheCapabilities {
        crucible_models::PromptCacheCapabilities::unknown("not reviewed")
    }
}

/// The fabricated vendor at `endpoint`, answering every request with
/// `status` and `body`.
fn relay(endpoint: Endpoint, status: u16, body: &str) -> (Chat<Relay>, std::sync::Arc<Replay>) {
    let replay = std::sync::Arc::new(Replay::new(status, body));
    (
        Chat::at(
            endpoint,
            Box::new(HeaderKey::new(
                ApiKey::new("fabricated-relay-key"),
                Header::bearer(),
            )),
            Box::new(std::sync::Arc::clone(&replay)),
        ),
        replay,
    )
}

/// A question, thought about at `effort`, with one file the vendor cannot
/// take.
fn asking(effort: Effort) -> Request<'static> {
    let mut transcript = Transcript::new();
    transcript
        .push(Message::said("What is in this picture?"))
        .expect("a valid transcript");
    Request {
        purpose: crucible_models::RequestPurpose::Turn,
        model: "relay-1",
        transcript: Box::leak(Box::new(transcript)),
        tools: &[],
        attached: Box::leak(Box::new([Attached {
            message: 0,
            index: 0,
            media_type: "application/pdf",
            modality: Modality::Pdf,
            content: Content::Bytes(b"%PDF"),
        }])),
        max_tokens: 512,
        system: None,
        effort: Some(effort),
        prompt_cache: None,
    }
}

/// An answer, and what it cost in the fabricated vendor's own counts.
const ANSWER: &str = concat!(
    r#"data: {"choices":[{"index":0,"delta":{"content":"A cat."},"finish_reason":"stop"}]}"#,
    "\n\n",
    r#"data: {"choices":[],"usage":{"fresh_tokens":40,"reused_tokens":60,"answer_tokens":3}}"#,
    "\n\n",
    "data: [DONE]\n\n",
);

#[test]
fn a_second_dialect_sends_its_own_address_headers_rung_and_name() {
    let (provider, replay) = relay(RELAY, 200, ANSWER);
    let cancel = Cancel::new();

    let _stream = crucible_runtime::answered!(provider.stream(asking(Effort::Xhigh), &cancel))
        .expect("the request is sent");

    let sent = replay.sent();
    assert_eq!(sent.url, RELAY.as_str());
    let headers: Vec<(&str, &str)> = sent
        .headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        headers,
        [
            ("content-type", "application/json"),
            ("accept", "text/event-stream"),
            ("x-client", "relay-test"),
            ("authorization", "Bearer fabricated-relay-key"),
        ]
    );
    let body: Value = serde_json::from_str(&sent.body).expect("the body is JSON");
    assert_eq!(body.pointer("/model"), Some(&Value::from("relay-1")));
    assert_eq!(
        body.pointer("/reasoning_effort"),
        Some(&Value::from("deep"))
    );
    assert_eq!(
        body.pointer("/messages/0/content/0/text"),
        Some(&Value::from(
            "attachment omitted: Relay requests do not support pdf input"
        ))
    );
    assert_eq!(provider.name(), "relay");
    assert!(!provider.spells().contains(Modality::Image));
}

#[test]
fn a_second_dialect_reads_the_answer_with_its_own_counts() {
    let (provider, _) = relay(RELAY, 200, ANSWER);
    let cancel = Cancel::new();
    let mut stream = crucible_runtime::answered!(provider.stream(asking(Effort::Low), &cancel))
        .expect("the request is sent");

    let mut deltas = Vec::new();
    while let Some(delta) = crucible_runtime::answered!(stream.next()) {
        deltas.push(delta.expect("the answer reads"));
    }

    let counted = ProviderUsage::new(
        InputTokenUsage::disjoint(Some(40), Some(60), None).expect("valid counts"),
        Some(3),
        None,
        None,
        &[],
    )
    .expect("valid counts");
    assert_eq!(
        deltas,
        [
            Delta::Text("A cat.".into()),
            Delta::Stopped(StopReason::Yielded),
            Delta::Usage(counted),
        ]
    );
}

#[test]
fn a_second_dialect_names_itself_in_what_goes_wrong() {
    let (provider, _) = relay(
        RELAY,
        200,
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"A\"}}]}\n\n",
    );
    let cancel = Cancel::new();
    let mut stream = crucible_runtime::answered!(provider.stream(asking(Effort::Low), &cancel))
        .expect("the request is sent");

    let mut failed = None;
    while let Some(delta) = crucible_runtime::answered!(stream.next()) {
        if let Err(problem) = delta {
            failed = Some(problem);
        }
    }

    assert!(
        matches!(
            failed,
            Some(ProviderError::Transport {
                provider: "relay",
                ..
            })
        ),
        "{failed:?}"
    );
}

#[test]
fn a_second_dialect_elsewhere_than_its_own_address_is_a_custom_endpoint() {
    let (own, _) = relay(RELAY, 200, ANSWER);
    let (elsewhere, _) = relay(
        Endpoint::fixed("https://gateway.example/v1/chat/completions"),
        200,
        ANSWER,
    );

    assert!(!own.prompt_cache_route().custom_endpoint);
    assert_eq!(
        own.prompt_cache_route().request_shape_version,
        "relay-chat-completions-v1"
    );
    assert!(elsewhere.prompt_cache_route().custom_endpoint);
    assert_eq!(
        elsewhere.prompt_cache_capabilities("relay-1"),
        crucible_models::PromptCacheCapabilities::unknown("custom endpoint")
    );
}
