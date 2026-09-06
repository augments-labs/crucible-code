//! Search-only proof, citation projection and shared transport failure paths.

use super::*;

fn searched() -> Vec<Value> {
    vec![
        json!({"type":"google_search_call","id":"search","arguments":{"queries":["Rust"]}}),
        json!({"type":"google_search_result","call_id":"search","result":[{"search_suggestions":"<div>native-suggestions-canary</div>"}]}),
        json!({"type":"model_output","content":[{"type":"text","text":"é Rust","annotations":[{"type":"url_citation","url":"https://rust-lang.org/","title":"Rust","start_index":3,"end_index":7}]}]}),
    ]
}

#[test]
fn google_search_cites_text_when_native_results_only_supply_suggestions() {
    let (source, replay) = source(&searched());
    let found = source.search("Rust", &Cancel::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(&*found.first().unwrap().extract, "Rust");
    assert!(!format!("{found:?}{source:?}").contains("native-suggestions-canary"));
    let sent = replay.sent();
    assert_eq!(sent.url, Google::VENDOR.as_str());
    assert!(
        sent.headers
            .iter()
            .any(|(key, value)| key == "x-goog-api-key" && value == "web-key-canary")
    );
    assert!(!sent.headers.iter().any(|(key, _)| key == "authorization"));
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(body.get("model").unwrap(), "gemini-3.8-flash");
    assert_eq!(body.get("stream").unwrap(), true);
    assert!(!sent.body.contains("previous_interaction_id"));
}

#[test]
fn google_search_deduplicates_urls_and_accepts_absent_optional_offsets() {
    let mut steps = json!(searched());
    *steps.pointer_mut("/2/content/0/annotations").unwrap() = json!([
        {"type":"url_citation","url":"https://rust-lang.org/"},
        {"type":"url_citation","url":"https://rust-lang.org/","title":"duplicate"},
        {"type":"url_citation","url":"javascript:bad"},
        {"type":"url_citation","url":"https://user:password@example.com/"},
        {"type":"url_citation","url":"https://example.com/\nforged"}
    ]);
    let (source, _) = source(steps.as_array().unwrap());
    assert_eq!(
        source.search("Rust", &Cancel::new()).unwrap(),
        [SearchResult {
            url: "https://rust-lang.org/".into(),
            title: "https://rust-lang.org/".into(),
            extract: "".into(),
        }]
    );
}

#[test]
fn google_search_requires_successful_matching_native_results_and_valid_citations() {
    for (pointer, replacement) in [
        ("/0/type", json!("url_context_call")),
        ("/1/call_id", json!("unmatched")),
        ("/1/result", json!({"error":"bad"})),
        ("/2/content/0/annotations", json!({})),
        ("/2/content/0/annotations/0/url", Value::Null),
        ("/2/content/0/annotations/0/start_index", json!(1)),
        ("/2/content/0/annotations/0/end_index", json!(100)),
        ("/2/content/0/annotations/0/start_index", json!(7)),
        ("/2/content/0/annotations/0/end_index", Value::Null),
    ] {
        let mut steps = json!(searched());
        *steps.pointer_mut(pointer).unwrap() = replacement;
        let (source, _) = source(steps.as_array().unwrap());
        assert!(source.search("Rust", &Cancel::new()).is_err(), "{pointer}");
    }
    for flag in [json!(true), json!("false")] {
        let mut steps = json!(searched());
        steps
            .pointer_mut("/1")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("is_error".into(), flag);
        let (source, _) = source(steps.as_array().unwrap());
        assert!(source.search("Rust", &Cancel::new()).is_err());
    }
    for skipped in [0, 1] {
        let mut steps = searched();
        steps.remove(skipped);
        let (source, _) = source(&steps);
        assert!(source.search("Rust", &Cancel::new()).is_err());
    }
}

#[test]
fn google_search_accepts_empty_results_only_after_a_native_search() {
    let mut steps = searched();
    steps.pop();
    let (source, _) = source(&steps);
    assert!(source.search("Rust", &Cancel::new()).unwrap().is_empty());
    let (source, _) = super::source(&searched().into_iter().skip(2).collect::<Vec<_>>());
    assert!(source.search("Rust", &Cancel::new()).is_err());
}

#[test]
fn google_search_rejects_partial_late_failed_and_oversized_streams() {
    let good = answer(&searched());
    let end = good
        .rfind("data: {\"event_type\":\"interaction.completed\"")
        .unwrap();
    for (status, body) in [
        (200, good.get(..end).unwrap().to_owned()),
        (
            200,
            good.replace("\"status\":\"completed\"", "\"status\":\"incomplete\""),
        ),
        (
            200,
            format!(
                "{good}data: {{\"event_type\":\"error\",\"message\":\"private-canary web-key-canary\"}}\n\n"
            ),
        ),
        (403, "private-canary web-key-canary".into()),
        (
            200,
            format!(
                "{good}{}",
                ": padding\n\n".repeat(crate::web::MOST / 10 + 1)
            ),
        ),
    ] {
        let (source, _) = source_body(status, body);
        let error = source.search("Rust", &Cancel::new()).unwrap_err();
        let shown = format!("{error} {error:?}");
        assert!(!shown.contains("private-canary"));
        assert!(!shown.contains("web-key-canary"));
    }
}

#[test]
fn google_search_prior_cancellation_never_posts() {
    let (source, replay) = source(&searched());
    let cancel = Cancel::new();
    cancel.request();
    assert!(matches!(
        source.search("Rust", &cancel),
        Err(SourceError::Cancelled("google"))
    ));
    assert!(replay.sent().url.is_empty());
}

#[test]
fn google_search_bounds_overlapping_citation_extracts_before_copying() {
    let mut steps = json!(searched());
    let text = "a".repeat(1_000_000);
    let citations: Vec<_> = (0..10).map(|n| json!({
        "type":"url_citation","url":format!("https://example.com/{n}"),"start_index":0,"end_index":text.len()
    })).collect();
    *steps.pointer_mut("/2/content/0/text").unwrap() = json!(text);
    *steps.pointer_mut("/2/content/0/annotations").unwrap() = json!(citations);
    let (source, _) = source(steps.as_array().unwrap());
    assert!(source.search("Rust", &Cancel::new()).is_err());
}
