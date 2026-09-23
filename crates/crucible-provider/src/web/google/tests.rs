//! Synthetic streamed side requests, independent native steps and citations.

use super::*;
use crate::{Google, transport::Replay};
use crucible_credentials::{ApiKey, Header, HeaderKey};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::io::{self, Read};
use std::sync::Arc;
use std::time::Duration;

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

/// A body that hands its whole content over in one read, then sleeps past
/// `wait` before confirming the clean end that closed it.
///
/// Modeled on `web/tests.rs`'s `WholeThenLateEnd` (the #695 sibling this fix
/// follows) and `refusal.rs`'s fixture of the same name: the answer already
/// sits whole in the parser's buffer, complete with the event that says the
/// model is done, and only the read confirming there is nothing further
/// arrives once the wait has already run out. A read that hands over more
/// content instead would need a further attempt to confirm the stream's
/// end, and that attempt is rightly bound by the same wait — it is the
/// clean end itself, not a content chunk, that this proves is kept.
struct WholeThenLateEnd {
    body: Option<Vec<u8>>,
    wait: Duration,
}

impl Read for WholeThenLateEnd {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        let Some(body) = self.body.take() else {
            std::thread::sleep(self.wait.saturating_add(Duration::from_millis(20)));
            return Ok(0);
        };
        let took = body.len().min(into.len());
        into.get_mut(..took)
            .unwrap_or_default()
            .copy_from_slice(body.get(..took).unwrap_or_default());
        Ok(took)
    }
}

