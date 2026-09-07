//! Synthetic streamed side requests, independent native steps and citations.

use super::*;
use crate::{Google, transport::Replay};
use crucible_core::{ApiKey, Header, HeaderKey};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::sync::Arc;

fn answer(steps: &[Value]) -> String {
    let mut body = String::new();
    for (index, step) in steps.iter().enumerate() {
        for value in [
            json!({"event_type":"step.start","index":index,"step":step}),
            json!({"event_type":"step.stop","index":index}),
        ] {
            write!(body, "data: {value}\n\n").unwrap();
        }
    }
    body.push_str("data: {\"event_type\":\"interaction.completed\",\"interaction\":{\"status\":\"completed\"}}\n\n");
    body
}

fn source(steps: &[Value]) -> (GoogleWeb, Arc<Replay>) {
    source_body(200, answer(steps))
}

fn source_body(status: u16, body: String) -> (GoogleWeb, Arc<Replay>) {
    let replay = Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(
        ApiKey::new("web-key-canary"),
        Header::bare("x-goog-api-key"),
    );
    (
        GoogleWeb::new(
            Google::VENDOR,
            Box::new(credential),
            Box::new(Arc::clone(&replay)),
            "gemini-3.8-flash",
        ),
        replay,
    )
}

#[test]
fn google_web_cancellation_during_request_setup_is_not_a_failed_tool_result() {
    #[derive(Debug)]
    struct DuringSetup;
    impl Transport for DuringSetup {
        fn post(
            &self,
            _: &str,
            _: Outgoing,
            _: String,
            cancel: &Cancel,
        ) -> Result<crate::Response, crate::TransportError> {
            cancel.request();
            Err(crate::TransportError::Cancelled)
        }
    }
    let source = GoogleWeb::new(
        Google::VENDOR,
        Box::new(HeaderKey::new(
            ApiKey::new("fixture-only"),
            Header::bare("x-goog-api-key"),
        )),
        Box::new(DuringSetup),
        "gemini-3.8-flash",
    );
    let cancel = Cancel::new();
    let result = source.fetch("https://example.com/", &cancel);
    assert!(
        matches!(result, Err(SourceError::Cancelled("google"))),
        "{result:?}"
    );
}

#[test]
fn google_fetch_requires_the_requested_retrieval_and_enables_no_search() {
    let url = "https://example.com/page";
    let (source, replay) = source(&[
        json!({"type":"url_context_call","id":"fetch","arguments":{"urls":[url]}}),
        json!({"type":"url_context_result","call_id":"fetch","result":[{"url":url,"status":"success"}]}),
        json!({"type":"model_output","content":[{"type":"text","text":"Page text","annotations":[{"type":"url_citation","url":url,"title":"A page","start_index":0,"end_index":9}]}]}),
    ]);
    let page = source.fetch(url, &Cancel::new()).unwrap();
    assert_eq!(
        page,
        Page {
            url: url.into(),
            title: Some("A page".into()),
            text: "Page text".into()
        }
    );
    let sent: Value = serde_json::from_str(&replay.sent().body).unwrap();
    assert_eq!(sent.get("tools").unwrap(), &json!([{"type":"url_context"}]));
    assert_eq!(
        sent.pointer("/generation_config/max_output_tokens")
            .and_then(Value::as_u64),
        Some(32768)
    );
    assert!(
        source
            .fetch("https://other.example/", &Cancel::new())
            .is_err()
    );
    assert!(source.fetch("file:///secret", &Cancel::new()).is_err());
}

fn fetched(url: &str) -> Vec<Value> {
    vec![
        json!({"type":"url_context_call","id":"fetch","arguments":{"urls":[url]}}),
        json!({"type":"url_context_result","call_id":"fetch","is_error":false,"result":[{"url":url,"status":"success"}]}),
        json!({"type":"model_output","content":[{"type":"text","text":"é page","annotations":[{"type":"url_citation","url":url,"start_index":0,"end_index":7}]}]}),
    ]
}

#[test]
fn google_fetch_refuses_error_results_even_with_successful_url_metadata() {
    let url = "https://example.com/page";
    let mut steps = fetched(url);
    *steps.get_mut(1).unwrap().get_mut("is_error").unwrap() = json!(true);
    let (source, _) = source(&steps);
    assert!(source.fetch(url, &Cancel::new()).is_err());
}

#[test]
fn google_fetch_bounds_the_whole_stream_even_after_a_complete_answer() {
    let url = "https://example.com/page";
    let mut body = answer(&fetched(url));
    body.push_str(&": padding\n\n".repeat(super::super::MOST / 10 + 1));
    let replay = Arc::new(Replay::new(200, body));
    let credential = HeaderKey::new(
        ApiKey::new("web-key-canary"),
        Header::bare("x-goog-api-key"),
    );
    let source = GoogleWeb::new(
        Google::VENDOR,
        Box::new(credential),
        Box::new(replay),
        "gemini-3.8-flash",
    );
    assert!(source.fetch(url, &Cancel::new()).is_err());
}

#[test]
fn google_fetch_accepts_documented_optional_citation_offsets() {
    let url = "https://example.com/page";
    let mut steps = fetched(url);
    *steps
        .get_mut(2)
        .unwrap()
        .pointer_mut("/content/0/annotations/0")
        .unwrap() = json!({"type":"url_citation","url":url});
    let (source, _) = source(&steps);
    assert_eq!(&*source.fetch(url, &Cancel::new()).unwrap().text, "é page");
}

