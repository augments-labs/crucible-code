//! Framework history kept beside, but not confused with, provider messages.
//!
//! A provider receives the deliberately closed [`Message`] vocabulary. The
//! framework needs a wider history for attempts, interruptions, invocation
//! recovery, and extension state, so those records live as [`RunItem`]s and
//! cross the provider boundary only through an explicit projection.

use std::collections::BTreeMap;
use std::fmt;

use sha2::{Digest as _, Sha256};

use crate::{
    Ancestry, Message, PromptCacheFact, SandboxFact, TOOL_ARGUMENT_BYTES, TOOL_CALL_ID_BYTES,
    TOOL_NAME_BYTES, TOOL_RESULT_BYTES, ToolId, ToolResult, Transcript,
};

use crate::interruption::{InvocationId, InvocationRecord, JournalEntryId, PendingAction};

const COMPACTION_DIGEST_DOMAIN: &[u8] = b"crucible:journal-compaction:v1\0";
const CALL_RESULT_KEY_DOMAIN: &[u8] = b"crucible:call-result-key:v1\0";

/// Most framework records retained in one in-memory history.
pub const MAX_RUN_ITEMS: usize = 4_096;
/// Most encoded bytes retained for one framework journal record.
pub const MAX_RUN_ITEM_BYTES: usize = 2 * 1_024 * 1_024;
/// Most in-memory bytes retained by one framework item.
pub const MAX_RUN_ITEM_RETAINED_BYTES: usize = 20 * 1_024 * 1_024;
/// Most in-memory bytes retained by one framework history.
pub const MAX_RUN_HISTORY_BYTES: usize = 64 * 1_024 * 1_024;
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

/// One framework-history record.
///
/// `Message` is nested rather than widened: providers still match the same
/// closed enum, while non-message records are skipped unless code explicitly
/// projects them.
#[derive(Clone)]
pub enum RunItem {
    /// One ordinary provider-visible conversation message.
    Message {
        /// Execution that produced or admitted it.
        ancestry: Ancestry,
        /// The closed provider vocabulary.
        message: Message,
    },
    /// One normalized, bounded prompt-cache attempt fact.
    ProviderAttempt {
        /// Execution whose request produced the fact.
        ancestry: Ancestry,
        /// Typed metadata with no prompt, routing key, or resource handle.
        fact: PromptCacheFact,
    },
    /// One bounded sandbox lifecycle fact, invisible to providers.
    Sandbox {
        /// Execution whose tool call owns the sandbox.
        ancestry: Ancestry,
        /// Fixed call attribution.
        call: ToolId,
        /// Redacted typed lifecycle fact.
        fact: SandboxFact,
    },
    /// One durable interruption point.
    Interrupt(PendingAction),
    /// One prepared, started, or finished tool invocation.
    Invocation(InvocationRecord),
    /// One completed transcript compaction, without copying its recap text.
    Compaction(CompactionRecord),
    /// Versioned extension state, invisible to a provider by default.
    Custom(CustomEntry),
}

impl RunItem {
    /// Admits one closed message into framework history after retained fields
    /// have been checked at the storage boundary.
    ///
    /// # Errors
    ///
    /// [`JournalError::InvalidField`] when provider-controlled call/result
    /// fields cross their existing tool or result ceilings.
    pub fn message(ancestry: Ancestry, message: Message) -> Result<Self, JournalError> {
        let item = Self::Message { ancestry, message };
        item.validate_retained()?;
        Ok(item)
    }

    /// Records one already-bounded prompt-cache fact.
    #[must_use]
    pub fn provider_attempt(ancestry: Ancestry, fact: PromptCacheFact) -> Self {
        Self::ProviderAttempt { ancestry, fact }
    }

    /// Records one sandbox fact under fixed run and call attribution.
    ///
    /// # Errors
    ///
    /// An empty or oversized call identity is refused at the journal boundary.
    pub fn sandbox(
        ancestry: Ancestry,
        call: ToolId,
        fact: SandboxFact,
    ) -> Result<Self, JournalError> {
        if call.as_str().is_empty() || call.as_str().len() > TOOL_CALL_ID_BYTES {
            return Err(JournalError::InvalidField("sandbox tool call id"));
        }
        Ok(Self::Sandbox {
            ancestry,
            call,
            fact,
        })
    }

