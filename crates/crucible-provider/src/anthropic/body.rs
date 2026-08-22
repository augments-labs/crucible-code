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

use crucible_core::{Attached, Content, Message, Request, StopReason, ToolResult, ToolSchema};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

use crate::json::{Array, Json, Object, described, object};

/// The whole request body.
pub(super) fn serialize(request: &Request<'_>) -> String {
    let mut json = Json::new();
    json.object(|body| {
        body.text("model", request.model);
        body.number("max_tokens", request.max_tokens);
        body.boolean("stream", true);
        body.array("messages", |messages| write_messages(messages, request));

        // Absent rather than null: the API rejects a null system prompt, and a
        // session without one is the ordinary case.
        if let Some(system) = request.system {
            body.text("system", system);
        }

        // Only where somebody chose one. Anthropic's own default is what a model
        // gets otherwise, and it is per-model — the field is served by the models
        // that reason and refused by the ones that do not, so sending it unasked
        // would turn "I never touched effort" into a 400 on whichever of them is
        // not on the list this week.
        if let Some(effort) = request.effort {
            body.object("output_config", |config| {
                config.text("effort", effort.as_str());
            });
        }

        if !request.tools.is_empty() {
            body.array("tools", |tools| {
                for schema in request.tools {
                    tools.object(|tool| write_tool(tool, schema));
                }
            });
        }
    });
    json.finish()
}

#[cfg(test)]
fn build(request: &Request<'_>) -> Value {
    serde_json::from_str(&serialize(request)).expect("request body is JSON")
}

/// Every message that has something in it, in order.
fn write_messages(messages: &mut Array<'_>, request: &Request<'_>) {
    for (nth, message) in request.transcript.messages().iter().enumerate() {
        write_message(messages, message, nth, request.attached);
    }
}

