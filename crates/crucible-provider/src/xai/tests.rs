//! xAI's dialect, against the examples xAI's own pages print.
//!
//! Every example below is copied from the page named above it, as it read on
//! 2026-10-01. Where no page prints what a test needs, the test says so and
//! builds the smallest body the vendor's reference allows.

use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::{Delta, DeltaStream, Provider, ProviderError, Request, RequestPurpose};
use crucible_runtime::Cancel;
use crucible_types::{
    InputTokenUsage, Message, PromptCacheMechanism, PromptCacheRetentionClass,
    ProviderNumericDetail, ProviderUsage, StopReason, ToolId, ToolSchema, Transcript,
};
use serde_json::{Value, json};

use super::*;
use crate::transport::Replay;

/// A request that declares a function tool.
///
/// <https://docs.x.ai/developers/tools/function-calling>, quick start, the
/// request body.
const REQUEST: &str = r#"{
  "model": "grok-4.7",
  "input": [
    {"role": "user", "content": "What is the temperature in San Francisco?"}
  ],
  "tools": [
    {
      "type": "function",
      "name": "get_temperature",
      "description": "Get current temperature for a location",
      "parameters": {
        "type": "object",
        "properties": {
          "location": {"type": "string", "description": "City name"},
          "unit": {"type": "string", "enum": ["celsius", "fahrenheit"], "default": "fahrenheit"}
        },
        "required": ["location"]
      }
    }
  ]
}"#;

/// A whole response, whose usage counts reasoning outside the output.
///
/// <https://docs.x.ai/developers/rest-api-reference/inference/responses>,
/// `POST /v1/responses`, example response.
const RESPONSE: &str = r#"{
  "created_at": 1754475266,
  "id": "ad5663da-63e6-86c6-e0be-ff15effa8357",
  "model": "latest",
  "object": "response",
  "output": [{
    "content": [{
      "type": "output_text",
      "text": "101 multiplied by 3 is 303.",
      "logprobs": null,
      "annotations": []
    }],
    "id": "msg_ad5663da-63e6-86c6-e0be-ff15effa8357",
    "role": "assistant",
    "type": "message",
    "status": "completed"
  }],
  "status": "completed",
  "usage": {
    "input_tokens": 32,
    "output_tokens": 9,
    "total_tokens": 151,
    "input_tokens_details": {"cached_tokens": 8},
    "output_tokens_details": {"reasoning_tokens": 110},
    "num_sources_used": 0,
    "num_server_side_tools_used": 0
  },
  "service_tier": "default",
  "store": true
}"#;

/// A failure, in the nested shape.
///
/// <https://docs.x.ai/developers/advanced-api-usage/websocket-mode>, errors;
/// the page says its events are those of the Responses stream.
const FAILURE: &str = r#"{
  "type": "error",
  "status": 400,
  "error": {
    "code": "previous_response_not_found",
    "message": "Previous response with id 'resp_abc' not found.",
    "param": "previous_response_id"
  }
}"#;

/// The key a test provider sends, which no assertion expects to see.
const SECRET: &str = "synthetic-xai-test-key";