    /// The producing execution.
    #[must_use]
    pub const fn ancestry(&self) -> Ancestry {
        match self {
            Self::Message { ancestry, .. }
            | Self::ProviderAttempt { ancestry, .. }
            | Self::Sandbox { ancestry, .. } => *ancestry,
            Self::Interrupt(action) => action.ancestry(),
            Self::Invocation(invocation) => invocation.ancestry(),
            Self::Compaction(compaction) => compaction.ancestry(),
            Self::Custom(entry) => entry.ancestry(),
        }
    }

    /// The provider-visible message, only where this item inherently is one.
    #[must_use]
    pub const fn model_message(&self) -> Option<&Message> {
        match self {
            Self::Message { message, .. } => Some(message),
            Self::ProviderAttempt { .. }
            | Self::Sandbox { .. }
            | Self::Interrupt(_)
            | Self::Invocation(_)
            | Self::Compaction(_)
            | Self::Custom(_) => None,
        }
    }

    /// Prompt-cache metadata carried by this item.
    #[must_use]
    pub const fn prompt_cache_fact(&self) -> Option<&PromptCacheFact> {
        match self {
            Self::ProviderAttempt { fact, .. } => Some(fact),
            _ => None,
        }
    }

    /// Sandbox fact carried by this item, with its call attribution.
    #[must_use]
    pub const fn sandbox_fact(&self) -> Option<(&ToolId, &SandboxFact)> {
        match self {
            Self::Sandbox { call, fact, .. } => Some((call, fact)),
            _ => None,
        }
    }

    /// Rechecks every open-variant field at a persistence boundary.
    ///
    /// Constructors validate the ordinary path, while this method also
    /// protects stores from callers that directly construct a public enum
    /// variant around an otherwise valid domain value.
    ///
    /// # Errors
    ///
    /// [`JournalError::InvalidField`] when a retained tool/action field crosses
    /// its fixed boundary.
    pub fn validate_retained(&self) -> Result<(), JournalError> {
        if item_retained_bytes(self) > MAX_RUN_ITEM_RETAINED_BYTES {
            return Err(JournalError::InvalidField("run item retained bytes"));
        }
        match self {
            Self::Message { message, .. } => validate_message(message),
            Self::Interrupt(action) => {
                if action.expires_at() == 0 {
                    return Err(JournalError::InvalidField("pending action expiry"));
                }
                if let Some(call) = action.call() {
                    validate_call(call)?;
                }
                Ok(())
            }
            Self::Invocation(invocation) => {
                validate_call(invocation.call())?;
                if let crate::InvocationState::Finished { output, .. } = invocation.state()
                    && (output.text().len() > TOOL_RESULT_BYTES
                        || serde_json::to_string(output.text())
                            .map_or(true, |encoded| encoded.len() > TOOL_RESULT_BYTES))
                {
                    return Err(JournalError::InvalidField("tool result"));
                }
                Ok(())
            }
            Self::ProviderAttempt { fact, .. } => validate_cache_fact(fact),
            Self::Sandbox { call, .. } => {
                if call.as_str().is_empty() || call.as_str().len() > TOOL_CALL_ID_BYTES {
                    return Err(JournalError::InvalidField("sandbox tool call id"));
                }
                Ok(())
            }
            Self::Compaction(_) | Self::Custom(_) => Ok(()),
        }
    }
}

impl fmt::Debug for RunItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message { ancestry, message } => f
                .debug_struct("Message")
                .field("ancestry", ancestry)
                .field("message", message)
                .finish(),
            Self::ProviderAttempt { ancestry, fact } => f
                .debug_struct("ProviderAttempt")
                .field("ancestry", ancestry)
                .field("fact", fact)
                .finish(),
            Self::Sandbox {
                ancestry,
                call: _,
                fact,
            } => f
                .debug_struct("Sandbox")
                .field("ancestry", ancestry)
                .field("call", &"[redacted]")
                .field("fact", fact)
                .finish(),
            Self::Interrupt(action) => f.debug_tuple("Interrupt").field(action).finish(),
            Self::Invocation(invocation) => f.debug_tuple("Invocation").field(invocation).finish(),
            Self::Compaction(compaction) => f.debug_tuple("Compaction").field(compaction).finish(),
            Self::Custom(entry) => f.debug_tuple("Custom").field(entry).finish(),
        }
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
        bounded_word("custom source", &source)?;
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

