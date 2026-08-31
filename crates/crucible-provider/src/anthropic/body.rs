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

use crucible_core::{
    Attached, Content, Message, Modality, PromptCacheBoundary, PromptCacheEncoding,
    PromptCacheIneligibleReason, PromptCacheMechanism, PromptCacheRetentionClass, Request,
    StopReason, ToolResult, ToolSchema,
};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

use crate::json::{Array, Json, Object, described, object};

/// The whole request body.
pub(super) fn serialize(request: &Request<'_>) -> String {
    let automatic = automatic_retention(request);
    let explicit = explicit_placement(request);
    let mut json = Json::new();
    json.object(|body| {
        body.text("model", request.model);
        body.number("max_tokens", request.max_tokens);
        body.boolean("stream", true);

        if let Some(retention) = automatic {
            write_cache_control(body, retention);
        }
        body.array("messages", |messages| {
            write_messages(messages, request, explicit);
        });

        // Absent rather than null: the API rejects a null system prompt, and a
        // session without one is the ordinary case.
        if let Some(system) = request.system {
            if let Some(ExplicitPlacement::System(retention)) = explicit {
                body.array("system", |content| {
                    content.object(|block| {
                        block.text("type", "text");
                        block.text("text", system);
                        write_cache_control(block, retention);
                    });
                });
            } else {
                body.text("system", system);
            }
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
                for (index, schema) in request.tools.iter().enumerate() {
                    let retention = match explicit {
                        Some(ExplicitPlacement::Tools(retention))
                            if index + 1 == request.tools.len() =>
                        {
                            Some(retention)
                        }
                        _ => None,
                    };
                    tools.object(|tool| write_tool(tool, schema, retention));
                }
            });
        }
    });
    json.finish()
}

/// The cache metadata [`serialize`] adds for this exact request.
pub(super) fn prompt_cache_encoding(request: &Request<'_>) -> PromptCacheEncoding {
    let Some(selected) = request
        .prompt_cache
        .and_then(|cache| cache.selection.selected())
    else {
        return PromptCacheEncoding::NoControlIntended;
    };
    match selected.mechanism() {
        PromptCacheMechanism::AutomaticPrefix => PromptCacheEncoding::AutomaticHintEncoded,
        PromptCacheMechanism::ExplicitBreakpoints => {
            if explicit_placement(request).is_some() {
                PromptCacheEncoding::BreakpointsEncoded(1)
            } else {
                PromptCacheEncoding::Failed(PromptCacheIneligibleReason::UnsupportedBoundary)
            }
        }
        PromptCacheMechanism::ProviderManagedUsageOnly => {
            PromptCacheEncoding::NoExtraControlEncoded
        }
        PromptCacheMechanism::PersistentContent => {
            PromptCacheEncoding::Failed(PromptCacheIneligibleReason::Unsupported)
        }
    }
}

/// The exact request-level automatic control selected for this request.
fn automatic_retention(request: &Request<'_>) -> Option<PromptCacheRetentionClass> {
    request
        .prompt_cache?
        .selection
        .selected()
        .filter(|selected| selected.mechanism() == PromptCacheMechanism::AutomaticPrefix)
        .map(crucible_core::PromptCacheSelected::retention)
}

/// One explicit marker at the latest provider-visible legal stable boundary.
///
/// A single marker caches the complete stable prefix and stays inside the
/// four-breakpoint ceiling. The neutral plan already proved content,
/// threshold, retention and point limits; lowering repeats the placement
/// check so a hand-built request cannot emit an unreviewed marker.
fn explicit_placement(request: &Request<'_>) -> Option<ExplicitPlacement> {
    let cache = request.prompt_cache?;
    let selected = cache
        .selection
        .selected()
        .filter(|selected| selected.mechanism() == PromptCacheMechanism::ExplicitBreakpoints)?;
    let capability = cache
        .capabilities
        .mechanisms()
        .iter()
        .find(|candidate| candidate.mechanism() == PromptCacheMechanism::ExplicitBreakpoints)?;
    if capability.maximum_breakpoints() == 0
        || !capability.retentions().contains(&selected.retention())
    {
        return None;
    }

    cache
        .plan
        .boundaries()
        .iter()
        .rev()
        .find(|point| capability.boundaries().contains(&point.kind()))
        .and_then(|point| match point.kind() {
            PromptCacheBoundary::AfterSystem => request
                .system
                .map(|_| ExplicitPlacement::System(selected.retention())),
            PromptCacheBoundary::AfterTools => (!request.tools.is_empty())
                .then_some(ExplicitPlacement::Tools(selected.retention())),
            PromptCacheBoundary::AfterMessage | PromptCacheBoundary::AfterContent => point
                .message()
                .and_then(|message| usize::try_from(message).ok())
                .filter(|message| *message < request.transcript.messages().len())
                .map(|message| ExplicitPlacement::Message(message, selected.retention())),
        })
}

