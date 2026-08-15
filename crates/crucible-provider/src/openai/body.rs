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
#[cfg(test)]
use serde_json::Value;

use crate::json::{Array, Json, Object, described};

/// The whole request body.
pub(super) fn serialize(request: &Request<'_>) -> String {
    let mut json = Json::new();
    json.object(|body| {
        body.text("model", request.model);
        body.boolean("stream", true);
        body.number("max_output_tokens", request.max_tokens);

        // This endpoint retains a response for retrieval unless told
        // otherwise, and what a coding agent sends is somebody's source.
        body.boolean("store", false);

        // This endpoint counts reasoning and visible output together. The
        // request's ceiling is for generated tokens, so `max_output_tokens` is
        // its exact wire counterpart rather than a visible-answer promise.

        // A field rather than a message, which is the whole difference from the
        // older endpoint.
        if let Some(system) = request.system {
            body.text("instructions", system);
        }

        if let Some(effort) = request.effort {
            body.object("reasoning", |reasoning| {
                reasoning.text("effort", effort.as_str());
            });
        }

        body.array("input", |input| write_input(input, request));

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

/// The transcript, as the flat list of items this endpoint reads.
fn write_input(items: &mut Array<'_>, request: &Request<'_>) {
    for message in request.transcript.messages() {
        append(items, message);
    }
}

/// One message, as however many items this wire needs.
///
/// Appends rather than maps because the counts differ both ways: a turn that
/// said something and then called three tools is four items, and a turn that
/// called none is one.
fn append(items: &mut Array<'_>, message: &Message) {
    match message {
        Message::User(text) => items.object(|item| {
            item.text("role", "user");
            item.text("content", text);
        }),
        Message::Agent { text, calls, stop } => {
            // A model that goes straight to a tool says nothing first, and an
            // empty message is an item with no content for the model to read
            // back. The calls beside it carry the turn instead.
            if !text.is_empty() {
                items.object(|item| {
                    item.text("role", "assistant");
                    item.text("content", text);
                });
            }

            for call in calls {
                items.object(|item| write_call(item, call));
            }

            // An item of its own after the answer, and only where this message
            // put an answer in front of it. Left off, the model reads its own
            // half-sentence as a turn it chose to end — on the next turn of
            // this session and on every turn of a continued one.
            let answered = !text.is_empty() || !calls.is_empty();

            if let Some(said) = StopReason::cut(*stop).filter(|_| answered) {
                items.object(|item| {
                    item.text("role", "assistant");
                    item.text("content", said);
                });
            }
        }
        Message::ToolResults(results) => {
            for result in results {
                items.object(|item| write_result(item, result));
            }
        }
    }
}

/// One call the model made, as its own item.
fn write_call(item: &mut Object<'_>, call: &ToolCall) {
    item.text("type", "function_call");
    item.text("call_id", call.id.as_str());
    item.text("name", &call.name);
    item.text("arguments", arguments(call.args.as_str()));
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
fn write_result(item: &mut Object<'_>, result: &ToolResult) {
    let text = result.output.text();
    item.text("type", "function_call_output");
    item.text("call_id", result.id.as_str());
    if result.output.is_failed() {
        item.prefixed_text("output", "error: ", text);
    } else {
        item.text("output", text);
    }
}

/// One tool, as advertised.
///
/// Flat, unlike the older endpoint's nesting under a `function` object. `strict`
/// is stated rather than left out: strict mode requires every property to be
/// required and additional ones refused, which is not what these schemas say, so
/// the answer is no and saying it is what keeps a later default from changing
/// how a tool is validated.
fn write_tool(tool: &mut Object<'_>, schema: &ToolSchema) {
    let (parameters, description) = described(schema.schema);
    tool.text("type", "function");
    tool.text("name", schema.name);
    tool.text("description", &description);
    tool.value("parameters", &serde_json::Value::Object(parameters));
    tool.boolean("strict", false);
}

#[cfg(test)]
mod tests;
