//! Framework history kept beside, but not confused with, provider messages.
//!
//! A provider receives the deliberately closed [`Message`] vocabulary. The
//! framework needs a wider history for attempts, interruptions, invocation
//! recovery, and extension state, so those records live as [`RunItem`]s and
//! cross the provider boundary only through an explicit projection.
//!
//! The format-neutral half of this model — keys, receipts, extension entries
//! and the conversation-writing seam — belongs to `crucible-storage`. What
//! stays here is what carries an execution fact: a prompt-cache attempt or a
//! sandbox lifecycle a store cannot describe without the runtime that made it.

use std::collections::BTreeMap;
use std::fmt;

use crucible_storage::{
    CallResultKey, CallResultReceipt, CallResultStoreError, CompactionRecord, CustomEntry,
    CustomProjector, InvocationRecord, InvocationState, JournalError, PendingAction,
};
use crucible_types::{
    Ancestry, Diff, Message, RecordedToolOutput, StopReason, TOOL_ARGUMENT_BYTES,
    TOOL_CALL_ID_BYTES, TOOL_NAME_BYTES, TOOL_RESULT_BYTES, ToolCall, ToolId, ToolResult,
    Transcript,
};

use crate::{PromptCacheFact, SandboxFact};

/// Most framework records retained in one in-memory history.
pub const MAX_RUN_ITEMS: usize = 4_096;
/// Most encoded bytes retained for one framework journal record.
pub const MAX_RUN_ITEM_BYTES: usize = 2 * 1_024 * 1_024;
/// Most in-memory bytes retained by one framework item.
pub const MAX_RUN_ITEM_RETAINED_BYTES: usize = 20 * 1_024 * 1_024;
/// Most in-memory bytes retained by one framework history.
pub const MAX_RUN_HISTORY_BYTES: usize = 64 * 1_024 * 1_024;

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
    Invocation {
        /// The call, its state and whatever result was recorded for it.
        record: InvocationRecord,
        /// The change lines a rewrite showed, for the reader alone.
        ///
        /// Beside the record rather than inside it, because the model was
        /// never sent them: a recorded result carrying a preview would say
        /// different things to the two readers of one call. The protected
        /// display journal keeps them so a replay draws the row a reader
        /// watched; no provider projection ever reaches them.
        preview: Option<Diff>,
    },
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
            Self::Invocation { record, .. } => record.ancestry(),
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
            | Self::Invocation { .. }
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
            Self::Invocation { record, .. } => {
                validate_call(record.call())?;
                if let InvocationState::Finished { output, .. } = record.state()
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
            Self::Invocation { record, preview } => f
                .debug_struct("Invocation")
                .field("record", record)
                .field("preview", preview)
                .finish(),
            Self::Compaction(compaction) => f.debug_tuple("Compaction").field(compaction).finish(),
            Self::Custom(entry) => f.debug_tuple("Custom").field(entry).finish(),
        }
    }
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
                | RunItem::Invocation { .. }
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
                if !matches!(stop, Some(StopReason::Yielded | StopReason::WantsTools)) {
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

fn validate_call(call: &ToolCall) -> Result<(), JournalError> {
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
            JournalError::check_word("cache capability version", fact.capability_version)?;
            if let Some(revision) = fact.model_revision {
                JournalError::check_word("cache model revision", revision)?;
            }
            JournalError::check_word("cache policy version", fact.policy_version.as_str())?;
            JournalError::check_word("cache request shape version", fact.request_shape_version)
        }
        PromptCacheFact::UsageReported(fact) => {
            if let Some(version) = fact.cost.pricing_version {
                JournalError::check_word("cache pricing version", version)?;
            }
            if let Some(source) = fact.cost.source_url {
                JournalError::check_word("cache pricing source", source)?;
            }
            Ok(())
        }
        PromptCacheFact::RequestEncoded(_) | PromptCacheFact::ResourceChanged(_) => Ok(()),
    }
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
        RunItem::Invocation { record, .. } => {
            let state = match record.state() {
                InvocationState::Finished { output, .. } => output_retained_bytes(output),
                InvocationState::Prepared | InvocationState::Started => 0,
            };
            // The preview is charged nowhere, exactly as it was charged nowhere
            // when it travelled inside the output this walks. What bounds it is
            // its own shape -- a fixed line count of fixed-width lines -- not
            // this ceiling.
            call_retained_bytes(record.call())
                .saturating_add(state)
                .saturating_add(record.idempotency_key().map_or(0, |key| key.as_str().len()))
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

fn call_retained_bytes(call: &ToolCall) -> usize {
    call.id
        .as_str()
        .len()
        .saturating_add(call.name.len())
        .saturating_add(call.args.as_str().len())
}

fn output_retained_bytes(output: &RecordedToolOutput) -> usize {
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
