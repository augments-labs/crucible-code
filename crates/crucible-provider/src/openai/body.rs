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

use crucible_core::{
    Attached, Content, Message, Modality, Request, StopReason, ToolCall, ToolResult, ToolSchema,
};
#[cfg(test)]
use serde_json::Value;

use super::Serving;
use crate::json::{Array, Json, Object, described};

/// The whole request body, as `serving` accepts it.
pub(super) fn serialize(request: &Request<'_>, serving: Serving) -> String {
    let mut json = Json::new();
    json.object(|body| {
        body.text("model", request.model);
        body.boolean("stream", true);

        // This endpoint counts reasoning and visible output together. The
        // request's ceiling is for generated tokens, so `max_output_tokens` is
        // its exact wire counterpart rather than a visible-answer promise.
        //
        // The plan backend does not implement the field, and refuses the whole
        // request over it rather than ignoring it — so every turn failed with
        // `Unsupported parameter: max_output_tokens` before the ceiling was
        // asked where it was going. Left off, the plan's own ceiling applies,
        // which is the only one that service was ever going to honour.
        if serving == Serving::Api {
            body.number("max_output_tokens", request.max_tokens);
        }

        // This endpoint retains a response for retrieval unless told
        // otherwise, and what a coding agent sends is somebody's source.
        body.boolean("store", false);

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

/// The body the published API receives, which is the whole of it.
#[cfg(test)]
fn build(request: &Request<'_>) -> Value {
    served(request, Serving::Api)
}

/// The body one of the two services receives.
#[cfg(test)]
fn served(request: &Request<'_>, serving: Serving) -> Value {
    serde_json::from_str(&serialize(request, serving)).expect("request body is JSON")
}

/// The transcript, as the flat list of items this endpoint reads.
fn write_input(items: &mut Array<'_>, request: &Request<'_>) {
    for (nth, message) in request.transcript.messages().iter().enumerate() {
        append(items, message, nth, request.attached);
    }
}

/// One message, as however many items this wire needs.
///
/// Appends rather than maps because the counts differ both ways: a turn that
/// said something and then called three tools is four items, and a turn that
/// called none is one.
fn append(items: &mut Array<'_>, message: &Message, nth: usize, attached: &[Attached<'_>]) {
    match message {
        Message::Context(fragment) => items.object(|item| {
            item.text("role", "user");
            item.text("content", fragment.text());
        }),
        Message::User { text, .. } => items.object(|item| {
            item.text("role", "user");

            let mut files = attached.iter().filter(|one| one.message == nth).peekable();
            if files.peek().is_none() {
                item.text("content", text);
                return;
            }

            // Parts rather than a string, and the picture ahead of the words —
            // which is what the vendor's own guidance asks for, and the words
            // are one prompt behind however many files it named.
            item.array("content", |content| {
                for one in files {
                    content.object(|part| write_attached(part, one));
                }
                // A prompt that named a file and said nothing else is the
                // picture alone, rather than a part carrying no words.
                if !text.is_empty() {
                    content.object(|part| {
                        part.text("type", "input_text");
                        part.text("text", text);
                    });
                }
            });
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
            // One message's files, handed out in the order the results claim
            // them: an attachment's index is its place across the whole
            // message, so each result takes as many as it holds and the next
            // one starts where it stopped.
            let mut files = attached.iter().filter(|one| one.message == nth);
            for result in results {
                let found: Vec<_> = files
                    .by_ref()
                    .take(result.output.attachments().len())
                    .collect();
                items.object(|item| write_result(item, result, &found));
            }
        }
    }
}

/// One attached file, or the line standing where it would have been.
///
/// The sentence is printed rather than composed: the runner is the only thing
/// that knows which of its three reasons applies, and a part that invented its
/// own wording would be a fourth.
///
/// `detail` is left off. It is optional, and the endpoint's own default is a
/// better answer than one this harness would have to guess per image.
///
/// `filename` is not optional beside base64, and is the attachment's place in
/// the transcript rather than the name the person typed: a provider is handed
/// what the runner resolved and never a path, and the prompt travelling beside
/// it already says which file they meant.
fn write_attached(part: &mut Object<'_>, attached: &Attached<'_>) {
    match attached.content {
        // Every modality is spelled out rather than caught by a wildcard, so a
        // sixth one added to the enum arrives here as a compiler error rather
        // than as a picture. What keeps the other three from reaching this at
        // all is `spells()`, which names the two this answers and is tested
        // against it.
        Content::Bytes(bytes) => match attached.modality {
            Modality::Pdf => {
                part.text("type", "input_file");
                part.text(
                    "filename",
                    &format!("attachment-{}-{}.pdf", attached.message, attached.index),
                );
                part.prefixed_encoded(
                    "file_data",
                    &format!("data:{};base64,", attached.media_type),
                    bytes,
                );
            }
            Modality::Text | Modality::Image | Modality::Video | Modality::Audio => {
                part.text("type", "input_image");
                part.prefixed_encoded(
                    "image_url",
                    &format!("data:{};base64,", attached.media_type),
                    bytes,
                );
            }
        },
        Content::Instead(line) => {
            part.text("type", "input_text");
            part.text("text", line);
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
fn write_result(item: &mut Object<'_>, result: &ToolResult, found: &[&Attached<'_>]) {
    let text = result.output.text();
    let failed = result.output.is_failed();
    item.text("type", "function_call_output");
    item.text("call_id", result.id.as_str());

    // A string where a tool answered in words alone, which is every call made
    // before a tool could find a file.
    if found.is_empty() {
        if failed {
            item.prefixed_text("output", "error: ", text);
        } else {
            item.text("output", text);
        }
        return;
    }

    // Parts, with the words leading — the other way round from a prompt, where
    // the picture is what the vendor reads better first. Here the words are
    // what say which file is which, and the prefix that marks a failure is on
    // them because this wire still has nowhere else to put it.
    item.array("output", |output| {
        if failed {
            output.object(|part| {
                part.text("type", "input_text");
                part.prefixed_text("text", "error: ", text);
            });
        } else if !text.is_empty() {
            output.object(|part| {
                part.text("type", "input_text");
                part.text("text", text);
            });
        }
        for one in found {
            output.object(|part| write_attached(part, one));
        }
    });
}

/// One tool, as advertised.
///
/// Flat, unlike the older endpoint's nesting under a `function` object. `strict`
/// is stated rather than left out: strict mode requires every property to be
/// required and additional ones refused, which is not what these schemas say, so
/// the answer is no and saying it is what keeps a later default from changing
/// how a tool is validated.
fn write_tool(tool: &mut Object<'_>, schema: &ToolSchema<'_>) {
    let (parameters, description) = described(schema.schema);
    tool.text("type", "function");
    tool.text("name", schema.name);
    tool.text("description", &description);
    tool.value("parameters", &serde_json::Value::Object(parameters));
    tool.boolean("strict", false);
}

#[cfg(test)]
mod tests;
