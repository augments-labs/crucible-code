//! What a side request sends, and what it reads back out of a real answer.

use std::sync::Arc;

use crucible_core::{ApiKey, Fetch, Header, HeaderKey, Host, Search};
use serde_json::json;

use super::*;
use crate::transport::Replay;

/// The exact key that must never appear anywhere but a header value.
const SECRET: &str = "sk-ant-do-not-log-me";

/// An answer shaped as the vendor documents one: a decision to search, the
/// query it ran, the results, and prose citing them.
fn answer() -> String {
    json!({
        "role": "assistant",
        "content": [
            { "type": "text", "text": "I'll look that up." },
            {
                "type": "server_tool_use",
                "id": "srvtoolu_1",
                "name": "web_search",
                "input": { "query": "claude shannon birth date" }
            },
            {
                "type": "web_search_tool_result",
                "tool_use_id": "srvtoolu_1",
                "content": [
                    {
                        "type": "web_search_result",
                        "url": "https://en.wikipedia.org/wiki/Claude_Shannon",
                        "title": "Claude Shannon - Wikipedia",
                        "encrypted_content": "EqgfCioIARgBIiQ3YTAwMjY1Mi1",
                        "page_age": "April 30, 2025"
                    },
                    {
                        "type": "web_search_result",
                        "url": "https://example.com/uncited",
                        "title": "Nobody quoted this",
                        "encrypted_content": "Eo8BCioIAhgBIiQyYjQ0OWJmZi1"
                    }
                ]
            },
            {
                "type": "text",
                "text": "Shannon was born on April 30, 1916.",
                "citations": [
                    {
                        "type": "web_search_result_location",
                        "url": "https://en.wikipedia.org/wiki/Claude_Shannon",
                        "title": "Claude Shannon - Wikipedia",
                        "encrypted_index": "Eo8BCioIAhgBIiQ",
                        "cited_text": "Claude Elwood Shannon (April 30, 1916 - February 24, 2001)"
                    }
                ]
            }
        ],
        "stop_reason": "end_turn"
    })
    .to_string()
}

fn built(status: u16, body: impl Into<String>) -> (AnthropicWeb, Arc<Replay>) {
    let replay = Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bare("x-api-key"));

    (
        AnthropicWeb::new(
            Endpoint::fixed("https://api.anthropic.com/v1/messages"),
            Box::new(credential),
            Box::new(Arc::clone(&replay)),
            "claude-opus-5",
        ),
        replay,
    )
}

fn source(status: u16, body: impl Into<String>) -> AnthropicWeb {
    built(status, body).0
}

#[test]
fn a_result_takes_its_extract_from_the_citation_written_off_it() {
    // The vendor's own result carries no readable body — it arrives encrypted
    // and only that vendor's model can read it. What is readable is the line
    // the model quoted, so the two are matched by address.
    let found = source(200, answer())
        .search("when was claude shannon born", &Cancel::new())
        .expect("an answer that parses");

    assert_eq!(found.len(), 2);

    let first = found.first().expect("a first result");
    assert_eq!(first.title.as_ref(), "Claude Shannon - Wikipedia");
    assert_eq!(
        first.url.as_ref(),
        "https://en.wikipedia.org/wiki/Claude_Shannon"
    );
    assert!(
        first.extract.contains("April 30, 1916"),
        "{}",
        first.extract
    );
}

#[test]
fn a_result_nothing_was_quoted_from_keeps_its_place() {
    // It is still an address worth fetching, and dropping it would make the
    // answer depend on what the model happened to write about rather than on
    // what the search found.
    let found = source(200, answer())
        .search("x", &Cancel::new())
        .expect("an answer that parses");

    let second = found.get(1).expect("a second result");
    assert_eq!(second.url.as_ref(), "https://example.com/uncited");
    assert_eq!(second.extract.as_ref(), "");
}

#[test]
fn a_search_declares_the_server_tool_and_sends_the_query_as_the_message() {
    let (source, replay) = built(200, answer());
    source.search("rust async traits", &Cancel::new()).ok();

    let sent: serde_json::Value =
        serde_json::from_str(&replay.sent().body).expect("a body that is JSON");

    assert_eq!(sent.pointer("/tools/0/type").unwrap(), &json!(SEARCH_TOOL));
    assert_eq!(sent.pointer("/tools/0/name").unwrap(), &json!("web_search"));
    assert_eq!(
        sent.pointer("/messages/0/content").unwrap(),
        &json!("rust async traits"),
    );
    assert_eq!(sent.pointer("/model").unwrap(), &json!("claude-opus-5"));

    // No `stream`: a side request is one small answer that is useless in
    // halves, and nobody is watching it arrive.
    assert!(sent.get("stream").is_none(), "{sent}");
}

