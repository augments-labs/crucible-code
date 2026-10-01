//! A request, in the Responses shape.
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
//!
//! What differs between the vendors that serve it is asked of the
//! [`Dialect`]: which of its services takes the output ceiling and the explicit
//! cache breakpoint, what asks its cache to keep a prefix, how a fast answer is
//! asked for, how a tool is declared and how words before a call are marked.
//! A model whose turns go back as the items it answered with is written by its
//! [`Replay`] instead.

use crucible_models::{Attached, Content, PromptCacheBoundary, ProviderError, Request};
use crucible_types::{
    ContinuationScope, Message, Modality, PromptCacheEncoding, PromptCacheIneligibleReason,
    PromptCacheMechanism, PromptCacheRetentionClass, StopReason, ToolCall, ToolResult, ToolSchema,
};

use super::{Dialect, Replay};
use crate::json::{Array, Json, Object, described};

/// The whole request body, as `route` accepts it, asking for the fast tier
/// where `fast`. `scope` is where a model that replays keeps its items, and
/// nothing for one that does not.
pub(crate) fn serialize<D: Dialect>(
    request: &Request<'_>,
    route: D::Route,
    scope: Option<ContinuationScope>,
    fast: bool,
) -> Result<String, ProviderError> {
    let explicit_message = explicit_message::<D>(request);
    let explicit = explicit_message.filter(|_| D::breakpoints(route));
    let mut json = Json::new();
    let mut outcome = Ok(());
    json.object(|body| {
        body.text("model", request.model);
        if fast && let Some(tier) = D::FAST_TIER {
            body.text("service_tier", tier);
        }
        body.boolean("stream", true);

        // This endpoint counts reasoning and visible output together. The
        // request's ceiling is for generated tokens, so `max_output_tokens` is
        // its exact wire counterpart rather than a visible-answer promise.
        //
        // A service that does not implement the field may refuse the whole
        // request over it rather than ignore it, so the dialect says where it
        // goes. Left off, the service's own ceiling applies, which is the only
        // one that service was ever going to honour.
        if D::ceiling(route) {
            body.number("max_output_tokens", request.max_tokens);
        }

        // This endpoint retains a response for retrieval unless told
        // otherwise, and what a coding agent sends is somebody's source.
        body.boolean("store", false);

        cache::<D>(body, request, explicit);

        // A field rather than a message, which is the whole difference from the
        // older endpoint.
        if let Some(system) = request.system {
            body.text("instructions", system);
        }

        if let Some(scope) = scope {
            outcome = D::Replay::write(body, request, scope, explicit);
        } else {
            if let Some(effort) = request.effort {
                body.object("reasoning", |reasoning| {
                    reasoning.text("effort", effort.as_str());
                });
            }
            body.array("input", |input| write_input::<D>(input, request, explicit));
        }

        // Absent rather than empty: an empty array is refused rather than read
        // as a session with no tools.
        if !request.tools.is_empty() {
            body.array("tools", |tools| {
                for schema in request.tools {
                    tools.object(|tool| write_tool::<D>(tool, schema));
                }
            });
        }
    });
    outcome.map(|()| json.finish())
}

/// What asks the prompt cache for something, where the request selected a
/// mechanism. `explicit` is the message a breakpoint marks, where the route
/// takes one.
fn cache<D: Dialect>(body: &mut Object<'_>, request: &Request<'_>, explicit: Option<usize>) {
    let Some(cache) = request.prompt_cache else {
        return;
    };
    let Some(selected) = cache.selection.selected() else {
        return;
    };
    match selected.mechanism() {
        PromptCacheMechanism::AutomaticPrefix => {
            if let Some(key) = cache.routing_key {
                body.text("prompt_cache_key", key.as_str());
            }
            if let Some(hint) = D::retention(request.model, cache.policy.retention().class()) {
                hint(body);
            }
        }
        PromptCacheMechanism::ExplicitBreakpoints if explicit.is_some() => {
            body.object("prompt_cache_options", |options| {
                options.text("mode", "explicit");
                if cache.policy.retention().class() == PromptCacheRetentionClass::Ephemeral {
                    options.text("ttl", "30m");
                }
            });
        }
        PromptCacheMechanism::ProviderManagedUsageOnly
        | PromptCacheMechanism::ExplicitBreakpoints
        | PromptCacheMechanism::PersistentContent => {}
    }
}

