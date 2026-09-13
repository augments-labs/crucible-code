//! Keys, receipts and extension records the framework history is written from.
//!
//! A provider receives the deliberately closed [`Message`] vocabulary. The
//! framework needs a wider history for attempts, interruptions, invocation
//! recovery and extension state. The format-neutral half of that history —
//! the identity a stored call result is keyed by, the receipt a durable sink
//! returns, and the records that carry no execution fact — lives here, so a
//! store can be written against it without depending on the runtime that
//! produced the facts.

use std::fmt;

use sha2::{Digest as _, Sha256};

use crucible_types::{Ancestry, Message, ToolId};

use crate::interruption::{InvocationId, JournalEntryId};

const COMPACTION_DIGEST_DOMAIN: &[u8] = b"crucible:journal-compaction:v1\0";
const CALL_RESULT_KEY_DOMAIN: &[u8] = b"crucible:call-result-key:v1\0";

/// Most bytes retained in one opaque extension payload.
pub const MAX_CUSTOM_DATA_BYTES: usize = 32_768;
/// Most bytes retained in one extension namespace, source, or response id.
pub const MAX_JOURNAL_WORD_BYTES: usize = 256;

/// Source-qualified identity under which one recorded call may own one result.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CallResultKey([u8; 32]);

impl CallResultKey {
    /// Restores a key from a protected persistence codec.
    #[must_use]
    pub const fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// Derives one stable key from the exact invocation, ancestry, and call.
    #[must_use]
    pub fn derive(ancestry: Ancestry, invocation: InvocationId, call: &ToolId) -> Self {
        let mut digest = Sha256::new();
        digest.update(CALL_RESULT_KEY_DOMAIN);
        for field in [
            ancestry.run().to_string(),
            ancestry
                .parent()
                .map_or_else(String::new, |id| id.to_string()),
            ancestry.root().to_string(),
            ancestry.depth().to_string(),
            invocation.to_string(),
            call.as_str().to_owned(),
        ] {
            digest.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
            digest.update(field.as_bytes());
        }
        Self(digest.finalize().into())
    }

    /// Protected bytes used by persistence codecs and transaction journals.
    #[must_use]
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for CallResultKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CallResultKey([redacted])")
    }
}

/// Durable receipt returned for an idempotently stored call result.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallResultReceipt([u8; 32]);

impl CallResultReceipt {
    /// Creates a receipt from the sink's canonical payload digest.
    #[must_use]
    pub const fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// Protected digest bytes bound into the sandbox WAL.
    #[must_use]
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for CallResultReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CallResultReceipt([redacted])")
    }
}

/// Why the one-result durable sink could not complete an idempotent insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CallResultStoreError {
    /// This run has no durable result store.
    #[error("durable call-result storage is unavailable")]
    Unavailable,
    /// The key already names a different logical result.
    #[error("call-result identity is already occupied by different content")]
    Conflict,
    /// The supplied key, call, or result crossed a storage invariant.
    #[error("call-result record is invalid")]
    Invalid,
    /// The protected store could not durably complete its operation.
    #[error("durable call-result storage failed")]
    Storage,
}

/// Why framework history could not be retained or projected safely.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    /// The history reached its fixed item ceiling.
    #[error("run history reached its {0}-item limit")]
    TooManyItems(usize),
    /// The history crossed its fixed aggregate retained-data ceiling.
    #[error("run history reached its {0}-byte retained-data limit")]
    TooManyBytes(usize),
    /// A retained field was empty, too large, or contained a control byte.
    #[error("invalid bounded journal field {0}")]
    InvalidField(&'static str),
    /// Opaque custom data was not one bounded JSON value.
    #[error("custom journal data is not one bounded JSON value")]
    InvalidCustomData,
    /// A provider call id appeared a second time in one projected history.
    #[error("tool call {0} was recorded more than once")]
    DuplicateCall(ToolId),
    /// A result had no earlier provider-visible call.
    #[error("tool result {0} has no recorded call")]
    OrphanedResult(ToolId),
    /// One provider-visible call received more than one result.
    #[error("tool call {0} received more than one result")]
    DuplicateResult(ToolId),
    /// Projection stopped while a provider-visible call was unanswered.
    #[error("tool call {0} has no result")]
    UnansweredCall(ToolId),
}

impl JournalError {
    /// Accepts one bounded, control-free word retained in framework history.
    ///
    /// The history that applies this rule to its own records lives above this
    /// crate, so the rule is stated once by the error that reports it.
    ///
    /// # Errors
    ///
    /// [`JournalError::InvalidField`] for an empty, oversized, or
    /// control-bearing word.
    pub fn check_word(field: &'static str, value: &str) -> Result<(), Self> {
        if value.is_empty()
            || value.len() > MAX_JOURNAL_WORD_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(Self::InvalidField(field));
        }
        Ok(())
    }
}