/// Ordered framework history for one execution/session view.
#[derive(Debug, Clone, Default)]
pub struct RunHistory {
    items: Vec<RunItem>,
    retained: usize,
}

impl RunHistory {
    /// Empty history.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            items: Vec::new(),
            retained: 0,
        }
    }

    /// Appends one record under the global retained count.
    ///
    /// # Errors
    ///
    /// [`JournalError::TooManyItems`] at the fixed ceiling.
    pub fn push(&mut self, item: RunItem) -> Result<(), JournalError> {
        item.validate_retained()?;
        if self.items.len() >= MAX_RUN_ITEMS {
            return Err(JournalError::TooManyItems(MAX_RUN_ITEMS));
        }
        let retained = self.retained.saturating_add(item_retained_bytes(&item));
        if retained > MAX_RUN_HISTORY_BYTES {
            return Err(JournalError::TooManyBytes(MAX_RUN_HISTORY_BYTES));
        }
        self.items.push(item);
        self.retained = retained;
        Ok(())
    }

    /// Retained records in append order.
    #[must_use]
    pub fn items(&self) -> &[RunItem] {
        &self.items
    }

    /// Projects only inherent conversation records.
    ///
    /// # Errors
    ///
    /// Refuses any call/result sequence that is not exactly paired.
    pub fn project(&self) -> Result<Transcript, JournalError> {
        self.project_messages(None)
    }

    /// Projects conversation records and custom entries explicitly accepted by
    /// `projector`.
    ///
    /// # Errors
    ///
    /// Refuses any call/result sequence that is not exactly paired, including
    /// one produced by the projector.
    pub fn project_with(
        &self,
        projector: &dyn CustomProjector,
    ) -> Result<Transcript, JournalError> {
        self.project_messages(Some(projector))
    }

    fn project_messages(
        &self,
        projector: Option<&dyn CustomProjector>,
    ) -> Result<Transcript, JournalError> {
        let mut transcript = Transcript::new();
        let mut calls = BTreeMap::<ToolId, bool>::new();

        for item in &self.items {
            let projected = match item {
                RunItem::Message { message, .. } => Some(message.clone()),
                RunItem::Custom(entry) => projector.and_then(|one| one.project(entry)),
                RunItem::ProviderAttempt { .. }
                | RunItem::Sandbox { .. }
                | RunItem::Interrupt(_)
                | RunItem::Invocation(_)
                | RunItem::Compaction(_) => None,
            };
            if let Some(message) = projected {
                validate_message(&message)?;
                validate_projection(&message, &mut calls)?;
                transcript
                    .push(message)
                    .map_err(|_| JournalError::InvalidField("provider continuation"))?;
            }
        }

        if let Some((id, _)) = calls.iter().find(|(_, answered)| !**answered) {
            return Err(JournalError::UnansweredCall(id.clone()));
        }
        Ok(transcript)
    }
}

/// The conversation-writing seam used by a runner.
pub trait SessionStore: Send + Sync {
    /// Appends one closed conversation message.
    fn append_message(&self, message: &Message);
}

/// The framework-history writing seam used by runners and invocation workers.
pub trait JournalStore: Send + Sync {
    /// Appends one already bounded framework record.
    fn append_run_item(&self, item: &RunItem);

    /// Durably inserts one source-qualified result exactly once.
    ///
    /// Implementations must return the same receipt when the same key and
    /// logical result are repeated, and [`CallResultStoreError::Conflict`]
    /// when the key is already bound to different content. The default keeps
    /// in-memory and test journals fail closed at a background-acceptance
    /// boundary instead of pretending they are durable.
    ///
    /// # Errors
    ///
    /// Storage is unavailable, the key conflicts with different content, the
    /// result is invalid, or the protected write could not complete durably.
    fn put_call_result(
        &self,
        _key: CallResultKey,
        _result: &ToolResult,
    ) -> Result<CallResultReceipt, CallResultStoreError> {
        Err(CallResultStoreError::Unavailable)
    }

