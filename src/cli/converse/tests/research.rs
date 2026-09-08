//! Real web tools over in-memory sources, through the live event loop.

use std::sync::Arc;

use crucible_core::{Cancel, Fetch, Host, Page, Search, SearchResponse, SourceError};

use super::*;

struct Web;

impl Search for Web {
    fn name(&self) -> &'static str {
        "fixture"
    }
    fn reaches(&self) -> Host {
        Host::Named {
            sent: "https://example.com".into(),
            host: "example.com".into(),
        }
    }
    fn search(&self, _: &str, _: &Cancel) -> Result<SearchResponse, SourceError> {
        Ok(SearchResponse::results(Vec::new()))
    }
}

impl Fetch for Web {
    fn name(&self) -> &'static str {
        "fixture"
    }
    fn reaches(&self, url: &str) -> Host {
        Host::Named {
            sent: url.into(),
            host: "example.com".into(),
        }
    }
    fn fetch(&self, url: &str, _: &Cancel) -> Result<Page, SourceError> {
        if url.ends_with("missing") {
            return Err(SourceError::Refused {
                named: "fixture",
                status: 404,
                message: "page missing".into(),
            });
        }
        Ok(Page {
            url: url.into(),
            title: Some("Reference".into()),
            text: "retained page text".into(),
        })
    }
}

fn researching(failed: bool) -> String {
    let mut tools = Tools::new();
    tools
        .add_builtin(crucible_tools::WebSearch::new(Arc::new(Web)))
        .unwrap();
    tools
        .add_builtin(crucible_tools::WebFetch::new(Arc::new(Web)))
        .unwrap();
    let mut batch = Vec::new();
    for (at, name) in ["web_search", "web_search", "web_fetch", "web_fetch"]
        .iter()
        .enumerate()
    {
        batch.push(Delta::ToolStarted {
            id: ToolId::new(format!("c-{at}")),
            name: (*name).into(),
        });
        let args = if *name == "web_search" {
            serde_json::json!({"query":"rust reference"})
        } else {
            serde_json::json!({"url":if failed && at == 2 {"https://example.com/missing"} else {"https://example.com/reference"}})
        };
        batch.push(Delta::ToolArgs(args.to_string().into()));
    }
    batch.push(Delta::Stopped(StopReason::WantsTools));
    let runner = scripted(
        Script::new(vec![batch, saying("Research finished.")]),
        tools,
    )
    .permitting(crucible_core::Permission::with(
        crucible_core::Mode::FullAccess,
        crucible_core::Rules::new(),
    ));
    let mut renderer = Renderer::new(Recording::new(100, 30));
    let mut input = Cursor::new(b"research\n".to_vec());
    converse(runner, &mut renderer, &plain(), &opening(), &mut input).unwrap();
    renderer.terminal().picture().said().join("\n")
}

#[test]
fn successful_web_calls_share_a_live_transcript_group_without_a_hint() {
    let picture = researching(false);
    assert!(
        picture.contains("Searched the web 2 times, fetched 2 pages"),
        "{picture}"
    );
    assert!(!picture.contains("ctrl+o"), "{picture}");
    assert!(!picture.contains("WebFetch("), "{picture}");
}

#[test]
fn a_failed_web_call_keeps_its_error_visible() {
    let picture = researching(true);
    assert!(picture.contains("Searched the web 2 times"), "{picture}");
    assert!(picture.contains("page missing"), "{picture}");
    assert!(
        picture.contains("WebFetch(https://example.com/missing)"),
        "{picture}"
    );
    assert!(!picture.contains("fetched 2 pages"), "{picture}");
}
