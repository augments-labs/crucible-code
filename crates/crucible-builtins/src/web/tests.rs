//! What the two web tools answer with, over sources that answer from memory.

use crucible_runtime::Cancel;
use crucible_tools::{Fetch, Host, Page, Search, SearchResponse, SearchResult, SourceError, Tool};
use crucible_types::{ResultProvenance, ToolArgs};

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

    fn search(&self, _query: &str, _cancel: &Cancel) -> Result<SearchResponse, SourceError> {
        Ok(self.0.clone().into())
    }
}

/// A grounded search that answers with answer text, citations, and suggestions.
struct GroundedAnswers {
    answer: &'static str,
    results: Vec<SearchResult>,
    suggestions: &'static str,
}

impl Search for GroundedAnswers {
    fn name(&self) -> &'static str {
        "google"
    }

    fn reaches(&self) -> Host {
        Host::Named {
            sent: "https://generativelanguage.googleapis.com".into(),
            host: "generativelanguage.googleapis.com".into(),
        }
    }

    fn search(&self, _query: &str, _cancel: &Cancel) -> Result<SearchResponse, SourceError> {
        Ok(SearchResponse::grounded(
            self.answer,
            self.results.clone(),
            self.suggestions,
        ))
    }
}

/// A grounded search whose vendor keeps what it answers to its own models.
struct Kept;

impl Search for Kept {
    fn name(&self) -> &'static str {
        "google"
    }

    fn reaches(&self) -> Host {
        Host::Named {
            sent: "https://generativelanguage.googleapis.com".into(),
            host: "generativelanguage.googleapis.com".into(),
        }
    }

    fn restricts(&self) -> Option<&'static str> {
        Some("[cleared — kept to the vendor that answered it]")
    }

    fn search(&self, _query: &str, _cancel: &Cancel) -> Result<SearchResponse, SourceError> {
        Ok(SearchResponse::grounded(
            "an answer",
            vec![SearchResult {
                title: "A page".into(),
                url: "https://example.com".into(),
                extract: "what it says".into(),
            }],
            "",
        ))
    }
}

/// A source whose terms are longer than a result can carry, and which says
/// whether it was asked anything anyway.
struct Oversized {
    notice: &'static str,
    asked: std::sync::atomic::AtomicBool,
}

impl Search for Oversized {
    fn name(&self) -> &'static str {
        "oversized"
    }

    fn reaches(&self) -> Host {
        Host::Named {
            sent: "https://search.example/".into(),
            host: "search.example".into(),
        }
    }

    fn restricts(&self) -> Option<&'static str> {
        Some(self.notice)
    }

    fn search(&self, _query: &str, _cancel: &Cancel) -> Result<SearchResponse, SourceError> {
        self.asked.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(SearchResponse::results(Vec::new()))
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

    fn search(&self, _query: &str, _cancel: &Cancel) -> Result<SearchResponse, SourceError> {
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

/// A source that refuses with a reply of the test's own making.
struct Refuses(String);

impl Search for Refuses {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn reaches(&self) -> Host {
        Host::Named {
            sent: "https://search.example/".into(),
            host: "search.example".into(),
        }
    }

    fn search(&self, _query: &str, _cancel: &Cancel) -> Result<SearchResponse, SourceError> {
        Err(SourceError::Refused {
            named: "fake",
            status: 400,
            message: self.0.as_str().into(),
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
    WebSearch::new(Arc::new(Answers(results)))
}

/// The head of an answer, for a failure message that must not print a reply
/// the size of the bound.
fn head(said: &str) -> String {
    said.chars().take(80).collect()
}

fn fetching(url: &str, title: Option<&str>, text: &str) -> WebFetch {
    WebFetch::new(Arc::new(Pages(Page {
        url: url.into(),
        title: title.map(Into::into),
        text: text.into(),
    })))
}

#[test]
fn a_result_carries_its_title_its_address_and_its_extract() {
    let tool = searching(vec![result("Serde", "https://serde.rs", "A framework.")]);
    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"query":"serde"}"#),
        &crate::sample::context(),
    ))
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
    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"query":"nothing"}"#),
        &crate::sample::context(),
    ))
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
    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"query":"x","limit":2}"#),
        &crate::sample::context(),
    ))
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

    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"url":"https://docs.rs/serde"}"#),
        &crate::sample::context(),
    ))
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

    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"url":"https://docs.rs/serde"}"#),
        &crate::sample::context(),
    ))
    .expect("a source that answers");

    assert!(!output.is_failed(), "{}", output.text());
    assert!(output.text().contains("the body"), "{}", output.text());
}

