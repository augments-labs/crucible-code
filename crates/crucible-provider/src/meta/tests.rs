//! Meta's dialect, against the examples Meta's own pages print.
//!
//! Every example below is copied from the page named above it, as it read on
//! 2026-10-01. Where no page prints what a test needs, the test says so and
//! builds the smallest body the vendor's schema allows.

use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::{Delta, DeltaStream, Provider, ProviderError, Request, RequestPurpose};
use crucible_runtime::Cancel;
use crucible_types::{
    InputTokenUsage, Message, ProviderNumericDetail, ProviderUsage, RecordedToolOutput, StopReason,
    ToolArgs, ToolCall, ToolId, ToolResult, ToolSchema, Transcript,
};
use serde_json::{Value, json};

use super::*;
use crate::fake::inclusive_usage;
use crate::transport::Replay;

/// Assistant text that comes before a function call, as it must be sent back.
///
/// <https://dev.meta.ai/docs/protocols/responses>, "Message phase".
const COMMENTARY: &str = r#"{
  "type": "message",
  "role": "assistant",
  "phase": "commentary",
  "content": [{"type": "output_text", "text": "Let me check the weather first."}]
}"#;

/// A function tool in the flat Responses form.
///
/// <https://dev.meta.ai/docs/tool-calling>, "Defining tools (Responses flat
/// format)".
const TOOL: &str = r#"{
  "type": "function",
  "name": "get_weather",
  "description": "Return current weather for the given location.",
  "parameters": {
    "type": "object",
    "properties": {
      "location": {
        "type": "string",
        "description": "City name, e.g. 'Seattle'"
      }
    },
    "required": ["location"],
    "additionalProperties": false
  }
}"#;

/// A whole response.
///
/// <https://dev.meta.ai/docs/protocols/responses>, "Basic usage", example
/// response.
const RESPONSE: &str = r#"{
  "id": "resp_abc123",
  "object": "response",
  "created_at": 1714502400,
  "status": "completed",
  "model": "muse-spark-1.3",
  "store": true,
  "temperature": 1.0,
  "top_p": 1.0,
  "max_output_tokens": null,
  "tools": [],
  "tool_choice": "auto",
  "parallel_tool_calls": true,
  "service_tier": "auto",
  "output": [
    {
      "id": "msg_abc123:456",
      "type": "message",
      "role": "assistant",
      "status": "completed",
      "content": [
        {
          "type": "output_text",
          "text": "The capital of France is **Paris**.",
          "annotations": []
        }
      ]
    }
  ],
  "usage": {
    "input_tokens": 69,
    "output_tokens": 163,
    "total_tokens": 232
  }
}"#;

/// What a response that read from the cache reports.
///
/// <https://dev.meta.ai/docs/prompt-caching>, "See what was cached".
const CACHED: &str = r#"{
  "usage": {
    "input_tokens": 1847,
    "output_tokens": 98,
    "total_tokens": 1945,
    "input_tokens_details": {
      "cached_tokens": 1792
    },
    "output_tokens_details": {
      "reasoning_tokens": 0
    }
  }
}"#;

/// The key a test provider sends, which no assertion expects to see.
const SECRET: &str = "LLM|0|synthetic-meta-test-key";

fn provider(body: &str) -> (Meta, std::sync::Arc<Replay>) {
    let replay = std::sync::Arc::new(Replay::new(200, body));
    let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bearer());
    (
        Meta::at(
            Meta::VENDOR,
            Box::new(credential),
            Box::new(std::sync::Arc::clone(&replay)),
        ),
        replay,
    )
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("a documented example is JSON")
}

/// `value` with `key` set to `new`.
fn set(value: &mut Value, key: &str, new: Value) {
    value.as_object_mut().unwrap().insert(key.into(), new);
}

/// One event, framed as the stream frames it.
fn event(payload: &Value) -> String {
    format!("data: {payload}\n\n")
}

/// Everything a stream hands back, its failures as their words.
fn read(stream: &mut dyn DeltaStream) -> Vec<Result<Delta, String>> {
    let mut out = Vec::new();
    while let Some(delta) = crucible_runtime::answered!(stream.next()) {
        out.push(delta.map_err(|problem: ProviderError| problem.to_string()));
    }
    out
}

