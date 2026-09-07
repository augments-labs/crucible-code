//! Native web sources compose through real tools, runner and protected sessions.
//!
//! One loopback recipient serves coding and side requests in their observable
//! order. Fetched text survives restart and recap; side-request private
//! state never joins coding history. A new recipient/key must be used by both.

use super::*;
use crucible_core::{AgentId, ApiKey, ContinuationPart, Effort, Header, HeaderKey};
use crucible_provider::{Endpoint, GoogleWeb, Https};
use crucible_runner::{AgentSpec, Compaction, ContextInputs, Model, RunPolicy, Runner, Tools};
use crucible_tools::WebFetch;
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::sync::Arc;

const URL: &str = "https://example.com/page";
const FETCH_TEXT: &str = "Native fetched page.";

fn web_runner(
    sample: &Sample,
    model: &str,
    vendor: &Vendor,
    session: Session,
    key: &str,
) -> Runner {
    let source = Arc::new(GoogleWeb::new(
        vendor.endpoint.clone(),
        Box::new(HeaderKey::new(
            ApiKey::new(key),
            Header::bare("x-goog-api-key"),
        )),
        Box::new(Https::new()),
        model,
    ));
    let mut tools = Tools::new();
    tools
        .add_builtin(WebFetch::new(source))
        .expect("valid fixture");
    Runner::new(
        provider(model, vendor.endpoint.clone(), key),
        tools,
        AgentSpec::new(
            AgentId::new("web-fixture"),
            Model {
                name: model.into(),
                max_tokens: 4096,
                window: Some(200_000),
                accepts: None,
                effort: Some(Effort::High),
            },
        ),
        ContextInputs::new(sample.workspace().root()),
        session,
    )
    .under(RunPolicy {
        compaction: Compaction {
            keep_tokens: 1,
            ..Compaction::default()
        },
        ..RunPolicy::default()
    })
}

fn stream(steps: Vec<Value>, status: &str) -> String {
    let mut body = String::new();
    for (index, step) in steps.into_iter().enumerate() {
        writeln!(
            body,
            "data: {}\n",
            json!({"event_type":"step.start","index":index,"step":step})
        )
        .expect("valid fixture");
        writeln!(
            body,
            "data: {}\n",
            json!({"event_type":"step.stop","index":index})
        )
        .expect("valid fixture");
    }
    writeln!(
        body,
        "data: {}\n",
        json!({"event_type":"interaction.completed","interaction":{"status":status}})
    )
    .expect("valid fixture");
    body
}

fn calls() -> String {
    stream(
        vec![
            json!({"type":"function_call","id":"web_fetch","name":"web_fetch","arguments":{"url":URL}}),
        ],
        "requires_action",
    )
}

fn native() -> String {
    stream(
        vec![
            json!({"type":"thought","signature":"web-private-never-replay","summary":[]}),
            json!({"type":"url_context_call","id":"native-fetch","arguments":{"urls":[URL]}}),
            json!({"type":"url_context_result","call_id":"native-fetch","result":[{"url":URL,"status":"success"}]}),
            json!({"type":"model_output","content":[{"type":"text","text":FETCH_TEXT,"annotations":[{"type":"url_citation","url":URL,"title":"A source","start_index":0,"end_index":FETCH_TEXT.len()}]}]}),
        ],
        "completed",
    )
}

fn assert_no_side_state(history: &Transcript) {
    for message in history.messages() {
        if let Message::Agent {
            continuation: Some(state),
            ..
        } = message
        {
            for part in state.parts() {
                let data = match part {
                    ContinuationPart::Opaque(data)
                    | ContinuationPart::Text { data, .. }
                    | ContinuationPart::Call { data, .. } => data,
                };
                assert!(!data.as_str().contains("web-private-never-replay"));
            }
        }
    }
}

fn assert_side(model: &str, endpoint: &Endpoint, sent: &Sent, key: &str) {
    let body = &sent.body;
    let headers = sent.headers.to_ascii_lowercase();
    assert!(headers.starts_with("post /interactions?alt=sse http/1.1\r\n"));
    assert!(headers.contains(&format!("x-goog-api-key: {key}\r\n")));
    assert!(!headers.contains("authorization:"));
    let host = endpoint
        .as_str()
        .strip_prefix("http://")
        .expect("valid fixture")
        .split('/')
        .next()
        .expect("valid fixture");
    assert!(headers.contains(&format!("host: {host}\r\n")));
    assert_eq!(body.get("model").expect("valid fixture"), model);
    assert_eq!(body.get("stream").expect("valid fixture"), true);
    assert_eq!(body.get("store").expect("valid fixture"), false);
    assert_eq!(
        body.get("tools").expect("valid fixture"),
        &json!([{"type": "url_context"}])
    );
    assert_eq!(
        body.pointer("/generation_config/max_output_tokens")
            .expect("valid fixture"),
        32768
    );
    assert!(
        body.get("input")
            .expect("valid fixture")
            .as_str()
            .expect("valid fixture")
            .contains(URL)
    );
    let text = body.to_string();
    for absent in [
        "previous_interaction_id",
        "private-state",
        "fixture checkpoint",
        "begin web work",
        key,
    ] {
        assert!(!text.contains(absent), "side request imported {absent}");
    }
}

