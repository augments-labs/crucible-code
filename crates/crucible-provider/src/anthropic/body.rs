//! A request, in Anthropic's shape.
//!
//! One direction only: domain types in, JSON out. The response travels the
//! other way through [`super::wire`], and keeping the two apart is what stops a
//! change to one shape from quietly altering the other.
//!
//! Fields are inserted rather than assigned by index. Indexing a JSON value
//! panics on anything that is not the container it expected, and nothing that
//! builds a request may be one bad assumption away from taking the process
//! down.

use crucible_core::{Message, Request, ToolResult, ToolSchema, Transcript};
use serde_json::{Map, Value, json};

use crate::json::{described, object};

/// The whole request body.
pub(super) fn build(request: &Request) -> Value {
    let mut body = Map::new();
    body.insert("model".to_owned(), json!(&*request.model));
    body.insert("max_tokens".to_owned(), json!(request.max_tokens));
    body.insert("stream".to_owned(), json!(true));
    body.insert("messages".to_owned(), json!(messages(&request.transcript)));

    // Absent rather than null: the API rejects a null system prompt, and a
    // session without one is the ordinary case.
    if let Some(system) = &request.system {
        body.insert("system".to_owned(), json!(&**system));
    }

    if !request.tools.is_empty() {
        let tools = request.tools.iter().map(tool).collect();
        body.insert("tools".to_owned(), Value::Array(tools));
    }

    Value::Object(body)
}

/// Every message that has something in it, in order.
fn messages(transcript: &Transcript) -> Vec<Value> {
    transcript.messages().iter().filter_map(message).collect()
}

/// One message, or nothing if it would carry no content.
///
/// Empty is refused at both levels this wire has: an empty text block, and a
/// message whose blocks all turned out to be empty ones. The second is why this
/// returns an option — dropping the block but keeping the message that held it
/// only moves the refusal up a level.
fn message(message: &Message) -> Option<Value> {
    match message {
        Message::User(text) => Some(json!({ "role": "user", "content": &**text })),
        Message::Agent { text, calls, .. } => {
            let mut content = Vec::with_capacity(calls.len() + 1);

            // An empty text block is refused by the API, and the model produces
            // one every time it calls a tool without saying anything first.
            if !text.is_empty() {
                content.push(json!({ "type": "text", "text": &**text }));
            }

            for call in calls {
                content.push(json!({
                    "type": "tool_use",
                    "id": call.id.as_str(),
                    "name": &*call.name,
                    "input": Value::Object(object(call.args.as_str())),
                }));
            }

            // Nothing said and nothing asked for: a turn cancelled or filtered
            // before the model's first word. It is recorded, so the message is
            // in the session file and would be sent on every turn after it —
            // one bad turn making the session refuse to continue at all.
            if content.is_empty() {
                return None;
            }

            Some(json!({ "role": "assistant", "content": content }))
        }
        // Results are the user's turn as far as the API is concerned: the model
        // asked, and this is the answer coming back to it.
        Message::ToolResults(results) => Some(json!({
            "role": "user",
            "content": results.iter().map(result).collect::<Vec<_>>(),
        })),
    }
}

/// One tool result.
fn result(result: &ToolResult) -> Value {
    let mut block = Map::new();
    block.insert("type".to_owned(), json!("tool_result"));
    block.insert("tool_use_id".to_owned(), json!(result.id.as_str()));
    block.insert("content".to_owned(), json!(result.output.text()));

    // Only when true. The field means "the model should treat this as having
    // gone wrong", and sending it as false on every success is noise in a
    // payload that is resent on every turn.
    if result.output.is_failed() {
        block.insert("is_error".to_owned(), json!(true));
    }

    Value::Object(block)
}

/// One tool, as advertised.
fn tool(schema: &ToolSchema) -> Value {
    let (input, description) = described(schema.schema);

    json!({
        "name": schema.name,
        "description": description,
        "input_schema": Value::Object(input),
    })
}

#[cfg(test)]
mod tests {
    use crucible_core::{StopReason, ToolArgs, ToolCall, ToolId, ToolOutput};

    use super::*;

    /// What a pointer finds when there is nothing there.
    const NOTHING: Value = Value::Null;

