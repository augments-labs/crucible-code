//! A request, in OpenAI's shape.
//!
//! One direction only: domain types in, JSON out. The response travels the
//! other way through [`super::wire`], and keeping the two apart is what stops a
//! change to one shape from quietly altering the other.
//!
//! What differs from the other protocol is here rather than spread through the
//! crate: standing instructions are a message rather than a field, tool
//! arguments travel as JSON *text* rather than as an object, a turn's tool
//! results are one message each rather than one message together, the token
//! ceiling has another name, and a failed result is marked by a prefix on the
//! text because there is no field for it.

use crucible_core::{Message, Request, ToolCall, ToolResult, ToolSchema};
use serde_json::{Map, Value, json};

use crate::json::described;

/// The whole request body.
pub(super) fn build(request: &Request) -> Value {
    let mut body = Map::new();
    body.insert("model".to_owned(), json!(&*request.model));
    body.insert("stream".to_owned(), json!(true));

    // `max_tokens` is the older name, now deprecated, and the models that
    // reason before answering refuse it outright. Not quite a rename: this one
    // bounds the reasoning as well as the visible answer, so the same number
    // buys a shorter reply from a model that thinks first.
    body.insert(
        "max_completion_tokens".to_owned(),
        json!(request.max_tokens),
    );
    body.insert("messages".to_owned(), Value::Array(messages(request)));

    // Absent rather than empty: an empty array is refused rather than read as
    // a session with no tools.
    if !request.tools.is_empty() {
        let tools = request.tools.iter().map(tool).collect();
        body.insert("tools".to_owned(), Value::Array(tools));
    }

    Value::Object(body)
}

/// Every message, in order, behind the standing instructions.
///
/// The system prompt is a message here rather than a field of its own. There is
/// no field for it on this wire, and one sent anyway is refused outright: this
/// endpoint rejects a top-level field it does not know rather than skipping it,
/// so the mistake would cost every turn rather than the instructions alone.
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
/// Appends rather than maps because the counts differ: a turn that called three
/// tools holds one message of results, and this protocol answers each call in a
/// message that names it.
fn append(messages: &mut Vec<Value>, message: &Message) {
    match message {
        Message::User(text) => messages.push(json!({ "role": "user", "content": &**text })),
        Message::Agent { text, calls, .. } => messages.push(agent(text, calls)),
        Message::ToolResults(results) => messages.extend(results.iter().map(result)),
    }
}

/// What the model said, and what it asked to run.
fn agent(text: &str, calls: &[ToolCall]) -> Value {
    let mut message = Map::new();
    message.insert("role".to_owned(), json!("assistant"));

    // A model that goes straight to a tool says nothing first, and null is how
    // this wire spells that. Null with no calls to carry it is a different
    // thing — a message with neither content nor purpose, which this wire
    // requires one of — so an answer that came back empty stays an empty
    // string. A cancelled turn and a filtered one both reach here.
    let content = if text.is_empty() && !calls.is_empty() {
        Value::Null
    } else {
        json!(text)
    };
    message.insert("content".to_owned(), content);

    if !calls.is_empty() {
        let calls = calls.iter().map(call).collect();
        message.insert("tool_calls".to_owned(), Value::Array(calls));
    }

    Value::Object(message)
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
