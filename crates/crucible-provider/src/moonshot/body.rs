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
use serde_json::{Map, Value, json};

use crate::json::described;

/// The whole request body.
pub(super) fn build(request: &Request) -> Value {
    let mut body = Map::new();
    body.insert("model".to_owned(), json!(&*request.model));
    body.insert("max_tokens".to_owned(), json!(request.max_tokens));
    body.insert("stream".to_owned(), json!(true));
    body.insert("messages".to_owned(), json!(messages(request)));

    // Beside `model` rather than nested, which is where this older wire shape
    // puts it. Only where somebody chose one: this vendor serves three of the
    // five rungs, so a request nobody had an opinion about is one that cannot
    // be refused for naming a rung the model has never heard of.
    if let Some(effort) = request.effort {
        body.insert("reasoning_effort".to_owned(), json!(effort.as_str()));
    }

    // Absent rather than empty: an empty array is refused rather than read as
    // a session with no tools.
    if !request.tools.is_empty() {
        let tools = request.tools.iter().map(tool).collect();
        body.insert("tools".to_owned(), Value::Array(tools));
    }

    Value::Object(body)
}

/// The transcript, as the list of messages this endpoint reads.
///
/// Standing instructions go in front of it as a message of their own, which is
/// the only place this wire has for them. It is a weaker promise than a field —
/// the model may answer the instructions rather than obey them — and it is the
/// one this endpoint offers.
fn messages(request: &Request) -> Vec<Value> {
    let mut messages = Vec::new();

    if let Some(system) = &request.system {
        messages.push(json!({ "role": "system", "content": &**system }));
    }

    for message in request.transcript.messages() {
        append(&mut messages, message);
    }

    messages
}

/// One message, as however many this wire needs for it.
///
/// Appends rather than maps because the counts differ both ways: a turn that
/// called three tools is answered by three messages, and a turn cut short is
/// two.
fn append(messages: &mut Vec<Value>, message: &Message) {
    match message {
        Message::User(text) => messages.push(json!({ "role": "user", "content": &**text })),
        Message::Agent { text, calls, stop } => {
            let mut assistant = Map::new();
            assistant.insert("role".to_owned(), json!("assistant"));

            // Both fields are optional and one of them has to be there. A model
            // that goes straight to a tool says nothing first, and a message
            // with neither is one the API refuses.
            if !text.is_empty() {
                assistant.insert("content".to_owned(), json!(&**text));
            }
            if !calls.is_empty() {
                assistant.insert(
                    "tool_calls".to_owned(),
                    Value::Array(calls.iter().map(call).collect()),
                );
            }

            // Nothing said and nothing asked for: a turn cancelled or filtered
            // before the model's first word. It is recorded, so it would be
            // sent on every turn after it — one bad turn making the session
            // refuse to continue at all.
            if assistant.len() == 1 {
                return;
            }

            messages.push(Value::Object(assistant));

            // A message of its own after the answer. Left off, the model reads
            // its own half-sentence as a turn it chose to end — on the next
            // turn of this session and on every turn of a continued one.
            //
            // It cannot follow a message that carries tool calls: this wire
            // requires the next message after those to be their results, and
            // one in between is a request the API refuses outright. A turn
            // holding calls ended by asking for them, which is not a cut.
            if let Some(said) = StopReason::cut(*stop).filter(|_| calls.is_empty()) {
                messages.push(json!({ "role": "assistant", "content": said }));
            }
        }
        // One message each, and a role of their own. Answered by `tool_call_id`
        // rather than by position, which is what lets a turn's results arrive
        // in any order.
        Message::ToolResults(results) => messages.extend(results.iter().map(result)),
    }
}

/// One call the model made.
fn call(call: &ToolCall) -> Value {
    json!({
        "id": call.id.as_str(),
        "type": "function",
        "function": {
            "name": &*call.name,
            "arguments": arguments(call.args.as_str()),
        },
    })
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
fn result(result: &ToolResult) -> Value {
    let text = result.output.text();

    // There is no field for a failure on this wire. Unmarked, "no such file:
    // x" reads as the contents of a file that was read successfully.
    let content = if result.output.is_failed() {
        format!("error: {text}")
    } else {
        text.to_owned()
    };

    json!({
        "role": "tool",
        "tool_call_id": result.id.as_str(),
        "content": content,
    })
}

/// One tool, as advertised.
///
/// Nested under a `function` object, which is where this endpoint keeps a
/// tool's name and schema and where the newer one does not.
fn tool(schema: &ToolSchema) -> Value {
    let (parameters, description) = described(schema.schema);

    json!({
        "type": "function",
        "function": {
            "name": schema.name,
            "description": description,
            "parameters": Value::Object(parameters),
        },
    })
}

#[cfg(test)]
mod tests;