#[test]
fn a_page_says_where_it_actually_came_from() {
    // Not where it was asked for. Everything the model does next with this page
    // depends on where it ended up, a redirect being the case that matters.
    let tool = fetching("https://example.com/moved-here", Some("Moved"), "the body");

    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"url":"https://example.com/asked-for"}"#),
        &crate::sample::context(),
    ))
    .expect("a source that answers");

    let said = output.text();
    assert!(said.contains("https://example.com/moved-here"), "{said}");
    assert!(said.contains("the body"), "{said}");
}

#[test]
fn a_source_that_could_not_answer_is_a_failed_result_and_not_a_broken_tool() {
    // The turn carries on and the model is told, the same as a file that is not
    // there. A source being down is not a breakdown of the mechanism.
    let tool = WebSearch::new(Arc::new(Breaks(false)));
    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"query":"x"}"#),
        &crate::sample::context(),
    ))
    .expect("a source failure to reach the model rather than the runner");

    assert!(output.is_failed());
    assert_eq!(output.text(), "web source error: fake: HTTP 503: busy\n");
}

#[test]
fn a_refusal_longer_than_the_bound_says_what_it_left_out() {
    // A refusal carries the service's whole reply, which can be a whole error
    // page. Cut with nothing saying so, it reads to the model as everything the
    // service said — so the model works around a problem it was told half of.
    let line = "the service explained itself at length\n";
    let lines = 2_000;
    let tool = WebSearch::new(Arc::new(Refuses(line.repeat(lines))));

    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"query":"x"}"#),
        &crate::sample::context(),
    ))
    .expect("a source failure to reach the model rather than the runner");

    assert!(output.is_failed());
    let said = output.text();
    assert!(
        said.starts_with(&format!("web source error: fake: HTTP 400: {line}")),
        "the refusal lost the source, the status and the head of the reply: {:?}",
        head(said),
    );
    // `CUT` is the widest ending this answer can earn, so everything in front
    // of it is room the head had, and a head that took it stops one line short.
    assert!(
        said.len() > bound::OUTPUT - super::CUT.len() - line.len(),
        "the answer stopped a whole line short of the room it had: {} bytes",
        said.len(),
    );
    // Every line of the answer but its ending came from the reply, so the count
    // the ending owes is the reply's lines less the ones that survived.
    let shown = said.matches(line.trim_end()).count();
    let ending = format!("[{} more lines not shown.]", lines - shown);
    assert_eq!(
        said.lines().next_back(),
        Some(ending.as_str()),
        "the ending does not name the lines the answer left out",
    );
    assert!(
        said.len() <= bound::OUTPUT,
        "the bound did not hold: {}",
        said.len(),
    );
}

#[test]
fn a_refusal_of_one_line_over_the_bound_still_names_the_source_and_the_status() {
    // A minified error body is one line, and a bound that keeps whole lines
    // kept none of it: what came back was that the tool could not answer, with
    // neither the vendor nor the status the service refused with in it.
    let tool = WebSearch::new(Arc::new(Refuses("x".repeat(bound::OUTPUT * 2))));

    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"query":"x"}"#),
        &crate::sample::context(),
    ))
    .expect("a source failure to reach the model rather than the runner");

    assert!(output.is_failed());
    let said = output.text();
    let kept = said
        .strip_suffix(super::CUT)
        .expect("a reply twice the bound to end by saying it was cut");
    assert!(
        kept.starts_with("web source error: fake: HTTP 400: x"),
        "the refusal lost the source and the status: {:?}",
        head(said),
    );
    // One line earns the widest ending this answer has, and the head is
    // entitled to everything in front of it.
    assert_eq!(
        kept.len(),
        bound::OUTPUT - super::CUT.len(),
        "the head did not fill the room its ending left it",
    );
    assert!(
        said.len() <= bound::OUTPUT,
        "the bound did not hold: {}",
        said.len(),
    );
}

#[test]
fn a_refusal_cut_inside_a_line_is_cut_between_characters() {
    // The bound is a count of bytes and a reply is somebody else's text, so
    // the byte the cut lands on is not a character boundary: behind this
    // thirty-four byte prefix, three-byte characters put a continuation byte
    // there. Cut on the byte and the answer is a panic, or nothing at all.
    let letter = "€";
    let body = letter.repeat(bound::OUTPUT);
    let tool = WebSearch::new(Arc::new(Refuses(body.clone())));

    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"query":"x"}"#),
        &crate::sample::context(),
    ))
    .expect("a source failure to reach the model rather than the runner");

    assert!(output.is_failed());
    let said = output.text();
    let reply = format!("web source error: fake: HTTP 400: {body}");
    let kept = said
        .strip_suffix(super::CUT)
        .expect("a reply three times the bound to end by saying it was cut");
    assert!(
        reply.starts_with(kept),
        "the answer is not the head of the reply: {:?}",
        head(said),
    );
    assert!(
        kept.len() + letter.len() > bound::OUTPUT - super::CUT.len(),
        "the head stopped {} bytes short of the room its ending left it",
        bound::OUTPUT - super::CUT.len() - kept.len(),
    );
    assert!(
        said.len() <= bound::OUTPUT,
        "the bound did not hold: {}",
        said.len(),
    );
}

