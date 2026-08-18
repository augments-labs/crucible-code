//! What a look-up finds, what it offers afterwards, and what it refuses to.

use crucible_core::Unwatched;

use super::*;
use crate::sample;

fn held() -> Vec<Held> {
    vec![
        Held {
            name: "web_search".into(),
            about: "Searches the web and returns titles, addresses and short extracts.".into(),
        },
        Held {
            name: "web_fetch".into(),
            about: "Fetches one web page and returns it as text.".into(),
        },
        Held {
            name: "todo_write".into(),
            about: "Writes down the plan for the work in hand.".into(),
        },
        Held {
            name: "notes".into(),
            about: "Keeps a note about the work in hand.".into(),
        },
    ]
}

fn searching() -> (ToolSearch, Revealed) {
    let revealed = Revealed::new();
    (ToolSearch::new(held(), revealed.clone()), revealed)
}

fn looked_up(query: &str) -> (String, Revealed) {
    let (tool, revealed) = searching();
    let args = format!(
        r#"{{"query":{}}}"#,
        serde_json::to_string(query).expect("a query that encodes")
    );
    let output = tool
        .run(sample::allowed(&tool, &args), &Unwatched)
        .expect("a search to answer");

    (output.text().to_owned(), revealed)
}

#[test]
fn what_is_found_becomes_callable() {
    // The whole point. Finding a tool is not reading about it — it is in the
    // list from the next request onward, which is what the answer promises.
    let (said, revealed) = looked_up("web search");

    assert!(revealed.holds("web_search"), "{said}");
    assert!(said.contains("web_search"), "{said}");
    assert!(said.contains("tool list"), "{said}");
}

#[test]
fn nothing_unasked_for_is_offered() {
    let (said, revealed) = looked_up("todo");

    assert!(revealed.holds("todo_write"), "{said}");
    assert!(
        !revealed.holds("web_search"),
        "a search for one thing offered another",
    );
}

#[test]
fn a_name_given_exactly_wins() {
    // A model that knows what it wants says so and is right, ahead of anything
    // that merely mentions the same word.
    let (said, revealed) = looked_up("web_fetch");

    assert!(revealed.holds("web_fetch"), "{said}");
}

#[test]
fn an_empty_query_is_not_a_way_to_ask_for_everything() {
    // The hole worth closing: matching by vacuum would make the one tool that
    // is always advertised into a way of asking for the whole registry, which
    // is what deferring them was for.
    // An absent query never reaches the matching at all — the arguments are
    // refused, which is the same answer by an earlier route.
    let (tool, revealed) = searching();
    tool.run(sample::allowed(&tool, r#"{"query":""}"#), &Unwatched)
        .expect_err("an empty query to be refused as arguments");
    assert!(!revealed.holds("web_search"));

    // These do reach it, and must come back with nothing.
    for query in ["   ", "a", "??", "-- --"] {
        let (tool, revealed) = searching();
        let args = format!(
            r#"{{"query":{}}}"#,
            serde_json::to_string(query).expect("a query that encodes")
        );
        tool.run(sample::allowed(&tool, &args), &Unwatched)
            .expect("a search to answer");

        for name in ["web_search", "web_fetch", "todo_write", "notes"] {
            assert!(!revealed.holds(name), "{query:?} offered {name}");
        }
    }
}

#[test]
fn one_search_offers_at_most_a_few() {
    // A query broad enough to reach everything is the other way to ask for the
    // whole registry at once. The model can search again; it cannot ask once
    // and be handed all of it.
    let (_, revealed) = looked_up("web page plan note work hand text");

    let offered = ["web_search", "web_fetch", "todo_write", "notes"]
        .into_iter()
        .filter(|name| revealed.holds(name))
        .count();

    assert!(offered <= MOST, "one search offered {offered}");
}

#[test]
fn a_query_matching_nothing_says_so_and_offers_nothing() {
    // Not a failure: asking for something absent is an answer, and the model
    // should learn that what it can already see is the whole of it.
    let (tool, revealed) = searching();
    let output = tool
        .run(
            sample::allowed(&tool, r#"{"query":"frobnicate"}"#),
            &Unwatched,
        )
        .expect("a search to answer");

    assert!(!output.is_failed(), "{}", output.text());
    assert!(
        output.text().contains("Nothing held back"),
        "{}",
        output.text()
    );
    assert!(!revealed.holds("web_search"));
}

#[test]
fn the_answer_never_carries_a_schema() {
    // Printing one would spend the tokens this exists to save twice over: once
    // in the answer, and again in the tool list it then joins.
    let (said, _) = looked_up("web");

    assert!(!said.contains("\"type\""), "{said}");
    assert!(!said.contains("properties"), "{said}");
}

#[test]
fn a_session_deferring_nothing_has_nothing_to_look_up() {
    assert!(ToolSearch::new(Vec::new(), Revealed::new()).is_empty());
    assert!(!ToolSearch::new(held(), Revealed::new()).is_empty());
}

#[test]
fn looking_the_same_tool_up_twice_is_not_a_mistake() {
    let (tool, revealed) = searching();
    for _ in 0..2 {
        let output = tool
            .run(
                sample::allowed(&tool, r#"{"query":"web_search"}"#),
                &Unwatched,
            )
            .expect("a search to answer");
        assert!(!output.is_failed());
    }

    assert!(revealed.holds("web_search"));
}
