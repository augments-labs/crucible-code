//! A vendor on this wire that wants its model's reasoning back, written here
//! alone: what the wire keeps, what it sends back and where, and the request
//! fields a dialect may set its own way.

use crucible_credentials::{ApiKey, Header, HeaderKey, Outgoing};
use crucible_models::{Delta, Provider, ProviderError, Request, RequestPurpose};
use crucible_runtime::Cancel;
use crucible_types::{
    CONTINUATION_BYTES, ContinuationScope, Message, Modalities, Modality, ProviderContinuation,
    StopReason, ToolArgs, ToolCall, ToolId, ToolResult, ToolSchema, Transcript,
};
use serde_json::{Value, json};

use super::wire::Thought;
use super::{Chat, Dialect, Reasoning};
use crate::completions::testing::field;
use crate::endpoint::Endpoint;
use crate::fake::found;
use crate::transport::Replay;

/// Where the fabricated vendor serves this wire.
const THINKER: Endpoint = Endpoint::fixed("https://think.example/v1/chat/completions");

/// A fabricated vendor that wants every earlier answer's reasoning back once
/// tools are sent, counts its usage unasked, takes its ceiling under another
/// name, needs a flag on every request, and reports a failure inside an answer.
#[derive(Debug)]
struct Thinker;

impl Dialect for Thinker {
    const NAME: &'static str = "thinker";
    const TITLE: &'static str = "Thinker";
    const ADDRESSES: &'static [Endpoint] = &[THINKER];
    const SHAPE: &'static str = "thinker-chat-completions-v1";
    const USAGE_ASKED: bool = false;
    const CEILING: &'static str = "max_completion_tokens";
    const FLAGS: &'static [(&'static str, bool)] = &[("split_thoughts", true)];
    type Kept = Thought;

    fn spells() -> Modalities {
        Modalities::empty().insert(Modality::Text)
    }

    fn headers(_outgoing: &mut Outgoing) {}

    fn usage(payload: &Value) -> Result<Option<Delta>, ProviderError> {
        super::usage::inclusive(payload, Self::NAME, None)
    }

    fn prompt_cache(_model: &str) -> crucible_models::PromptCacheCapabilities {
        crucible_models::PromptCacheCapabilities::unknown("not reviewed")
    }

    fn reasoning(model: &str) -> Reasoning {
        if model == "thinker-quiet" {
            Reasoning::Unread
        } else {
            Reasoning::Required
        }
    }

    fn failure(payload: &Value) -> Option<ProviderError> {
        let status = payload.get("status")?;
        let code = status.get("code").and_then(Value::as_u64)?;
        (code != 0).then(|| ProviderError::Upstream {
            provider: Self::NAME,
            kind: code.to_string().into(),
            message: status
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
        })
    }

    fn stopped(reason: &str) -> Option<StopReason> {
        (reason == "window_full").then_some(StopReason::OutOfTokens)
    }
}

/// The fabricated vendor, answering every request with `body`.
fn thinker(body: &str) -> (Chat<Thinker>, std::sync::Arc<Replay>) {
    keyed("fabricated-thinker-key", body)
}

/// The same, reached with `key`.
fn keyed(key: &str, body: &str) -> (Chat<Thinker>, std::sync::Arc<Replay>) {
    let replay = std::sync::Arc::new(Replay::new(200, body));
    (
        Chat::at(
            THINKER,
            Box::new(HeaderKey::new(ApiKey::new(key), Header::bearer())),
            Box::new(std::sync::Arc::clone(&replay)),
        ),
        replay,
    )
}

/// The one tool the fabricated conversation offers.
const TOOLS: &[ToolSchema<'static>] = &[ToolSchema {
    name: "lookup",
    schema: r#"{"type":"object","description":"Looks a word up."}"#,
}];