#[test]
fn a_refusal_that_fills_the_room_exactly_does_not_claim_a_cut() {
    // A reply of one line filling the room an ending leaves to the byte fits
    // the bound whole, newline and all. A sentence saying the rest was cut is
    // then about nothing, and the model is told it is missing what it is
    // holding.
    let prefix = "web source error: fake: HTTP 400: ";
    let body = "x".repeat(bound::OUTPUT - super::CUT.len() - prefix.len());
    let tool = WebSearch::new(Arc::new(Refuses(body.clone())));

    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"query":"x"}"#),
        &crate::sample::context(),
    ))
    .expect("a source failure to reach the model rather than the runner");

    assert!(output.is_failed());
    let said = output.text();
    assert!(
        !said.ends_with(super::CUT),
        "the answer claimed a cut it did not make: {} bytes",
        said.len(),
    );
    assert!(
        said == format!("{prefix}{body}\n"),
        "a reply that fits whole did not come back whole: {:?}, {} bytes",
        head(said),
        said.len(),
    );
}

#[test]
fn a_refusal_whose_whole_lines_fit_the_bound_comes_back_whole() {
    // Room for an ending is owed only to an answer that leaves something out.
    // Kept back from one that fits, it cuts a reply the bound would have
    // carried and then tells the model the reply was too long.
    let prefix = "web source error: fake: HTTP 400: ";
    let first = "A".repeat(bound::OUTPUT - super::CUT.len() - prefix.len());
    let tool = WebSearch::new(Arc::new(Refuses(format!("{first}\nB"))));

    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"query":"x"}"#),
        &crate::sample::context(),
    ))
    .expect("a source failure to reach the model rather than the runner");

    assert!(output.is_failed());
    let said = output.text();
    assert!(
        !said.ends_with(super::CUT) && !said.ends_with("more lines not shown.]"),
        "a reply inside the bound was said to have been cut: {} bytes",
        said.len(),
    );
    assert!(
        said == format!("{prefix}{first}\nB\n"),
        "a reply inside the bound did not come back whole: {:?}, {} bytes",
        head(said),
        said.len(),
    );
}

#[test]
fn a_refusal_of_lines_inside_the_bound_is_not_cut_for_an_ending_it_does_not_need() {
    // Two lines taking, newlines included, the bound less half the widest
    // ending: past what is left once an ending's room is kept, inside the
    // bound itself.
    let each = (bound::OUTPUT - super::CUT.len() / 2) / 2;
    assert!(
        2 * each > bound::OUTPUT - super::CUT.len() && 2 * each <= bound::OUTPUT,
        "the reply no longer lands between the ending's room and the bound",
    );
    let prefix = "web source error: fake: HTTP 400: ";
    let first = "A".repeat(each - 1 - prefix.len());
    let second = "B".repeat(each - 1);
    let tool = WebSearch::new(Arc::new(Refuses(format!("{first}\n{second}"))));

    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"query":"x"}"#),
        &crate::sample::context(),
    ))
    .expect("a source failure to reach the model rather than the runner");

    assert!(output.is_failed());
    let said = output.text();
    assert!(
        !said.ends_with(super::CUT) && !said.ends_with("more lines not shown.]"),
        "a reply inside the bound was said to have been cut: {} bytes",
        said.len(),
    );
    assert!(
        said == format!("{prefix}{first}\n{second}\n"),
        "a reply inside the bound did not come back whole: {:?}, {} bytes",
        head(said),
        said.len(),
    );
}

#[test]
fn a_cancelled_search_ends_the_call_rather_than_answering_it() {
    let tool = WebSearch::new(Arc::new(Breaks(true)));
    let problem = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"query":"x"}"#),
        &crate::sample::context(),
    ))
    .expect_err("cancellation not to come back as an answer");

    assert!(matches!(problem, ToolError::Cancelled(ref tool) if &**tool == "web_search"));
}