#[derive(Clone, Copy)]
enum ExplicitPlacement {
    System(PromptCacheRetentionClass),
    Tools(PromptCacheRetentionClass),
    Message(usize, PromptCacheRetentionClass),
}

fn write_cache_control(parent: &mut Object<'_>, retention: PromptCacheRetentionClass) {
    parent.object("cache_control", |control| {
        control.text("type", "ephemeral");
        if retention == PromptCacheRetentionClass::Extended {
            control.text("ttl", "1h");
        }
    });
}

#[cfg(test)]
fn build(request: &Request<'_>) -> Value {
    serde_json::from_str(&serialize(request)).expect("request body is JSON")
}

/// Every message that has something in it, in order.
fn write_messages(
    messages: &mut Array<'_>,
    request: &Request<'_>,
    explicit: Option<ExplicitPlacement>,
) {
    for (nth, message) in request.transcript.messages().iter().enumerate() {
        let retention = match explicit {
            Some(ExplicitPlacement::Message(target, retention)) if target == nth => Some(retention),
            _ => None,
        };
        write_message(messages, message, nth, request.attached, retention);
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
    cache_retention: Option<PromptCacheRetentionClass>,
) {
    match message {
        Message::Context(fragment) => messages.object(|message| {
            message.text("role", "user");
            if let Some(retention) = cache_retention {
                message.array("content", |content| {
                    content.object(|block| {
                        block.text("type", "text");
                        block.text("text", fragment.text());
                        write_cache_control(block, retention);
                    });
                });
            } else {
                message.text("content", fragment.text());
            }
        }),
        Message::User { text, .. } => messages.object(|message| {
            message.text("role", "user");

            let mut files = attached.iter().filter(|one| one.message == nth).peekable();
            if files.peek().is_none() {
                if let Some(retention) = cache_retention {
                    message.array("content", |content| {
                        content.object(|block| {
                            block.text("type", "text");
                            block.text("text", text);
                            write_cache_control(block, retention);
                        });
                    });
                } else {
                    message.text("content", text);
                }
                return;
            }

            // The vendor's own guidance is that the picture reads better
            // ahead of the words, and the words are one prompt behind however
            // many files it named.
            message.array("content", |content| {
                let file_count = files.clone().count();
                for (index, one) in files.enumerate() {
                    content.object(|block| {
                        write_attached(block, one);
                        if text.is_empty()
                            && index + 1 == file_count
                            && let Some(retention) = cache_retention
                        {
                            write_cache_control(block, retention);
                        }
                    });
                }
                // A prompt that named a file and said nothing else is the
                // picture alone: an empty text block is one this vendor
                // refuses the request over rather than ignores.
                if !text.is_empty() {
                    content.object(|block| {
                        block.text("type", "text");
                        block.text("text", text);
                        if let Some(retention) = cache_retention {
                            write_cache_control(block, retention);
                        }
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
                    let cut = StopReason::cut(*stop);
                    // An empty text block is refused by the API, and the model
                    // produces one when it calls a tool without speaking first.
                    if !text.is_empty() {
                        content.object(|block| {
                            block.text("type", "text");
                            block.text("text", text);
                            if calls.is_empty()
                                && cut.is_none()
                                && let Some(retention) = cache_retention
                            {
                                write_cache_control(block, retention);
                            }
                        });
                    }

                    for (index, call) in calls.iter().enumerate() {
                        let input = Value::Object(object(call.args.as_str()));
                        content.object(|block| {
                            block.text("type", "tool_use");
                            block.text("id", call.id.as_str());
                            block.text("name", &call.name);
                            block.value("input", &input);
                            if index + 1 == calls.len()
                                && cut.is_none()
                                && let Some(retention) = cache_retention
                            {
                                write_cache_control(block, retention);
                            }
                        });
                    }

                    // A block of its own after a cut answer. Left off, the model
                    // reads its half-sentence as a turn it chose to end.
                    if let Some(said) = cut {
                        content.object(|block| {
                            block.text("type", "text");
                            block.text("text", said);
                            if let Some(retention) = cache_retention {
                                write_cache_control(block, retention);
                            }
                        });
                    }
                });
            });
        }
        // Results are the user's turn as far as the API is concerned: the model
        // asked, and this is the answer coming back to it.
        Message::ToolResults(results) => messages.object(|message| {
            message.text("role", "user");
            // One message's files, handed out in the order the results claim
            // them: an attachment's index is its place across the whole
            // message, so each result takes as many as it holds and the next
            // one starts where it stopped.
            let mut files = attached.iter().filter(|one| one.message == nth);
            message.array("content", |content| {
                for (index, result) in results.iter().enumerate() {
                    let found: Vec<_> = files
                        .by_ref()
                        .take(result.output.attachments().len())
                        .collect();
                    content.object(|block| {
                        write_result(block, result, &found);
                        if index + 1 == results.len()
                            && let Some(retention) = cache_retention
                        {
                            write_cache_control(block, retention);
                        }
                    });
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
            block.text("type", spelling(attached.modality));
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

/// The word above the source, which is the whole of the difference between the
/// two blocks this protocol carries bytes in: a `document` and an `image` are
/// the same `base64` source under two names.
///
/// Every modality is spelled out rather than caught by a wildcard, so a sixth
/// one added to the enum arrives here as a compiler error rather than as a
/// picture. What keeps the other three from reaching this at all is `spells()`,
/// which names the two this answers and is tested against it.
const fn spelling(modality: Modality) -> &'static str {
    match modality {
        Modality::Pdf => "document",
        Modality::Text | Modality::Image | Modality::Video | Modality::Audio => "image",
    }
}

/// One tool result, and whatever files the tool found for it.
///
/// The words lead and the files follow, which is the other way round from a
/// prompt: there the picture is what the vendor reads better first, and here
/// the words are what say which file is which.
fn write_result(block: &mut Object<'_>, result: &ToolResult, found: &[&Attached<'_>]) {
    block.text("type", "tool_result");
    block.text("tool_use_id", result.id.as_str());

    // A string where a tool answered in words alone, which is every call made
    // before a tool could find a file.
    if found.is_empty() {
        block.text("content", result.output.text());
    } else {
        block.array("content", |content| {
            // A tool that found a picture and said nothing about it is the
            // picture alone: an empty text block is one this vendor refuses
            // the request over rather than ignores.
            if !result.output.text().is_empty() {
                content.object(|part| {
                    part.text("type", "text");
                    part.text("text", result.output.text());
                });
            }
            for one in found {
                content.object(|part| write_attached(part, one));
            }
        });
    }

    if result.output.is_failed() {
        block.boolean("is_error", true);
    }
}

/// One tool, as advertised.
fn write_tool(
    tool: &mut Object<'_>,
    schema: &ToolSchema<'_>,
    cache_retention: Option<PromptCacheRetentionClass>,
) {
    let (input, description) = described(schema.schema);
    tool.text("name", schema.name);
    tool.text("description", &description);
    tool.value("input_schema", &Value::Object(input));
    if let Some(retention) = cache_retention {
        write_cache_control(tool, retention);
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::{
        Attached, Change, Changed, Content, Diff, Effort, Fragment, Line, Modality, ToolArgs,
        ToolCall, ToolId, ToolOutput, Transcript,
    };

    use super::*;
    use crate::fake::{cached, failed, found, observed, picture};

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
            prompt_cache: None,
        }
    }

    /// The four bytes a PNG starts with, which encode to `iVBORw==`.
    const PIXEL: &[u8] = &[0x89, b'P', b'N', b'G'];

    /// The line the runner writes in place of a file it did not send.
    const INSTEAD: &str = "holiday.png is not attached to this request, to keep the request \
         within its size limit: read it again if you need it.";

    /// The five bytes a PDF starts with, which encode to `JVBERi0=`.
    const PAGES: &[u8] = b"%PDF-";

    /// A prompt the runner resolved one picture for.
    fn holding(text: &str, content: Content<'static>) -> Request<'static> {
        carrying(text, "image/png", Modality::Image, content)
    }

    /// A prompt the runner resolved one attachment of a stated kind for.
    fn carrying(
        text: &str,
        media_type: &'static str,
        modality: Modality,
        content: Content<'static>,
    ) -> Request<'static> {
        // The transcript's own reference is deliberately absent: a provider
        // reads what the runner resolved and never a path.
        let mut request = request(said(text));
        request.attached = Box::leak(Box::new([Attached {
            message: 0,
            index: 0,
            media_type,
            modality,
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
    fn default_automatic_caching_adds_only_the_short_lived_native_control() {
        let plain = build(&request(said("hello")));
        let cached = build(&cached(
            request(said("hello")),
            PromptCacheMechanism::AutomaticPrefix,
            PromptCacheRetentionClass::ProviderDefault,
            false,
        ));

        assert_eq!(at(&cached, "/cache_control"), &json!({"type": "ephemeral"}));
        let mut without_control = cached;
        without_control
            .as_object_mut()
            .expect("body object")
            .remove("cache_control");
        assert_eq!(without_control, plain);
    }

    #[test]
    fn observe_only_preserves_the_request_bytes() {
        let plain = request(said("hello"));
        let observed = observed(request(said("hello")));

        assert_eq!(serialize(&observed), serialize(&plain));
    }

    #[test]
    fn extended_automatic_caching_uses_the_documented_one_hour_ttl() {
        let cached = cached(
            request(said("hello")),
            PromptCacheMechanism::AutomaticPrefix,
            PromptCacheRetentionClass::Extended,
            false,
        );

        assert_eq!(
            at(&build(&cached), "/cache_control"),
            &json!({"type": "ephemeral", "ttl": "1h"})
        );
    }

    #[test]
    fn explicit_caching_marks_the_latest_legal_system_tool_or_message_boundary() {
        let mut system = request(said("current"));
        system.system = Some("stable instructions");
        let system = cached(
            system,
            PromptCacheMechanism::ExplicitBreakpoints,
            PromptCacheRetentionClass::ProviderDefault,
            false,
        );
        assert_eq!(
            at(&build(&system), "/system/0/cache_control"),
            &json!({"type": "ephemeral"})
        );

        let mut tools = request(said("current"));
        tools.tools = Box::leak(Box::new([ToolSchema {
            name: "read",
            schema: r#"{"description":"Read","type":"object"}"#,
        }]));
        let tools = cached(
            tools,
            PromptCacheMechanism::ExplicitBreakpoints,
            PromptCacheRetentionClass::Extended,
            false,
        );
        assert_eq!(
            at(&build(&tools), "/tools/0/cache_control"),
            &json!({"type": "ephemeral", "ttl": "1h"})
        );

        let mut history = said("earlier");
        history.push(Message::Agent {
            text: "answer".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        });
        history.push(Message::said("current"));
        let history = cached(
            request(history),
            PromptCacheMechanism::ExplicitBreakpoints,
            PromptCacheRetentionClass::ProviderDefault,
            false,
        );
        assert_eq!(
            at(&build(&history), "/messages/1/content/0/cache_control"),
            &json!({"type": "ephemeral"})
        );
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
    fn typed_context_is_sent_as_retained_model_input() {
        let mut transcript = Transcript::new();
        transcript.push(Message::Context(Fragment::new(
            "workspace",
            "Workspace: /src",
        )));
        transcript.push(Message::said("continue"));

        let body = build(&request(transcript));

        assert_eq!(
            at(&body, "/messages/0"),
            &json!({"role": "user", "content": "Workspace: /src"})
        );
        assert_eq!(at(&body, "/messages/1/content"), &json!("continue"));
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

    /// The shape this vendor's documentation described on 2026-08-23: the same
    /// `source` a picture travels in, under `document` instead of `image`. The
    /// word above the source is the whole of the difference, and it is the one
    /// thing a request carrying a PDF gets wrong if this block is a picture's.
    #[test]
    fn a_pdf_is_a_document_block_before_the_prompt() {
        let body = build(&carrying(
            "what does this say",
            "application/pdf",
            Modality::Pdf,
            Content::Bytes(PAGES),
        ));

        assert_eq!(
            at(&body, "/messages/0/content"),
            &json!([
                {
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": "application/pdf",
                        "data": "JVBERi0="
                    }
                },
                { "type": "text", "text": "what does this say" }
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

    /// A turn whose tool results the runner resolved these attachments for.
    fn answering(results: Vec<ToolResult>, attached: Vec<Attached<'static>>) -> Request<'static> {
        let mut transcript = said("find me one");
        transcript.push(Message::ToolResults(results));
        let mut request = request(transcript);
        request.attached = Box::leak(attached.into_boxed_slice());
        request
    }

    /// One file the runner resolved, at its place across the message above.
    fn resolved(
        index: usize,
        media_type: &'static str,
        modality: Modality,
        content: Content<'static>,
    ) -> Attached<'static> {
        Attached {
            message: 1,
            index,
            media_type,
            modality,
            content,
        }
    }

    /// The picture a tool found, resolved.
    fn resolved_picture(index: usize) -> Attached<'static> {
        resolved(index, "image/png", Modality::Image, Content::Bytes(PIXEL))
    }

    /// One call, answered with the words and files a tool came back with.
    fn one(output: ToolOutput) -> Vec<ToolResult> {
        vec![ToolResult {
            id: ToolId::new("call_1"),
            output,
        }]
    }

    #[test]
    fn a_tool_that_found_a_picture_sends_it_inside_the_result() {
        let body = build(&answering(
            one(found("one match: holiday.png", vec![picture()])),
            vec![resolved_picture(0)],
        ));

        assert_eq!(
            at(&body, "/messages/1/content/0"),
            &json!({
                "type": "tool_result",
                "tool_use_id": "call_1",
                "content": [
                    { "type": "text", "text": "one match: holiday.png" },
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "iVBORw=="
                        }
                    }
                ]
            }),
            "{body}"
        );
    }

    #[test]
    fn a_tool_that_only_found_a_picture_sends_no_empty_text_block() {
        let body = build(&answering(
            one(found("", vec![picture()])),
            vec![resolved_picture(0)],
        ));

        assert_eq!(
            at(&body, "/messages/1/content/0/content"),
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

    #[test]
    fn a_failed_result_that_found_a_file_still_says_it_failed() {
        let body = build(&answering(
            one(failed("could not open it", vec![picture()])),
            vec![resolved_picture(0)],
        ));

        assert_eq!(
            at(&body, "/messages/1/content/0/is_error"),
            &json!(true),
            "{body}"
        );
    }

    #[test]
    fn each_result_gets_the_files_its_own_call_found() {
        let body = build(&answering(
            vec![
                ToolResult {
                    id: ToolId::new("call_1"),
                    output: found("first", vec![picture()]),
                },
                ToolResult {
                    id: ToolId::new("call_2"),
                    output: found("second", vec![picture()]),
                },
            ],
            vec![
                resolved_picture(0),
                resolved(1, "application/pdf", Modality::Pdf, Content::Bytes(PAGES)),
            ],
        ));

        assert_eq!(
            at(&body, "/messages/1/content/0/content/1/type"),
            &json!("image"),
            "the first call found the picture: {body}"
        );
        assert_eq!(
            at(&body, "/messages/1/content/1/content/1/type"),
            &json!("document"),
            "the second found the document: {body}"
        );
    }

    /// What the reader was shown is for the reader, and the request must not be
    /// able to tell.
    ///
    /// The bytes rather than the value: two bodies that parse alike could still
    /// have been written differently, and what is promised is the request itself
    /// rather than a reading of it. Green before an output carries anything
    /// reader-side and green afterwards — a guard written beside the change it
    /// exists to catch would be recording that change instead of catching it.
    ///
    /// Both shapes, because only one of them is the shape a request is ever built
    /// from. The lines are dropped where the call answers, before the result joins
    /// the transcript, so a result on its way to a provider carries the counts and
    /// never the diff — and a guard that only varied the diff would be pinning the
    /// shape this path cannot produce while leaving the shape it always produces
    /// free to move.
    #[test]
    fn the_request_body_is_the_same_whatever_the_reader_was_shown() {
        let text = "fn main() {}";
        let plain = serialize(&answering(one(ToolOutput::ok(text)), Vec::new()));
        let shown = serialize(&answering(
            one(ToolOutput::ok(text).showing(Diff::new([Line::new(1, Change::Added, text)]))),
            Vec::new(),
        ));

        let counted = serialize(&answering(
            one(ToolOutput::ok(text).counting(Changed::new(2, 1))),
            Vec::new(),
        ));

        assert_eq!(plain, shown, "the reader's copy reached the wire");
        assert_eq!(plain, counted, "the reader's counts reached the wire");
    }
}