    /// Removes accepted sidecars only after their ordinary result message and
    /// companion journal metadata have crossed the sink's durability barrier.
    ///
    /// The default is for in-memory journals, which cannot own sidecars.
    fn settle_call_results(&self) {}
}

fn validate_projection(
    message: &Message,
    calls: &mut BTreeMap<ToolId, bool>,
) -> Result<(), JournalError> {
    if !matches!(message, Message::ToolResults(_))
        && let Some((id, _)) = calls.iter().find(|(_, answered)| !**answered)
    {
        return Err(JournalError::UnansweredCall(id.clone()));
    }
    match message {
        Message::Agent { calls: asked, .. } => {
            for call in asked {
                if calls.insert(call.id.clone(), false).is_some() {
                    return Err(JournalError::DuplicateCall(call.id.clone()));
                }
            }
        }
        Message::ToolResults(results) => {
            for result in results {
                let Some(answered) = calls.get_mut(&result.id) else {
                    return Err(JournalError::OrphanedResult(result.id.clone()));
                };
                if *answered {
                    return Err(JournalError::DuplicateResult(result.id.clone()));
                }
                *answered = true;
            }
        }
        Message::Context(_) | Message::User { .. } => {}
    }
    Ok(())
}

fn validate_message(message: &Message) -> Result<(), JournalError> {
    if message_retained_bytes(message) > MAX_RUN_ITEM_RETAINED_BYTES {
        return Err(JournalError::InvalidField("message retained bytes"));
    }
    match message {
        Message::Agent {
            text,
            calls,
            stop,
            continuation,
        } => {
            if let Some(state) = continuation {
                if !matches!(
                    stop,
                    Some(crate::StopReason::Yielded | crate::StopReason::WantsTools)
                ) {
                    return Err(JournalError::InvalidField(
                        "unfinished provider continuation",
                    ));
                }
                state
                    .validate(text, calls.len())
                    .map_err(|_| JournalError::InvalidField("provider continuation"))?;
            }
            for call in calls {
                validate_call(call)?;
            }
        }
        Message::ToolResults(results) => {
            for result in results {
                if result.id.as_str().is_empty()
                    || result.id.as_str().len() > TOOL_CALL_ID_BYTES
                    || result.output.text().len() > TOOL_RESULT_BYTES
                    || serde_json::to_string(result.output.text())
                        .map_or(true, |encoded| encoded.len() > TOOL_RESULT_BYTES)
                {
                    return Err(JournalError::InvalidField("tool result"));
                }
            }
        }
        Message::Context(_) | Message::User { .. } => {}
    }
    Ok(())
}

fn validate_call(call: &crate::ToolCall) -> Result<(), JournalError> {
    if call.id.as_str().is_empty() || call.id.as_str().len() > TOOL_CALL_ID_BYTES {
        return Err(JournalError::InvalidField("tool call id"));
    }
    if call.name.is_empty() || call.name.len() > TOOL_NAME_BYTES {
        return Err(JournalError::InvalidField("tool name"));
    }
    if call.args.as_str().len() > TOOL_ARGUMENT_BYTES {
        return Err(JournalError::InvalidField("tool arguments"));
    }
    Ok(())
}

fn validate_cache_fact(fact: &PromptCacheFact) -> Result<(), JournalError> {
    match fact {
        PromptCacheFact::Planned(fact) => {
            bounded_word("cache capability version", fact.capability_version)?;
            if let Some(revision) = fact.model_revision {
                bounded_word("cache model revision", revision)?;
            }
            bounded_word("cache policy version", fact.policy_version.as_str())?;
            bounded_word("cache request shape version", fact.request_shape_version)
        }
        PromptCacheFact::UsageReported(fact) => {
            if let Some(version) = fact.cost.pricing_version {
                bounded_word("cache pricing version", version)?;
            }
            if let Some(source) = fact.cost.source_url {
                bounded_word("cache pricing source", source)?;
            }
            Ok(())
        }
        PromptCacheFact::RequestEncoded(_) | PromptCacheFact::ResourceChanged(_) => Ok(()),
    }
}