#[test]
fn a_page_over_the_bound_comes_back_cut_rather_than_empty() {
    // It used to come back with nothing at all: the page went to the bound as
    // one item, and `within` keeps whole items. Most pages worth fetching are
    // over the bound, so `web_fetch` answered almost nothing with almost
    // everything.
    let long = "a line of some length that repeats\n".repeat(2_000);
    let tool = fetching("https://example.com/long", Some("Long"), &long);

    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"url":"https://example.com/long"}"#),
        &crate::sample::context(),
    ))
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

#[test]
fn a_grounded_search_shows_answer_citations_and_suggestions_together() {
    let source = Arc::new(GroundedAnswers {
        answer: "Rust is a systems programming language focusing on safety and speed.",
        results: vec![SearchResult {
            title: "Rust Home".into(),
            url: "https://www.rust-lang.org".into(),
            extract: "Official website".into(),
        }],
        suggestions: "- [learn rust](https://www.google.com/search?q=learn+rust)",
    });
    let tool = WebSearch::new(source);
    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"query":"rust language"}"#),
        &crate::sample::context(),
    ))
    .expect("a source that answers");

    assert!(!output.is_failed());
    let said = output.text();
    assert!(
        said.contains("Rust is a systems programming language"),
        "{said}"
    );
    assert!(said.contains("Sources:"), "{said}");
    assert!(said.contains("Rust Home"), "{said}");
    assert!(said.contains("https://www.rust-lang.org"), "{said}");
    assert!(said.contains("Search Suggestions:"), "{said}");
    assert!(said.contains("learn rust"), "{said}");
    assert!(
        said.contains("https://www.google.com/search?q=learn+rust"),
        "{said}"
    );
}

#[test]
fn search_schema_does_not_encourage_automated_fetch_crawling() {
    let tool = searching(Vec::new());
    let schema = tool.schema();
    assert!(
        !schema.contains("follow a result with web_fetch"),
        "search schema encourages automated crawling: {schema}"
    );
}

#[test]
fn web_calls_volunteer_for_lookup_grouping() {
    assert!(
        searching(Vec::new())
            .looking(&ToolArgs::new(r#"{"query":"rust"}"#))
            .is_some()
    );
    assert!(
        fetching("https://example.com", None, "page")
            .looking(&ToolArgs::new(r#"{"url":"https://example.com"}"#))
            .is_some()
    );
}

#[test]
fn a_search_result_says_which_vendor_answered_it_and_what_its_terms_keep() {
    // The tool is the one place that knows a result came from a source rather
    // than from this machine, so it is where the result has to say so. A vendor
    // with no term is still named: an answer an older build wrote says nothing,
    // and nothing is how that answer is told apart from this one.
    let kept = WebSearch::new(Arc::new(Kept));
    let output = crucible_runtime::answered!(kept.run(
        sample::allowed(&kept, r#"{"query":"rust"}"#),
        &crate::sample::context(),
    ))
    .expect("a source that answers");
    assert_eq!(
        output.provenance(),
        &ResultProvenance::answered(
            "google",
            Some("[cleared — kept to the vendor that answered it]")
        )
        .expect("a bounded term")
    );

    let open = WebSearch::new(Arc::new(Answers(Vec::new())));
    let output = crucible_runtime::answered!(open.run(
        sample::allowed(&open, r#"{"query":"rust"}"#),
        &crate::sample::context(),
    ))
    .expect("a source that answers");
    assert_eq!(
        output.into_recorded().provenance(),
        &ResultProvenance::answered("fake", None).expect("a bounded vendor")
    );
}

#[test]
fn a_source_whose_terms_do_not_fit_a_result_is_never_asked() {
    // Its answer would leave here saying less than the vendor's terms require,
    // so the call fails before anything reaches the source.
    let notice: &'static str = Box::leak(
        "n".repeat(crucible_types::RESULT_NOTICE_BYTES + 1)
            .into_boxed_str(),
    );
    let source = Arc::new(Oversized {
        notice,
        asked: std::sync::atomic::AtomicBool::new(false),
    });
    let tool = WebSearch::new(Arc::clone(&source) as Arc<dyn Search>);
    let output = crucible_runtime::answered!(tool.run(
        sample::allowed(&tool, r#"{"query":"rust"}"#),
        &crate::sample::context(),
    ))
    .expect("a refusal is an answer, not an error");

    assert!(output.is_failed(), "{}", output.text());
    assert!(output.text().contains("do not fit"), "{}", output.text());
    assert!(
        !source.asked.load(std::sync::atomic::Ordering::SeqCst),
        "the source was asked although its terms could not be carried"
    );
}
