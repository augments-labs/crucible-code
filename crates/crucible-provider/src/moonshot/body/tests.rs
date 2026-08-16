use crucible_core::{Effort, ToolArgs, ToolCall, ToolId, ToolOutput, Transcript};

use super::*;

/// What a pointer finds when there is nothing there.
const NOTHING: Value = Value::Null;

fn request(transcript: Transcript) -> Request<'static> {
    Request {
        model: "kimi-test",
        transcript: Box::leak(Box::new(transcript)),
        tools: &[],
        max_tokens: 1024,
        system: None,
        effort: None,
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
    let body = build(&request(said("hello")));

    assert_eq!(at(&body, "/model"), &json!("kimi-test"));
    assert_eq!(at(&body, "/stream"), &json!(true));
    assert_eq!(at(&body, "/max_tokens"), &json!(1024));
    assert_eq!(
        at(&body, "/messages/0"),
        &json!({"role": "user", "content": "hello"})
    );
}

#[test]
fn a_streaming_request_asks_for_the_counts_that_say_what_it_cost() {
    // This endpoint sends them only when asked for, and sends them after the
    // answer in a chunk of its own. Left out of the request, a turn runs and
    // the row above the box has nothing to say about what it has spent.
    let body = build(&request(said("hello")));

    assert_eq!(at(&body, "/stream_options/include_usage"), &json!(true));
}

#[test]
fn a_session_nobody_told_how_hard_to_think_says_nothing_about_it() {
    // Kimi's own default is what a model gets otherwise, and this vendor serves
    // three of the five rungs — so a request nobody had an opinion about is one
    // that cannot be refused for naming a rung this model has never heard of.
    let body = build(&request(said("hello")));

    assert_eq!(at(&body, "/reasoning_effort"), &NOTHING);
}

#[test]
fn an_effort_somebody_chose_reaches_the_model_at_the_top_of_the_body() {
    // Not nested. This wire is the older chat-completions shape, where the
    // field sits beside `model` rather than under an object of its own.
    let mut asking = request(said("hello"));
    asking.effort = Some(Effort::Max);

    assert_eq!(at(&build(&asking), "/reasoning_effort"), &json!("max"));
}

#[test]
fn standing_instructions_lead_the_transcript_as_a_message_of_their_own() {
    // There is no field for them on this endpoint. First rather than anywhere
    // else: the model reads them as the frame the rest is answered in.
    let mut asking = request(said("hello"));
    asking.system = Some("be brief");

    let body = build(&asking);

    assert_eq!(
        at(&body, "/messages/0"),
        &json!({"role": "system", "content": "be brief"})
    );
    assert_eq!(at(&body, "/messages/1/role"), &json!("user"));
}

#[test]
fn a_session_without_standing_instructions_sends_no_message_for_them() {
    let body = build(&request(said("hello")));

    assert_eq!(at(&body, "/messages/0/role"), &json!("user"));
}

#[test]
fn a_tool_call_rides_on_the_message_the_model_made_it_in() {
    // The model has to see its own call in the transcript, or the result that
    // follows answers a question it never asked.
    let mut transcript = said("read it");
    transcript.push(Message::Agent {
        text: "let me look".into(),
        calls: vec![ToolCall {
            id: ToolId::new("call_1"),
            name: "read".into(),
            args: ToolArgs::new(r#"{"path":"src/main.rs"}"#),
        }],
        stop: Some(StopReason::WantsTools),
    });

    let body = build(&request(transcript));

    assert_eq!(
        at(&body, "/messages/1"),
        &json!({
            "role": "assistant",
            "content": "let me look",
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": {"name": "read", "arguments": r#"{"path":"src/main.rs"}"#},
            }],
        })
    );
}

#[test]
fn arguments_go_back_as_the_text_the_model_wrote() {
    // Re-encoded, the model is handed back something it did not write and the
    // arguments it sees stop matching the ones it produced. A tool that takes
    // none is called with no text at all, and an empty string is not JSON.
    let mut transcript = said("go");
    transcript.push(Message::Agent {
        text: String::new().into(),
        calls: vec![ToolCall {
            id: ToolId::new("call_1"),
            name: "clock".into(),
            args: ToolArgs::new("  "),
        }],
        stop: Some(StopReason::WantsTools),
    });

    let body = build(&request(transcript));

    assert_eq!(
        at(&body, "/messages/1/tool_calls/0/function/arguments"),
        &json!("{}")
    );
    assert!(
        at(&body, "/messages/1").get("content").is_none(),
        "a model that went straight to a tool sent an empty message with it"
    );
}