    fn request(transcript: Transcript) -> Request {
        Request {
            model: "claude-test".into(),
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

        assert_eq!(at(&body, "/model"), &json!("claude-test"));
        assert_eq!(at(&body, "/stream"), &json!(true));
        assert_eq!(at(&body, "/max_tokens"), &json!(1024));
    }

    #[test]
    fn a_session_without_a_system_prompt_sends_no_system_field() {
        let body = build(&request(said("hello")));

        assert!(
            body.get("system").is_none(),
            "a null system prompt is refused: {body}"
        );
    }

    #[test]
    fn a_system_prompt_is_sent_when_there_is_one() {
        let mut request = request(said("hello"));
        request.system = Some("be brief".into());

        assert_eq!(at(&build(&request), "/system"), &json!("be brief"));
    }

    #[test]
    fn a_user_message_carries_what_was_typed() {
        let body = build(&request(said("hello")));

        assert_eq!(at(&body, "/messages/0/role"), &json!("user"));
        assert_eq!(at(&body, "/messages/0/content"), &json!("hello"));
    }

    #[test]
    fn a_tool_call_the_model_made_goes_back_as_a_tool_use_block() {
        // The model has to see its own call in the transcript, or the result
        // that follows answers a question it never asked.
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
        assert_eq!(
            at(&body, "/messages/1/content/0"),
            &json!({"type": "text", "text": "let me look"})
        );
        assert_eq!(
            at(&body, "/messages/1/content/1"),
            &json!({
                "type": "tool_use",
                "id": "call_1",
                "name": "read",
                "input": {"path": "src/main.rs"},
            })
        );
    }

    #[test]
    fn a_tool_call_with_no_words_before_it_sends_no_text_block() {
        // The API refuses an empty text block, and a model that goes straight
        // to a tool produces one on every turn it does so.
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
        let content = at(&body, "/messages/1/content");

        assert_eq!(content.as_array().map(Vec::len), Some(1));
        assert_eq!(at(&body, "/messages/1/content/0/type"), &json!("tool_use"));
    }

    #[test]
    fn a_turn_that_produced_nothing_at_all_does_not_send_an_empty_message() {
        // A turn cancelled or filtered before the model's first word records an
        // agent message with no text and no calls. An empty content array is a
        // 400 — and because the message is in the session file, `--continue`
        // would send it again on every turn from then on. The session would be
        // permanently unusable, and nothing about the failure would say why.
        let mut transcript = said("go");
        transcript.push(Message::Agent {
            stop: Some(StopReason::WantsTools),
            text: String::new().into(),
            calls: Vec::new(),
        });

        let body = build(&request(transcript));

        assert_eq!(
            at(&body, "/messages").as_array().map(Vec::len),
            Some(1),
            "a message with no blocks in it is refused: {body}"
        );
        assert_eq!(at(&body, "/messages/0/content"), &json!("go"));
    }

    #[test]
    fn a_tool_that_takes_no_arguments_still_sends_an_object() {
        // No arguments means no argument text arrived at all. Sending that
        // through as an empty string is a 400.
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

        assert_eq!(at(&body, "/messages/1/content/0/input"), &json!({}));
    }

    #[test]
    fn a_result_answers_the_call_that_asked() {
        let mut transcript = said("go");
        transcript.push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call_1"),
            output: ToolOutput::ok("fn main() {}"),
        }]));

        let body = build(&request(transcript));

        assert_eq!(at(&body, "/messages/1/role"), &json!("user"));
        assert_eq!(
            at(&body, "/messages/1/content/0"),
            &json!({
                "type": "tool_result",
                "tool_use_id": "call_1",
                "content": "fn main() {}",
            }),
            "nothing went wrong, so nothing says it did"
        );
    }

    #[test]
    fn a_failed_result_is_marked_so_the_model_can_react() {
        let mut transcript = said("go");
        transcript.push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call_1"),
            output: ToolOutput::failed("no such file"),
        }]));

        let body = build(&request(transcript));

        assert_eq!(at(&body, "/messages/1/content/0/is_error"), &json!(true));
    }

    #[test]
    fn a_tool_is_advertised_with_its_schema_and_its_description() {
        let mut request = request(said("go"));
        request.tools = vec![ToolSchema {
            name: "read",
            schema: r#"{"description":"Reads a file.","type":"object","properties":{"path":{"type":"string"}}}"#,
        }];

        let body = build(&request);

        assert_eq!(at(&body, "/tools/0/name"), &json!("read"));
        assert_eq!(at(&body, "/tools/0/description"), &json!("Reads a file."));
        assert_eq!(at(&body, "/tools/0/input_schema/type"), &json!("object"));
        assert_eq!(
            at(&body, "/tools/0/input_schema/description"),
            &NOTHING,
            "the description belongs to the tool, not to its arguments"
        );
    }

    #[test]
    fn a_session_with_no_tools_sends_no_tools_field() {
        let body = build(&request(said("hello")));

        assert!(body.get("tools").is_none(), "an empty tool list is not one");
    }
}