#[test]
fn a_fetched_page_read_whole_is_delivered_when_its_close_arrives_late() {
    // Proves the fix through the same pipeline `GoogleWeb::ask` builds —
    // `Limited` wrapped by the Interactions SSE wire that parses its
    // events — rather than against `Limited` alone: the bug replaced a page
    // whose close confirmed right as the wait ran out with "Google web
    // response exceeded its deadline" instead of the retrieved text, even
    // though the whole answer, including the event saying the model was
    // done, had already arrived.
    let url = "https://example.com/page";
    let body = answer(&fetched(url)).into_bytes();
    let wait = Duration::from_millis(5);

    let reading = WholeThenLateEnd {
        body: Some(body),
        wait,
    };
    let limited =
        super::read::Limited::new(Box::new(reading), Cancel::new(), super::super::MOST, wait);
    let wire = crate::google::wire::Interactions::new(
        "gemini-3.8-flash",
        crucible_types::ContinuationScope::from_digest([7; 32]),
    )
    .unwrap();
    let mut stream = crate::stream::Response::with_wire(
        Box::new(limited),
        Cancel::new(),
        crucible_credentials::Redactions::default(),
        wire,
    );

    let mut text = String::new();
    let mut stop = None;
    while let Some(delta) = stream.next_delta() {
        match delta.expect("a page read whole must not fail when its close arrives late") {
            Delta::Text(part) => text.push_str(&part),
            Delta::Stopped(reason) => stop = Some(reason),
            _ => {}
        }
    }

    assert_eq!(text, "é page");
    assert_eq!(stop, Some(StopReason::Yielded));
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

fn searched(query: &str, answer: &str, suggestions: &str, url: &str, title: &str) -> Vec<Value> {
    vec![
        json!({
            "type": "thought",
            "content": [{"type": "text", "text": "planning to search..."}]
        }),
        json!({
            "type": "google_search_call",
            "id": "search_1",
            "arguments": {"queries": [query]}
        }),
        json!({
            "type": "google_search_result",
            "call_id": "search_1",
            "result": [{"search_suggestions": suggestions}]
        }),
        json!({
            "type": "model_output",
            "content": [{
                "type": "text",
                "text": answer,
                "annotations": [{
                    "type": "url_citation",
                    "url": url,
                    "title": title,
                    "start_index": 0,
                    "end_index": answer.len()
                }]
            }]
        }),
    ]
}

#[test]
fn google_search_success_with_grounded_answer_citations_and_suggestions() {
    let query = "rust programming";
    let answer_text = "Rust is a systems programming language.";
    let suggestions_html =
        "<div><a href=\"https://www.google.com/search?q=learn+rust\">learn rust</a></div>";
    let url = "https://www.rust-lang.org";
    let title = "Rust Language";

    let (source, replay) = source(&searched(query, answer_text, suggestions_html, url, title));
    let response = source
        .search(query, &Cancel::new())
        .expect("google search should succeed");

    assert_eq!(response.answer.as_deref(), Some(answer_text));
    assert_eq!(response.results.len(), 1);
    let first = response.results.first().expect("search result");
    assert_eq!(&*first.url, url);
    assert_eq!(&*first.title, title);
    assert_eq!(&*first.extract, answer_text);
    assert_eq!(
        response.suggestions.as_deref(),
        Some("- [learn rust](https://www.google.com/search?q=learn+rust)")
    );

    let sent = replay.sent();
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(
        body.get("model").and_then(Value::as_str),
        Some("gemini-3.8-flash")
    );
    assert_eq!(body.get("stream").and_then(Value::as_bool), Some(true));
    assert_eq!(body.get("store").and_then(Value::as_bool), Some(false));
    assert_eq!(
        body.pointer("/generation_config/max_output_tokens")
            .and_then(Value::as_u64),
        Some(u64::from(CEILING))
    );
    assert_eq!(
        body.pointer("/tools/0/type").and_then(Value::as_str),
        Some("google_search")
    );
}

#[test]
fn google_search_missing_suggestions_is_refused() {
    let mut steps = searched(
        "query",
        "Answer",
        "<div>suggestions</div>",
        "https://example.com",
        "Title",
    );
    // Remove search_suggestions from google_search_result
    if let Some(step) = steps.get_mut(2) {
        *step = json!({
            "type": "google_search_result",
            "call_id": "search_1",
            "result": [{}]
        });
    }
    let (source, _) = source(&steps);
    let error = source.search("query", &Cancel::new()).unwrap_err();
    assert!(
        format!("{error}").contains("missing Google search suggestions"),
        "{error}"
    );
}

#[test]
fn google_search_missing_citations_is_refused() {
    let mut steps = searched(
        "query",
        "Answer without citations",
        "<div>suggestions</div>",
        "https://example.com",
        "Title",
    );
    // Remove annotations from model_output
    if let Some(step) = steps.get_mut(3) {
        *step = json!({
            "type": "model_output",
            "content": [{
                "type": "text",
                "text": "Answer without citations",
                "annotations": []
            }]
        });
    }
    let (source, _) = source(&steps);
    let error = source.search("query", &Cancel::new()).unwrap_err();
    assert!(
        format!("{error}").contains("Google search response carried no citations"),
        "{error}"
    );
}

#[test]
fn google_search_error_payload_is_refused() {
    let mut steps = searched(
        "query",
        "Answer",
        "<div>suggestions</div>",
        "https://example.com",
        "Title",
    );
    if let Some(step) = steps.get_mut(2) {
        *step = json!({
            "type": "google_search_result",
            "call_id": "search_1",
            "is_error": true,
            "result": [{"search_suggestions": "<div>suggestions</div>"}]
        });
    }
    let (source, _) = source(&steps);
    let error = source.search("query", &Cancel::new()).unwrap_err();
    assert!(
        format!("{error}").contains("Google search returned an error"),
        "{error}"
    );
}

#[test]
fn google_search_prior_cancellation_never_posts() {
    let (source, replay) = source(&searched(
        "query",
        "Answer",
        "<div>suggestions</div>",
        "https://example.com",
        "Title",
    ));
    let cancel = Cancel::new();
    cancel.request();
    let error = source.search("query", &cancel).unwrap_err();
    assert!(matches!(error, SourceError::Cancelled("google")));
    assert!(replay.sent().url.is_empty());
}

#[test]
fn google_search_keeps_its_results_to_google_models_in_the_words_its_provider_uses() {
    // The grounding source and the provider are two faces of one vendor's term:
    // a result the source answered carries the restriction, and a session that
    // leaves the provider without such a record falls back on the provider's
    // own statement. Two sentences for one term would make which one a reader
    // is shown depend on how the session got here.
    let (web, _) = source(&[]);
    let provider = Google::at(
        Google::VENDOR,
        Box::new(HeaderKey::new(
            ApiKey::new("provider-key-canary"),
            Header::bare("x-goog-api-key"),
        )),
        Box::new(Arc::new(Replay::new(200, answer(&[])))),
    );

    assert_eq!(
        Search::name(&web),
        crucible_models::Provider::name(&provider)
    );
    let restricts = Search::restricts(&web);
    assert!(
        restricts.is_some(),
        "Google grounding answered a search with nothing keeping it to Google models"
    );
    assert_eq!(
        restricts,
        crucible_models::Provider::restricts_results(&provider)
    );
}
