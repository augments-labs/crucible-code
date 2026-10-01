//! A vendor that wants its model's earlier reasoning back gets it, on the
//! second request of a turn with a tool and on the first after a resume.
//!
//! `DeepSeek` on Chat Completions: its thinking-mode guide says a request with
//! tools carries each earlier answer's `reasoning_content`. What crucible keeps
//! of it rides beside the answer as its private continuation, so the session
//! log holds it and a resumed conversation sends it again. The responses are
//! built in the shape of the guide's own stream: reasoning, then the answer or
//! the call, then the reason it stopped.

use std::sync::Arc;

use crucible_credentials::{ApiKey, Credential, Header, HeaderKey};
use crucible_session::Session;
use crucible_types::{
    Continuation, ContinuationData, ContinuationPart, ContinuationScope, Message,
    ProviderContinuation, RecordedToolOutput, StopReason, ToolArgs, ToolCall, ToolId, ToolResult,
};
use serde_json::{Value, json};

use super::support::{KEY, Sample, Vendor, turn};

const MODEL: &str = "deepseek-flash";

/// What a log records the kept reasoning as. Written into session logs, so a
/// change to it is a change to every log already written.
const PROTOCOL: &str = "chat-completions-reasoning-v1";

/// One streamed answer: `reasoning`, then `text`, then a call to the fixture
/// tool where `call` names its step.
fn thinking(reasoning: &str, text: &str, call: Option<u8>) -> String {
    let mut chunks = vec![
        json!({"choices": [{"index": 0, "delta": {"role": "assistant", "reasoning_content": reasoning}, "finish_reason": null}]}),
        json!({"choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}]}),
    ];
    if let Some(step) = call {
        chunks.push(json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0,
            "id": format!("call-{step}"), "type": "function", "function": {"name": "fixture",
            "arguments": json!({"step": step.to_string()}).to_string()}}]}, "finish_reason": null}]}));
    }
    chunks.push(json!({"choices": [{"index": 0, "delta": {},
        "finish_reason": if call.is_some() { "tool_calls" } else { "stop" }}]}));
    let mut body = String::new();
    for chunk in chunks {
        body.push_str("data: ");
        body.push_str(&chunk.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// The reasoning each assistant message of `body` sends back, in order.
fn sent_back(body: &Value) -> Vec<Option<String>> {
    body.get("messages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|message| message.get("role") == Some(&json!("assistant")))
        .map(|message| {
            message
                .get("reasoning_content")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

#[test]
fn deepseek_gets_each_earlier_answer_s_reasoning_live_and_after_a_resume() {
    let sample = Sample::new();
    let vendor = Vendor::new(
        MODEL,
        [
            thinking("reasoning-before-the-call", "calling", Some(1)),
            thinking("reasoning-after-the-result", "done", None),
            thinking("reasoning-after-the-resume", "resumed", None),
        ],
    );
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a log");
    let mut run = sample.runner(MODEL, &vendor, session);
    assert_eq!(turn(&mut run, "use a tool", &sample), StopReason::Yielded);
    drop(run);

    let (session, history) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the log just written");
    let mut run = sample.runner(MODEL, &vendor, session).resuming(history);
    assert_eq!(turn(&mut run, "continue", &sample), StopReason::Yielded);
    drop(run);

    let requests = vendor.requests();
    assert_eq!(requests.len(), 3);
    let live = requests.get(1).expect("the request after the call");
    assert_eq!(
        sent_back(&live.body),
        [Some("reasoning-before-the-call".to_owned())],
        "live: {}",
        live.body
    );
    let resumed = requests.get(2).expect("the first request after the resume");
    assert_eq!(
        sent_back(&resumed.body),
        [
            Some("reasoning-before-the-call".to_owned()),
            Some("reasoning-after-the-result".to_owned()),
        ],
        "resumed: {}",
        resumed.body
    );
}

/// What the fabricated log says an answer kept, bound where the provider below
/// sends: the same key at the same address.
fn kept(reasoning: &str, text: &str, calls: usize, endpoint: &str) -> ProviderContinuation {
    let scope = ContinuationScope::new(
        HeaderKey::new(ApiKey::new(KEY), Header::bearer()).scope(),
        endpoint,
    );
    let mut state = Continuation::new(PROTOCOL, MODEL, scope).expect("a valid identity");
    state
        .push(ContinuationPart::Opaque(
            ContinuationData::new(reasoning).expect("a bounded reasoning"),
        ))
        .expect("room for the reasoning");
    if !text.is_empty() {
        state
            .push(ContinuationPart::Text {
                start: 0,
                end: text.len(),
                data: ContinuationData::new("").expect("an empty part"),
            })
            .expect("room for the text");
    }
    for index in 0..calls {
        state
            .push(ContinuationPart::Call {
                index,
                data: ContinuationData::new("").expect("an empty part"),
            })
            .expect("room for the call");
    }
    let stop = if calls > 0 {
        StopReason::WantsTools
    } else {
        StopReason::Yielded
    };
    state
        .finish(text, calls, Some(stop))
        .expect("a finished continuation")
}

#[test]
fn a_fabricated_log_s_reasoning_reaches_deepseek_on_the_first_request_after_a_resume() {
    let sample = Sample::new();
    let vendor = Vendor::new(MODEL, [thinking("fresh", "resumed", None)]);
    let endpoint = vendor.endpoint.as_str().to_owned();

    // A conversation no vendor answered: written through the session as a run
    // would have written it, and ended.
    let session =
        Arc::new(Session::start(&sample.logs(), &sample.workspace(), None).expect("a log"));
    for message in [
        Message::said("use a tool"),
        Message::Agent {
            text: "".into(),
            calls: vec![ToolCall {
                id: ToolId::new("call-1"),
                name: "fixture".into(),
                args: ToolArgs::new(r#"{"step":"1"}"#),
            }],
            stop: Some(StopReason::WantsTools),
            continuation: Some(kept("fabricated-reasoning-one", "", 1, &endpoint)),
        },
        Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call-1"),
            output: RecordedToolOutput::ok("fixture-result-1"),
        }]),
        Message::Agent {
            text: "done".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
            continuation: Some(kept("fabricated-reasoning-two", "done", 0, &endpoint)),
        },
    ] {
        session.append(&message);
    }
    drop(session.finish());
    drop(session);

    let (session, history) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the fabricated log");
    let mut run = sample.runner(MODEL, &vendor, session).resuming(history);
    assert_eq!(turn(&mut run, "continue", &sample), StopReason::Yielded);
    drop(run);

    let requests = vendor.requests();
    let first = requests
        .first()
        .expect("the first request after the resume");
    assert_eq!(
        sent_back(&first.body),
        [
            Some("fabricated-reasoning-one".to_owned()),
            Some("fabricated-reasoning-two".to_owned()),
        ],
        "{}",
        first.body
    );
}
