//! Meta's hosted search, against the examples Meta's search page prints.
//!
//! Copied from <https://dev.meta.ai/docs/search-grounding> as it read on
//! 2026-10-01. The page prints a whole response and no stream, so the response
//! is framed in the terminal event Meta's schema names. It prints no failed
//! call either: the test that needs one builds the smallest item the schema of
//! a `web_search_call` allows.

use super::*;

/// The key, which must never leave a header.
const META_KEY: &str = "LLM|meta-do-not-log-me";

/// The page's example request, "Basic usage", curl.
const REQUEST: &str = r#"{
  "model": "muse-spark-1.3",
  "input": "Who won the most recent Formula 1 race?",
  "tools": [
    {
      "type": "web_search"
    }
  ]
}"#;

/// The page's example response, "Basic usage", "Example response". The page
/// ends the object with a line reading `...`, which is left off.
const RESPONSE: &str = r#"{
  "id": "resp_123",
  "object": "response",
  "created_at": 1778250764,
  "status": "completed",
  "model": "muse-spark-1.3",
  "output": [
    {
      "id": "ws_789",
      "type": "web_search_call",
      "status": "completed"
    },
    {
      "id": "msg_b4d6e9c2ff37410a",
      "type": "message",
      "status": "completed",
      "role": "assistant",
      "content": [
        {
          "type": "output_text",
          "text": "**Muse Spark** was announced by Meta on **Wednesday, April 8, 2026**.\n\n- It was unveiled as the first AI model from Meta's new \"Muse\" family, developed by Meta Superintelligence Labs \n- Meta described it as a \"small and fast by design\" multimodal model built for real-time reasoning across WhatsApp, Instagram, Facebook, and Meta's smart glasses \n\nThe announcement came in a blog post on April 8, kicking off the week of April 6–10 that TechTarget covered in its roundup published April 10, 2026 .",
          "annotations": [
            {
              "type": "url_citation",
              "url": "https://www.techtarget.com/searchcio/feature/Weekly-news-roundup-Claude-Mythos-concerns-Muse-Spark-debut-and-US-infrastructure-disruption",
              "title": "Weekly news roundup: Claude Mythos concerns, Muse Spark debut and U.S. infrastructure disruption | TechTarget",
              "start_index": 38,
              "end_index": 76
            },
            {
              "type": "url_citation",
              "url": "https://www.thehindubusinessline.com/info-tech/meta-launches-muse-spark-1.1-ai-bets-big-on-superintelligence-push/article70840923.ece",
              "title": "Meta unveils Muse Spark AI model to compete in superintelligence race",
              "start_index": 175,
              "end_index": 225
            },
            {
              "type": "url_citation",
              "url": "https://www.techtarget.com/searchcio/feature/Weekly-news-roundup-Claude-Mythos-concerns-Muse-Spark-debut-and-US-infrastructure-disruption",
              "title": "Weekly news roundup: Claude Mythos concerns, Muse Spark debut and U.S. infrastructure disruption | TechTarget",
              "start_index": 380,
              "end_index": 460
            }
          ]
        }
      ]
    }
  ]
}"#;

/// The body of a refused key.
///
/// <https://dev.meta.ai/docs/errors>, "401".
const UNAUTHORIZED: &str = r#"{
  "error": {
    "message": "Unauthorized",
    "type": "authentication_error",
    "param": null,
    "code": "invalid_api_key"
  }
}"#;

/// `response` as the terminal event of a stream, then the line Meta's own
/// sample code stops on.
fn finished(response: &Value) -> String {
    let event = json!({ "type": "response.completed", "response": response });
    format!("event: response.completed\ndata: {event}\n\ndata: [DONE]\n\n")
}

fn printed() -> Value {
    serde_json::from_str(RESPONSE).expect("the page's response is JSON")
}

fn meta(status: u16, body: impl Into<String>) -> (MetaWeb, Arc<Replay>) {
    let replay = Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(ApiKey::new(META_KEY), Header::bearer());
    (
        MetaWeb::new(
            Endpoint::fixed("https://api.meta.ai/v1/responses"),
            Box::new(credential),
            Box::new(Arc::clone(&replay)),
            "muse-spark-1.3",
        ),
        replay,
    )
}

