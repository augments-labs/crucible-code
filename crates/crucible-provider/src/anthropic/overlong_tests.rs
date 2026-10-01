//! Anthropic's refusal of a request too large for the model, told from its
//! other refusals by its type and the opening of its message.

use super::*;
use crate::transport::Replay;
use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::RequestPurpose;
use crucible_types::{Message, Transcript};
use serde_json::{Value, json};
use std::sync::Arc;

/// The refusal of a prompt too long for the model, constructed: see
/// `fixtures/SOURCES.md`.
const TOO_LONG: &str = include_str!("fixtures/error-400-prompt-too-long.json");

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

/// What a request answered with `status` and `body` fails with.
fn refused(status: u16, body: &str) -> ProviderError {
    let replay = Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(
        ApiKey::new("synthetic-overlong-key"),
        Header::bare("x-api-key"),
    );
    let provider = Anthropic::at(VENDOR, Box::new(credential), Box::new(replay));
    let cancel = Cancel::new();
    match crucible_runtime::answered!(provider.stream(asking("claude-opus-5"), &cancel)) {
        Err(problem) => problem,
        Ok(_) => panic!("a refused request has no stream"),
    }
}

/// [`TOO_LONG`] with its message replaced by `said`.
fn saying(said: &str) -> String {
    let mut body: Value = serde_json::from_str(TOO_LONG).unwrap();
    *body.pointer_mut("/error/message").unwrap() = json!(said);
    body.to_string()
}

#[test]
fn a_prompt_too_long_for_the_model_is_compacted_for_rather_than_ending_the_turn() {
    let problem = refused(400, TOO_LONG);

    assert!(
        matches!(
            problem,
            ProviderError::WindowExceeded {
                provider: "anthropic"
            }
        ),
        "{problem:?}"
    );
    assert!(!problem.transient(), "the same request will not fit again");
}

#[test]
fn the_documented_words_alone_are_a_prompt_too_long_whatever_follows_them() {
    // The context windows page names the refusal by its opening words only,
    // so what follows them is not required to be the counts seen in practice.
    for said in [
        "prompt is too long",
        "prompt is too long: many tokens > 200000 maximum",
    ] {
        let problem = refused(400, &saying(said));

        assert!(
            matches!(
                problem,
                ProviderError::WindowExceeded {
                    provider: "anthropic"
                }
            ),
            "{said}: {problem:?}"
        );
    }
}

#[test]
fn every_other_invalid_request_stays_a_refusal() {
    for (status, body) in [
        (400, saying("max_tokens: must be greater than 0")),
        (400, saying("messages: roles must alternate")),
        // The words in another case.
        (
            400,
            saying("Prompt is too long: 208310 tokens > 200000 maximum"),
        ),
        // The same body under another status, or another type.
        (413, TOO_LONG.to_owned()),
        (500, TOO_LONG.to_owned()),
        (400, TOO_LONG.replace("invalid_request_error", "api_error")),
    ] {
        let problem = refused(status, &body);

        assert!(
            matches!(problem, ProviderError::Refused { .. }),
            "{status} {body}: {problem:?}"
        );
    }
}

#[test]
fn the_words_echoed_inside_another_refusal_are_not_a_prompt_too_long() {
    // A refusal that quotes the words back from what was sent, rather than
    // opening with them as its own sentence.
    let body = saying(
        "messages.0.content: 'prompt is too long: 208310 tokens > 200000 maximum' is not allowed",
    );

    let problem = refused(400, &body);

    assert!(
        matches!(problem, ProviderError::Refused { status: 400, .. }),
        "{problem:?}"
    );
}