/// `transcript`, asked of `model` with or without the tool.
fn asking(model: &'static str, transcript: Transcript, tools: bool) -> Request<'static> {
    Request {
        purpose: RequestPurpose::Turn,
        model,
        transcript: Box::leak(Box::new(transcript)),
        tools: if tools { TOOLS } else { &[] },
        attached: &[],
        max_tokens: 512,
        system: None,
        effort: None,
        prompt_cache: None,
    }
}

/// A question and nothing else.
fn question() -> Transcript {
    let mut transcript = Transcript::new();
    transcript
        .push(Message::said("Define cat."))
        .expect("a valid transcript");
    transcript
}

/// A response that thinks, says a little, then calls the tool.
const THOUGHT_THEN_CALL: &str = concat!(
    r#"data: {"choices":[{"index":0,"delta":{"reasoning_content":"The user wants "}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"index":0,"delta":{"reasoning_content":"a definition."}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"index":0,"delta":{"content":"Looking."}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"lookup","arguments":"{\"word\":\"cat\"}"}}]}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
    "\n\n",
    "data: [DONE]\n\n",
);

/// Every delta of one response to `request`.
fn read(provider: &Chat<Thinker>, request: Request<'static>) -> Vec<Delta> {
    let cancel = Cancel::new();
    let mut stream = crucible_runtime::answered!(provider.stream(request, &cancel))
        .expect("the request is sent");
    let mut deltas = Vec::new();
    while let Some(delta) = crucible_runtime::answered!(stream.next()) {
        deltas.push(delta.expect("the answer reads"));
    }
    deltas
}

/// The continuation among `deltas`, finished the way the runner finishes one.
fn finished(deltas: &[Delta], text: &str, calls: usize) -> Option<ProviderContinuation> {
    deltas.iter().find_map(|delta| match delta {
        Delta::Continuation(state) => Some(
            state
                .clone()
                .finish(text, calls, Some(StopReason::WantsTools))
                .expect("the continuation covers the answer"),
        ),
        _ => None,
    })
}

/// The conversation after the call, with `continuation` beside the answer.
fn answered(continuation: Option<ProviderContinuation>) -> Transcript {
    let mut transcript = question();
    transcript
        .push(Message::Agent {
            text: "Looking.".into(),
            calls: vec![ToolCall {
                id: ToolId::new("call-1"),
                name: "lookup".into(),
                args: ToolArgs::new(r#"{"word":"cat"}"#),
            }],
            stop: Some(StopReason::WantsTools),
            continuation,
        })
        .expect("a valid transcript");
    transcript
        .push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call-1"),
            output: found("A small feline.", Vec::new()),
        }]))
        .expect("a valid transcript");
    transcript
}

