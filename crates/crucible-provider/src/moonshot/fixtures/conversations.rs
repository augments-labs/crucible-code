// The fabricated conversations and responses Kimi's fixtures were written
// over. Included, not a module: the differential tests beside it and the
// allocation count under `tests/` read the same cases, and the second sees only
// what this crate makes public, so everything here is named by its full path.

/// Every conversation a request fixture is written for, by the file it is
/// written to.
fn conversations() -> Vec<(&'static str, crucible_models::Request<'static>)> {
    use crucible_models::{Attached, Content, Effort, Request, RequestPurpose};
    use crucible_types::{
        Message, Modality, StopReason, ToolArgs, ToolCall, ToolId, ToolResult, ToolSchema,
        Transcript,
    };

    fn leak<T>(value: T) -> &'static T {
        Box::leak(Box::new(value))
    }

    fn transcript(messages: Vec<Message>) -> &'static Transcript {
        let mut transcript = Transcript::new();
        for message in messages {
            transcript
                .push(message)
                .expect("a fixture transcript is valid");
        }
        leak(transcript)
    }

    fn asking(model: &'static str, messages: Vec<Message>) -> Request<'static> {
        Request {
            purpose: RequestPurpose::Turn,
            model,
            transcript: transcript(messages),
            tools: &[],
            attached: &[],
            max_tokens: 4096,
            system: None,
            effort: None,
            prompt_cache: None,
        }
    }

    fn called(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: ToolId::new(id),
            name: name.into(),
            args: ToolArgs::new(args),
        }
    }

    fn calling(text: &str, calls: Vec<ToolCall>) -> Message {
        Message::Agent {
            continuation: None,
            text: text.into(),
            calls,
            stop: Some(StopReason::WantsTools),
        }
    }

    fn answered(id: &str, output: crucible_tools::ToolOutput) -> ToolResult {
        ToolResult {
            id: ToolId::new(id),
            output: output.into_recorded(),
        }
    }

    let tools: &'static [ToolSchema<'static>] = leak([
        ToolSchema {
            name: "read",
            schema: r#"{"description":"Reads a file","type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
        },
        ToolSchema {
            name: "clock",
            schema: r#"{"description":"Says the time","type":"object","properties":{}}"#,
        },
    ]);

    let mut all = Vec::new();

    let mut plain = asking("kimi-for-coding", vec![Message::said("Say hello.")]);
    plain.system = Some("You are a careful assistant.\nAnswer briefly.");
    all.push(("plain", plain));

    let mut tool = asking(
        "kimi-for-coding",
        vec![
            Message::said("What is in src/main.rs?"),
            calling(
                "let me look",
                vec![called("call_1", "read", r#"{"path":"src/main.rs"}"#)],
            ),
            Message::ToolResults(vec![answered(
                "call_1",
                crucible_tools::ToolOutput::ok("fn main() {\n    println!(\"hi\");\n}\n"),
            )]),
        ],
    );
    tool.tools = tools;
    all.push(("tool-call-and-result", tool));

    let mut two = asking(
        "k3",
        vec![
            Message::said("Read both and tell me the time."),
            calling(
                "",
                vec![
                    called("call_a", "read", r#"{"path":"a.rs"}"#),
                    called("call_b", "clock", "  "),
                ],
            ),
            Message::ToolResults(vec![
                answered(
                    "call_a",
                    crucible_tools::ToolOutput::failed("a.rs: no such file"),
                ),
                answered("call_b", crucible_tools::ToolOutput::ok("12:00")),
            ]),
            Message::Agent {
                continuation: None,
                text: "It is noon, and a.rs is not th".into(),
                calls: Vec::new(),
                stop: Some(StopReason::OutOfTokens),
            },
            Message::said("go on"),
        ],
    );
    two.tools = tools;
    all.push(("two-calls-a-failure-and-a-cut", two));

    let mut image = asking(
        "kimi-for-coding",
        vec![Message::said("What is in this picture?")],
    );
    image.attached = leak([Attached {
        message: 0,
        index: 0,
        media_type: "image/png",
        modality: Modality::Image,
        content: Content::Bytes(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
    }]);
    all.push(("image", image));

    let mut video = asking("k3", vec![Message::said("")]);
    video.attached = leak([
        Attached {
            message: 0,
            index: 0,
            media_type: "video/mp4",
            modality: Modality::Video,
            content: Content::Bytes(&[0, 0, 0, 0x18, b'f', b't', b'y', b'p']),
        },
        Attached {
            message: 0,
            index: 1,
            media_type: "image/png",
            modality: Modality::Image,
            content: Content::Instead(
                "holiday.png is not attached to this request, to keep the request within its \
                 size limit: read it again if you need it.",
            ),
        },
    ]);
    all.push(("video-and-a-file-not-sent", video));

    for (name, effort) in [
        ("effort-low", Effort::Low),
        ("effort-medium", Effort::Medium),
        ("effort-high", Effort::High),
        ("effort-xhigh", Effort::Xhigh),
        ("effort-max", Effort::Max),
    ] {
        let mut rung = asking("kimi-for-coding", vec![Message::said("Think, then answer.")]);
        rung.effort = Some(effort);
        all.push((name, rung));
    }

    let mut highspeed = asking(
        "kimi-for-coding-highspeed",
        vec![Message::said("Quickly: 2 + 2?")],
    );
    highspeed.effort = Some(Effort::Low);
    highspeed.max_tokens = 256;
    all.push(("highspeed", highspeed));

    let mut recap = asking(
        "kimi-for-coding",
        vec![
            Message::said("old question"),
            calling("old answer", vec![called("call_1", "read", "{}")]),
            Message::ToolResults(vec![answered(
                "call_1",
                crucible_tools::ToolOutput::ok("old result"),
            )]),
            Message::said("summarize"),
        ],
    );
    recap.purpose = RequestPurpose::Recap;
    all.push(("recap", recap));

    all
}

/// Every response a stream fixture is written for, by the file it is written
/// to: the status it arrives with and the bytes of its body.
fn streams() -> Vec<(&'static str, u16, &'static str)> {
    vec![
        (
            "text",
            200,
            concat!(
                r#"data: {"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
                "\n\n",
                r#"data: {"id":"chat-1","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#,
                "\n\n",
                r#"data: {"id":"chat-1","choices":[{"index":0,"delta":{"content":", world"},"finish_reason":null}]}"#,
                "\n\n",
                r#"data: {"id":"chat-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
                "\n\n",
                r#"data: {"id":"chat-1","choices":[],"usage":{"prompt_tokens":9,"completion_tokens":4,"total_tokens":13}}"#,
                "\n\n",
                "data: [DONE]\n\n",
            ),
        ),
        (
            "reasoning-then-text",
            200,
            concat!(
                r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"The user wants"},"finish_reason":null}]}"#,
                "\n\n",
                r#"data: {"choices":[{"index":0,"delta":{"reasoning_content":" a number."},"finish_reason":null}]}"#,
                "\n\n",
                r#"data: {"choices":[{"index":0,"delta":{"content":"4"},"finish_reason":null}]}"#,
                "\n\n",
                r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
                "\n\n",
                r#"data: {"choices":[],"usage":{"prompt_tokens":1200,"cached_tokens":1024,"completion_tokens":30,"total_tokens":1230,"completion_tokens_details":{"reasoning_tokens":22}}}"#,
                "\n\n",
                "data: [DONE]\n\n",
            ),
        ),
        (
            "tool-calls-split-across-events",
            200,
            concat!(
                r#"data: {"choices":[{"index":0,"delta":{"content":"Reading."},"finish_reason":null}]}"#,
                "\n\n",
                r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read","arguments":""}}]},"finish_reason":null}]}"#,
                "\n\n",
                r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]},"finish_reason":null}]}"#,
                "\n\n",
                r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a.rs\"}"}},{"index":1,"id":"call_2","type":"function","function":{"name":"clock","arguments":"{}"}}]},"finish_reason":null}]}"#,
                "\n\n",
                r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
                "\n\n",
                r#"data: {"choices":[],"usage":{"prompt_tokens":50,"completion_tokens":20}}"#,
                "\n\n",
                "data: [DONE]\n\n",
            ),
        ),
        (
            "filtered",
            200,
            concat!(
                r#"data: {"choices":[{"index":0,"delta":{"content":"I can"},"finish_reason":null}]}"#,
                "\n\n",
                r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"content_filter"}]}"#,
                "\n\n",
                "data: [DONE]\n\n",
            ),
        ),
        (
            "out-of-tokens-and-an-unknown-reason",
            200,
            concat!(
                r#"data: {"choices":[{"index":0,"delta":{"content":"Half"},"finish_reason":"length"}]}"#,
                "\n\n",
                r#"data: {"choices":[{"index":0,"delta":{"content":" more"},"finish_reason":"something_new"}]}"#,
                "\n\n",
                "data: [DONE]\n\n",
            ),
        ),
        (
            "a-failure-mid-stream",
            200,
            concat!(
                r#"data: {"choices":[{"index":0,"delta":{"content":"Hel"},"finish_reason":null}]}"#,
                "\n\n",
                r#"data: {"error":{"type":"rate_limit_reached_error","message":"too many requests"}}"#,
                "\n\n",
            ),
        ),
        (
            "cut-short",
            200,
            concat!(
                r#"data: {"choices":[{"index":0,"delta":{"content":"Hel"},"finish_reason":null}]}"#,
                "\n\n",
            ),
        ),
        (
            "a-stray-fragment",
            200,
            concat!(
                r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":3,"function":{"arguments":"{}"}}]},"finish_reason":null}]}"#,
                "\n\n",
            ),
        ),
        (
            "a-call-announced-without-an-id",
            200,
            concat!(
                r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"read","arguments":""}}]},"finish_reason":null}]}"#,
                "\n\n",
            ),
        ),
        (
            "heartbeats-and-a-payload-that-is-not-json",
            200,
            concat!(
                ": keep-alive\n\n",
                "data: \n\n",
                r#"data: {"choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":null}]}"#,
                "\n\n",
                "data: {not json\n\n",
            ),
        ),
        (
            "refused",
            429,
            r#"{"error":{"type":"rate_limit_reached_error","message":"Your account is being rate limited, please retry later."}}"#,
        ),
    ]
}