#[test]
fn a_result_answers_the_call_it_was_made_against_in_a_message_of_its_own() {
    let mut transcript = said("read it");
    transcript.push(Message::Agent {
        text: String::new().into(),
        calls: vec![ToolCall {
            id: ToolId::new("call_1"),
            name: "read".into(),
            args: ToolArgs::new("{}"),
        }],
        stop: Some(StopReason::WantsTools),
    });
    transcript.push(Message::ToolResults(vec![ToolResult {
        id: ToolId::new("call_1"),
        output: ToolOutput::ok("fn main() {}"),
    }]));

    let body = build(&request(transcript));

    assert_eq!(
        at(&body, "/messages/2"),
        &json!({
            "role": "tool",
            "tool_call_id": "call_1",
            "content": "fn main() {}",
        })
    );
}

#[test]
fn a_result_that_failed_says_so_in_its_text() {
    // There is no field for it here. Unmarked, "no such file: a.rs" reads as
    // the contents of a file that was read successfully.
    let mut transcript = said("read it");
    transcript.push(Message::ToolResults(vec![ToolResult {
        id: ToolId::new("call_1"),
        output: ToolOutput::failed("no such file: a.rs"),
    }]));

    let body = build(&request(transcript));

    assert_eq!(
        at(&body, "/messages/1/content"),
        &json!("error: no such file: a.rs")
    );
}

#[test]
fn a_turn_that_produced_nothing_at_all_does_not_send_an_empty_message() {
    // A turn cancelled before the model's first word records an agent message
    // with no text and no calls. The API refuses one, and because the message
    // is in the session file it would be sent again on every turn from then
    // on — the session permanently unusable, with nothing saying why.
    let mut transcript = said("go");
    transcript.push(Message::Agent {
        text: String::new().into(),
        calls: Vec::new(),
        stop: Some(StopReason::Cancelled),
    });

    let body = build(&request(transcript));

    assert_eq!(
        at(&body, "/messages").as_array().map(Vec::len),
        Some(1),
        "an empty turn was sent: {body}"
    );
}

#[test]
fn an_answer_that_was_cut_off_is_followed_by_a_message_saying_so() {
    // Left off, the model reads its own half-sentence as a turn it chose to
    // end, on this turn and on every turn of a continued session.
    let mut transcript = said("go");
    transcript.push(Message::Agent {
        text: "I was saying".into(),
        calls: Vec::new(),
        stop: Some(StopReason::OutOfTokens),
    });

    let body = build(&request(transcript));

    assert_eq!(at(&body, "/messages/2/role"), &json!("assistant"));
    assert!(
        at(&body, "/messages/2/content")
            .as_str()
            .is_some_and(|said| said.contains("cut off")),
        "the model was not told its answer stopped short: {body}"
    );
}

#[test]
fn nothing_comes_between_a_turns_tool_calls_and_their_results() {
    // This wire requires the message after one carrying tool calls to be their
    // results, and refuses the request outright otherwise. A turn holding calls
    // ended by asking for them, which is not an answer that was cut short.
    let mut transcript = said("go");
    transcript.push(Message::Agent {
        text: "looking".into(),
        calls: vec![ToolCall {
            id: ToolId::new("call_1"),
            name: "read".into(),
            args: ToolArgs::new("{}"),
        }],
        stop: Some(StopReason::Cancelled),
    });
    transcript.push(Message::ToolResults(vec![ToolResult {
        id: ToolId::new("call_1"),
        output: ToolOutput::ok("fn main() {}"),
    }]));

    let body = build(&request(transcript));

    assert_eq!(
        at(&body, "/messages/2/role"),
        &json!("tool"),
        "a message came between the calls and their results: {body}"
    );
}

#[test]
fn a_tool_is_advertised_under_the_function_this_endpoint_nests_it_in() {
    let mut asking = request(said("hello"));
    asking.tools = Box::leak(Box::new([ToolSchema {
        name: "read",
        schema: r#"{"description":"Reads a file","type":"object",
                    "properties":{"path":{"type":"string"}}}"#,
    }]));

    let body = build(&asking);

    assert_eq!(at(&body, "/tools/0/type"), &json!("function"));
    assert_eq!(at(&body, "/tools/0/function/name"), &json!("read"));
    assert_eq!(
        at(&body, "/tools/0/function/description"),
        &json!("Reads a file")
    );
    assert_eq!(
        at(&body, "/tools/0/function/parameters/properties/path/type"),
        &json!("string")
    );
    assert!(
        at(&body, "/tools/0/function/parameters")
            .get("description")
            .is_none(),
        "the description belongs to the tool, not to its arguments: {body}"
    );
}

#[test]
fn a_session_with_no_tools_sends_no_tools_field() {
    // An empty array is refused rather than read as a session without tools.
    let body = build(&request(said("hello")));

    assert!(body.get("tools").is_none(), "an empty tool list was sent");
}
