//! What one request body says, field by field.
//!
//! Separate from the builder next door only because the builder reached the
//! per-file cap.

use crucible_core::{StopReason, ToolArgs, ToolId, ToolOutput, Transcript};

use super::*;

/// What a pointer finds when there is nothing there.
const NOTHING: Value = Value::Null;

fn request(transcript: Transcript) -> Request {
    Request {
        model: "gpt-test".into(),
        transcript,
        tools: Vec::new(),
        max_tokens: 1024,
        system: None,
    }
}

fn said(text: &str) -> Transcript {
    let mut transcript = Transcript::new();
    transcript.push(Message::User(text.into()));
    transcript
}

/// One value by JSON pointer.
///
/// Indexing a `Value` panics on a shape that is not what it expected, which
/// turns a wrong assertion into a stack trace instead of a diff.
fn at<'a>(body: &'a Value, path: &str) -> &'a Value {
    body.pointer(path).unwrap_or(&NOTHING)
}

#[test]
fn a_request_streams_and_names_its_model() {
    // Not streaming would mean the answer appears all at once at the end,
    // which is the whole experience this harness is built around.
    let body = build(&request(said("hello")));

    assert_eq!(at(&body, "/model"), &json!("gpt-test"));
    assert_eq!(at(&body, "/stream"), &json!(true));
}

#[test]
fn the_token_ceiling_is_sent_under_the_name_every_model_accepts() {
    // `max_tokens` means the same thing and is refused outright by the
    // models that reason before answering. This one is taken by all of them.
    let body = build(&request(said("hello")));

    assert_eq!(at(&body, "/max_completion_tokens"), &json!(1024));
    assert!(
        body.get("max_tokens").is_none(),
        "the older name went out too: {body}"
    );
}

#[test]
fn a_system_prompt_is_the_first_message_rather_than_a_field() {
    // There is no field for it in this protocol. Sending one would be
    // ignored, and the model would work without its instructions.
    let mut request = request(said("hello"));
    request.system = Some("be brief".into());

    let body = build(&request);

    assert_eq!(at(&body, "/messages/0/role"), &json!("system"));
    assert_eq!(at(&body, "/messages/0/content"), &json!("be brief"));
    assert_eq!(at(&body, "/messages/1/role"), &json!("user"));
    assert!(body.get("system").is_none(), "{body}");
}

#[test]
fn a_session_without_a_system_prompt_starts_at_what_was_typed() {
    let body = build(&request(said("hello")));

    assert_eq!(at(&body, "/messages/0/role"), &json!("user"));
    assert_eq!(at(&body, "/messages/0/content"), &json!("hello"));
}