/// The cache metadata [`serialize`] adds for this exact request.
pub(crate) fn prompt_cache_encoding<D: Dialect>(
    request: &Request<'_>,
    route: D::Route,
) -> PromptCacheEncoding {
    let Some(cache) = request.prompt_cache else {
        return PromptCacheEncoding::NoControlIntended;
    };
    let Some(selected) = cache.selection.selected() else {
        return PromptCacheEncoding::NoControlIntended;
    };
    match selected.mechanism() {
        PromptCacheMechanism::ProviderManagedUsageOnly => {
            PromptCacheEncoding::NoExtraControlEncoded
        }
        PromptCacheMechanism::AutomaticPrefix => {
            let hinted = cache.routing_key.is_some()
                || D::retention(request.model, selected.retention()).is_some();
            if hinted {
                PromptCacheEncoding::AutomaticHintEncoded
            } else {
                PromptCacheEncoding::NoExtraControlEncoded
            }
        }
        PromptCacheMechanism::ExplicitBreakpoints if D::breakpoints(route) => {
            if explicit_message::<D>(request).is_some() {
                PromptCacheEncoding::BreakpointsEncoded(1)
            } else {
                PromptCacheEncoding::Failed(PromptCacheIneligibleReason::UnsupportedBoundary)
            }
        }
        PromptCacheMechanism::ExplicitBreakpoints | PromptCacheMechanism::PersistentContent => {
            PromptCacheEncoding::Failed(PromptCacheIneligibleReason::Unsupported)
        }
    }
}

/// The transcript, as the flat list of items this endpoint reads.
fn write_input<D: Dialect>(
    items: &mut Array<'_>,
    request: &Request<'_>,
    explicit_message: Option<usize>,
) {
    let mut history = crate::history::LegacyHistory::default();
    for (nth, message) in request.transcript.messages().iter().enumerate() {
        if history.neutral(message) || request.purpose == crucible_models::RequestPurpose::Recap {
            visible(items, message, explicit_message == Some(nth));
            continue;
        }
        append::<D>(
            items,
            message,
            nth,
            request.attached,
            explicit_message == Some(nth),
        );
    }
}

/// A message no item can carry as itself, as what a reader would see of it,
/// from the user.
fn visible(items: &mut Array<'_>, message: &Message, breakpoint: bool) {
    items.object(|item| {
        item.text("role", "user");
        if breakpoint {
            item.array("content", |content| {
                content.object(|part| {
                    part.text("type", "input_text");
                    part.text_with("text", |write| crate::history::visible(message, write));
                    part.object("prompt_cache_breakpoint", |marker| {
                        marker.text("mode", "explicit");
                    });
                });
            });
        } else {
            item.text_with("content", |write| crate::history::visible(message, write));
        }
    });
}