/// Bounded metadata for one completed transcript compaction.
///
/// The ordinary session compaction line remains the owner of the recap text.
/// This journal record keeps only counts and a domain-separated digest, so
/// framework history can correlate the transition without copying model-visible
/// content into a second persistence concept.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CompactionRecord {
    ancestry: Ancestry,
    replaced: usize,
    recap_bytes: usize,
    recap_digest: [u8; 32],
}

impl CompactionRecord {
    /// Records one completed replacement from the exact recap stored by the
    /// conversation session.
    #[must_use]
    pub fn new(ancestry: Ancestry, replaced: usize, recap: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(COMPACTION_DIGEST_DOMAIN);
        digest.update(recap.as_bytes());
        Self {
            ancestry,
            replaced,
            recap_bytes: recap.len(),
            recap_digest: digest.finalize().into(),
        }
    }

    /// Execution that requested the compaction.
    #[must_use]
    pub const fn ancestry(self) -> Ancestry {
        self.ancestry
    }

    /// Raw transcript messages replaced by the recap.
    #[must_use]
    pub const fn replaced(self) -> usize {
        self.replaced
    }

    /// UTF-8 bytes in the exact stored recap.
    #[must_use]
    pub const fn recap_bytes(self) -> usize {
        self.recap_bytes
    }

    /// Domain-separated digest used only to correlate this metadata with the
    /// conversation-owned recap.
    #[must_use]
    pub const fn recap_digest(self) -> [u8; 32] {
        self.recap_digest
    }
}

impl fmt::Debug for CompactionRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompactionRecord")
            .field("ancestry", &self.ancestry)
            .field("replaced", &self.replaced)
            .field("recap_bytes", &self.recap_bytes)
            .field("recap_digest", &"[redacted]")
            .finish()
    }
}

/// One versioned, namespaced extension entry.
#[derive(Clone)]
pub struct CustomEntry {
    id: JournalEntryId,
    ancestry: Ancestry,
    namespace: Box<str>,
    schema_version: u32,
    data: Box<str>,
    source: Box<str>,
}

impl CustomEntry {
    /// Builds one bounded opaque entry under a stable extension namespace.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError`] for an invalid namespace/source, a zero schema
    /// version, oversized data, or data that is not exactly one JSON value.
    pub fn new(
        namespace: impl Into<Box<str>>,
        schema_version: u32,
        data: impl Into<Box<str>>,
        source: impl Into<Box<str>>,
    ) -> Result<Self, JournalError> {
        Self::for_run(
            JournalEntryId::new(),
            Ancestry::new(),
            namespace,
            schema_version,
            data,
            source,
        )
    }

    /// Restores or builds an entry under an exact identity and ancestry.
    ///
    /// # Errors
    ///
    /// The same validation as [`Self::new`].
    // Stable identity/ancestry and the four extension-owned fields are all
    // independent wire data; an artificial carrier would enforce nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn for_run(
        id: JournalEntryId,
        ancestry: Ancestry,
        namespace: impl Into<Box<str>>,
        schema_version: u32,
        data: impl Into<Box<str>>,
        source: impl Into<Box<str>>,
    ) -> Result<Self, JournalError> {
        let namespace = namespace.into();
        if schema_version == 0
            || namespace.is_empty()
            || namespace.len() > MAX_JOURNAL_WORD_BYTES
            || !namespace.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            })
        {
            return Err(JournalError::InvalidField("custom namespace"));
        }
        let source = source.into();
        JournalError::check_word("custom source", &source)?;
        let data = data.into();
        if data.len() > MAX_CUSTOM_DATA_BYTES
            || serde_json::from_str::<serde_json::Value>(&data).is_err()
        {
            return Err(JournalError::InvalidCustomData);
        }
        Ok(Self {
            id,
            ancestry,
            namespace,
            schema_version,
            data,
            source,
        })
    }

    /// Stable entry identity.
    #[must_use]
    pub const fn id(&self) -> JournalEntryId {
        self.id
    }

    /// Producing execution.
    #[must_use]
    pub const fn ancestry(&self) -> Ancestry {
        self.ancestry
    }

    /// Extension namespace.
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Namespace-local schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Opaque JSON for its owning extension or an explicit projector.
    #[must_use]
    pub fn data(&self) -> &str {
        &self.data
    }

    /// Source registration identity.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }
}

impl fmt::Debug for CustomEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CustomEntry")
            .field("id", &self.id)
            .field("ancestry", &self.ancestry)
            .field("namespace", &self.namespace)
            .field("schema_version", &self.schema_version)
            .field("data", &"[redacted]")
            .field("source", &self.source)
            .finish()
    }
}

/// Explicit opt-in for turning opaque extension state into a closed message.
pub trait CustomProjector {
    /// Returns a provider-visible message, or leaves this custom entry private.
    fn project(&self, entry: &CustomEntry) -> Option<Message>;
}

/// The conversation-writing seam used by a runner.
pub trait SessionStore: Send + Sync {
    /// Appends one closed conversation message.
    fn append_message(&self, message: &Message);
}