#[test]
fn words_before_a_call_go_back_as_commentary_and_the_tool_as_documented() {
    // Sent back as an ordinary answer, words in front of a call are refused
    // with a 400 on the next request of every tool turn the model spoke in.
    let mut transcript = Transcript::new();
    transcript
        .push(Message::said("What's the weather in Seattle?"))
        .unwrap();
    transcript
        .push(Message::Agent {
            continuation: None,
            text: "Let me check the weather first.".into(),
            calls: vec![ToolCall {
                id: ToolId::new("call_abc123"),
                name: "get_weather".into(),
                args: ToolArgs::new(r#"{"location":"Seattle"}"#),
            }],
            stop: Some(StopReason::WantsTools),
        })
        .unwrap();
    transcript
        .push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call_abc123"),
            output: RecordedToolOutput::ok("12 degrees and raining"),
        }]))
        .unwrap();
    let tool = json(TOOL);
    let mut schema = tool.get("parameters").unwrap().clone();
    schema.as_object_mut().unwrap().insert(
        "description".into(),
        tool.get("description").unwrap().clone(),
    );
    let schema = schema.to_string();
    let tools = [ToolSchema {
        name: "get_weather",
        schema: &schema,
    }];
    let request = Request {
        purpose: RequestPurpose::Turn,
        model: "muse-spark-1.3",
        transcript: &transcript,
        tools: &tools,
        attached: &[],
        max_tokens: 8192,
        system: None,
        effort: None,
        prompt_cache: None,
    };
    let (meta, replay) = provider("");

    let _ = crucible_runtime::answered!(meta.stream(request, &Cancel::new()));

    let sent = replay.sent();
    assert_eq!(sent.url, "https://api.meta.ai/v1/responses");
    let body = json(&sent.body);
    assert_eq!(body.pointer("/input/1"), Some(&json(COMMENTARY)));
    assert_eq!(
        body.pointer("/input/2"),
        Some(&json!({
            "type": "function_call",
            "call_id": "call_abc123",
            "name": "get_weather",
            "arguments": "{\"location\":\"Seattle\"}",
        }))
    );
    let mut written = body.pointer("/tools/0").unwrap().clone();
    written.as_object_mut().unwrap().remove("strict");
    assert_eq!(written, tool);
    // The vendor takes `auto` alone, and a request that names none gets it.
    assert!(body.get("tool_choice").is_none(), "{body}");
    assert!(!sent.body.contains(SECRET));
}

#[test]
fn a_documented_answer_arrives_whole_and_the_line_that_closes_the_stream_is_not_read() {
    // No Meta page prints a stream. These events are the smallest the
    // vendor's event schema allows around its documented response, and the
    // closing line is the one Meta's own streaming sample stops on.
    let response = json(RESPONSE);
    let message = response.pointer("/output/0").unwrap().clone();
    let mut opened = message.clone();
    set(&mut opened, "status", json!("in_progress"));
    set(&mut opened, "content", json!([]));
    let said = message.pointer("/content/0/text").unwrap().clone();
    let mut created = response.clone();
    set(&mut created, "status", json!("in_progress"));
    set(&mut created, "output", json!([]));
    created.as_object_mut().unwrap().remove("usage");
    let body = [
        json!({"type":"response.created","sequence_number":0,"response":created}),
        json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":opened}),
        json!({"type":"response.output_text.delta","sequence_number":2,"item_id":"msg_abc123:456","output_index":0,"content_index":0,"delta":said}),
        json!({"type":"response.output_text.done","sequence_number":3,"item_id":"msg_abc123:456","output_index":0,"content_index":0,"text":said}),
        json!({"type":"response.output_item.done","sequence_number":4,"output_index":0,"item":message}),
        json!({"type":"response.completed","sequence_number":5,"response":response}),
    ]
    .iter()
    .map(event)
    .chain(["data: [DONE]\n\n".to_owned()])
    .collect::<String>();
    let (meta, _) = provider(&body);

    let mut stream = crucible_runtime::answered!(
        meta.stream(asking("What is the capital of France?"), &Cancel::new())
    )
    .unwrap();

    assert_eq!(
        read(stream.as_mut()),
        vec![
            Ok(Delta::Text("The capital of France is **Paris**.".into())),
            Ok(inclusive_usage(Some(69), None, Some(163))),
            Ok(Delta::Stopped(StopReason::Yielded)),
        ]
    );
}