/// One message, as however many items this wire needs.
///
/// Appends rather than maps because the counts differ both ways: a turn that
/// said something and then called three tools is four items, and a turn that
/// called none is one.
pub(crate) fn append<D: Dialect>(
    items: &mut Array<'_>,
    message: &Message,
    nth: usize,
    attached: &[Attached<'_>],
    breakpoint: bool,
) {
    match message {
        Message::Context(fragment) => items.object(|item| {
            item.text("role", "user");
            if breakpoint {
                item.array("content", |content| {
                    content.object(|part| write_input_text(part, fragment.text(), true));
                });
            } else {
                item.text("content", fragment.text());
            }
        }),
        Message::User { text, .. } => items.object(|item| {
            item.text("role", "user");

            let mut files = attached.iter().filter(|one| one.message == nth).peekable();
            if files.peek().is_none() {
                if breakpoint {
                    item.array("content", |content| {
                        content.object(|part| write_input_text(part, text, true));
                    });
                } else {
                    item.text("content", text);
                }
                return;
            }

            // Parts rather than a string, and the picture ahead of the words,
            // which is what the vendor's own guidance asks for, and the words
            // are one prompt behind however many files it named.
            item.array("content", |content| {
                for one in files {
                    content.object(|part| write_attached(part, one));
                }
                // A prompt that named a file and said nothing else is the
                // picture alone, rather than a part carrying no words.
                if !text.is_empty() {
                    content.object(|part| write_input_text(part, text, breakpoint));
                }
            });
        }),
        Message::Agent {
            continuation: _,
            text,
            calls,
            stop,
        } => {
            let answered = !text.is_empty() || !calls.is_empty();
            let cut = StopReason::cut(*stop).filter(|_| answered);

            // A model that goes straight to a tool says nothing first, and an
            // empty message is an item with no content for the model to read
            // back. The calls beside it carry the turn instead.
            if !text.is_empty() {
                items.object(|item| {
                    if D::COMMENTARY && !calls.is_empty() {
                        commentary(item, text);
                        return;
                    }
                    item.text("role", "assistant");
                    if breakpoint && cut.is_none() && calls.is_empty() {
                        item.array("content", |content| {
                            content.object(|part| write_input_text(part, text, true));
                        });
                    } else {
                        item.text("content", text);
                    }
                });
            }

            for call in calls {
                items.object(|item| write_call(item, call));
            }

            // An item of its own after the answer, and only where this message
            // put an answer in front of it. Left off, the model reads its own
            // half-sentence as a turn it chose to end, on the next turn of
            // this session and on every turn of a continued one.
            if let Some(said) = cut {
                items.object(|item| {
                    item.text("role", "assistant");
                    if breakpoint {
                        item.array("content", |content| {
                            content.object(|part| write_input_text(part, said, true));
                        });
                    } else {
                        item.text("content", said);
                    }
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

/// What the model said before calling a tool, marked as said on the way there.
///
/// The whole message item rather than a role and a string, because the mark is
/// one of its fields: read back without it, words in front of a call are an
/// answer the turn then carries on past.
fn commentary(item: &mut Object<'_>, text: &str) {
    item.text("type", "message");
    item.text("role", "assistant");
    item.text("phase", "commentary");
    item.array("content", |content| {
        content.object(|part| {
            part.text("type", "output_text");
            part.text("text", text);
        });
    });
}

/// The latest stable message whose last wire item can carry an explicit marker.
fn explicit_message<D: Dialect>(request: &Request<'_>) -> Option<usize> {
    let cache = request.prompt_cache?;
    let selected = cache.selection.selected()?;
    if selected.mechanism() != PromptCacheMechanism::ExplicitBreakpoints {
        return None;
    }
    let capability = cache
        .capabilities
        .mechanisms()
        .iter()
        .find(|candidate| candidate.mechanism() == PromptCacheMechanism::ExplicitBreakpoints)?;
    if capability.maximum_breakpoints() == 0 {
        return None;
    }

    let point = cache
        .plan
        .boundaries()
        .iter()
        .rev()
        .filter(|point| {
            // Replayed items go back unchanged. Select an earlier supported
            // input_text boundary rather than adding fields to encrypted
            // reasoning or output-item content whose marker support is
            // unreviewed.
            !D::Replay::replays(request.model)
                || point
                    .message()
                    .and_then(|index| usize::try_from(index).ok())
                    .and_then(|index| request.transcript.messages().get(index))
                    .is_some_and(|message| {
                        matches!(message, Message::User { .. } | Message::Context(_))
                            && explicitly_markable(message)
                    })
        })
        .find(|point| {
            matches!(
                point.kind(),
                PromptCacheBoundary::AfterMessage | PromptCacheBoundary::AfterContent
            ) && capability.boundaries().contains(&point.kind())
        })?;
    let message = usize::try_from(point.message()?).ok()?;
    request
        .transcript
        .messages()
        .get(message)
        .is_some_and(explicitly_markable)
        .then_some(message)
}

/// Whether the complete provider-visible message ends in a text content block.
fn explicitly_markable(message: &Message) -> bool {
    match message {
        Message::Context(fragment) => !fragment.text().is_empty(),
        Message::User { text, .. } => !text.is_empty(),
        Message::Agent {
            continuation: _,
            text,
            calls,
            stop,
        } => {
            let answered = !text.is_empty() || !calls.is_empty();
            StopReason::cut(*stop).is_some_and(|_| answered)
                || (!text.is_empty() && calls.is_empty())
        }
        // Function-call outputs can end in attachment content. Until a neutral
        // content-block boundary records the exact last emitted part, refusing
        // that placement is safer than marking an earlier item as the message
        // boundary.
        Message::ToolResults(_) => false,
    }
}

/// One input-text part, optionally carrying an explicit cache boundary.
fn write_input_text(part: &mut Object<'_>, text: &str, breakpoint: bool) {
    part.text("type", "input_text");
    part.text("text", text);
    if breakpoint {
        part.object("prompt_cache_breakpoint", |marker| {
            marker.text("mode", "explicit");
        });
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

    // Parts, with the words leading: the other way round from a prompt, where
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
/// Flat, unlike the older endpoint's nesting under a `function` object. Where
/// the dialect says so, `strict` is stated rather than left out: strict mode
/// requires every property to be required and additional ones refused, which is
/// not what these schemas say, so the answer is no and saying it is what keeps
/// a later default from changing how a tool is validated.
fn write_tool<D: Dialect>(tool: &mut Object<'_>, schema: &ToolSchema<'_>) {
    let (parameters, description) = described(schema.schema);
    tool.text("type", "function");
    tool.text("name", schema.name);
    tool.text("description", &description);
    tool.value("parameters", &serde_json::Value::Object(parameters));
    if D::STRICT {
        tool.boolean("strict", false);
    }
}
