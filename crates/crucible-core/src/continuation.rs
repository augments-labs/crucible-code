//! Ordered, private provider state beside an agent's visible answer.
//!
//! Adapters own the payload vocabulary. This module owns resource limits and
//! references into text and calls that the transcript already owns. A pending
//! continuation becomes replayable only when the whole response ends cleanly;
//! a complete signature from a broken stream is not a complete response.

use std::fmt;
use std::mem::size_of;

use sha2::{Digest as _, Sha256};

use crate::{CredentialScopeId, StopReason};

/// Maximum additional retained or encoded bytes in one response's continuation.
pub const CONTINUATION_BYTES: usize = 1024 * 1024;
/// Maximum semantic parts, independent of the number of network fragments.
pub const CONTINUATION_PARTS: usize = 1024;
/// Maximum continuation bytes retained across the active transcript.
pub const CONTINUATION_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const WORD_BYTES: usize = 256;

/// A private identity for the exact recipient and credential of opaque state.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ContinuationScope([u8; 32]);

impl ContinuationScope {
    /// Binds to the checked recipient, including its route, without retaining it.
    #[must_use]
    pub fn new(credential: CredentialScopeId, recipient: &str) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"crucible:provider-continuation:v1\0");
        hash.update(credential.bytes());
        hash.update(recipient.as_bytes());
        Self(hash.finalize().into())
    }

    /// Restores the identity from protected session storage.
    #[must_use]
    pub const fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// Bytes for the protected persistence codec, never diagnostics.
    #[must_use]
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for ContinuationScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ContinuationScope([redacted])")
    }
}

/// Why continuation cannot be retained or replayed. No payload enters an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ContinuationError {
    /// Resource admission failed before storing additional data.
    #[error("provider continuation exceeds its resource limit")]
    Limit,
    /// The protocol or model name is empty or contains invalid metadata.
    #[error("invalid provider continuation identity")]
    Identity,
    /// Ordered parts do not cover the owning text and calls exactly once.
    #[error("provider continuation does not match its answer")]
    References,
    /// Only a clean terminal response can contribute replayable state.
    #[error("unfinished response cannot carry provider continuation")]
    Unfinished,
}

/// One bounded adapter-owned payload or set of attributes.
#[derive(Clone, PartialEq, Eq)]
pub struct ContinuationData(Box<str>);

impl ContinuationData {
    /// Checks retained and JSON-escaped size before copying the payload.
    ///
    /// # Errors
    /// [`ContinuationError::Limit`] if either representation exceeds its limit.
    pub fn new(data: &str) -> Result<Self, ContinuationError> {
        if data.len() > CONTINUATION_BYTES || encoded_string_bytes(data) > CONTINUATION_BYTES {
            return Err(ContinuationError::Limit);
        }
        Ok(Self(data.into()))
    }

    /// Payload for its owning adapter or the protected session codec only.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ContinuationData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ContinuationData([redacted])")
    }
}

/// A semantic part, in its original position in the provider's response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationPart {
    /// UTF-8 byte range in the visible text, with adapter-owned attributes.
    Text {
        /// Inclusive byte offset.
        start: usize,
        /// Exclusive byte offset.
        end: usize,
        /// Signature, grouping or other native attributes; never visible prose.
        data: ContinuationData,
    },
    /// A call already owned by the answer, with its native attributes.
    Call {
        /// Index in the answer's call vector.
        index: usize,
        /// Adapter-owned attributes, not another copy of arguments.
        data: ContinuationData,
    },
    /// A private step not represented by visible text or a local tool call.
    Opaque(ContinuationData),
}

impl ContinuationPart {
    fn data(&self) -> &ContinuationData {
        match self {
            Self::Text { data, .. } | Self::Call { data, .. } | Self::Opaque(data) => data,
        }
    }

    fn encoded_bytes(&self) -> usize {
        let data = encoded_string_bytes(self.data().as_str());
        match self {
            Self::Text { start, end, .. } => 20 + digits(*start) + digits(*end) + data,
            Self::Call { index, .. } => 17 + digits(*index) + data,
            Self::Opaque(_) => 11 + data,
        }
    }
}