#[test]
fn a_tool_call_goes_back_with_its_arguments_as_the_text_the_model_wrote() {
    // The field is a string on this wire. Handing back a re-encoded object
    // would give the model something it did not write, and the arguments it
    // sees would stop matching the ones it produced.
    let mut transcript = said("read it");
    transcript.push(Message::Agent {
        stop: Some(StopReason::WantsTools),
        text: "let me look".into(),
        calls: vec![ToolCall {
            id: ToolId::new("call_1"),
            name: "read".into(),
            args: ToolArgs::new(r#"{"path":"src/main.rs"}"#),
        }],
    });

    let body = build(&request(transcript));

    assert_eq!(at(&body, "/messages/1/role"), &json!("assistant"));
    assert_eq!(at(&body, "/messages/1/content"), &json!("let me look"));
    assert_eq!(
        at(&body, "/messages/1/tool_calls/0"),
        &json!({
            "id": "call_1",
            "type": "function",
            "function": {"name": "read", "arguments": r#"{"path":"src/main.rs"}"#},
        })
    );
}

#[test]
fn a_tool_call_with_no_words_before_it_sends_no_content() {
    // A model that goes straight to a tool says nothing first, and an empty
    // string is not the same as having said nothing.
    let mut transcript = said("go");
    transcript.push(Message::Agent {
        stop: Some(StopReason::WantsTools),
        text: String::new().into(),
        calls: vec![ToolCall {
            id: ToolId::new("call_1"),
            name: "read".into(),
            args: ToolArgs::new("{}"),
        }],
    });

    let body = build(&request(transcript));

    assert_eq!(at(&body, "/messages/1/content"), &NOTHING);
    assert_eq!(
        at(&body, "/messages/1/tool_calls/0/function/name"),
        &json!("read")
    );
}

#[test]
fn a_turn_that_produced_nothing_at_all_still_sends_something_to_hold() {
    // The pair of the test above, and the reason its condition names the
    // calls as well as the text. A turn cancelled or filtered before the
    // model's first word has neither, and null with no calls to carry it is
    // a message this wire has no use for. It is recorded, so it would go
    // out on every turn after it rather than once.
    let mut transcript = said("go");
    transcript.push(Message::Agent {
        stop: Some(StopReason::WantsTools),
        text: String::new().into(),
        calls: Vec::new(),
    });

    let body = build(&request(transcript));

    assert_eq!(at(&body, "/messages/1/content"), &json!(""));
    assert_eq!(at(&body, "/messages/1/tool_calls"), &NOTHING);
}

#[test]
fn a_tool_that_takes_no_arguments_still_sends_parsable_text() {
    // No arguments means no argument text arrived at all, and an empty
    // string is not JSON on the other side.
    let mut transcript = said("go");
    transcript.push(Message::Agent {
        stop: Some(StopReason::WantsTools),
        text: String::new().into(),
        calls: vec![ToolCall {
            id: ToolId::new("call_1"),
            name: "pwd".into(),
            args: ToolArgs::new(""),
        }],
    });

    let body = build(&request(transcript));

    assert_eq!(
        at(&body, "/messages/1/tool_calls/0/function/arguments"),
        &json!("{}")
    );
}

#[test]
fn every_result_of_a_turn_is_its_own_message() {
    // The other protocol carries them together. Here each one names the
    // call it answers, so two results are two messages or the second one
    // has nowhere to say which call it belongs to.
    let mut transcript = said("go");
    transcript.push(Message::ToolResults(vec![
        ToolResult {
            id: ToolId::new("call_1"),
            output: ToolOutput::ok("fn main() {}"),
        },
        ToolResult {
            id: ToolId::new("call_2"),
            output: ToolOutput::ok("src/main.rs"),
        },
    ]));

    let body = build(&request(transcript));

    assert_eq!(
        at(&body, "/messages/1"),
        &json!({"role": "tool", "tool_call_id": "call_1", "content": "fn main() {}"})
    );
    assert_eq!(
        at(&body, "/messages/2"),
        &json!({"role": "tool", "tool_call_id": "call_2", "content": "src/main.rs"})
    );
}

#[test]
fn a_failed_result_says_so_in_the_only_place_this_wire_has() {
    // There is no field for it. Unmarked, "no such file: x" reads as the
    // contents of a file that was read successfully.
    let mut transcript = said("go");
    transcript.push(Message::ToolResults(vec![ToolResult {
        id: ToolId::new("call_1"),
        output: ToolOutput::failed("no such file"),
    }]));

    let body = build(&request(transcript));

    assert_eq!(
        at(&body, "/messages/1/content"),
        &json!("error: no such file")
    );
}

#[test]
fn a_tool_is_advertised_with_its_schema_and_its_description() {
    let mut request = request(said("go"));
    request.tools = vec![ToolSchema {
        name: "read",
        schema: r#"{"description":"Reads a file.","type":"object","properties":{"path":{"type":"string"}}}"#,
    }];

    let body = build(&request);

    assert_eq!(at(&body, "/tools/0/type"), &json!("function"));
    assert_eq!(at(&body, "/tools/0/function/name"), &json!("read"));
    assert_eq!(
        at(&body, "/tools/0/function/description"),
        &json!("Reads a file.")
    );
    assert_eq!(
        at(&body, "/tools/0/function/parameters/type"),
        &json!("object")
    );
    assert_eq!(
        at(&body, "/tools/0/function/parameters/description"),
        &NOTHING,
        "the description belongs to the tool, not to its arguments"
    );
}

#[test]
fn a_session_with_no_tools_sends_no_tools_field() {
    // An empty array is refused rather than treated as none.
    let body = build(&request(said("hello")));

    assert!(body.get("tools").is_none(), "an empty tool list is not one");
}