#[test]
fn native_google_web_survives_restart_compaction_and_recipient_rotation() {
    for model in MODELS.into_iter().take(4) {
        let sample = Sample::new();
        let key = "synthetic-first-key";
        let vendor = Vendor::new(
            model,
            [
                calls(),
                native(),
                response(model, None, "web finished"),
                response(model, None, "resumed"),
                response(model, None, &recap()),
                response(model, None, "compacted"),
            ],
        );
        let session =
            Session::start(&sample.logs(), &sample.workspace(), None).expect("valid fixture");
        let mut run = web_runner(&sample, model, &vendor, session, key);
        assert_eq!(
            turn(&mut run, "begin web work", &sample),
            StopReason::Yielded
        );
        assert_eq!(sample.approved(), 1);
        let visible = run
            .transcript()
            .messages()
            .iter()
            .filter_map(|message| {
                if let Message::ToolResults(results) = message {
                    Some(results)
                } else {
                    None
                }
            })
            .flatten()
            .map(|result| {
                assert!(!result.output.is_failed(), "{}", result.output.text());
                result.output.text()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(visible.contains(FETCH_TEXT));
        assert_no_side_state(run.transcript());
        let before = run.transcript().clone();
        drop(run);

        let (session, history) =
            Session::resume(&sample.logs(), &sample.workspace()).expect("valid fixture");
        assert_eq!(before.messages(), history.messages());
        assert_no_side_state(&history);
        let mut run = web_runner(&sample, model, &vendor, session, key).resuming(history);
        assert_eq!(
            turn(&mut run, "resume the research", &sample),
            StopReason::Yielded
        );
        assert_eq!(sample.approved(), 1, "resume re-executed web tools");
        let cancel = crucible_core::Cancel::new();
        let steer = crucible_core::Steer::new();
        let aside = crucible_core::Aside::new();
        let context = run.starting(&sample, &cancel, &steer, &aside);
        assert!(matches!(
            run.compact(Compacting::Asked, &context, &mut Spend::default())
                .expect("valid fixture"),
            Room::Made(_)
        ));
        assert_eq!(
            turn(&mut run, "continue compacted research", &sample),
            StopReason::Yielded
        );
        drop(run);

        let sent = vendor.requests();
        assert_eq!(sent.len(), 6);
        assert_side(
            model,
            &vendor.endpoint,
            sent.get(1).expect("valid fixture"),
            key,
        );
        for index in [2, 3, 4] {
            let text = sent.get(index).expect("valid fixture").body.to_string();
            assert!(
                text.contains(FETCH_TEXT),
                "fetch text lost at request {index}"
            );
            assert!(!text.contains("web-private-never-replay"));
        }

        let next = Vendor::new(model, [calls(), native(), response(model, None, "rotated")]);
        let key = "synthetic-rotated-key";
        let (session, history) =
            Session::resume(&sample.logs(), &sample.workspace()).expect("valid fixture");
        let mut run = web_runner(&sample, model, &next, session, key).resuming(history);
        assert_eq!(turn(&mut run, "fetch again", &sample), StopReason::Yielded);
        assert_eq!(sample.approved(), 2);
        assert_no_side_state(run.transcript());
        let sent = next.requests();
        assert_eq!(sent.len(), 3);
        assert!(
            !sent
                .first()
                .expect("valid fixture")
                .body
                .to_string()
                .contains("private-state")
        );
        assert_side(
            model,
            &next.endpoint,
            sent.get(1).expect("valid fixture"),
            key,
        );
        assert!(!sample.events().contains("web-private-never-replay"));
    }
}

#[test]
fn unfinished_google_native_work_never_executes_local_calls_or_survives_restart() {
    for model in MODELS.into_iter().take(4) {
        for status in ["incomplete", "cancelled", "requires_action"] {
            let sample = Sample::new();
            let vendor = Vendor::new(
                model,
                [stream(
                    vec![
                        json!({"type":"thought","signature":"private-state","summary":[]}),
                        json!({"type":"google_search_call","id":"pending-search","arguments":{"queries":["Rust"]}}),
                        json!({"type":"function_call","id":"call-1","name":"fixture","arguments":{"step":"1"}}),
                    ],
                    status,
                )],
            );
            let session =
                Session::start(&sample.logs(), &sample.workspace(), None).expect("valid fixture");
            let mut run = sample.runner(model, &vendor, session);
            let _ = try_turn(&mut run, "work", &sample);
            assert!(sample.executed().is_empty(), "{model} {status}");
            assert_eq!(sample.approved(), 0);
            assert!(
                run.transcript()
                    .messages()
                    .iter()
                    .all(|m| m.continuation_bytes() == 0)
            );
            drop(run);
            let (_, history) =
                Session::resume(&sample.logs(), &sample.workspace()).expect("valid fixture");
            assert!(
                history
                    .messages()
                    .iter()
                    .all(|m| m.continuation_bytes() == 0)
            );
            assert_eq!(vendor.requests().len(), 1);
        }
    }
}