/// Bounded response state awaiting the receiver's terminal and reference checks.
///
/// This is the streaming value, not the value attached to a durable message.
#[derive(Clone, PartialEq, Eq)]
pub struct Continuation {
    protocol: Box<str>,
    model: Box<str>,
    scope: ContinuationScope,
    parts: Vec<ContinuationPart>,
    payload_bytes: usize,
    encoded_bytes: usize,
}

impl fmt::Debug for Continuation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Continuation")
            .field("parts", &self.parts.len())
            .field("payload", &"[redacted]")
            .finish_non_exhaustive()
    }
}

impl Continuation {
    /// Starts one ordered response. Model compatibility remains adapter-owned.
    ///
    /// # Errors
    /// [`ContinuationError::Identity`] for invalid bounded protocol/model names.
    pub fn new(
        protocol: &str,
        model: &str,
        scope: ContinuationScope,
    ) -> Result<Self, ContinuationError> {
        for word in [protocol, model] {
            if word.is_empty() || word.len() > WORD_BYTES || word.chars().any(char::is_control) {
                return Err(ContinuationError::Identity);
            }
        }
        Ok(Self {
            protocol: protocol.into(),
            model: model.into(),
            scope,
            parts: Vec::new(),
            payload_bytes: protocol.len() + model.len(),
            // Includes object punctuation, encoded scope and the empty array.
            encoded_bytes: 110 + encoded_string_bytes(protocol) + encoded_string_bytes(model),
        })
    }

    /// Admits one semantic part before expanding the retained vector.
    ///
    /// # Errors
    /// [`ContinuationError::Limit`] if any response bound would be exceeded.
    pub fn push(&mut self, part: ContinuationPart) -> Result<(), ContinuationError> {
        let length = self.parts.len() + 1;
        let payload = self
            .payload_bytes
            .saturating_add(part.data().as_str().len());
        let retained = size_of::<Self>()
            .saturating_add(payload)
            .saturating_add(length * size_of::<ContinuationPart>());
        let encoded = self.encoded_bytes.saturating_add(part.encoded_bytes() + 1);
        if length > CONTINUATION_PARTS
            || retained > CONTINUATION_BYTES
            || encoded > CONTINUATION_BYTES
        {
            return Err(ContinuationError::Limit);
        }
        // Exact reservation avoids retaining an unaccounted geometric capacity.
        self.parts.reserve_exact(1);
        self.parts.push(part);
        self.payload_bytes = payload;
        self.encoded_bytes = encoded;
        Ok(())
    }

    /// Additional retained bytes, not a token count.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.payload_bytes
            + self.parts.capacity() * size_of::<ContinuationPart>()
    }

    /// Finalizes a clean response against the text/calls the receiver assembled.
    ///
    /// # Errors
    /// [`ContinuationError::Unfinished`] without a successful terminal stop, or
    /// [`ContinuationError::References`] for missing, duplicate or invalid parts.
    pub fn finish(
        self,
        text: &str,
        calls: usize,
        stop: Option<StopReason>,
    ) -> Result<ProviderContinuation, ContinuationError> {
        if !matches!(stop, Some(StopReason::Yielded | StopReason::WantsTools)) {
            return Err(ContinuationError::Unfinished);
        }
        self.validate(text, calls)?;
        Ok(ProviderContinuation(self))
    }

    fn validate(&self, text: &str, calls: usize) -> Result<(), ContinuationError> {
        let mut offset = 0;
        let mut call = 0;
        for part in &self.parts {
            match part {
                ContinuationPart::Text { start, end, .. } => {
                    if *start != offset || end < start || text.get(*start..*end).is_none() {
                        return Err(ContinuationError::References);
                    }
                    offset = *end;
                }
                ContinuationPart::Call { index, .. } => {
                    if *index != call || call >= calls {
                        return Err(ContinuationError::References);
                    }
                    call += 1;
                }
                ContinuationPart::Opaque(_) => {}
            }
        }
        if offset != text.len() || call != calls || self.parts.is_empty() {
            return Err(ContinuationError::References);
        }
        Ok(())
    }
}

/// Finalized, bounded private state attached to exactly one complete answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderContinuation(Continuation);