#[test]
fn the_key_travels_in_a_header_and_nowhere_else() {
    let (source, replay) = built(200, answer());
    source.search("x", &Cancel::new()).ok();

    let sent = replay.sent();
    assert!(!sent.body.contains(SECRET), "the key reached the body");
    assert!(!sent.url.contains(SECRET), "the key reached the address");
    assert!(
        sent.headers
            .iter()
            .any(|(name, value)| name == "x-api-key" && value == SECRET),
        "the key did not reach its header",
    );
}

#[test]
fn a_search_reaches_the_vendor_host_a_rule_would_be_written_about() {
    assert_eq!(
        Search::reaches(&source(200, answer())),
        Host::Named {
            url: "https://api.anthropic.com/v1/messages".into(),
            host: "api.anthropic.com".into(),
        }
    );
}

#[test]
fn a_refusal_carries_the_status_and_never_the_key() {
    let problem = source(401, r#"{"error":{"message":"invalid x-api-key"}}"#)
        .search("x", &Cancel::new())
        .expect_err("a 401 to be refused");

    let said = problem.to_string();
    assert!(said.contains("401"), "{said}");
    assert!(!said.contains(SECRET), "the key reached an error: {said}");
}

#[test]
fn an_answer_that_is_not_json_is_a_protocol_failure_rather_than_a_panic() {
    let problem = source(200, "<html>a gateway wrote this</html>")
        .search("x", &Cancel::new())
        .expect_err("unparseable bytes to be refused");

    assert!(matches!(problem, SourceError::Protocol { .. }), "{problem}");
}

#[test]
fn a_search_that_found_nothing_answers_with_nothing_rather_than_failing() {
    let empty = json!({ "content": [{ "type": "text", "text": "I could not find it." }] });
    let found = source(200, empty.to_string())
        .search("x", &Cancel::new())
        .expect("an answer with no results to parse");

    assert!(found.is_empty());
}

#[test]
fn a_cancelled_search_sends_nothing() {
    let cancel = Cancel::new();
    cancel.request();

    let problem = source(200, answer())
        .search("x", &cancel)
        .expect_err("a cancelled call not to be sent");

    assert!(matches!(problem, SourceError::Cancelled(_)), "{problem}");
}

#[test]
fn an_address_carrying_user_information_is_opaque_and_never_fetched() {
    // The whole reason the opaque shape exists. A lenient read of this address
    // says `docs.rs`; the request would go to `evil.example`.
    let source = source(200, answer());

    assert!(matches!(
        Fetch::reaches(&source, "https://docs.rs@evil.example/"),
        Host::Opaque(_)
    ));

    let problem = source
        .fetch("https://docs.rs@evil.example/", &Cancel::new())
        .expect_err("an address that names no host to be refused before it is sent");

    assert!(matches!(problem, SourceError::Address(_)), "{problem}");
}

#[test]
fn a_scheme_that_is_not_http_is_refused_before_anything_is_sent() {
    let source = source(200, answer());

    for address in ["file:///etc/passwd", "ftp://example.com/x", "not a url"] {
        assert!(
            matches!(
                source.fetch(address, &Cancel::new()),
                Err(SourceError::Address(_))
            ),
            "{address} was not refused",
        );
    }
}

#[test]
fn a_fetched_page_reports_where_it_ended_up() {
    let moved = json!({
        "content": [{
            "type": "web_fetch_tool_result",
            "tool_use_id": "srvtoolu_2",
            "content": {
                "type": "web_fetch_result",
                "url": "https://example.com/moved-here",
                "content": {
                    "type": "document",
                    "title": "Moved",
                    "source": { "type": "text", "media_type": "text/plain", "data": "the body" }
                },
                "retrieved_at": "2026-08-18T10:30:00Z"
            }
        }]
    });

    let page = source(200, moved.to_string())
        .fetch("https://example.com/asked-for", &Cancel::new())
        .expect("a page that parses");

    assert_eq!(page.url.as_ref(), "https://example.com/moved-here");
    assert_eq!(page.title.as_deref(), Some("Moved"));
    assert_eq!(page.text.as_ref(), "the body");
}

#[test]
fn a_fetch_the_vendor_refused_says_which_way_it_refused() {
    let refused = json!({
        "content": [{
            "type": "web_fetch_tool_result",
            "tool_use_id": "srvtoolu_2",
            "content": { "type": "web_fetch_tool_result_error", "error_code": "url_not_accessible" }
        }]
    });

    let problem = source(200, refused.to_string())
        .fetch("https://example.com/gone", &Cancel::new())
        .expect_err("an error block to be a failure");

    assert!(
        problem.to_string().contains("url_not_accessible"),
        "{problem}"
    );
}

/// An answer shaped as the Responses API documents one: a search action, then
/// prose whose citations mark which run each address supports.
fn responded(text: &str, annotations: &serde_json::Value) -> String {
    json!({
        "output": [
            { "type": "web_search_call", "id": "ws_1", "status": "completed" },
            {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": text, "annotations": annotations.clone() }]
            }
        ]
    })
    .to_string()
}

fn openai(status: u16, body: impl Into<String>) -> (OpenAiWeb, Arc<Replay>) {
    let replay = Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bearer());

    (
        OpenAiWeb::new(
            Endpoint::fixed("https://api.openai.com/v1/responses"),
            Box::new(credential),
            Box::new(Arc::clone(&replay)),
            "gpt-5.6",
        ),
        replay,
    )
}

