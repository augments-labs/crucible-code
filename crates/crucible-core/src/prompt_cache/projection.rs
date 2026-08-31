//! Canonical cache-relevant projection of the exact provider request.

use std::fmt;

use crate::{Attached, Content, Message, Modality, Request, StopReason, ToolResult};
use serde_json::{Map, Value};

use super::{PromptCacheBoundary, PromptCacheContent};

/// Maximum neutral boundaries retained for one request plan.
pub const MAX_PROMPT_CACHE_BOUNDARIES: usize = 64;

/// Provider-visible content kinds present in a stable prefix.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PromptCacheContentSet(u8);

impl PromptCacheContentSet {
    /// No stable provider-visible content.
    pub const NONE: Self = Self(0);

    /// Adds one content kind.
    #[must_use]
    pub const fn with(self, content: PromptCacheContent) -> Self {
        Self(self.0 | content_bit(content))
    }

    /// Whether this set contains a content kind.
    #[must_use]
    pub const fn contains(self, content: PromptCacheContent) -> bool {
        self.0 & content_bit(content) != 0
    }

    /// Whether every kind in this set occurs in `allowed`.
    #[must_use]
    pub fn is_subset_of(self, allowed: &[PromptCacheContent]) -> bool {
        ALL_CONTENT
            .into_iter()
            .all(|kind| !self.contains(kind) || allowed.contains(&kind))
    }

    /// Whether the prefix contains nothing provider-visible.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

const ALL_CONTENT: [PromptCacheContent; 6] = [
    PromptCacheContent::Text,
    PromptCacheContent::Tools,
    PromptCacheContent::Images,
    PromptCacheContent::Documents,
    PromptCacheContent::Audio,
    PromptCacheContent::Video,
];

const fn content_bit(content: PromptCacheContent) -> u8 {
    match content {
        PromptCacheContent::Text => 1,
        PromptCacheContent::Tools => 2,
        PromptCacheContent::Images => 4,
        PromptCacheContent::Documents => 8,
        PromptCacheContent::Audio => 16,
        PromptCacheContent::Video => 32,
    }
}

/// One legal neutral cache boundary in semantic segment order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCacheBoundaryPoint {
    kind: PromptCacheBoundary,
    segment: u32,
    message: Option<u32>,
}

impl PromptCacheBoundaryPoint {
    #[cfg(test)]
    pub(super) const fn fixture(kind: PromptCacheBoundary, segment: u32) -> Self {
        Self {
            kind,
            segment,
            message: None,
        }
    }

    /// Boundary kind.
    #[must_use]
    pub const fn kind(&self) -> PromptCacheBoundary {
        self.kind
    }

    /// Stable logical segment ordinal after which this boundary sits.
    #[must_use]
    pub const fn segment(&self) -> u32 {
        self.segment
    }

    /// Transcript message ordinal for message boundaries.
    #[must_use]
    pub const fn message(&self) -> Option<u32> {
        self.message
    }
}

/// A request was too large to describe with the bounded projection metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PromptCacheProjectionError {
    /// A message/segment ordinal did not fit the stable wire type.
    #[error("prompt-cache projection exceeded its message/segment ordinal limit")]
    TooManySegments,
    /// Canonical JSON serialization failed unexpectedly.
    #[error("prompt-cache projection could not serialize canonical tool JSON")]
    CanonicalJson,
}

/// Metadata for the canonical provider-facing stable prefix.
#[derive(Clone, PartialEq, Eq)]
pub struct PromptCacheProjection {
    stable_messages: usize,
    stable_bytes: u64,
    estimated_tokens: u64,
    boundaries: Box<[PromptCacheBoundaryPoint]>,
    content: PromptCacheContentSet,
}

