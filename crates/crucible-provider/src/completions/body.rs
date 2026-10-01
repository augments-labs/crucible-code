//! A request, in the Chat Completions shape.
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

use crucible_models::{Attached, Content, Request};
use crucible_types::{
    ContinuationPart, ContinuationScope, Message, Modality, PromptCacheEncoding,
    PromptCacheIneligibleReason, PromptCacheMechanism, ProviderContinuation, StopReason, ToolCall,
    ToolResult, ToolSchema,
};

use super::{Dialect, Reasoning};
use crate::json::{Array, Json, Object, described};

/// The whole request body, as `D` reads it, with no reasoning to send back.
#[cfg(test)]
pub(crate) fn serialize<D: Dialect>(request: &Request<'_>) -> String {
    serialize_for::<D>(request, None)
}

/// The whole request body, as `D` reads it, sending back the reasoning kept
/// under `scope` where the vendor wants it.
pub(crate) fn serialize_for<D: Dialect>(
    request: &Request<'_>,
    scope: Option<ContinuationScope>,
) -> String {
    let mut json = Json::new();
    json.object(|body| {
        body.text("model", request.model);
        body.number(D::CEILING, request.max_tokens);
        body.boolean("stream", true);

        // What a response cost, which this endpoint sends only when it is asked
        // to. It arrives after the answer in a chunk of its own, so asking for
        // it costs nothing but the field.
        if D::USAGE_ASKED {
            body.object("stream_options", |options| {
                options.boolean("include_usage", true);
            });
        }

        for (flag, set) in D::FLAGS {
            body.boolean(flag, *set);
        }

        if let Some(key) = request
            .prompt_cache
            .and_then(|cache| cache.selection.selected().map(|selected| (cache, selected)))
            .filter(|(_, selected)| selected.mechanism() == PromptCacheMechanism::AutomaticPrefix)
            .and_then(|(cache, _)| cache.routing_key)
        {
            body.text("prompt_cache_key", key.as_str());
        }

        body.array("messages", |messages| {
            write_messages::<D>(messages, request, back::<D>(request, scope));
        });

        // Beside `model` rather than nested, which is where this older wire
        // shape puts it. Only where somebody chose one, and spelled as the
        // dialect's vendor reads it.
        if let Some(effort) = request.effort {
            body.text("reasoning_effort", D::effort(effort));
        }

        // Absent rather than empty: the wire's first vendor refuses an empty
        // array rather than read it as a session with no tools.
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

/// The cache metadata [`serialize`] adds for this exact request.
pub(crate) fn prompt_cache_encoding(request: &Request<'_>) -> PromptCacheEncoding {
    let Some(selected) = request
        .prompt_cache
        .and_then(|cache| cache.selection.selected())
    else {
        return PromptCacheEncoding::NoControlIntended;
    };
    match selected.mechanism() {
        PromptCacheMechanism::ProviderManagedUsageOnly => {
            PromptCacheEncoding::NoExtraControlEncoded
        }
        PromptCacheMechanism::AutomaticPrefix => {
            if request
                .prompt_cache
                .and_then(|cache| cache.routing_key)
                .is_some()
            {
                PromptCacheEncoding::AutomaticHintEncoded
            } else {
                PromptCacheEncoding::NoExtraControlEncoded
            }
        }
        PromptCacheMechanism::ExplicitBreakpoints | PromptCacheMechanism::PersistentContent => {
            PromptCacheEncoding::Failed(PromptCacheIneligibleReason::Unsupported)
        }
    }
}

/// The transcript, as the list of messages this endpoint reads.
///
/// Standing instructions go in front of it as a message of their own, which is
/// the only place this wire has for them. It is a weaker promise than a field
/// (the model may answer the instructions rather than obey them), and it is
/// the one this endpoint offers.
fn write_messages<D: Dialect>(messages: &mut Array<'_>, request: &Request<'_>, back: Back) {
    let mut history = crate::history::LegacyHistory::default();
    if let Some(system) = request.system {
        messages.object(|message| {
            message.text("role", "system");
            message.text("content", system);
        });
    }

    for (nth, message) in request.transcript.messages().iter().enumerate() {
        // An answer whose reasoning this wire kept for this credential and
        // address is this vendor's own, and goes back as an answer.
        let own = |state: &ProviderContinuation| match back {
            Back::Kept(scope) | Back::Every(scope) => {
                state.protocol() == super::wire::PROTOCOL && state.scope() == scope
            }
            Back::Nothing => false,
        };
        if history.neutral_unless(message, own)
            || request.purpose == crucible_models::RequestPurpose::Recap
        {
            messages.object(|item| {
                item.text("role", "user");
                item.text_with("content", |write| crate::history::visible(message, write));
            });
            continue;
        }
        append::<D>(messages, message, nth, request.attached, back);
    }
}

/// What this request sends back of the reasoning its earlier answers kept.
#[derive(Debug, Clone, Copy)]
enum Back {
    /// Nothing: the vendor keeps none, or nothing was kept under this scope.
    Nothing,
    /// What each answer kept, under `scope`.
    Kept(ContinuationScope),
    /// The same, and empty for an answer that kept none.
    Every(ContinuationScope),
}

/// What `request` sends back to `D`, given where it is going.
fn back<D: Dialect>(request: &Request<'_>, scope: Option<ContinuationScope>) -> Back {
    match (D::reasoning(request.model), scope) {
        (Reasoning::Unread, _) | (_, None) => Back::Nothing,
        (Reasoning::Required, Some(scope)) if !request.tools.is_empty() => Back::Every(scope),
        (Reasoning::Returned | Reasoning::Required, Some(scope)) => Back::Kept(scope),
    }
}

/// The reasoning an answer kept, where it was kept by this wire for this
/// credential at this address: another vendor's private state is not this
/// vendor's to read, and the same vendor reached another way is another scope.
fn kept(continuation: Option<&ProviderContinuation>, scope: ContinuationScope) -> Option<&str> {
    let state = continuation
        .filter(|state| state.protocol() == super::wire::PROTOCOL && state.scope() == scope)?;
    state.parts().iter().find_map(|part| match part {
        ContinuationPart::Opaque(data) => Some(data.as_str()),
        ContinuationPart::Text { .. } | ContinuationPart::Call { .. } => None,
    })
}

/// One message, as however many this wire needs for it.
///
/// Appends rather than maps because the counts differ both ways: a turn that
/// called three tools is answered by three messages, and a turn cut short is
/// two.
fn append<D: Dialect>(
    messages: &mut Array<'_>,
    message: &Message,
    nth: usize,
    attached: &[Attached<'_>],
    back: Back,
) {
    match message {
        Message::Context(fragment) => messages.object(|message| {
            message.text("role", "user");
            message.text("content", fragment.text());
        }),
        Message::User { text, .. } => messages.object(|message| {
            message.text("role", "user");

            let mut files = attached.iter().filter(|one| one.message == nth).peekable();
            if files.peek().is_none() {
                message.text("content", text);
                return;
            }

            // Parts rather than a string, and the picture ahead of the words,
            // which is the order every one of these protocols asks for. The
            // words are one prompt behind however many files it named.
            message.array("content", |content| {
                for one in files {
                    content.object(|part| write_attached::<D>(part, one));
                }
                // A prompt that named a file and said nothing else is the
                // picture alone, rather than a part carrying no words.
                if !text.is_empty() {
                    content.object(|part| {
                        part.text("type", "text");
                        part.text("text", text);
                    });
                }
            });
        }),
        Message::Agent {
            continuation,
            text,
            calls,
            stop,
        } => {
            // Both fields are optional and one of them has to be there. A model
            // that goes straight to a tool says nothing first, and a message
            // with neither is one the wire's first vendor refuses.
            // Nothing said and nothing asked for: a turn cancelled or filtered
            // before the model's first word. It is recorded, so it would be
            // sent on every turn after it: one bad turn making the session
            // refuse to continue at all.
            if text.is_empty() && calls.is_empty() {
                return;
            }

            messages.object(|assistant| {
                assistant.text("role", "assistant");
                if !text.is_empty() {
                    assistant.text("content", text);
                }
                let reasoning = match back {
                    Back::Nothing => None,
                    Back::Kept(scope) => kept(continuation.as_ref(), scope),
                    Back::Every(scope) => kept(continuation.as_ref(), scope).or(Some("")),
                };
                if let Some(reasoning) = reasoning {
                    assistant.text("reasoning_content", reasoning);
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
            // its own half-sentence as a turn it chose to end, on the next
            // turn of this session and on every turn of a continued one.
            //
            // It cannot follow a message that carries tool calls: this wire
            // requires the next message after those to be their results, and
            // one in between is a request its first vendor refuses outright. A
            // turn holding calls ended by asking for them, which is not a cut.
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
                messages.object(|message| write_result::<D>(message, result, &found));
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
/// The URL is an object of its own rather than the string the neighbouring
/// protocol takes: one nesting deeper, for the same bytes, which is why the
/// three of these are written out separately instead of shared. The provider's
/// declared modalities and the runner's intersection make every byte attachment
/// here an image or video. If that invariant is ever broken, a valid diagnostic
/// text part is safer than either mislabelling bytes as an image or emitting an
/// empty object.
fn write_attached<D: Dialect>(part: &mut Object<'_>, attached: &Attached<'_>) {
    match attached.content {
        Content::Bytes(bytes) => {
            let field = match attached.modality {
                Modality::Image => "image_url",
                Modality::Video => "video_url",
                Modality::Text | Modality::Pdf | Modality::Audio => {
                    part.text("type", "text");
                    part.text(
                        "text",
                        &format!(
                            "attachment omitted: {} requests do not support {} input",
                            D::TITLE,
                            attached.modality.as_str()
                        ),
                    );
                    return;
                }
            };
            part.text("type", field);
            part.object(field, |url| {
                url.prefixed_encoded(
                    "url",
                    &format!("data:{};base64,", attached.media_type),
                    bytes,
                );
            });
        }
        Content::Instead(line) => {
            part.text("type", "text");
            part.text("text", line);
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

/// One tool result, as its own message, and whatever files the tool found.
///
/// The parts array rests on the first vendor's published schema rather than on
/// a worked example: as read on 2026-08-22 the schema allows parts for
/// `content` and does not narrow them by role, and no example shows a tool
/// message using one. A result that found nothing keeps the string it always
/// sent, so this is reached only by a call that went looking for a file.
fn write_result<D: Dialect>(
    message: &mut Object<'_>,
    result: &ToolResult,
    found: &[&Attached<'_>],
) {
    let text = result.output.text();
    let failed = result.output.is_failed();
    message.text("role", "tool");
    message.text("tool_call_id", result.id.as_str());

    if found.is_empty() {
        if failed {
            message.prefixed_text("content", "error: ", text);
        } else {
            message.text("content", text);
        }
        return;
    }

    // The words lead, which is the other way round from a prompt: there the
    // picture is what the wire's first vendor reads better first, and here the
    // words are what say which file is which.
    message.array("content", |content| {
        if failed {
            content.object(|part| {
                part.text("type", "text");
                part.prefixed_text("text", "error: ", text);
            });
        } else if !text.is_empty() {
            content.object(|part| {
                part.text("type", "text");
                part.text("text", text);
            });
        }
        for one in found {
            content.object(|part| write_attached::<D>(part, one));
        }
    });
}

/// One tool, as advertised.
///
/// Nested under a `function` object, which is where this endpoint keeps a
/// tool's name and schema and where the newer one does not.
fn write_tool(tool: &mut Object<'_>, schema: &ToolSchema<'_>) {
    let (parameters, description) = described(schema.schema);
    tool.text("type", "function");
    tool.object("function", |function| {
        function.text("name", schema.name);
        function.text("description", &description);
        function.value("parameters", &serde_json::Value::Object(parameters));
    });
}
