//! A request, in `MoonshotAI`'s shape.
//!
//! One direction only: domain types in, JSON out. The response travels the
//! other way through [`super::wire`], and keeping the two apart is what stops a
//! change to one shape from quietly altering the other.
//!
//! A transcript is a list of messages, as it is for Anthropic and is not for
//! the newer OpenAI protocol. What differs from Anthropic is where the parts of
//! a turn go: standing instructions are a message rather than a field, a tool
//! call rides on the assistant message rather than in its content, a result is
//! a message of its own with a role of its own, argument text travels as JSON
//! *text* rather than as an object, and how hard to think is a field beside
//! `model` rather than an object of its own.

use crucible_core::{Message, Request, StopReason, ToolCall, ToolResult, ToolSchema};
#[cfg(test)]
use serde_json::{Value, json};

use crate::json::{Array, Json, Object, described};

/// The whole request body.
pub(super) fn serialize(request: &Request<'_>) -> String {
    let mut json = Json::new();
    json.object(|body| {
        body.text("model", request.model);
        body.number("max_tokens", request.max_tokens);
        body.boolean("stream", true);

        // What a response cost, which this endpoint sends only when it is asked
        // to. It arrives after the answer in a chunk of its own, so asking for
        // it costs nothing but the field.
        body.object("stream_options", |options| {
            options.boolean("include_usage", true);
        });

        body.array("messages", |messages| write_messages(messages, request));

        // Beside `model` rather than nested, which is where this older wire
        // shape puts it. Only where somebody chose one: this vendor serves
        // three of the five rungs.
        if let Some(effort) = request.effort {
            body.text("reasoning_effort", effort.as_str());
        }

        // Absent rather than empty: an empty array is refused rather than read
        // as a session with no tools.
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

/// The transcript, as the list of messages this endpoint reads.
///
/// Standing instructions go in front of it as a message of their own, which is
/// the only place this wire has for them. It is a weaker promise than a field —
/// the model may answer the instructions rather than obey them — and it is the
/// one this endpoint offers.
fn write_messages(messages: &mut Array<'_>, request: &Request<'_>) {
    if let Some(system) = request.system {
        messages.object(|message| {
            message.text("role", "system");
            message.text("content", system);
        });
    }

    for message in request.transcript.messages() {
        append(messages, message);
    }
}

/// One message, as however many this wire needs for it.
///
/// Appends rather than maps because the counts differ both ways: a turn that
/// called three tools is answered by three messages, and a turn cut short is
/// two.
fn append(messages: &mut Array<'_>, message: &Message) {
    match message {
        Message::User { text, .. } => messages.object(|message| {
            message.text("role", "user");
            message.text("content", text);
        }),
        Message::Agent { text, calls, stop } => {
            // Both fields are optional and one of them has to be there. A model
            // that goes straight to a tool says nothing first, and a message
            // with neither is one the API refuses.
            // Nothing said and nothing asked for: a turn cancelled or filtered
            // before the model's first word. It is recorded, so it would be
            // sent on every turn after it — one bad turn making the session
            // refuse to continue at all.
            if text.is_empty() && calls.is_empty() {
                return;
            }

            messages.object(|assistant| {
                assistant.text("role", "assistant");
                if !text.is_empty() {
                    assistant.text("content", text);
                }
                if !calls.is_empty() {
                    assistant.array("tool_calls", |items| {
                        for call in calls {
                            items.object(|item| write_call(item, call));
                        }
                    });
                }
            });

            // A message of its own after the answer. Left off, the model reads
            // its own half-sentence as a turn it chose to end — on the next
            // turn of this session and on every turn of a continued one.
            //
            // It cannot follow a message that carries tool calls: this wire
            // requires the next message after those to be their results, and
            // one in between is a request the API refuses outright. A turn
            // holding calls ended by asking for them, which is not a cut.
            if let Some(said) = StopReason::cut(*stop).filter(|_| calls.is_empty()) {
                messages.object(|message| {
                    message.text("role", "assistant");
                    message.text("content", said);
                });
            }
        }
        // One message each, and a role of their own. Answered by `tool_call_id`
        // rather than by position, which is what lets a turn's results arrive
        // in any order.
        Message::ToolResults(results) => {
            for result in results {
                messages.object(|message| write_result(message, result));
            }
        }
    }
}

/// One call the model made.
fn write_call(item: &mut Object<'_>, call: &ToolCall) {
    item.text("id", call.id.as_str());
    item.text("type", "function");
    item.object("function", |function| {
        function.text("name", &call.name);
        function.text("arguments", arguments(call.args.as_str()));
    });
}

/// Argument text, as the model wrote it.
///
/// A string rather than an object, which is this field's type. Parsing and
/// re-encoding would hand the model back something it did not write, and the
/// arguments it sees would stop matching the ones it produced.
///
/// A tool that takes no arguments is called with no argument text at all, and
/// an empty string is not JSON on the other side.
fn arguments(args: &str) -> &str {
    if args.trim().is_empty() { "{}" } else { args }
}

/// One tool result, as its own message.
fn write_result(message: &mut Object<'_>, result: &ToolResult) {
    let text = result.output.text();
    message.text("role", "tool");
    message.text("tool_call_id", result.id.as_str());
    if result.output.is_failed() {
        message.prefixed_text("content", "error: ", text);
    } else {
        message.text("content", text);
    }
}

/// One tool, as advertised.
///
/// Nested under a `function` object, which is where this endpoint keeps a
/// tool's name and schema and where the newer one does not.
fn write_tool(tool: &mut Object<'_>, schema: &ToolSchema) {
    let (parameters, description) = described(schema.schema);
    tool.text("type", "function");
    tool.object("function", |function| {
        function.text("name", schema.name);
        function.text("description", &description);
        function.value("parameters", &serde_json::Value::Object(parameters));
    });
}

#[cfg(test)]
mod tests;
