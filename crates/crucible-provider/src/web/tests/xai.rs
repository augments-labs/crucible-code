//! xAI's hosted search, against the examples xAI's pages print.
//!
//! Each example is copied from the page named above it, as it read on
//! 2026-10-01. No xAI page prints a Responses stream, a search call's item, a
//! failed call or an HTTP error body: where a test needs one it says where the
//! shape comes from, and builds no more than that.

use super::*;

/// The key, which must never leave a header.
const XAI_KEY: &str = "xai-do-not-log-me";

/// The page's example request.
///
/// <https://docs.x.ai/developers/tools/web-search>, cURL basic usage.
const REQUEST: &str = r#"{
  "model": "grok-4.7",
  "input": [
    {
      "role": "user",
      "content": "What is xAI?"
    }
  ],
  "tools": [
    {
      "type": "web_search"
    }
  ]
}"#;

/// An inline citation, whose title is the citation's number.
///
/// <https://docs.x.ai/developers/tools/citations>, inline citations.
const CITATION: &str = r#"{
  "type": "url_citation",
  "url": "https://example.com",
  "start_index": 208,
  "end_index": 235,
  "title": "1"
}"#;

/// The error event xAI prints, nested under `error`.
///
/// <https://docs.x.ai/developers/advanced-api-usage/websocket-mode>, errors;
/// the page says its events are those of the Responses stream.
const NESTED: &str = r#"{
  "type": "error",
  "status": 400,
  "error": {
    "code": "previous_response_not_found",
    "message": "Previous response with id 'resp_abc' not found.",
    "param": "previous_response_id"
  }
}"#;

/// The search call an xAI stream carries, by name and arguments rather than an
/// action. No xAI page prints one: this is the item of a stream another
/// harness recorded against xAI's API (the Vercel AI SDK's xAI fixtures).
fn call(status: &str) -> Value {
    json!({
        "id": "fc_1",
        "type": "web_search_call",
        "status": status,
        "name": "web_search",
        "arguments": "{\"query\":\"what is xAI\",\"num_results\":5}"
    })
}

/// An answer citing [`CITATION`]: prose long enough for its indices to land.
fn answer(calls: &[Value]) -> Value {
    let citation: Value = serde_json::from_str(CITATION).expect("the page's citation is JSON");
    let text = format!(
        "{}xAI makes Grok, an AI model.{}",
        " ".repeat(208),
        " ".repeat(8)
    );
    let mut output: Vec<Value> = calls.to_vec();
    output.push(json!({
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": text, "annotations": [citation] }]
    }));
    json!({ "status": "completed", "output": output })
}

fn finished(response: &Value) -> String {
    let event = json!({ "type": "response.completed", "response": response });
    format!("event: response.completed\ndata: {event}\n\n")
}

fn xai(status: u16, body: impl Into<String>) -> (XaiWeb, Arc<Replay>) {
    let replay = Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(ApiKey::new(XAI_KEY), Header::bearer());
    (
        XaiWeb::new(
            Endpoint::fixed("https://api.x.ai/v1/responses"),
            Box::new(credential),
            Box::new(Arc::clone(&replay)),
            "grok-4.7",
        ),
        replay,
    )
}

#[test]
fn an_xai_search_sends_the_page_s_request_made_to_search_and_unkept() {
    let (source, replay) = xai(200, finished(&answer(&[call("completed")])));
    source
        .answered_search("What is xAI?", &Cancel::new())
        .expect("an answer that reads");

    let sent = replay.sent();
    assert_eq!(sent.url, "https://api.x.ai/v1/responses");
    let body: Value = serde_json::from_str(&sent.body).expect("a JSON body");
    let page: Value = serde_json::from_str(REQUEST).expect("the page's request is JSON");
    assert_eq!(body.get("model"), page.get("model"));
    assert_eq!(body.get("input"), page.get("input"));
    assert_eq!(body.get("tools"), page.get("tools"));
    assert_eq!(body.get("tool_choice"), Some(&json!("required")));
    assert_eq!(body.get("store"), Some(&json!(false)));
    assert_eq!(body.get("stream"), Some(&json!(true)));
    assert!(!sent.body.contains(XAI_KEY));
}

#[test]
fn a_citation_xai_numbers_is_titled_by_its_address() {
    let found = xai(200, finished(&answer(&[call("completed")])))
        .0
        .answered_search("What is xAI?", &Cancel::new())
        .expect("an answer that reads");

    let first = found.first().expect("the cited address");
    assert_eq!(first.url.as_ref(), "https://example.com");
    assert_eq!(first.title.as_ref(), "https://example.com");
    assert_eq!(first.extract.as_ref(), "xAI makes Grok, an AI model");
}

#[test]
fn a_search_call_xai_reports_failed_is_said_in_its_words_and_the_turn_goes_on() {
    // "Failed attempts are not charged": a call fails on its own and the
    // request around it does not. No page prints the failed item; this is the
    // recorded one with its status changed.
    let problem = xai(200, finished(&answer(&[call("failed")])))
        .0
        .answered_search("x", &Cancel::new())
        .expect_err("a failed call is not a search that found nothing");

    assert!(
        matches!(problem, SourceError::Protocol { named: "xai", .. }),
        "{problem:?}"
    );
    let said = problem.to_string();
    assert!(said.contains("web_search_call fc_1 failed"), "{said}");
}

#[test]
fn an_xai_refusal_is_its_sentence_in_either_shape_and_never_the_key() {
    // Neither shape is printed by an xAI page for an HTTP error. The nested one
    // is how its printed events word a failure; the flat one, with a sentence
    // where the code is, is from xAI's own harness's corpus of its errors.
    for (body, said) in [
        (
            r#"{"error":{"code":"invalid_argument","message":"model 'nope' does not exist"}}"#,
            "invalid_argument: model 'nope' does not exist",
        ),
        (
            r#"{"code":"Client specified an invalid argument","error":"model 'nope' does not exist"}"#,
            "Client specified an invalid argument: model 'nope' does not exist",
        ),
        (
            r#"{"code":429,"error":"You ran out of credits."}"#,
            "429: You ran out of credits.",
        ),
    ] {
        let problem = xai(400, body)
            .0
            .answered_search("x", &Cancel::new())
            .expect_err("a refusal");
        let SourceError::Refused { message, .. } = &problem else {
            panic!("a refusal, not {problem:?}");
        };
        assert_eq!(message.as_ref(), said);
        assert!(!format!("{problem:?}").contains(XAI_KEY));
    }
}

#[test]
fn an_xai_failure_event_is_read_from_under_error() {
    let stream = format!("event: error\ndata: {}\n\n", NESTED.replace('\n', ""));

    let problem = xai(200, stream)
        .0
        .answered_search("x", &Cancel::new())
        .expect_err("a failure event");

    let said = problem.to_string();
    assert!(said.contains("previous_response_not_found"), "{said}");
    assert!(
        said.contains("Previous response with id 'resp_abc' not found."),
        "{said}"
    );
}