#[test]
fn google_fetch_rejects_wrong_calls_destinations_ranges_and_native_tools() {
    let url = "https://example.com/page";
    for (pointer, replacement) in [
        ("/0/arguments/urls", json!(["https://other.example/"])),
        ("/1/call_id", json!("unmatched")),
        ("/1/is_error", json!("false")),
        ("/1/result/0/status", json!("paywall")),
        ("/1/result/0/status", json!("unsafe")),
        ("/1/result/0/url", json!("https://other.example/")),
        (
            "/2/content/0/annotations/0/url",
            json!("https://other.example/"),
        ),
        ("/2/content/0/annotations/0/end_index", json!(100)),
        ("/2/content/0/annotations/0/start_index", json!(1)),
        ("/0/type", json!("google_search_call")),
    ] {
        let mut steps = json!(fetched(url));
        *steps.pointer_mut(pointer).unwrap() = replacement;
        let (source, _) = source(steps.as_array().unwrap());
        assert!(source.fetch(url, &Cancel::new()).is_err(), "{pointer}");
    }
}

#[test]
fn google_fetch_uses_key_only_and_does_not_replay_a_remote_interaction() {
    let url = "https://example.com/page";
    let (source, replay) = source(&fetched(url));
    assert_eq!(&*source.fetch(url, &Cancel::new()).unwrap().text, "é page");
    let sent = replay.sent();
    assert_eq!(
        sent.url,
        "https://generativelanguage.googleapis.com/v1beta/interactions?alt=sse"
    );
    assert!(
        sent.headers
            .iter()
            .any(|(name, value)| name == "x-goog-api-key" && value == "web-key-canary")
    );
    assert!(!sent.headers.iter().any(|(name, _)| name == "authorization"));
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(
        body.get("model").and_then(Value::as_str),
        Some("gemini-3.8-flash")
    );
    assert_eq!(body.get("stream").and_then(Value::as_bool), Some(true));
    assert_eq!(body.get("store").and_then(Value::as_bool), Some(false));
    assert!(body.get("previous_interaction_id").is_none());
    assert!(!sent.body.contains("web-key-canary"));
}

#[test]
fn google_fetch_never_returns_partial_or_failed_streams_or_echoes_private_errors() {
    let url = "https://example.com/page";
    let good = answer(&fetched(url));
    let end = good
        .rfind("data: {\"event_type\":\"interaction.completed\"")
        .unwrap();
    let prefix = good.get(..end).unwrap();
    for (status, body) in [
        (200, prefix.to_owned()),
        (
            200,
            good.replace("\"status\":\"completed\"", "\"status\":\"incomplete\""),
        ),
        (
            200,
            format!(
                "{good}data: {{\"event_type\":\"error\",\"message\":\"private-signature-canary web-key-canary\"}}\n\n"
            ),
        ),
        (
            200,
            "data: private-signature-canary web-key-canary\n\n".into(),
        ),
        (403, "private-signature-canary web-key-canary".into()),
    ] {
        let (source, _) = source_body(status, body);
        let error = source.fetch(url, &Cancel::new()).unwrap_err();
        let shown = format!("{error} {error:?}");
        assert!(!shown.contains("private-signature-canary"));
        assert!(!shown.contains("web-key-canary"));
    }
}

#[test]
fn google_fetch_invalid_urls_and_prior_cancellation_never_post() {
    let (source, replay) = source(&fetched("https://example.com/page"));
    for url in [
        "file:///secret",
        "https://example.com@other.example/",
        "https://example.com/\nhttps://other.example/",
        "relative",
    ] {
        assert!(source.fetch(url, &Cancel::new()).is_err());
    }
    let cancel = Cancel::new();
    cancel.request();
    assert!(matches!(
        source.fetch("https://example.com/page", &cancel),
        Err(SourceError::Cancelled("google"))
    ));
    assert!(replay.sent().url.is_empty());
}

#[test]
fn google_fetch_cancellation_during_a_quiet_read_discards_even_completed_text() {
    use crate::transport::{Paused, Response, Said, TransportError};
    #[derive(Debug)]
    struct Cancelling;
    impl Transport for Cancelling {
        fn post(
            &self,
            _: &str,
            _: Outgoing,
            _: String,
            cancel: &Cancel,
        ) -> Result<Response, TransportError> {
            let cancel = cancel.clone();
            let body = Paused::saying([
                Said::Bytes(answer(&fetched("https://example.com/page")).into_bytes()),
                Said::Nothing,
            ])
            .meanwhile(move || cancel.request());
            Ok(Response {
                status: 200,
                body: Box::new(body),
            })
        }
    }
    let credential = HeaderKey::new(
        ApiKey::new("web-key-canary"),
        Header::bare("x-goog-api-key"),
    );
    let source = GoogleWeb::new(
        Google::VENDOR,
        Box::new(credential),
        Box::new(Cancelling),
        "gemini-3.8-flash",
    );
    assert!(matches!(
        source.fetch("https://example.com/page", &Cancel::new()),
        Err(SourceError::Cancelled("google"))
    ));
}
