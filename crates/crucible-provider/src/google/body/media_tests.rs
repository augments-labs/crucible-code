//! Tool-result media stays with its call, including after stateless replay.

use super::serialize;
use crate::fake::{failed, found, picture};
use crucible_core::{
    Attached, Content, Continuation, ContinuationData, ContinuationPart, ContinuationScope,
    Message, Modality, Request, RequestPurpose, StopReason, ToolArgs, ToolCall, ToolId, ToolResult,
    Transcript,
};
use serde_json::{Value, json};

fn scope() -> ContinuationScope {
    ContinuationScope::from_digest([1; 32])
}

fn history() -> Transcript {
    let mut state = Continuation::new(super::super::PROTOCOL, "gemini-3.8-flash", scope()).unwrap();
    let mut calls = Vec::new();
    for (index, id) in ["first", "second"].into_iter().enumerate() {
        calls.push(ToolCall {
            id: ToolId::new(id),
            name: "read".into(),
            args: ToolArgs::new("{}"),
        });
        state
            .push(ContinuationPart::Call {
                index,
                data: ContinuationData::new(r#"{"type":"function_call"}"#).unwrap(),
            })
            .unwrap();
    }
    let mut transcript = Transcript::new();
    transcript
        .push(Message::Agent {
            text: "".into(),
            calls,
            stop: Some(StopReason::WantsTools),
            continuation: Some(state.finish("", 2, Some(StopReason::WantsTools)).unwrap()),
        })
        .unwrap();
    let file = |modality| crucible_core::Attachment {
        modality,
        ..picture()
    };
    transcript
        .push(Message::ToolResults(vec![
            ToolResult {
                id: ToolId::new("first"),
                output: found(
                    "first output",
                    vec![
                        file(Modality::Image),
                        file(Modality::Pdf),
                        file(Modality::Text),
                    ],
                ),
            },
            ToolResult {
                id: ToolId::new("second"),
                output: failed(
                    "second output",
                    vec![
                        file(Modality::Audio),
                        file(Modality::Video),
                        file(Modality::Image),
                    ],
                ),
            },
        ]))
        .unwrap();
    transcript
}

fn body(purpose: RequestPurpose, attached: &[Attached<'_>]) -> Value {
    let transcript = history();
    serde_json::from_str(
        &serialize(
            &Request {
                purpose,
                model: "gemini-3.8-flash",
                transcript: &transcript,
                tools: &[],
                max_tokens: 4096,
                system: None,
                effort: None,
                attached,
                prompt_cache: None,
            },
            scope(),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn result_media_preserves_all_modalities_and_call_associations() {
    let attached = [
        Attached {
            message: 1,
            index: 0,
            modality: Modality::Image,
            media_type: "image/png",
            content: Content::Bytes(b"image"),
        },
        Attached {
            message: 1,
            index: 1,
            modality: Modality::Pdf,
            media_type: "application/pdf",
            content: Content::Bytes(b"pdf"),
        },
        Attached {
            message: 1,
            index: 2,
            modality: Modality::Text,
            media_type: "text/plain",
            content: Content::Bytes("héllo".as_bytes()),
        },
        Attached {
            message: 1,
            index: 3,
            modality: Modality::Audio,
            media_type: "audio/wav",
            content: Content::Bytes(b"audio"),
        },
        Attached {
            message: 1,
            index: 4,
            modality: Modality::Video,
            media_type: "video/mp4",
            content: Content::Bytes(b"video"),
        },
        Attached {
            message: 1,
            index: 5,
            modality: Modality::Image,
            media_type: "image/png",
            content: Content::Instead("second image omitted"),
        },
    ];
    let value = body(RequestPurpose::Turn, &attached);
    assert_eq!(
        value
            .get("input")
            .unwrap()
            .as_array()
            .unwrap()
            .get(2..)
            .unwrap(),
        &[
            json!({"type":"function_result","call_id":"first","is_error":false,"result":[
                {"type":"text","text":"first output"},
                {"type":"image","mime_type":"image/png","data":"aW1hZ2U="},
                {"type":"text","text":"héllo"},
            ]}),
            json!({"type":"function_result","call_id":"second","is_error":true,"result":[
                {"type":"text","text":"second output"},
                {"type":"text","text":"second image omitted"},
            ]}),
            json!({"type":"user_input","content":[
                {"type":"text","text":"Attachments from tool call first:"},
                {"type":"document","mime_type":"application/pdf","data":"cGRm"},
            ]}),
            json!({"type":"user_input","content":[
                {"type":"text","text":"Attachments from tool call second:"},
                {"type":"audio","mime_type":"audio/wav","data":"YXVkaW8="},
                {"type":"video","mime_type":"video/mp4","data":"dmlkZW8="},
            ]}),
        ]
    );
    let recap = body(RequestPurpose::Recap, &attached);
    let recap = recap.to_string();
    for private in [
        "aW1hZ2U=",
        "cGRm",
        "YXVkaW8=",
        "dmlkZW8=",
        "function_result",
    ] {
        assert!(!recap.contains(private));
    }
}

#[test]
fn result_media_uses_message_wide_indexes_without_rebinding_missing_files() {
    let attached = [Attached {
        message: 1,
        index: 5,
        modality: Modality::Image,
        media_type: "image/png",
        content: Content::Instead("belongs to second"),
    }];
    let value = body(RequestPurpose::Turn, &attached);
    assert_eq!(
        value.pointer("/input/2/result"),
        Some(&json!("first output"))
    );
    assert_eq!(
        value.pointer("/input/3/result"),
        Some(&json!([
            {"type":"text","text":"second output"}, {"type":"text","text":"belongs to second"},
        ]))
    );
}
