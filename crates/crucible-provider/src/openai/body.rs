//! A request, in OpenAI's shape.
//!
//! One direction only: domain types in, JSON out. The response travels the
//! other way through [`super::wire`], and keeping the two apart is what stops a
//! change to one shape from quietly altering the other.
//!
//! What differs from the other protocol is here rather than spread through the
//! crate: standing instructions are a field with another name, a transcript is
//! a flat list of *items* rather than a list of messages, a tool call and its
//! result are items of their own rather than parts of a message, tool arguments
//! travel as JSON *text* rather than as an object, a tool is declared flat
//! rather than nested under a `function` object, a failed result is marked by a
//! prefix on the text because there is no field for it, and how hard to think
//! is nested under `reasoning` rather than named at the top of the body.

use crucible_core::{Message, Request, StopReason, ToolCall, ToolResult, ToolSchema};
use serde_json::{Map, Value, json};

use crate::json::described;

/// The whole request body.
pub(super) fn build(request: &Request) -> Value {
    let mut body = Map::new();
    body.insert("model".to_owned(), json!(&*request.model));
    body.insert("stream".to_owned(), json!(true));

    // Not kept by the vendor. This endpoint stores a response for retrieval
    // unless told otherwise, and what a coding agent sends is somebody's source
    // — so the default is the one setting here that has to be overridden rather
    // than accepted.
    body.insert("store".to_owned(), json!(false));

    // No ceiling is sent, and the field is left off deliberately rather than
    // forgotten. On this endpoint one number bounds the reasoning and the
    // visible answer together, and the models this protocol is for reason
    // before they answer: a figure chosen for an answer is one the model can
    // spend entirely on thinking, and the turn then ends having said nothing.
    // The model's own ceiling is the one that applies.

    // A field rather than a message, which is the whole difference from the
    // older endpoint. Sent as a message it would be one more thing the model
    // may answer rather than the instructions it answers under.
    if let Some(system) = &request.system {
        body.insert("instructions".to_owned(), json!(&**system));
    }

    // Only where somebody chose one. Which rungs a model serves is the model's
    // business on this endpoint and one it does not serve is a refusal, so a
    // session nobody has an opinion about sends nothing and takes the vendor's
    // own default for whichever model it is on.
    if let Some(effort) = request.effort {
        body.insert("reasoning".to_owned(), json!({ "effort": effort.as_str() }));
    }

    body.insert("input".to_owned(), Value::Array(input(request)));

    // Absent rather than empty: an empty array is refused rather than read as
    // a session with no tools.
    if !request.tools.is_empty() {
        let tools = request.tools.iter().map(tool).collect();
        body.insert("tools".to_owned(), Value::Array(tools));
    }

    Value::Object(body)
}

/// The transcript, as the flat list of items this endpoint reads.
fn input(request: &Request) -> Vec<Value> {
    let mut items = Vec::new();

    for message in request.transcript.messages() {
        append(&mut items, message);
    }

    items
}

/// One message, as however many items this wire needs for it.
///
/// Appends rather than maps because the counts differ both ways: a turn that
/// said something and then called three tools is four items, and a turn that
/// called none is one.
fn append(items: &mut Vec<Value>, message: &Message) {
    match message {
        Message::User(text) => items.push(json!({ "role": "user", "content": &**text })),
        Message::Agent { text, calls, stop } => {
            let before = items.len();

            // A model that goes straight to a tool says nothing first, and an
            // empty message is an item with no content for the model to read
            // back. The calls beside it carry the turn instead.
            if !text.is_empty() {
                items.push(json!({ "role": "assistant", "content": &**text }));
            }

            items.extend(calls.iter().map(call));

            // An item of its own after the answer, and only where this message
            // put an answer in front of it. Left off, the model reads its own
            // half-sentence as a turn it chose to end — on the next turn of
            // this session and on every turn of a continued one.
            let answered = items.len() > before;

            if let Some(said) = StopReason::cut(*stop).filter(|_| answered) {
                items.push(json!({ "role": "assistant", "content": said }));
            }
        }
        Message::ToolResults(results) => items.extend(results.iter().map(result)),
    }
}

/// One call the model made, as its own item.
fn call(call: &ToolCall) -> Value {
    json!({
        "type": "function_call",
        "call_id": call.id.as_str(),
        "name": &*call.name,
        "arguments": arguments(call.args.as_str()),
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

/// One tool result, as its own item.
///
/// Answered by `call_id` rather than by position, which is what lets a turn's
/// results arrive in any order and lets several calls be answered at once.
fn result(result: &ToolResult) -> Value {
    let text = result.output.text();

    // There is no field for a failure on this wire. Unmarked, "no such file:
    // x" reads as the contents of a file that was read successfully.
    let output = if result.output.is_failed() {
        format!("error: {text}")
    } else {
        text.to_owned()
    };

    json!({
        "type": "function_call_output",
        "call_id": result.id.as_str(),
        "output": output,
    })
}

/// One tool, as advertised.
///
/// Flat, unlike the older endpoint's nesting under a `function` object. `strict`
/// is stated rather than left out: strict mode requires every property to be
/// required and additional ones refused, which is not what these schemas say, so
/// the answer is no and saying it is what keeps a later default from changing
/// how a tool is validated.
fn tool(schema: &ToolSchema) -> Value {
    let (parameters, description) = described(schema.schema);

    json!({
        "type": "function",
        "name": schema.name,
        "description": description,
        "parameters": Value::Object(parameters),
        "strict": false,
    })
}

#[cfg(test)]
mod tests;