/// One message, unless it would carry no content.
///
/// Empty is refused at both levels this wire has: an empty text block, and a
/// message whose blocks all turned out to be empty ones. Dropping the block but
/// keeping the message that held it only moves the refusal up a level.
fn write_message(
    messages: &mut Array<'_>,
    message: &Message,
    nth: usize,
    attached: &[Attached<'_>],
) {
    match message {
        Message::User { text, .. } => messages.object(|message| {
            message.text("role", "user");

            let mut files = attached.iter().filter(|one| one.message == nth).peekable();
            if files.peek().is_none() {
                message.text("content", text);
                return;
            }

            // The vendor's own guidance is that the picture reads better
            // ahead of the words, and the words are one prompt behind however
            // many files it named.
            message.array("content", |content| {
                for one in files {
                    content.object(|block| write_attached(block, one));
                }
                // A prompt that named a file and said nothing else is the
                // picture alone: an empty text block is one this vendor
                // refuses the request over rather than ignores.
                if !text.is_empty() {
                    content.object(|block| {
                        block.text("type", "text");
                        block.text("text", text);
                    });
                }
            });
        }),
        Message::Agent { text, calls, stop } => {
            // Nothing said and nothing asked for: a turn cancelled or filtered
            // before the model's first word. It is recorded, so the message is
            // in the session file and would be sent on every turn after it —
            // one bad turn making the session refuse to continue at all.
            if text.is_empty() && calls.is_empty() {
                return;
            }

            messages.object(|message| {
                message.text("role", "assistant");
                message.array("content", |content| {
                    // An empty text block is refused by the API, and the model
                    // produces one when it calls a tool without speaking first.
                    if !text.is_empty() {
                        content.object(|block| {
                            block.text("type", "text");
                            block.text("text", text);
                        });
                    }

                    for call in calls {
                        let input = Value::Object(object(call.args.as_str()));
                        content.object(|block| {
                            block.text("type", "tool_use");
                            block.text("id", call.id.as_str());
                            block.text("name", &call.name);
                            block.value("input", &input);
                        });
                    }

                    // A block of its own after a cut answer. Left off, the model
                    // reads its half-sentence as a turn it chose to end.
                    if let Some(said) = StopReason::cut(*stop) {
                        content.object(|block| {
                            block.text("type", "text");
                            block.text("text", said);
                        });
                    }
                });
            });
        }
        // Results are the user's turn as far as the API is concerned: the model
        // asked, and this is the answer coming back to it.
        Message::ToolResults(results) => messages.object(|message| {
            message.text("role", "user");
            message.array("content", |content| {
                for result in results {
                    content.object(|block| write_result(block, result));
                }
            });
        }),
    }
}

/// One attached file, or the line standing where it would have been.
///
/// The sentence is printed rather than composed: the runner is the only thing
/// that knows which of its three reasons applies, and a block that invented its
/// own wording would be a fourth.
fn write_attached(block: &mut Object<'_>, attached: &Attached<'_>) {
    match attached.content {
        Content::Bytes(bytes) => {
            block.text("type", "image");
            block.object("source", |source| {
                source.text("type", "base64");
                source.text("media_type", attached.media_type);
                source.encoded("data", bytes);
            });
        }
        Content::Instead(line) => {
            block.text("type", "text");
            block.text("text", line);
        }
    }
}

/// One tool result.
fn write_result(block: &mut Object<'_>, result: &ToolResult) {
    block.text("type", "tool_result");
    block.text("tool_use_id", result.id.as_str());
    block.text("content", result.output.text());
    if result.output.is_failed() {
        block.boolean("is_error", true);
    }
}

/// One tool, as advertised.
fn write_tool(tool: &mut Object<'_>, schema: &ToolSchema) {
    let (input, description) = described(schema.schema);
    tool.text("name", schema.name);
    tool.text("description", &description);
    tool.value("input_schema", &Value::Object(input));
}

#[cfg(test)]
mod tests {
    use crucible_core::{
        Attached, Content, Effort, Modality, ToolArgs, ToolCall, ToolId, ToolOutput, Transcript,
    };

    use super::*;

    /// What a pointer finds when there is nothing there.
    const NOTHING: Value = Value::Null;

    fn request(transcript: Transcript) -> Request<'static> {
        Request {
            model: "claude-test",
            transcript: Box::leak(Box::new(transcript)),
            tools: &[],
            attached: &[],
            max_tokens: 1024,
            system: None,
            effort: None,
        }
    }

    /// The four bytes a PNG starts with, which encode to `iVBORw==`.
    const PIXEL: &[u8] = &[0x89, b'P', b'N', b'G'];

    /// The line the runner writes in place of a file it did not send.
    const INSTEAD: &str = "holiday.png is not attached to this request, to keep the request \
         within its size limit: read it again if you need it.";

    /// A prompt the runner resolved one attachment for.
    fn holding(text: &str, content: Content<'static>) -> Request<'static> {
        // The transcript's own reference is deliberately absent: a provider
        // reads what the runner resolved and never a path.
        let mut request = request(said(text));
        request.attached = Box::leak(Box::new([Attached {
            message: 0,
            index: 0,
            media_type: "image/png",
            modality: Modality::Image,
            content,
        }]));
        request
    }

    fn said(text: &str) -> Transcript {
        let mut transcript = Transcript::new();
        transcript.push(Message::said(text));
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
        request.system = Some("be brief");

        assert_eq!(at(&build(&request), "/system"), &json!("be brief"));
    }

    #[test]
    fn a_session_nobody_told_how_hard_to_think_says_nothing_about_it() {
        // The field is per-model here, and a model that does not serve it
        // refuses the whole request. Leaving it off is what keeps a session
        // nobody has an opinion about working on every model on the list.
        let body = build(&request(said("hello")));

        assert!(
            body.get("output_config").is_none(),
            "an effort nobody asked for is the vendor's to pick: {body}"
        );
    }

    #[test]
    fn an_effort_somebody_chose_reaches_the_model_as_output_config() {
        let mut request = request(said("hello"));
        request.effort = Some(Effort::Xhigh);

        assert_eq!(
            at(&build(&request), "/output_config/effort"),
            &json!("xhigh")
        );
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
            text: "let me look".into(),
            calls: vec![ToolCall {
                id: ToolId::new("call_1"),
                name: "read".into(),
                args: ToolArgs::new(r#"{"path":"src/main.rs"}"#),
            }],
            stop: Some(StopReason::WantsTools),
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
            text: String::new().into(),
            calls: vec![ToolCall {
                id: ToolId::new("call_1"),
                name: "read".into(),
                args: ToolArgs::new("{}"),
            }],
            stop: Some(StopReason::WantsTools),
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
            text: String::new().into(),
            calls: Vec::new(),
            stop: Some(StopReason::Cancelled),
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
            text: String::new().into(),
            calls: vec![ToolCall {
                id: ToolId::new("call_1"),
                name: "pwd".into(),
                args: ToolArgs::new(""),
            }],
            stop: Some(StopReason::WantsTools),
        });

        let body = build(&request(transcript));

        assert_eq!(at(&body, "/messages/1/content/0/input"), &json!({}));
    }

    #[test]
    fn a_turn_that_was_cut_off_is_not_sent_back_as_one_the_model_finished() {
        // The live notice tells the user; nothing told the model. So the next
        // turn — and every turn of a continued session — showed it its own
        // half-sentence as an answer it had chosen to end there.
        let mut transcript = said("write it all out");
        transcript.push(Message::Agent {
            text: "as I was say".into(),
            calls: Vec::new(),
            stop: Some(StopReason::OutOfTokens),
        });

        let body = build(&request(transcript));

        assert_eq!(
            at(&body, "/messages/1/content/0/text"),
            &json!("as I was say")
        );
        assert_eq!(
            at(&body, "/messages/1/content/1/text"),
            &json!(StopReason::cut(Some(StopReason::OutOfTokens)).expect("a cut-off turn")),
        );
    }

    #[test]
    fn a_turn_the_model_ended_itself_carries_no_note() {
        // The path taken every time. A note under each answer would be spent
        // on the ordinary ending and teach the model nothing.
        let mut transcript = said("hello");
        transcript.push(Message::Agent {
            text: "hello back".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        });

        let body = build(&request(transcript));

        assert_eq!(
            at(&body, "/messages/1/content").as_array().map(Vec::len),
            Some(1)
        );
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
        request.tools = Box::leak(Box::new([ToolSchema {
            name: "read",
            schema: r#"{"description":"Reads a file.","type":"object","properties":{"path":{"type":"string"}}}"#,
        }]));

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

    /// The shape this vendor's documentation described on 2026-08-23: an
    /// `image` block whose `source` is `base64`, ahead of the prompt, because
    /// the same page asks for the picture before the words.
    #[test]
    fn an_image_is_a_base64_source_block_before_the_prompt() {
        let body = build(&holding("what is in this", Content::Bytes(PIXEL)));

        assert_eq!(
            at(&body, "/messages/0/content"),
            &json!([
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": "iVBORw=="
                    }
                },
                { "type": "text", "text": "what is in this" }
            ]),
            "{body}"
        );
    }

    #[test]
    fn a_file_that_was_not_sent_is_the_runners_sentence_in_its_place() {
        let body = build(&holding("what is in this", Content::Instead(INSTEAD)));

        assert_eq!(
            at(&body, "/messages/0/content"),
            &json!([
                { "type": "text", "text": INSTEAD },
                { "type": "text", "text": "what is in this" }
            ]),
            "the sentence is printed, not composed: {body}"
        );
    }

    #[test]
    fn a_prompt_with_nothing_attached_is_the_string_it_always_was() {
        let body = build(&request(said("hello")));

        assert_eq!(at(&body, "/messages/0/content"), &json!("hello"), "{body}");
    }

    /// A prompt that named a file and said nothing else. The picture is the whole
    /// message, and an empty text part beside it is one this vendor refuses the
    /// request over rather than ignores.
    #[test]
    fn a_prompt_that_is_only_a_file_sends_no_empty_text_part() {
        let body = build(&holding("", Content::Bytes(PIXEL)));

        assert_eq!(
            at(&body, "/messages/0/content"),
            &json!([{
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/png",
                    "data": "iVBORw=="
                }
            }]),
            "{body}"
        );
    }
}
