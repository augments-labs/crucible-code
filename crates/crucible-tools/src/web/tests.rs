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
            sent: "https://search.example/".into(),
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
            sent: "https://search.example/".into(),
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
                sent: url.into(),
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
fn a_search_question_shows_the_query_and_the_host_it_goes_to() {
    // Both facts. The host was settled when the user chose a provider and is
    // the same whatever is asked; the query is the thing that actually leaves
    // the machine, and a question naming only the endpoint would be asking for
    // approval without quoting a word of the request.
    let tool = searching(Vec::new());

    assert_eq!(
        tool.sensitivity(&ToolArgs::new(r#"{"query":"anything at all"}"#)),
        Sensitivity::ReachesNetwork {
            host: Host::Named {
                sent: "anything at all".into(),
                host: "search.example".into(),
            },
        },
    );
}

#[test]
fn a_search_nobody_could_read_a_query_out_of_still_names_its_host() {
    // The call is refused a moment later by `run`; what this must not do is
    // lose the host a rule is written about while the arguments are unreadable.
    let tool = searching(Vec::new());

    let Sensitivity::ReachesNetwork { host } = tool.sensitivity(&ToolArgs::new("{}")) else {
        panic!("a search reaches the network");
    };
    assert_eq!(host.to_string(), "search.example");
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
fn a_redirect_to_another_host_does_not_come_back_under_the_first_one_s_verdict() {
    // The verdict was about the host in the address that was asked for. A
    // redirect elsewhere is a host nobody has been asked about, and answering
    // with its content would let one allowed host carry any other.
    let tool = fetching("https://evil.example/landed", None, "a page nobody allowed");

    let output = tool
        .run(
            sample::allowed(&tool, r#"{"url":"https://docs.rs/serde"}"#),
            &Unwatched,
        )
        .expect("a source that answers");

    assert!(output.is_failed());
    assert!(
        !output.text().contains("a page nobody allowed"),
        "the body of an unapproved host came back: {}",
        output.text(),
    );
    assert!(output.text().contains("evil.example"), "{}", output.text());
}

#[test]
fn a_redirect_inside_one_host_is_still_that_host_and_comes_back() {
    let tool = fetching("https://docs.rs/serde/latest/", Some("Serde"), "the body");

    let output = tool
        .run(
            sample::allowed(&tool, r#"{"url":"https://docs.rs/serde"}"#),
            &Unwatched,
        )
        .expect("a source that answers");

    assert!(!output.is_failed(), "{}", output.text());
    assert!(output.text().contains("the body"), "{}", output.text());
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

#[test]
fn a_page_over_the_bound_comes_back_cut_rather_than_empty() {
    // It used to come back with nothing at all: the page went to the bound as
    // one item, and `within` keeps whole items. Most pages worth fetching are
    // over the bound, so `web_fetch` answered almost nothing with almost
    // everything.
    let long = "a line of some length that repeats\n".repeat(2_000);
    let tool = fetching("https://example.com/long", Some("Long"), &long);

    let output = tool
        .run(
            sample::allowed(&tool, r#"{"url":"https://example.com/long"}"#),
            &Unwatched,
        )
        .expect("a source that answers");

    let said = output.text();
    assert!(!output.is_failed(), "{said}");
    assert!(
        said.contains("a line of some length"),
        "a long page came back with no content",
    );
    assert!(said.contains("not shown"), "nothing said what was cut");
    assert!(
        said.len() < 40_000,
        "the bound did not hold: {}",
        said.len()
    );
}