#[test]
fn a_citation_becomes_a_result_whose_extract_is_the_run_it_marks() {
    // This vendor reports results as annotations on its own prose rather than
    // as a block of their own, so the extract is sliced out of the very text
    // the citation annotates.
    let said = "Rust 1.97 shipped in August.";
    let body = responded(
        said,
        &json!([{
            "type": "url_citation",
            "url": "https://blog.rust-lang.org/2026/08/",
            "title": "Announcing Rust 1.97",
            "start_index": 0,
            "end_index": 17
        }]),
    );

    let found = openai(200, body)
        .0
        .search("rust release", &Cancel::new())
        .expect("an answer that parses");

    let first = found.first().expect("a result");
    assert_eq!(first.url.as_ref(), "https://blog.rust-lang.org/2026/08/");
    assert_eq!(first.title.as_ref(), "Announcing Rust 1.97");
    assert_eq!(first.extract.as_ref(), "Rust 1.97 shipped");
}

#[test]
fn an_index_that_would_panic_yields_no_extract_instead() {
    // The indices are the vendor's and the string is this program's. One past
    // the end, and one landing inside a multi-byte character, are both answers
    // a slice would die on.
    let said = "héllo";
    let body = responded(
        said,
        &json!([
            { "type": "url_citation", "url": "https://a.example", "start_index": 0, "end_index": 900 },
            { "type": "url_citation", "url": "https://b.example", "start_index": 1, "end_index": 2 }
        ]),
    );

    let found = openai(200, body)
        .0
        .search("x", &Cancel::new())
        .expect("an answer that parses");

    assert_eq!(found.len(), 2);
    for result in &found {
        assert_eq!(result.extract.as_ref(), "", "{}", result.url);
    }
}

#[test]
fn one_address_cited_twice_is_one_result() {
    let body = responded(
        "Two sentences. Both from one place.",
        &json!([
            { "type": "url_citation", "url": "https://one.example", "start_index": 0, "end_index": 14 },
            { "type": "url_citation", "url": "https://one.example", "start_index": 15, "end_index": 35 }
        ]),
    );

    let found = openai(200, body)
        .0
        .search("x", &Cancel::new())
        .expect("an answer that parses");

    assert_eq!(found.len(), 1);
}

#[test]
fn a_search_declares_the_hosted_tool_and_keeps_the_query_off_the_vendor_store() {
    let (source, replay) = openai(200, responded("x", &json!([])));
    source.search("rust async traits", &Cancel::new()).ok();

    let sent: serde_json::Value =
        serde_json::from_str(&replay.sent().body).expect("a body that is JSON");

    assert_eq!(sent.pointer("/tools/0/type").unwrap(), &json!("web_search"));
    assert_eq!(sent.pointer("/input").unwrap(), &json!("rust async traits"));
    assert_eq!(sent.pointer("/model").unwrap(), &json!("gpt-5.6"));

    // A query is the user's words, and this endpoint retains a response for
    // retrieval unless it is told not to.
    assert_eq!(sent.pointer("/store").unwrap(), &json!(false));
    assert!(!replay.sent().body.contains(SECRET));
}

#[test]
fn an_openai_search_reaches_the_vendor_host_a_rule_would_name() {
    assert_eq!(
        Search::reaches(&openai(200, responded("x", &json!([]))).0),
        Host::Named {
            url: "https://api.openai.com/v1/responses".into(),
            host: "api.openai.com".into(),
        }
    );
}