impl PromptCacheProjection {
    /// Inspects the exact borrowed request without retaining any prompt bytes.
    ///
    /// The stable transcript prefix ends immediately before the most recent
    /// user message. That preserves every earlier provider-visible item in
    /// order and leaves the current prompt, its context deltas, tool exchange,
    /// and steering in the volatile suffix. A later pass may extend the prefix;
    /// it never reorders a message to do so.
    ///
    /// # Errors
    ///
    /// Returns an error if request ordinals exceed their stable representation
    /// or canonical tool-schema serialization fails.
    pub fn inspect(request: &Request<'_>) -> Result<Self, PromptCacheProjectionError> {
        let stable_messages = request
            .transcript
            .messages()
            .iter()
            .rposition(|message| matches!(message, Message::User { .. }))
            .unwrap_or(0);
        let mut state = Walk::new(false);
        walk(request, stable_messages, &mut state, &mut |_| {})?;
        Ok(Self {
            stable_messages,
            stable_bytes: state.bytes,
            // This estimate is only an eligibility prediction. Provider usage
            // remains authoritative for context accounting and hit reporting.
            estimated_tokens: state.bytes.saturating_add(3) / 4,
            boundaries: state.boundaries.into_boxed_slice(),
            content: state.content,
        })
    }

    /// Streams the canonical stable-prefix bytes into a caller-owned digest.
    ///
    /// The callback is synchronous and each slice is borrowed only for the
    /// call. No prompt-sized buffer is allocated or retained.
    ///
    /// # Errors
    ///
    /// Returns an error if request ordinals exceed their stable representation
    /// or canonical tool-schema serialization fails.
    pub fn write_stable(
        &self,
        request: &Request<'_>,
        mut write: impl FnMut(&[u8]),
    ) -> Result<(), PromptCacheProjectionError> {
        let mut state = Walk::new(true);
        walk(request, self.stable_messages, &mut state, &mut write)?;
        debug_assert_eq!(state.bytes, self.stable_bytes);
        Ok(())
    }

    /// Number of complete transcript messages in the stable prefix.
    #[must_use]
    pub const fn stable_messages(&self) -> usize {
        self.stable_messages
    }

    /// Exact canonical projection byte count.
    #[must_use]
    pub const fn stable_bytes(&self) -> u64 {
        self.stable_bytes
    }

    /// Conservative provider-neutral token estimate used only for eligibility.
    #[must_use]
    pub const fn estimated_tokens(&self) -> u64 {
        self.estimated_tokens
    }

    /// Legal boundaries in semantic order.
    #[must_use]
    pub fn boundaries(&self) -> &[PromptCacheBoundaryPoint] {
        &self.boundaries
    }

    /// Provider-visible content kinds in the stable prefix.
    #[must_use]
    pub const fn content(&self) -> PromptCacheContentSet {
        self.content
    }
}

impl fmt::Debug for PromptCacheProjection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PromptCacheProjection")
            .field("stable_messages", &self.stable_messages)
            .field("stable_bytes", &self.stable_bytes)
            .field("estimated_tokens", &self.estimated_tokens)
            .field("boundaries", &self.boundaries)
            .field("content", &self.content)
            .finish()
    }
}

struct Walk {
    emit: bool,
    bytes: u64,
    segment: u32,
    boundaries: Vec<PromptCacheBoundaryPoint>,
    content: PromptCacheContentSet,
}

impl Walk {
    const fn new(emit: bool) -> Self {
        Self {
            emit,
            bytes: 0,
            segment: 0,
            boundaries: Vec::new(),
            content: PromptCacheContentSet::NONE,
        }
    }

    fn frame(&mut self, tag: u8, value: &[u8], write: &mut impl FnMut(&[u8])) {
        let length = u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes();
        self.bytes = self
            .bytes
            .saturating_add(1 + u64::try_from(length.len()).unwrap_or(8))
            .saturating_add(u64::try_from(value.len()).unwrap_or(u64::MAX));
        if self.emit {
            write(&[tag]);
            write(&length);
            write(value);
        }
    }

    fn marker(&mut self, tag: u8, write: &mut impl FnMut(&[u8])) {
        self.frame(tag, &[], write);
    }

    fn next_segment(&mut self) -> Result<u32, PromptCacheProjectionError> {
        self.segment = self
            .segment
            .checked_add(1)
            .ok_or(PromptCacheProjectionError::TooManySegments)?;
        Ok(self.segment)
    }