fn provider(status: u16, body: &str) -> (Xai, std::sync::Arc<Replay>) {
    let replay = std::sync::Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bearer());
    (
        Xai::at(
            Xai::VENDOR,
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

/// What `key` holds in `value`.
fn field(value: &Value, key: &str) -> Value {
    value.get(key).unwrap().clone()
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

fn asking(text: &str) -> Request<'static> {
    let mut transcript = Transcript::new();
    transcript.push(Message::said(text)).unwrap();
    Request {
        purpose: RequestPurpose::Turn,
        model: "grok-4.7",
        transcript: Box::leak(Box::new(transcript)),
        tools: &[],
        attached: &[],
        max_tokens: 8192,
        system: None,
        effort: None,
        prompt_cache: None,
    }
}

/// What the documented response costs, read with its reasoning counted
/// inside the output, where the rest of the wire counts it.
fn documented_usage() -> Delta {
    let input = InputTokenUsage::inclusive_read(Some(32), Some(8)).unwrap();
    let details = [
        ProviderNumericDetail::new("cached_tokens", 8).unwrap(),
        ProviderNumericDetail::new("reasoning_tokens", 110).unwrap(),
    ];
    Delta::Usage(ProviderUsage::new(input, Some(119), Some(110), Some(151), &details).unwrap())
}

#[test]
fn a_documented_request_is_sent_as_printed_with_only_the_cache_key_beside_it() {
    // The reference lists `prompt_cache_key` and none of the other cache
    // fields, and no `strict` on a function tool: what it does not list is
    // not sent.
    let documented = json(REQUEST);
    let tool = documented.pointer("/tools/0").unwrap();
    let mut schema = tool.get("parameters").unwrap().clone();
    schema.as_object_mut().unwrap().insert(
        "description".into(),
        tool.get("description").unwrap().clone(),
    );
    let schema = Box::leak(schema.to_string().into_boxed_str());
    let mut request = asking("What is the temperature in San Francisco?");
    request.tools = Box::leak(Box::new([ToolSchema {
        name: "get_temperature",
        schema,
    }]));
    let request = crate::fake::cached(
        request,
        PromptCacheMechanism::AutomaticPrefix,
        PromptCacheRetentionClass::Extended,
        true,
    );
    let (xai, replay) = provider(200, "");

    let _ = crucible_runtime::answered!(xai.stream(request, &Cancel::new()));

    let sent = replay.sent();
    assert_eq!(sent.url, "https://api.x.ai/v1/responses");
    let body = json(&sent.body);
    assert_eq!(body.get("model"), documented.get("model"));
    assert_eq!(body.get("input"), documented.get("input"));
    assert_eq!(body.get("tools"), documented.get("tools"));
    assert!(body.get("prompt_cache_key").is_some_and(Value::is_string));
    for absent in [
        "prompt_cache_options",
        "prompt_cache_retention",
        "tool_choice",
        "include",
    ] {
        assert!(body.get(absent).is_none(), "{absent}: {body}");
    }
    assert!(!sent.body.contains(SECRET));
}

#[test]
fn the_documented_answer_arrives_with_what_it_cost_although_reasoning_is_counted_outside_it() {
    // No xAI page prints a Responses stream. These events are the smallest
    // the reference's response object allows around the documented response,
    // whose usage has 110 reasoning tokens beside 9 of output and a total of
    // all three. Refusing that would fail every turn it is reported on.
    let response = json(RESPONSE);
    let message = response.pointer("/output/0").unwrap().clone();
    let mut opened = message.clone();
    set(&mut opened, "status", json!("in_progress"));
    set(&mut opened, "content", json!([]));
    let said = message.pointer("/content/0/text").unwrap().clone();
    let body = [
        json!({"type":"response.created","response":{"id":field(&response, "id"),"status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":opened}),
        json!({"type":"response.output_text.delta","item_id":field(&message, "id"),"output_index":0,"content_index":0,"delta":said}),
        json!({"type":"response.output_item.done","output_index":0,"item":message}),
        json!({"type":"response.completed","response":response}),
    ]
    .iter()
    .map(event)
    .collect::<String>();
    let (xai, _) = provider(200, &body);

    let mut stream =
        crucible_runtime::answered!(xai.stream(asking("What is 101 times 3?"), &Cancel::new()))
            .unwrap();

    assert_eq!(
        read(stream.as_mut()),
        vec![
            Ok(Delta::Text("101 multiplied by 3 is 303.".into())),
            Ok(documented_usage()),
            Ok(Delta::Stopped(StopReason::Yielded)),
        ]
    );
}

#[test]
fn a_call_whose_arguments_arrive_whole_on_the_finished_item_is_filled_in_once() {
    // The vendor says a streamed call "is returned in whole in a single
    // chunk"; no page prints the events. Built from the reference's function
    // call item, with its arguments on the finished item alone, and closed by
    // the line the vendor's Chat Completions stream ends with.
    let call = json!({"type":"function_call","id":"fc_123","call_id":"call_123","name":"get_temperature","arguments":"","status":"in_progress"});
    let mut done = call.clone();
    set(
        &mut done,
        "arguments",
        json!("{\"location\":\"San Francisco\"}"),
    );
    set(&mut done, "status", json!("completed"));
    let mut response = json(RESPONSE);
    set(&mut response, "output", json!([done]));
    let body = [
        json!({"type":"response.created","response":{"id":field(&response, "id"),"status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":call}),
        json!({"type":"response.output_item.done","output_index":0,"item":done}),
        json!({"type":"response.completed","response":response}),
    ]
    .iter()
    .map(event)
    .chain(["data: [DONE]\n\n".to_owned()])
    .collect::<String>();
    let (xai, _) = provider(200, &body);

    let mut stream = crucible_runtime::answered!(xai.stream(
        asking("What is the temperature in San Francisco?"),
        &Cancel::new()
    ))
    .unwrap();

    assert_eq!(
        read(stream.as_mut()),
        vec![
            Ok(Delta::ToolStarted {
                id: ToolId::new("call_123"),
                name: "get_temperature".into(),
            }),
            Ok(Delta::ToolArgs("{\"location\":\"San Francisco\"}".into())),
            Ok(documented_usage()),
            Ok(Delta::Stopped(StopReason::WantsTools)),
        ]
    );
}

#[test]
fn a_failure_in_the_stream_says_the_vendors_words_from_under_its_error() {
    // Nested, as the vendor prints its stream's failure events.
    let (xai, _) = provider(200, &event(&json(FAILURE)));
    let mut stream =
        crucible_runtime::answered!(xai.stream(asking("hello"), &Cancel::new())).unwrap();

    assert_eq!(
        read(stream.as_mut()),
        vec![Err(
            "xai: previous_response_not_found: Previous response with id 'resp_abc' not found."
                .to_owned()
        )]
    );
}

#[test]
fn a_refused_request_says_the_vendors_words_when_they_are_the_whole_of_its_error() {
    // Flat, with the sentence as `error` and a code beside it. No xAI page
    // prints a refused request's body; this one is from the corpus of error
    // bodies xAI's own open source coding harness parses.
    let flat =
        r#"{"code":"Client specified an invalid argument","error":"model 'nope' does not exist"}"#;
    let (xai, _) = provider(400, flat);

    let refused = crucible_runtime::answered!(xai.stream(asking("hello"), &Cancel::new()))
        .map(|_| ())
        .map_err(|problem| problem.to_string());

    assert_eq!(
        refused,
        Err("xai: HTTP 400: model 'nope' does not exist".to_owned())
    );
}

#[test]
fn every_model_caches_its_prefix_on_its_own_and_takes_a_routing_key() {
    let (provider, _) = provider(200, "");
    for model in ["grok-4.7", "grok-4.6"] {
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