impl ProviderContinuation {
    /// The open wire-protocol identity, interpreted only by the owning adapter.
    #[must_use]
    pub fn protocol(&self) -> &str {
        &self.0.protocol
    }
    /// Producing model; the adapter decides compatibility with another model.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.0.model
    }
    /// Exact credential and recipient binding.
    #[must_use]
    pub const fn scope(&self) -> ContinuationScope {
        self.0.scope
    }
    /// Original semantic order; borrowed payloads are never diagnostic output.
    #[must_use]
    pub fn parts(&self) -> &[ContinuationPart] {
        &self.0.parts
    }
    /// Additional memory retained by this response, independent of token usage.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.0.retained_bytes()
    }
    /// Checks that a caller has not rewritten the owning message's references.
    ///
    /// # Errors
    /// [`ContinuationError::References`] if visible text or calls no longer match.
    pub fn validate(&self, text: &str, calls: usize) -> Result<(), ContinuationError> {
        self.0.validate(text, calls)
    }
}

fn digits(value: usize) -> usize {
    value.checked_ilog10().map_or(1, |n| n as usize + 1)
}

fn encoded_string_bytes(value: &str) -> usize {
    value.bytes().fold(2, |total, byte| {
        total.saturating_add(match byte {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 8 | 12 => 2,
            0..=31 => 6,
            _ => 1,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, Transcript};

    fn pending() -> Continuation {
        Continuation::new("test-v1", "test", ContinuationScope::from_digest([0; 32])).unwrap()
    }

    fn opaque(value: &str) -> ContinuationPart {
        ContinuationPart::Opaque(ContinuationData::new(value).unwrap())
    }

    fn answer(payload: &str) -> Message {
        let mut state = pending();
        state.push(opaque(payload)).unwrap();
        Message::Agent {
            text: "".into(),
            calls: vec![],
            stop: Some(StopReason::Yielded),
            continuation: Some(state.finish("", 0, Some(StopReason::Yielded)).unwrap()),
        }
    }

    #[test]
    fn ordered_references_require_complete_utf8_coverage_and_clean_stop() {
        let data = || ContinuationData::new("").unwrap();
        let mut valid = pending();
        valid.push(opaque("signature")).unwrap();
        valid
            .push(ContinuationPart::Text {
                start: 0,
                end: 2,
                data: data(),
            })
            .unwrap();
        valid
            .push(ContinuationPart::Call {
                index: 0,
                data: data(),
            })
            .unwrap();
        valid
            .push(ContinuationPart::Text {
                start: 2,
                end: 3,
                data: data(),
            })
            .unwrap();
        assert!(
            valid
                .clone()
                .finish("é!", 1, Some(StopReason::WantsTools))
                .is_ok()
        );
        assert_eq!(
            valid.clone().finish("é!", 2, Some(StopReason::Yielded)),
            Err(ContinuationError::References)
        );
        assert_eq!(
            valid.clone().finish("é", 1, Some(StopReason::Yielded)),
            Err(ContinuationError::References)
        );
        for stop in [
            None,
            Some(StopReason::Cancelled),
            Some(StopReason::OutOfTokens),
            Some(StopReason::Unknown),
        ] {
            assert_eq!(
                valid.clone().finish("é!", 1, stop),
                Err(ContinuationError::Unfinished)
            );
        }
        for part in [
            ContinuationPart::Text {
                start: 0,
                end: 1,
                data: data(),
            },
            ContinuationPart::Call {
                index: 1,
                data: data(),
            },
        ] {
            let mut invalid = pending();
            invalid.push(part).unwrap();
            assert_eq!(
                invalid.finish("é", 1, Some(StopReason::Yielded)),
                Err(ContinuationError::References)
            );
        }
        assert_eq!(
            pending().finish("", 0, Some(StopReason::Yielded)),
            Err(ContinuationError::References)
        );
    }

    #[test]
    fn response_admission_counts_parts_allocations_and_json_escaping() {
        let mut state = pending();
        for _ in 0..CONTINUATION_PARTS {
            state.push(opaque("")).unwrap();
        }
        let before = state.clone();
        assert_eq!(state.push(opaque("")), Err(ContinuationError::Limit));
        assert_eq!(state, before, "rejection cannot mutate accepted state");
        let maximum =
            CONTINUATION_BYTES - pending().retained_bytes() - size_of::<ContinuationPart>();
        let mut state = pending();
        state.push(opaque(&"x".repeat(maximum))).unwrap();
        assert_eq!(state.retained_bytes(), CONTINUATION_BYTES);
        assert_eq!(
            pending().push(opaque(&"x".repeat(maximum + 1))),
            Err(ContinuationError::Limit)
        );
        let maximum = (CONTINUATION_BYTES - 2) / 6;
        assert!(ContinuationData::new(&"\0".repeat(maximum)).is_ok());
        assert_eq!(
            ContinuationData::new(&"\0".repeat(maximum + 1)),
            Err(ContinuationError::Limit)
        );
        let encoded_room = CONTINUATION_BYTES - pending().encoded_bytes - 14;
        assert!(
            pending()
                .push(opaque(&"\\".repeat(encoded_room / 2)))
                .is_ok()
        );
        assert_eq!(
            pending().push(opaque(&"\\".repeat(encoded_room / 2 + 1))),
            Err(ContinuationError::Limit)
        );
    }

    #[test]
    fn active_history_rejects_before_growth_and_releases_removed_state() {
        let maximum =
            CONTINUATION_BYTES - pending().retained_bytes() - size_of::<ContinuationPart>();
        let message = answer(&"x".repeat(maximum));
        let mut transcript = Transcript::new();
        for _ in 0..64 {
            transcript.push(message.clone()).unwrap();
        }
        assert_eq!(transcript.continuation_room(), 0);
        assert_eq!(transcript.push(answer("x")), Err(ContinuationError::Limit));
        assert_eq!(transcript.len(), 64);
        transcript.pop();
        assert_eq!(transcript.continuation_room(), CONTINUATION_BYTES);
        transcript.behind(1);
        assert_eq!(transcript.continuation_room(), 2 * CONTINUATION_BYTES);
        transcript.compacted(60, "notes");
        assert_eq!(transcript.continuation_room(), 62 * CONTINUATION_BYTES);
        transcript.forget();
        assert_eq!(transcript.continuation_room(), CONTINUATION_HISTORY_BYTES);
    }

    #[test]
    fn compaction_marks_old_prefixes_without_mutating_native_state() {
        let mut transcript = Transcript::new();
        transcript.push(Message::said("old")).unwrap();
        transcript.push(answer("old-signature")).unwrap();
        transcript.push(Message::said("keep")).unwrap();
        let kept = answer("kept-signature");
        transcript.push(kept.clone()).unwrap();
        assert!(!transcript.prefix_rewritten(3));
        transcript.compacted(2, "recap");
        assert_eq!(transcript.messages().last(), Some(&kept));
        assert!(
            transcript.prefix_rewritten(2),
            "the adapter must know that the retained prefix changed"
        );
        transcript.push(answer("fresh-signature")).unwrap();
        assert!(
            !transcript.prefix_rewritten(3),
            "new output is bound to the rewritten history already"
        );
        transcript.behind(1);
        transcript.pop();
        transcript.push(answer("replacement-signature")).unwrap();
        assert!(
            !transcript.prefix_rewritten(2),
            "removed history cannot invalidate a replacement answer"
        );
        transcript.forget();
        transcript.push(answer("new-session-signature")).unwrap();
        assert!(!transcript.prefix_rewritten(0));
    }

    #[test]
    fn scope_is_stable_only_for_the_same_credential_and_exact_recipient() {
        let credential = CredentialScopeId::from_digest([1; 32]);
        let first = ContinuationScope::new(credential, "https://provider.example/v1/route");
        assert_eq!(
            first,
            ContinuationScope::new(credential, "https://provider.example/v1/route")
        );
        assert_ne!(
            first,
            ContinuationScope::new(
                CredentialScopeId::from_digest([2; 32]),
                "https://provider.example/v1/route"
            )
        );
        assert_ne!(
            first,
            ContinuationScope::new(credential, "https://provider.example/v2/route")
        );
        assert_ne!(
            first,
            ContinuationScope::new(credential, "https://other.example/v1/route")
        );
    }

    #[test]
    fn private_payload_and_scope_are_absent_from_debug_and_errors() {
        let message = answer("signature-canary");
        assert!(!format!("{message:?}").contains("signature-canary"));
        let mut state = pending();
        state.push(opaque("signature-canary")).unwrap();
        assert!(!format!("{state:?}").contains("signature-canary"));
        assert!(!format!("{:?}", state.scope).contains("[0, 0"));
        assert!(
            !format!(
                "{}",
                state
                    .finish("missing", 0, Some(StopReason::Yielded))
                    .unwrap_err()
            )
            .contains("signature-canary")
        );
    }
}