    fn boundary(
        &mut self,
        kind: PromptCacheBoundary,
        message: Option<usize>,
    ) -> Result<(), PromptCacheProjectionError> {
        if self.boundaries.len() == MAX_PROMPT_CACHE_BOUNDARIES {
            // Explicit lowering needs the latest legal stable boundary, not an
            // unbounded catalogue of every earlier one. Drop only the oldest
            // marker metadata; `walk` still frames every stable byte into the
            // fingerprint and byte count.
            self.boundaries.remove(0);
        }
        self.boundaries.push(PromptCacheBoundaryPoint {
            kind,
            segment: self.segment,
            message: message
                .map(u32::try_from)
                .transpose()
                .map_err(|_| PromptCacheProjectionError::TooManySegments)?,
        });
        Ok(())
    }
}

fn walk(
    request: &Request<'_>,
    stable_messages: usize,
    state: &mut Walk,
    write: &mut impl FnMut(&[u8]),
) -> Result<(), PromptCacheProjectionError> {
    state.frame(0, b"crucible.prompt-cache.projection.v2", write);

    if let Some(system) = request.system {
        state.next_segment()?;
        state.content = state.content.with(PromptCacheContent::Text);
        state.frame(1, system.as_bytes(), write);
        state.boundary(PromptCacheBoundary::AfterSystem, None)?;
    }

    if !request.tools.is_empty() {
        state.next_segment()?;
        state.content = state.content.with(PromptCacheContent::Tools);
        state.marker(2, write);
        for tool in request.tools {
            state.frame(3, tool.name.as_bytes(), write);
            let (parameters, description) = described_tool_schema(tool.schema);
            let parameters = serde_json::to_vec(&Value::Object(parameters))
                .map_err(|_| PromptCacheProjectionError::CanonicalJson)?;
            state.frame(4, description.as_bytes(), write);
            state.frame(5, &parameters, write);
        }
        state.boundary(PromptCacheBoundary::AfterTools, None)?;
    }

    for (index, message) in request
        .transcript
        .messages()
        .iter()
        .take(stable_messages)
        .enumerate()
    {
        // Every shipped adapter drops an assistant turn that contains neither
        // text nor calls. It remains session truth, but it is not part of the
        // provider-facing projection and cannot own a cache boundary.
        if matches!(message, Message::Agent { text, calls, .. } if text.is_empty() && calls.is_empty())
        {
            continue;
        }
        state.next_segment()?;
        state.content = state.content.with(PromptCacheContent::Text);
        write_message(state, message, index, request.attached, write);
        state.boundary(PromptCacheBoundary::AfterMessage, Some(index))?;
    }

    Ok(())
}

fn described_tool_schema(schema: &str) -> (Map<String, Value>, String) {
    let mut arguments = match serde_json::from_str(schema) {
        Ok(Value::Object(fields)) => fields,
        _ => Map::new(),
    };
    let description = match arguments.remove("description") {
        Some(Value::String(text)) => text,
        _ => String::new(),
    };
    (arguments, description)
}