#[test]
fn a_streamed_call_arrives_named_and_filled_in_and_what_was_cached_is_counted() {
    // No Meta page prints a streamed call. The events are built from the
    // fields the vendor's schema lists for each, around its documented tool
    // and its documented cached usage; the item's `id` is the one its
    // argument fragments name in `item_id`.
    let usage = json(CACHED).get("usage").unwrap().clone();
    let call = json!({"type":"function_call","id":"fc_abc123","call_id":"call_abc123","name":"get_weather","arguments":"","status":"in_progress"});
    let mut done = call.clone();
    set(&mut done, "arguments", json!("{\"location\":\"Seattle\"}"));
    set(&mut done, "status", json!("completed"));
    let body = [
        json!({"type":"response.created","sequence_number":0,"response":{"id":"resp_abc123","object":"response","status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":call}),
        json!({"type":"response.function_call_arguments.delta","sequence_number":2,"item_id":"fc_abc123","output_index":0,"delta":"{\"location\":\"Seattle\"}"}),
        json!({"type":"response.function_call_arguments.done","sequence_number":3,"item_id":"fc_abc123","output_index":0,"name":"get_weather","arguments":"{\"location\":\"Seattle\"}"}),
        json!({"type":"response.output_item.done","sequence_number":4,"output_index":0,"item":done}),
        json!({"type":"response.completed","sequence_number":5,"response":{"id":"resp_abc123","object":"response","status":"completed","output":[done],"usage":usage}}),
    ]
    .iter()
    .map(event)
    .chain(["data: [DONE]\n\n".to_owned()])
    .collect::<String>();
    let (meta, _) = provider(&body);

    let mut stream = crucible_runtime::answered!(
        meta.stream(asking("What's the weather in Seattle?"), &Cancel::new())
    )
    .unwrap();

    let cached = InputTokenUsage::inclusive_read(Some(1847), Some(1792)).unwrap();
    let details = [
        ProviderNumericDetail::new("cached_tokens", 1792).unwrap(),
        ProviderNumericDetail::new("reasoning_tokens", 0).unwrap(),
    ];
    assert_eq!(
        read(stream.as_mut()),
        vec![
            Ok(Delta::ToolStarted {
                id: ToolId::new("call_abc123"),
                name: "get_weather".into(),
            }),
            Ok(Delta::ToolArgs("{\"location\":\"Seattle\"}".into())),
            Ok(Delta::Usage(
                ProviderUsage::new(cached, Some(98), Some(0), Some(1945), &details).unwrap()
            )),
            Ok(Delta::Stopped(StopReason::WantsTools)),
        ]
    );
}

fn asking(text: &str) -> Request<'static> {
    let mut transcript = Transcript::new();
    transcript.push(Message::said(text)).unwrap();
    Request {
        purpose: RequestPurpose::Turn,
        model: "muse-spark-1.3",
        transcript: Box::leak(Box::new(transcript)),
        tools: &[],
        attached: &[],
        max_tokens: 8192,
        system: None,
        effort: None,
        prompt_cache: None,
    }
}

#[test]
fn every_model_caches_its_prefix_on_its_own_and_takes_a_routing_key() {
    let (provider, _) = provider("");
    for model in [
        "muse-spark-1.3",
        "muse-spark-1.3-contributor",
        "muse-spark-1.2",
        "muse-spark-1.2-contributor",
    ] {
        let record = provider.prompt_cache_capabilities(model);
        assert_eq!(record.model_revision(), Some(model));
        assert!(
            record.mechanisms().iter().all(|one| one.mechanism()
                == crucible_types::PromptCacheMechanism::AutomaticPrefix
                && one.supports_routing_key()),
            "{model}"
        );
    }
}

/// Meta's own example of a request too large for the model.
///
/// <https://dev.meta.ai/docs/error-handling>, "Context window exceeded".
const CONTEXT_WINDOW: &str = include_str!("fixtures/error-400-context-window.json");

/// What a request answered with `status` and `body` fails with.
fn refused(status: u16, body: &str) -> ProviderError {
    let replay = std::sync::Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bearer());
    let provider = Meta::at(Meta::VENDOR, Box::new(credential), Box::new(replay));
    let cancel = Cancel::new();
    match crucible_runtime::answered!(provider.stream(asking("Hello"), &cancel)) {
        Err(problem) => problem,
        Ok(_) => panic!("a refused request has no stream"),
    }
}

#[test]
fn a_request_too_large_for_the_model_is_compacted_for_rather_than_ending_the_turn() {
    let problem = refused(400, CONTEXT_WINDOW);

    assert!(
        matches!(problem, ProviderError::WindowExceeded { provider: "meta" }),
        "{problem:?}"
    );
    assert!(!problem.transient(), "the same request will not fit again");
}

#[test]
fn every_other_documented_400_stays_a_refusal_in_meta_s_words() {
    for (body, said) in [
        (
            include_str!("fixtures/error-400-validation.json"),
            "`top_p`: The number must be `<= 1.0`.",
        ),
        (
            include_str!("fixtures/error-400-call-id.json"),
            "function_call_output call_id 'call_xyz'",
        ),
        (
            include_str!("fixtures/error-400-reasoning-order.json"),
            "Invalid conversation structure: reasoning items",
        ),
    ] {
        let problem = refused(400, body);

        assert!(
            matches!(
                &problem,
                ProviderError::Refused { status: 400, message, .. } if message.contains(said)
            ),
            "{problem:?}"
        );
    }
}

#[test]
fn the_words_alone_are_not_a_request_too_large() {
    // Each body below is the documented one with a single part of its shape
    // changed: the words are read only where the rest of it is the vendor's
    // refusal of an over-long request.
    let documented = json(CONTEXT_WINDOW);
    let changed = |pointer: &str, value: Value| {
        let mut body = documented.clone();
        *body.pointer_mut(pointer).unwrap() = value;
        body.to_string()
    };
    for (status, body) in [
        (400, changed("/error/param", json!("input"))),
        (400, changed("/error/code", json!("invalid_value"))),
        (400, changed("/error/type", json!("server_error"))),
        (
            400,
            changed(
                "/error/message",
                json!("However, The model's context length is only 1048576 tokens"),
            ),
        ),
        (500, documented.to_string()),
        (413, documented.to_string()),
    ] {
        let problem = refused(status, &body);

        assert!(
            matches!(problem, ProviderError::Refused { .. }),
            "{status} {body}: {problem:?}"
        );
    }
}