fn bounded_word(field: &'static str, value: &str) -> Result<(), JournalError> {
    if value.is_empty()
        || value.len() > MAX_JOURNAL_WORD_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(JournalError::InvalidField(field));
    }
    Ok(())
}

fn item_retained_bytes(item: &RunItem) -> usize {
    let base = 256_usize;
    base.saturating_add(match item {
        RunItem::Message { message, .. } => message_retained_bytes(message),
        RunItem::ProviderAttempt { fact, .. } => cache_fact_retained_bytes(fact),
        RunItem::Sandbox { call, .. } => call.as_str().len().saturating_add(4_096),
        RunItem::Interrupt(action) => action
            .call()
            .map_or_else(
                || match action {
                    PendingAction::HumanInput(human) => human.question().len(),
                    PendingAction::Approval(_) | PendingAction::ExternalTool(_) => 0,
                },
                call_retained_bytes,
            )
            .saturating_add(256),
        RunItem::Invocation(invocation) => {
            let state = match invocation.state() {
                crate::InvocationState::Finished { output, .. } => output_retained_bytes(output),
                crate::InvocationState::Prepared | crate::InvocationState::Started => 0,
            };
            call_retained_bytes(invocation.call())
                .saturating_add(state)
                .saturating_add(
                    invocation
                        .idempotency_key()
                        .map_or(0, |key| key.as_str().len()),
                )
        }
        RunItem::Compaction(_) => 128,
        RunItem::Custom(entry) => entry
            .namespace()
            .len()
            .saturating_add(entry.source().len())
            .saturating_add(entry.data().len()),
    })
}

fn message_retained_bytes(message: &Message) -> usize {
    match message {
        Message::Context(fragment) => fragment
            .section()
            .len()
            .saturating_add(fragment.text().len()),
        Message::User { text, attachments } => {
            attachments.iter().fold(text.len(), |bytes, item| {
                bytes
                    .saturating_add(128)
                    .saturating_add(item.path.len())
                    .saturating_add(item.media_type.len())
            })
        }
        Message::Agent { text, calls, .. } => calls.iter().fold(
            text.len().saturating_add(message.continuation_bytes()),
            |bytes, call| bytes.saturating_add(call_retained_bytes(call)),
        ),
        Message::ToolResults(results) => results.iter().fold(0_usize, |bytes, result| {
            bytes
                .saturating_add(result.id.as_str().len())
                .saturating_add(output_retained_bytes(&result.output))
        }),
    }
}

fn call_retained_bytes(call: &crate::ToolCall) -> usize {
    call.id
        .as_str()
        .len()
        .saturating_add(call.name.len())
        .saturating_add(call.args.as_str().len())
}

fn output_retained_bytes(output: &crate::ToolOutput) -> usize {
    output
        .attachments()
        .iter()
        .fold(output.text().len().saturating_add(256), |bytes, item| {
            bytes
                .saturating_add(128)
                .saturating_add(item.path.len())
                .saturating_add(item.media_type.len())
        })
}

fn cache_fact_retained_bytes(fact: &PromptCacheFact) -> usize {
    match fact {
        PromptCacheFact::Planned(fact) => fact
            .capability_version
            .len()
            .saturating_add(fact.model_revision.map_or(0, str::len))
            .saturating_add(fact.policy_version.as_str().len())
            .saturating_add(fact.request_shape_version.len())
            .saturating_add(
                fact.policy
                    .namespace()
                    .map_or(0, |namespace| namespace.as_str().len()),
            )
            .saturating_add(1_024),
        PromptCacheFact::RequestEncoded(_) => 512,
        PromptCacheFact::UsageReported(fact) => fact
            .usage
            .details()
            .iter()
            .fold(1_024_usize, |bytes, detail| {
                bytes.saturating_add(detail.label.len()).saturating_add(16)
            })
            .saturating_add(fact.cost.pricing_version.map_or(0, str::len))
            .saturating_add(fact.cost.source_url.map_or(0, str::len))
            .saturating_add(
                fact.cost
                    .currency
                    .map_or(0, |currency| currency.as_str().len()),
            ),
        PromptCacheFact::ResourceChanged(fact) => fact.resource.as_str().len().saturating_add(512),
    }
}