fn write_message(
    state: &mut Walk,
    message: &Message,
    index: usize,
    attached: &[Attached<'_>],
    write: &mut impl FnMut(&[u8]),
) {
    match message {
        Message::Context(fragment) => state.frame(10, fragment.text().as_bytes(), write),
        Message::User { text, .. } => {
            state.frame(11, text.as_bytes(), write);
            write_attachments(state, index, attached, write);
        }
        Message::Agent { text, calls, stop } => {
            state.frame(12, text.as_bytes(), write);
            for call in calls {
                state.frame(13, call.id.as_str().as_bytes(), write);
                state.frame(14, call.name.as_bytes(), write);
                state.frame(15, call.args.as_str().as_bytes(), write);
            }
            if let Some(cut) = StopReason::cut(*stop) {
                state.frame(16, cut.as_bytes(), write);
            }
        }
        Message::ToolResults(results) => {
            state.marker(17, write);
            for result in results {
                write_result(state, result, write);
            }
            write_attachments(state, index, attached, write);
        }
    }
}

fn write_result(state: &mut Walk, result: &ToolResult, write: &mut impl FnMut(&[u8])) {
    state.frame(18, result.id.as_str().as_bytes(), write);
    state.frame(19, result.output.text().as_bytes(), write);
    state.frame(20, &[u8::from(result.output.is_failed())], write);
}

fn write_attachments(
    state: &mut Walk,
    message: usize,
    attached: &[Attached<'_>],
    write: &mut impl FnMut(&[u8]),
) {
    for attachment in attached.iter().filter(|one| one.message == message) {
        state.frame(
            21,
            &u64::try_from(attachment.index)
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
            write,
        );
        state.frame(22, attachment.media_type.as_bytes(), write);
        state.frame(23, &[modality(attachment.modality)], write);
        match attachment.content {
            Content::Bytes(bytes) => state.frame(24, bytes, write),
            Content::Instead(text) => state.frame(25, text.as_bytes(), write),
        }
        state.content = state.content.with(match attachment.modality {
            Modality::Text => PromptCacheContent::Text,
            Modality::Image => PromptCacheContent::Images,
            Modality::Pdf => PromptCacheContent::Documents,
            Modality::Audio => PromptCacheContent::Audio,
            Modality::Video => PromptCacheContent::Video,
        });
    }
}

const fn modality(modality: Modality) -> u8 {
    match modality {
        Modality::Text => 0,
        Modality::Image => 1,
        Modality::Pdf => 2,
        Modality::Audio => 3,
        Modality::Video => 4,
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        Attached, Attachment, Content, Message, Modality, Request, StopReason, ToolSchema,
        Transcript,
    };

    use super::*;

    const TOOLS: &[ToolSchema<'_>] = &[
        ToolSchema {
            name: "read",
            schema: r#"{"description":"Read","type":"object"}"#,
        },
        ToolSchema {
            name: "edit",
            schema: r#"{"description":"Edit","type":"object"}"#,
        },
    ];

    fn transcript(current: &str, path: &str) -> Transcript {
        let mut transcript = Transcript::new();
        transcript.push(Message::User {
            text: "inspect the image".into(),
            attachments: Box::new([Attachment {
                path: path.into(),
                modality: Modality::Image,
                media_type: "image/png".into(),
                hash: [u8::try_from(path.len()).unwrap_or(u8::MAX); 32],
            }]),
        });
        transcript.push(Message::Agent {
            text: "seen".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        });
        transcript.push(Message::said(current));
        transcript
    }

    fn projected(current: &str, path: &str) -> Vec<u8> {
        let transcript = transcript(current, path);
        let attached = [Attached {
            message: 0,
            index: 0,
            media_type: "image/png",
            modality: Modality::Image,
            content: Content::Bytes(b"provider-visible-image"),
        }];
        let request = Request {
            model: "fixture-model",
            transcript: &transcript,
            tools: TOOLS,
            max_tokens: 1_024,
            system: Some("stable instructions"),
            effort: None,
            attached: &attached,
            prompt_cache: None,
        };
        let projection = PromptCacheProjection::inspect(&request).unwrap();
        let mut bytes = Vec::new();
        projection
            .write_stable(&request, |part| bytes.extend_from_slice(part))
            .unwrap();
        bytes
    }

    fn request<'a>(
        transcript: &'a Transcript,
        system: &'a str,
        tools: &'a [ToolSchema<'a>],
        attached: &'a [Attached<'a>],
    ) -> Request<'a> {
        Request {
            model: "fixture-model",
            transcript,
            tools,
            max_tokens: 1_024,
            system: Some(system),
            effort: None,
            attached,
            prompt_cache: None,
        }
    }

    #[test]
    fn volatile_current_turn_text_does_not_perturb_the_earlier_prefix() {
        assert_eq!(
            projected("first volatile question", "/one/private/path.png"),
            projected("different volatile question", "/one/private/path.png")
        );
    }

    #[test]
    fn raw_attachment_path_and_hash_never_enter_the_projection() {
        assert_eq!(
            projected("current", "/one/private/path.png"),
            projected("current", "/another/person/work.png")
        );
    }

    #[test]
    fn provider_visible_system_tools_history_and_bytes_do_perturb_the_prefix() {
        let first = transcript("current", "/raw/path.png");
        let second = transcript("current", "/raw/path.png");
        let attached = [Attached {
            message: 0,
            index: 0,
            media_type: "image/png",
            modality: Modality::Image,
            content: Content::Bytes(b"provider-visible-image"),
        }];
        let bytes = |request: Request<'_>| {
            let projection = PromptCacheProjection::inspect(&request).unwrap();
            let mut bytes = Vec::new();
            projection
                .write_stable(&request, |part| bytes.extend_from_slice(part))
                .unwrap();
            bytes
        };

        let baseline = bytes(request(&first, "stable instructions", TOOLS, &attached));
        assert_ne!(
            baseline,
            bytes(request(&second, "changed instructions", TOOLS, &attached))
        );
        assert_ne!(
            baseline,
            bytes(request(
                &second,
                "stable instructions",
                &[ToolSchema {
                    name: "read",
                    schema: r#"{"description":"Read","type":"object"}"#,
                }],
                &attached
            ))
        );
    }

    #[test]
    fn semantically_identical_tool_json_has_one_provider_projection() {
        let transcript = transcript("current", "/raw/path.png");
        let first = [ToolSchema {
            name: "read",
            schema: r#"{"description":"Read","type":"object","properties":{"path":{"type":"string"}}}"#,
        }];
        let reordered = [ToolSchema {
            name: "read",
            schema: r#"{
                "properties": { "path": { "type": "string" } },
                "type": "object",
                "description": "Read"
            }"#,
        }];
        let bytes = |tools: &[ToolSchema<'_>]| {
            let request = request(&transcript, "stable instructions", tools, &[]);
            let projection = PromptCacheProjection::inspect(&request).unwrap();
            let mut bytes = Vec::new();
            projection
                .write_stable(&request, |part| bytes.extend_from_slice(part))
                .unwrap();
            bytes
        };

        assert_eq!(bytes(&first), bytes(&reordered));
    }

    #[test]
    fn boundaries_preserve_system_tools_then_complete_message_order() {
        let transcript = transcript("current", "/raw/path.png");
        let request = Request {
            model: "fixture-model",
            transcript: &transcript,
            tools: TOOLS,
            max_tokens: 1_024,
            system: Some("stable instructions"),
            effort: None,
            attached: &[],
            prompt_cache: None,
        };

        let projection = PromptCacheProjection::inspect(&request).unwrap();

        assert_eq!(projection.stable_messages(), 2);
        assert_eq!(
            projection
                .boundaries()
                .iter()
                .map(PromptCacheBoundaryPoint::kind)
                .collect::<Vec<_>>(),
            vec![
                crate::PromptCacheBoundary::AfterSystem,
                crate::PromptCacheBoundary::AfterTools,
                crate::PromptCacheBoundary::AfterMessage,
                crate::PromptCacheBoundary::AfterMessage,
            ]
        );
    }

    fn long_transcript(first: &str) -> Transcript {
        let mut transcript = Transcript::new();
        for index in 0..70 {
            transcript.push(Message::said(if index == 0 { first } else { "history" }));
            transcript.push(Message::Agent {
                text: format!("answer-{index}").into(),
                calls: Vec::new(),
                stop: Some(StopReason::Yielded),
            });
        }
        transcript.push(Message::said("current volatile prompt"));
        transcript
    }

    #[test]
    fn long_prefixes_keep_the_latest_bounded_markers_without_truncating_hashed_content() {
        let first = long_transcript("oldest-a");
        let changed = long_transcript("oldest-b");
        let project = |transcript: &Transcript| {
            let request = request(transcript, "stable instructions", TOOLS, &[]);
            let projection = PromptCacheProjection::inspect(&request).unwrap();
            let mut bytes = Vec::new();
            projection
                .write_stable(&request, |part| bytes.extend_from_slice(part))
                .unwrap();
            (projection, bytes)
        };

        let (projection, bytes) = project(&first);
        let (_, changed_bytes) = project(&changed);

        assert_eq!(projection.boundaries().len(), MAX_PROMPT_CACHE_BOUNDARIES);
        assert!(
            projection
                .boundaries()
                .first()
                .is_some_and(|boundary| boundary.segment() > 1)
        );
        assert_eq!(
            projection
                .boundaries()
                .last()
                .and_then(PromptCacheBoundaryPoint::message),
            Some(u32::try_from(projection.stable_messages() - 1).unwrap())
        );
        assert_ne!(
            bytes, changed_bytes,
            "the dropped marker truncated stable bytes"
        );
    }
}