/// The assistant message of the request `replay` last received.
fn assistant(replay: &Replay) -> Value {
    let body: Value = serde_json::from_str(&replay.sent().body).expect("the body is JSON");
    field(&body, "/messages")
        .as_array()
        .and_then(|messages| {
            messages
                .iter()
                .find(|one| field(one, "/role") == "assistant")
        })
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
fn the_reasoning_an_answer_kept_goes_back_on_that_answer_in_the_next_request() {
    let (provider, replay) = thinker(THOUGHT_THEN_CALL);

    let deltas = read(&provider, asking("thinker-1", question(), true));
    let at = deltas
        .iter()
        .position(|delta| matches!(delta, Delta::Continuation(_)))
        .expect("the reasoning is kept");
    assert!(
        matches!(
            deltas.get(at + 1),
            Some(Delta::Stopped(StopReason::WantsTools))
        ),
        "{deltas:?}"
    );

    let kept = finished(&deltas, "Looking.", 1);
    let _ = read(&provider, asking("thinker-1", answered(kept), true));

    assert_eq!(
        field(&assistant(&replay), "/reasoning_content"),
        json!("The user wants a definition.")
    );
}

#[test]
fn an_answer_that_kept_none_is_sent_back_empty_only_where_tools_are_sent() {
    let (provider, replay) = thinker(THOUGHT_THEN_CALL);

    let _ = read(&provider, asking("thinker-1", answered(None), true));
    assert_eq!(field(&assistant(&replay), "/reasoning_content"), json!(""));

    let _ = read(&provider, asking("thinker-1", answered(None), false));
    assert_eq!(assistant(&replay).get("reasoning_content"), None);
}

#[test]
fn reasoning_kept_for_another_credential_or_address_is_not_sent() {
    let (provider, replay) = thinker(THOUGHT_THEN_CALL);
    let deltas = read(&provider, asking("thinker-1", question(), true));
    let kept = finished(&deltas, "Looking.", 1);

    // The same vendor, reached with another key.
    let (other, other_replay) = keyed("another-fabricated-key", THOUGHT_THEN_CALL);
    let _ = read(&other, asking("thinker-1", answered(kept), true));

    // Another key's answer is history this credential did not write: it is
    // described rather than replayed, and its reasoning goes nowhere.
    assert!(
        !other_replay.sent().body.contains("The user wants"),
        "{}",
        other_replay.sent().body
    );
    assert_ne!(
        ContinuationScope::new(provider.credential_scope, THINKER.as_str()),
        ContinuationScope::new(other.credential_scope, THINKER.as_str()),
    );
    let _ = replay;
}

#[test]
fn a_model_whose_reasoning_the_vendor_does_not_want_keeps_and_sends_none() {
    let (provider, replay) = thinker(THOUGHT_THEN_CALL);

    let deltas = read(&provider, asking("thinker-quiet", question(), true));
    assert!(
        !deltas
            .iter()
            .any(|delta| matches!(delta, Delta::Continuation(_))),
        "{deltas:?}"
    );

    let _ = read(&provider, asking("thinker-quiet", answered(None), true));
    assert_eq!(assistant(&replay).get("reasoning_content"), None);
}

#[test]
fn reasoning_too_long_to_keep_is_not_kept_at_all() {
    let long = "x".repeat(CONTINUATION_BYTES / 2 + 1);
    let event = |piece: &str| {
        format!(
            "data: {}\n\n",
            json!({"choices":[{"index":0,"delta":{"reasoning_content":piece}}]})
        )
    };
    let body = format!(
        "{}{}{}data: [DONE]\n\n",
        event(&long),
        event(&long),
        r#"data: {"choices":[{"index":0,"delta":{"content":"Done."},"finish_reason":"stop"}]}"#
            .to_owned()
            + "\n\n",
    );
    let (provider, _) = thinker(&body);

    let deltas = read(&provider, asking("thinker-1", question(), false));

    assert_eq!(
        deltas,
        [
            Delta::Text("Done.".into()),
            Delta::Stopped(StopReason::Yielded)
        ]
    );
}

#[test]
fn a_dialect_names_its_ceiling_asks_no_counts_and_sets_its_flags() {
    let (provider, replay) = thinker(THOUGHT_THEN_CALL);
    let _ = read(&provider, asking("thinker-1", question(), false));

    let body: Value = serde_json::from_str(&replay.sent().body).expect("the body is JSON");
    assert_eq!(field(&body, "/max_completion_tokens"), json!(512));
    assert_eq!(body.get("max_tokens"), None);
    assert_eq!(body.get("stream_options"), None);
    assert_eq!(field(&body, "/split_thoughts"), json!(true));
}

#[test]
fn a_failure_the_vendor_reports_inside_an_answer_ends_the_turn_in_its_words() {
    let body = concat!(
        r#"data: {"choices":[],"status":{"code":1008,"message":"insufficient balance"}}"#,
        "\n\n",
    );
    let (provider, _) = thinker(body);
    let cancel = Cancel::new();
    let mut stream = crucible_runtime::answered!(
        provider.stream(asking("thinker-1", question(), false), &cancel)
    )
    .expect("the request is sent");

    let first = crucible_runtime::answered!(stream.next()).expect("an event");

    assert!(
        matches!(
            &first,
            Err(ProviderError::Upstream { provider: "thinker", kind, message })
                if &**kind == "1008" && &**message == "insufficient balance"
        ),
        "{first:?}"
    );
}

#[test]
fn a_reason_to_stop_the_vendor_has_words_of_its_own_for_is_read_as_it_means() {
    let body = concat!(
        r#"data: {"choices":[{"index":0,"delta":{"content":"Part"},"finish_reason":"window_full"}]}"#,
        "\n\n",
        "data: [DONE]\n\n",
    );
    let (provider, _) = thinker(body);

    let deltas = read(&provider, asking("thinker-quiet", question(), false));

    assert_eq!(
        deltas.last(),
        Some(&Delta::Stopped(StopReason::OutOfTokens))
    );
}

/// A model whose reasoning goes back to its vendor is one whose dialect keeps
/// reasoning at all: the two are written apart, and a dialect that keeps
/// nothing would send back nothing, or empty strings, without a word.
#[test]
fn every_model_whose_reasoning_goes_back_is_of_a_dialect_that_keeps_it() {
    fn agrees<D: Dialect>(models: &[&str]) {
        for model in models {
            assert!(
                D::reasoning(model) == Reasoning::Unread || <D::Kept as super::wire::Keeps>::KEEPS,
                "{model}"
            );
        }
    }

    agrees::<crate::deepseek::DeepSeekChat>(&["deepseek-flash", "deepseek-v4-pro"]);
    agrees::<crate::mimo::MimoChat>(&["mimo-v2.6-pro", "mimo-v2.6-flash"]);
    agrees::<crate::minimax::MiniMaxChat>(&["MiniMax-M3", "MiniMax-M2.7"]);
    agrees::<crate::moonshot::Kimi>(&[
        "k3",
        "k3-256k",
        "kimi-for-coding",
        "kimi-for-coding-highspeed",
    ]);
    agrees::<crate::qwen::QwenChat>(&[
        "qwen3.8-max",
        "qwen3.8-flash",
        "qwen3.7-plus",
        "qwen3.6-plus",
    ]);
    agrees::<crate::zai::ZaiChat>(&["glm-5.3", "glm-5.3-flash", "glm-5.2"]);
    agrees::<Thinker>(&["thinker-1"]);
}
