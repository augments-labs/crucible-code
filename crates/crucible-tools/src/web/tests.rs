//! What the two web tools answer with, over sources that answer from memory.

use crucible_core::{
    Cancel, Fetch, Host, Page, Search, SearchResult, SourceError, Tool, ToolArgs, Unwatched,
};

use super::*;
use crate::sample;

/// A search that answers with whatever the test put in it.
struct Answers(Vec<SearchResult>);

impl Search for Answers {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn reaches(&self) -> Host {
        Host::Named {
            url: "https://search.example/".into(),
            host: "search.example".into(),
        }
    }

    fn search(&self, _query: &str, _cancel: &Cancel) -> Result<Vec<SearchResult>, SourceError> {
        Ok(self.0.clone())
    }
}

/// A source that cannot answer. `true` cancels; `false` refuses.
struct Breaks(bool);

impl Search for Breaks {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn reaches(&self) -> Host {
        Host::Named {
            url: "https://search.example/".into(),
            host: "search.example".into(),
        }
    }

    fn search(&self, _query: &str, _cancel: &Cancel) -> Result<Vec<SearchResult>, SourceError> {
        Err(if self.0 {
            SourceError::Cancelled("fake")
        } else {
            SourceError::Refused {
                named: "fake",
                status: 503,
                message: "busy".into(),
            }
        })
    }
}

/// A fetch that hands back one page, from wherever it says it ended up.
struct Pages(Page);

impl Fetch for Pages {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn reaches(&self, url: &str) -> Host {
        let named = url.strip_prefix("https://").and_then(|rest| {
            let host = rest.split('/').next()?;
            (!host.is_empty() && !host.contains('@')).then(|| host.to_owned())
        });

        match named {
            Some(host) => Host::Named {
                url: url.into(),
                host: host.into(),
            },
            None => Host::Opaque(url.into()),
        }
    }

    fn fetch(&self, _url: &str, _cancel: &Cancel) -> Result<Page, SourceError> {
        Ok(self.0.clone())
    }
}

fn result(title: &str, url: &str, extract: &str) -> SearchResult {
    SearchResult {
        title: title.into(),
        url: url.into(),
        extract: extract.into(),
    }
}

fn searching(results: Vec<SearchResult>) -> WebSearch {
    WebSearch::new(Arc::new(Answers(results)), Cancel::new())
}

fn fetching(url: &str, title: Option<&str>, text: &str) -> WebFetch {
    WebFetch::new(
        Arc::new(Pages(Page {
            url: url.into(),
            title: title.map(Into::into),
            text: text.into(),
        })),
        Cancel::new(),
    )
}

#[test]
fn a_result_carries_its_title_its_address_and_its_extract() {
    let tool = searching(vec![result("Serde", "https://serde.rs", "A framework.")]);
    let output = tool
        .run(sample::allowed(&tool, r#"{"query":"serde"}"#), &Unwatched)
        .expect("a source that answers");

    let said = output.text();
    assert!(said.contains("Serde"), "{said}");
    assert!(said.contains("https://serde.rs"), "{said}");
    assert!(said.contains("A framework."), "{said}");
    assert!(!output.is_failed());
}

#[test]
fn a_search_that_found_nothing_says_so_rather_than_answering_with_nothing() {
    // An empty answer and a failed one are different facts, and a model that
    // cannot tell them apart searches again for something that is not there.
    let tool = searching(Vec::new());
    let output = tool
        .run(sample::allowed(&tool, r#"{"query":"nothing"}"#), &Unwatched)
        .expect("a source that answers");

    assert!(!output.is_failed());
    assert!(output.text().contains("No results"), "{}", output.text());
}

#[test]
fn a_limit_keeps_that_many_and_counts_what_it_left() {
    let many = (0..5)
        .map(|at| result(&format!("Page {at}"), "https://example.com", "..."))
        .collect();
    let tool = searching(many);
    let output = tool
        .run(
            sample::allowed(&tool, r#"{"query":"x","limit":2}"#),
            &Unwatched,
        )
        .expect("a source that answers");

    let said = output.text();
    assert!(said.contains("Page 0") && said.contains("Page 1"), "{said}");
    assert!(!said.contains("Page 2"), "{said}");
    assert!(said.contains("3 not shown"), "{said}");
}

#[test]
fn a_search_reaches_the_host_its_source_names() {
    // Not the query and not an argument: what a search reaches was settled when
    // the user chose whose credential answers it.
    let tool = searching(Vec::new());

    assert_eq!(
        tool.sensitivity(&ToolArgs::new(r#"{"query":"anything at all"}"#)),
        Sensitivity::ReachesNetwork {
            host: Host::Named {
                url: "https://search.example/".into(),
                host: "search.example".into(),
            },
        },
    );
}

#[test]
fn a_fetch_reaches_the_host_it_was_pointed_at() {
    let tool = fetching("https://docs.rs/serde", None, "...");

    let Sensitivity::ReachesNetwork { host } =
        tool.sensitivity(&ToolArgs::new(r#"{"url":"https://docs.rs/serde"}"#))
    else {
        panic!("a fetch reaches the network");
    };

    assert_eq!(host.to_string(), "docs.rs");
}

#[test]
fn a_fetch_nobody_could_read_an_address_out_of_matches_no_host_rule() {
    // The whole point of the opaque shape. `https://docs.rs@evil.example/` is
    // the reading that guessing gets wrong, and a rule about `docs.rs` must not
    // reach it.
    let tool = fetching("https://evil.example", None, "...");

    let Sensitivity::ReachesNetwork { host } =
        tool.sensitivity(&ToolArgs::new(r#"{"url":"https://docs.rs@evil.example/"}"#))
    else {
        panic!("a fetch reaches the network");
    };

    assert!(
        matches!(host, Host::Opaque(_)),
        "an address carrying user information was read into a host",
    );
    assert_eq!(host.to_string(), "https://docs.rs@evil.example/");
}

#[test]
fn a_page_says_where_it_actually_came_from() {
    // Not where it was asked for. Everything the model does next with this page
    // depends on where it ended up, a redirect being the case that matters.
    let tool = fetching("https://example.com/moved-here", Some("Moved"), "the body");

    let output = tool
        .run(
            sample::allowed(&tool, r#"{"url":"https://example.com/asked-for"}"#),
            &Unwatched,
        )
        .expect("a source that answers");

    let said = output.text();
    assert!(said.contains("https://example.com/moved-here"), "{said}");
    assert!(said.contains("the body"), "{said}");
}

#[test]
fn a_source_that_could_not_answer_is_a_failed_result_and_not_a_broken_tool() {
    // The turn carries on and the model is told, the same as a file that is not
    // there. A source being down is not a breakdown of the mechanism.
    let tool = WebSearch::new(Arc::new(Breaks(false)), Cancel::new());
    let output = tool
        .run(sample::allowed(&tool, r#"{"query":"x"}"#), &Unwatched)
        .expect("a source failure to reach the model rather than the runner");

    assert!(output.is_failed());
    assert!(output.text().contains("503"), "{}", output.text());
}

#[test]
fn a_cancelled_search_ends_the_call_rather_than_answering_it() {
    let tool = WebSearch::new(Arc::new(Breaks(true)), Cancel::new());
    let problem = tool
        .run(sample::allowed(&tool, r#"{"query":"x"}"#), &Unwatched)
        .expect_err("cancellation not to come back as an answer");

    assert!(matches!(problem, ToolError::Cancelled("web_search")));
}