#[test]
fn a_meta_search_sends_the_page_s_request_streamed_unkept_and_with_no_tool_choice() {
    let (source, replay) = meta(200, finished(&printed()));
    source
        .answered_search("who won the most recent formula 1 race", &Cancel::new())
        .expect("the page's answer reads");

    let sent = replay.sent();
    assert_eq!(sent.url, "https://api.meta.ai/v1/responses");
    let body: Value = serde_json::from_str(&sent.body).expect("a JSON body");
    let page: Value = serde_json::from_str(REQUEST).expect("the page's request is JSON");
    assert_eq!(body.get("model"), page.get("model"));
    assert_eq!(body.get("tools"), page.get("tools"));
    // Meta answers any choice but `auto` with a 400.
    assert!(body.get("tool_choice").is_none(), "{body}");
    assert_eq!(body.get("stream"), Some(&json!(true)));
    assert_eq!(body.get("store"), Some(&json!(false)));
    assert_eq!(body.pointer("/input/0/role"), Some(&json!("user")));
    assert!(
        body.pointer("/input/0/content")
            .and_then(Value::as_str)
            .is_some_and(|said| said.contains("who won the most recent formula 1 race")),
        "{body}"
    );
    assert!(!sent.body.contains(META_KEY));
}

#[test]
fn a_meta_search_reads_the_addresses_its_printed_answer_cites() {
    let found = meta(200, finished(&printed()))
        .0
        .answered_search("when was muse spark announced", &Cancel::new())
        .expect("the page's answer reads");

    // Three citations of two addresses.
    assert_eq!(found.len(), 2, "{found:?}");
    let first = found.first().expect("a first result");
    assert!(
        first.url.starts_with("https://www.techtarget.com/"),
        "{first:?}"
    );
    assert!(first.title.starts_with("Weekly news roundup"), "{first:?}");
    // The run the page's indices mark, counted in characters as printed.
    assert_eq!(
        first.extract.as_ref(),
        "n **Wednesday, April 8, 2026**.\n\n- It "
    );
}

#[test]
fn a_search_call_meta_reports_failed_is_said_in_its_words_and_the_turn_goes_on() {
    let response = json!({
        "status": "completed",
        "output": [
            { "id": "ws_1", "type": "web_search_call", "status": "failed" },
            {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "I could not search.", "annotations": [] }]
            }
        ]
    });

    let problem = meta(200, finished(&response))
        .0
        .answered_search("x", &Cancel::new())
        .expect_err("a failed call is not a search that found nothing");

    // A protocol failure, which the tool reports as a failed result rather
    // than ending the turn; in the call's own status, not as "no search".
    assert!(
        matches!(problem, SourceError::Protocol { named: "meta", .. }),
        "{problem:?}"
    );
    let said = problem.to_string();
    assert!(said.contains("web_search_call ws_1 failed"), "{said}");
    assert!(!said.contains("without searching"), "{said}");
}

#[test]
fn a_failed_call_beside_a_completed_one_costs_only_its_own_results() {
    let mut response = printed();
    if let Some(output) = response.get_mut("output").and_then(Value::as_array_mut) {
        output.insert(
            0,
            json!({ "id": "ws_0", "type": "web_search_call", "status": "failed" }),
        );
    }

    let found = meta(200, finished(&response))
        .0
        .answered_search("x", &Cancel::new())
        .expect("the completed call's results");

    assert_eq!(found.len(), 2);
}

#[test]
fn a_meta_refusal_is_said_in_its_words_and_never_with_the_key() {
    let problem = meta(401, UNAUTHORIZED)
        .0
        .answered_search("x", &Cancel::new())
        .expect_err("a refused key");

    let SourceError::Refused {
        named,
        status,
        message,
    } = &problem
    else {
        panic!("a refusal, not {problem:?}");
    };
    assert_eq!((*named, *status), ("meta", 401));
    assert_eq!(message.as_ref(), "invalid_api_key: Unauthorized");
    assert!(!format!("{problem:?} {problem}").contains(META_KEY));
}

#[test]
fn an_answer_meta_wrote_without_searching_is_not_an_empty_result() {
    let response = json!({
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "From memory.", "annotations": [] }]
        }]
    });

    let problem = meta(200, finished(&response))
        .0
        .answered_search("x", &Cancel::new())
        .expect_err("an answer with no search call");

    assert!(
        problem.to_string().contains("without searching"),
        "{problem}"
    );
}

#[test]
fn a_meta_search_reaches_the_vendor_host_a_rule_would_name() {
    assert_eq!(
        Search::reaches(&meta(200, "").0),
        Host::Named {
            sent: "https://api.meta.ai/v1/responses".into(),
            host: "api.meta.ai".into(),
        }
    );
}

#[test]
fn a_refusal_echoing_the_key_in_an_escape_never_says_the_key() {
    let body =
        r#"{"error":{"code":"invalid_api_key","message":"rejected LLM|meta-do-not-l\u006fg-me"}}"#;

    let problem = meta(401, body)
        .0
        .answered_search("x", &Cancel::new())
        .expect_err("a refusal");

    assert!(
        !format!("{problem:?} {problem}").contains(META_KEY),
        "{problem}"
    );
}
