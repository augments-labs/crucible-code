//! Google Interactions requests, serialized into their one final allocation.
//!
//! History is local: no previous-interaction handle and no server-side storage.
//! This module alone translates compatible continuation into native input steps.

mod input;
mod results;

#[cfg(test)]
mod media_tests;
#[cfg(test)]
mod result_tests;

use crate::json::{Json, described};
use crucible_core::{ContinuationScope, ProviderError, Request};

pub(super) fn serialize(
    request: &Request<'_>,
    scope: ContinuationScope,
) -> Result<String, ProviderError> {
    if matches!(
        request.effort,
        Some(crucible_core::Effort::Xhigh | crucible_core::Effort::Max)
    ) {
        return Err(super::protocol(
            "Google thinking level must be low, medium or high",
        ));
    }
    let mut json = Json::new();
    let mut outcome = Ok(());
    json.object(|body| {
        body.text("model", request.model);
        body.boolean("stream", true);
        body.boolean("store", false);
        if let Some(system) = request.system {
            body.text("system_instruction", system);
        }
        body.object("generation_config", |config| {
            config.number("max_output_tokens", request.max_tokens);
            if let Some(effort) = request.effort {
                config.text("thinking_level", effort.as_str());
            }
        });
        body.array("input", |input| {
            outcome = input::write(input, request, scope);
        });
        if !request.tools.is_empty() {
            body.array("tools", |tools| {
                for schema in request.tools {
                    let (parameters, description) = described(schema.schema);
                    tools.object(|tool| {
                        tool.text("type", "function");
                        tool.text("name", schema.name);
                        tool.text("description", &description);
                        tool.value("parameters", &serde_json::Value::Object(parameters));
                    });
                }
            });
        }
    });
    outcome.map(|()| json.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crucible_core::{Effort, Message, RequestPurpose, ToolSchema, Transcript};
    use serde_json::{Value, json};

    #[test]
    fn unsupported_google_efforts_are_rejected_before_serialization() {
        let transcript = Transcript::new();
        for effort in [Effort::Xhigh, Effort::Max] {
            let request = Request {
                purpose: RequestPurpose::Turn,
                model: "gemini-3.1-pro-preview",
                transcript: &transcript,
                tools: &[],
                max_tokens: 4096,
                system: None,
                effort: Some(effort),
                attached: &[],
                prompt_cache: None,
            };
            assert!(serialize(&request, scope()).is_err());
        }
    }

    #[test]
    fn a_google_request_is_stateless_typed_and_uses_the_exact_generation_fields() {
        let mut transcript = Transcript::new();
        transcript.push(Message::said("hello")).unwrap();
        let request = Request {
            purpose: RequestPurpose::Turn,
            model: "gemini-3.8-flash",
            transcript: &transcript,
            tools: &[ToolSchema {
                name: "lookup",
                schema: r#"{"description":"Look up a file","type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
            }],
            max_tokens: 8192,
            system: Some("Be careful"),
            effort: Some(Effort::Medium),
            attached: &[],
            prompt_cache: None,
        };
        let actual: Value = serde_json::from_str(&serialize(&request, scope()).unwrap()).unwrap();
        assert_eq!(
            actual,
            json!({
                "model": "gemini-3.8-flash", "stream": true, "store": false,
                "system_instruction": "Be careful",
                "generation_config": {"max_output_tokens":8192, "thinking_level":"medium"},
                "input": [{"type":"user_input","content":[{"type":"text","text":"hello"}]}],
                "tools": [{"type":"function","name":"lookup","description":"Look up a file","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}]
            })
        );
    }

    fn scope() -> ContinuationScope {
        ContinuationScope::from_digest([1; 32])
    }

    fn text_attachment(bytes: &[u8]) -> Result<String, ProviderError> {
        use crucible_core::{Attached, Content, Modality};
        let mut transcript = Transcript::new();
        transcript.push(Message::said("read this")).unwrap();
        serialize(
            &Request {
                purpose: RequestPurpose::Turn,
                model: "gemini-3.8-flash",
                transcript: &transcript,
                tools: &[],
                max_tokens: 4096,
                system: None,
                effort: None,
                attached: &[Attached {
                    message: 0,
                    index: 0,
                    media_type: "text/plain",
                    modality: Modality::Text,
                    content: Content::Bytes(bytes),
                }],
                prompt_cache: None,
            },
            scope(),
        )
    }

    #[test]
    fn text_attachment_bytes_are_utf8_text_not_base64_media() {
        let body = text_attachment("héllo\n\"quoted\"".as_bytes()).unwrap();
        let value: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            value.pointer("/input/0/content/1"),
            Some(&json!({
                "type":"text", "text":"héllo\n\"quoted\""
            }))
        );
    }

    #[test]
    fn text_attachment_invalid_utf8_is_refused_without_echoing_bytes() {
        let result = text_attachment(b"private-file-canary\xff");
        assert!(result.is_err());
        assert!(!format!("{result:?}").contains("private-file-canary"));
    }

    #[test]
    fn google_attachments_use_interactions_media_parts_and_recap_omits_bytes() {
        use crucible_core::{Attached, Content, Modality};
        let mut transcript = Transcript::new();
        transcript.push(Message::said("inspect these")).unwrap();
        let attached = [
            Attached {
                message: 0,
                index: 0,
                media_type: "image/png",
                modality: Modality::Image,
                content: Content::Bytes(b"image"),
            },
            Attached {
                message: 0,
                index: 1,
                media_type: "application/pdf",
                modality: Modality::Pdf,
                content: Content::Bytes(b"pdf"),
            },
            Attached {
                message: 0,
                index: 2,
                media_type: "audio/wav",
                modality: Modality::Audio,
                content: Content::Bytes(b"audio"),
            },
            Attached {
                message: 0,
                index: 3,
                media_type: "video/mp4",
                modality: Modality::Video,
                content: Content::Bytes(b"video"),
            },
            Attached {
                message: 0,
                index: 4,
                media_type: "image/png",
                modality: Modality::Image,
                content: Content::Instead("image omitted"),
            },
        ];
        let request = Request {
            purpose: RequestPurpose::Turn,
            model: "gemini-3.8-flash",
            transcript: &transcript,
            tools: &[],
            max_tokens: 4096,
            system: None,
            effort: None,
            attached: &attached,
            prompt_cache: None,
        };
        let value: Value = serde_json::from_str(&serialize(&request, scope()).unwrap()).unwrap();
        assert_eq!(
            value.pointer("/input/0/content").unwrap(),
            &json!([
                {"type":"text","text":"inspect these"},
                {"type":"image","mime_type":"image/png","data":"aW1hZ2U="},
                {"type":"document","mime_type":"application/pdf","data":"cGRm"},
                {"type":"audio","mime_type":"audio/wav","data":"YXVkaW8="},
                {"type":"video","mime_type":"video/mp4","data":"dmlkZW8="},
                {"type":"text","text":"image omitted"},
            ])
        );
        let body = serialize(
            &Request {
                purpose: RequestPurpose::Recap,
                ..request
            },
            scope(),
        )
        .unwrap();
        assert!(!body.contains("aW1hZ2U="));
    }

    #[test]
    fn signed_history_is_replayed_exactly_across_google_model_switch_and_compaction() {
        use crucible_core::{
            Continuation, ContinuationData, ContinuationPart, StopReason, ToolArgs, ToolCall,
            ToolId, ToolOutput, ToolResult,
        };
        let mut state =
            Continuation::new(super::super::PROTOCOL, "gemini-3.7-flash", scope()).unwrap();
        for part in [
            ContinuationPart::Opaque(
                ContinuationData::new(
                    r#"{"type":"thought","signature":"signed-thought","summary":[]}"#,
                )
                .unwrap(),
            ),
            ContinuationPart::Opaque(
                ContinuationData::new(r#"{"output":{"type":"model_output"}}"#).unwrap(),
            ),
            ContinuationPart::Text {
                start: 0,
                end: 2,
                data: ContinuationData::new(r#"{"type":"text","annotations":[]}"#).unwrap(),
            },
            ContinuationPart::Text {
                start: 2,
                end: 3,
                data: ContinuationData::new(r#"{"type":"text"}"#).unwrap(),
            },
            ContinuationPart::Call {
                index: 0,
                data: ContinuationData::new(
                    r#"{"type":"function_call","signature":"signed-call"}"#,
                )
                .unwrap(),
            },
        ] {
            state.push(part).unwrap();
        }
        let mut transcript = Transcript::new();
        transcript.push(Message::said("old")).unwrap();
        transcript.push(Message::said("keep")).unwrap();
        transcript
            .push(Message::Agent {
                text: "é!".into(),
                calls: vec![ToolCall {
                    id: ToolId::new("call-1"),
                    name: "read".into(),
                    args: ToolArgs::new(r#"{"path":"a"}"#),
                }],
                stop: Some(StopReason::WantsTools),
                continuation: Some(state.finish("é!", 1, Some(StopReason::WantsTools)).unwrap()),
            })
            .unwrap();
        transcript
            .push(Message::ToolResults(vec![ToolResult {
                id: ToolId::new("call-1"),
                output: ToolOutput::ok("contents"),
            }]))
            .unwrap();
        transcript.compacted(1, "recap");
        let request = Request {
            purpose: RequestPurpose::Turn,
            model: "gemini-3.1-pro-preview",
            transcript: &transcript,
            tools: &[],
            max_tokens: 4096,
            system: None,
            effort: None,
            attached: &[],
            prompt_cache: None,
        };
        let actual: Value = serde_json::from_str(&serialize(&request, scope()).unwrap()).unwrap();
        let steps = actual.get("input").unwrap().as_array().unwrap();
        assert_eq!(
            steps.get(2..).unwrap(),
            &[
                json!({"type":"thought","signature":"signed-thought","summary":[]}),
                json!({"type":"model_output","content":[{"type":"text","text":"é","annotations":[]},{"type":"text","text":"!"}]}),
                json!({"type":"function_call","id":"call-1","name":"read","arguments":{"path":"a"},"signature":"signed-call"}),
                json!({"type":"function_result","call_id":"call-1","result":"contents","is_error":false}),
            ]
        );
        for (purpose, recipient) in [
            (RequestPurpose::Recap, scope()),
            (
                RequestPurpose::Turn,
                ContinuationScope::from_digest([2; 32]),
            ),
        ] {
            let request = Request { purpose, ..request };
            let body = serialize(&request, recipient).unwrap();
            assert!(!body.contains("signed-thought"));
            assert!(!body.contains("signed-call"));
            let value: Value = serde_json::from_str(&body).unwrap();
            assert!(
                value
                    .get("input")
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|step| step.get("type").and_then(Value::as_str) == Some("user_input"))
            );
        }
    }
}
