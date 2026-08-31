//! Owner-only, atomically replaced execution checkpoints.
//!
//! A checkpoint is neither the append-only conversation log nor extension
//! journal state. It is one bounded snapshot of unfinished execution, replaced
//! whole after each resolution/state transition and deleted when finished.

use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::sync::atomic::{AtomicU64, Ordering};

use crucible_core::{
    ActionId, ActionResolution, Ancestry, ApprovalDecision, CacheCheckpoint, CheckpointId,
    CheckpointStore, ExecutionCheckpoint, IdempotencyKey, InterruptionError, InvocationId,
    InvocationRecord, InvocationState, MAX_HUMAN_INPUT_BYTES, Modality, PendingAction,
    PendingActions, PendingApproval, PendingExternalTool, PendingHumanInput,
    PromptCacheFingerprint, PromptCacheResourceId, PromptCacheScopeDigest, ProviderAttemptId,
    ResumeDigest, ResumeScope, RunId, TOOL_RESULT_BYTES, ToolArgs, ToolCall, ToolEffect, ToolId,
    ToolOutcome, ToolOutput,
};
use serde_json::{Value, json};

/// Current execution-checkpoint document format.
pub const CHECKPOINT_FORMAT: u64 = 1;
/// Maximum encoded bytes in one checkpoint document.
pub const MAX_CHECKPOINT_BYTES: usize = 2 * 1024 * 1024;

const SUFFIX: &str = "checkpoint";

/// Why the protected checkpoint store could not complete an operation.
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    /// A private directory or file operation failed.
    #[error("could not {action} execution checkpoint: {source}")]
    Io {
        /// Operation that failed, without a sensitive path.
        action: &'static str,
        /// Operating-system failure.
        source: io::Error,
    },
    /// The document was malformed or used another format.
    #[error("execution checkpoint is not a readable format-{CHECKPOINT_FORMAT} document")]
    Unreadable,
    /// The encoded or on-disk document crossed its fixed ceiling.
    #[error("execution checkpoint exceeds its {MAX_CHECKPOINT_BYTES}-byte limit")]
    TooLarge,
    /// Typed core validation rejected restored state.
    #[error("execution checkpoint contains invalid state: {0}")]
    Invalid(#[from] InterruptionError),
}

/// A file-backed store rooted in the caller's dedicated checkpoint directory.
#[derive(Debug, Clone)]
pub struct FileCheckpointStore {
    directory: PathBuf,
}

impl FileCheckpointStore {
    /// Names a store without touching the filesystem.
    #[must_use]
    pub fn in_directory(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
        }
    }

    fn path(&self, id: CheckpointId) -> PathBuf {
        self.directory.join(format!("{id}.{SUFFIX}"))
    }

    fn private_directory(&self) -> Result<(), CheckpointError> {
        crucible_privacy::directory(&self.directory)
            .map_err(crucible_privacy::PrivacyError::into_io)
            .map_err(at("prepare the private directory"))
    }
}

impl CheckpointStore for FileCheckpointStore {
    type Error = CheckpointError;

    fn save(&mut self, checkpoint: &ExecutionCheckpoint) -> Result<(), Self::Error> {
        self.private_directory()?;
        let bytes = encode(checkpoint)?;
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(CheckpointError::TooLarge);
        }

        let destination = self.path(checkpoint.id());
        let mut temporary = Temporary::new(&self.directory)?;
        let file = temporary.file.as_mut().ok_or(CheckpointError::Unreadable)?;
        file.write_all(&bytes).map_err(at("write a replacement"))?;
        file.sync_all().map_err(at("sync a replacement"))?;
        drop(temporary.file.take());
        crucible_privacy::replace(&temporary.path, &destination)
            .map_err(crucible_privacy::PrivacyError::into_io)
            .map_err(at("replace the checkpoint"))?;
        temporary.landed = true;
        Ok(())
    }

    fn load(&self, id: CheckpointId) -> Result<Option<ExecutionCheckpoint>, Self::Error> {
        let path = self.path(id);
        match fs::metadata(&self.directory) {
            Ok(_) => self.private_directory()?,
            Err(problem) if problem.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(problem) => return Err(at("inspect the checkpoint directory")(problem)),
        }
        match crucible_privacy::tighten(&path) {
            Ok(_) => {}
            Err(problem) if problem.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(problem) => return Err(at("protect the checkpoint")(problem.into_io())),
        }
        let opened = crucible_privacy::open_read(&path)
            .map_err(crucible_privacy::PrivacyError::into_io)
            .map_err(at("open the checkpoint"))?;
        let mut bytes = Vec::new();
        opened
            .take(u64::try_from(MAX_CHECKPOINT_BYTES + 1).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes)
            .map_err(at("read the checkpoint"))?;
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(CheckpointError::TooLarge);
        }
        let checkpoint = decode(&bytes)?;
        if checkpoint.id() != id {
            return Err(CheckpointError::Unreadable);
        }
        Ok(Some(checkpoint))
    }

    fn remove(&mut self, id: CheckpointId) -> Result<(), Self::Error> {
        match fs::metadata(&self.directory) {
            Ok(_) => self.private_directory()?,
            Err(problem) if problem.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(problem) => return Err(at("inspect the checkpoint directory")(problem)),
        }
        let path = self.path(id);
        match crucible_privacy::open_read(&path) {
            Ok(opened) => drop(opened),
            Err(problem) if problem.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(problem) => return Err(at("validate the checkpoint")(problem.into_io())),
        }
        match fs::remove_file(&path) {
            Ok(()) => crucible_privacy::sync_parent(&path)
                .map_err(crucible_privacy::PrivacyError::into_io)
                .map_err(at("sync checkpoint removal")),
            Err(problem) if problem.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(problem) => Err(at("remove the checkpoint")(problem)),
        }
    }
}

fn encode(checkpoint: &ExecutionCheckpoint) -> Result<Vec<u8>, CheckpointError> {
    // Reject from the typed state before cloning it into a JSON value. The
    // writer below is the second boundary for escaping and structural bytes;
    // together they keep both retained state and encoded storage bounded.
    if retained_checkpoint_bytes(checkpoint) > MAX_CHECKPOINT_BYTES {
        return Err(CheckpointError::TooLarge);
    }
    let pending = checkpoint
        .pending()
        .entries()
        .map(|(action, resolution, completed)| {
            json!({
                "action": encode_action(action),
                "resolution": resolution.map(encode_resolution),
                "completed": completed,
            })
        })
        .collect::<Vec<_>>();
    let invocations = checkpoint
        .invocations()
        .iter()
        .map(encode_invocation)
        .collect::<Vec<_>>();
    let document = json!({
        "format": CHECKPOINT_FORMAT,
        "checkpoint": checkpoint.id().to_string(),
        "ancestry": encode_ancestry(checkpoint.ancestry()),
        "scope": encode_scope(checkpoint.scope()),
        "cache": checkpoint.cache().map(encode_cache),
        "created_at": checkpoint.created_at(),
        "expires_at": checkpoint.expires_at(),
        "pending": pending,
        "invocations": invocations,
    });
    let mut encoded = BoundedDocument::new();
    serde_json::to_writer(&mut encoded, &document).map_err(|problem| {
        if encoded.full {
            CheckpointError::TooLarge
        } else {
            let _ = problem;
            CheckpointError::Unreadable
        }
    })?;
    Ok(encoded.bytes)
}

fn retained_checkpoint_bytes(checkpoint: &ExecutionCheckpoint) -> usize {
    // Conservative structural allowance per record keeps the preflight cheap
    // and independent of serde_json's object spelling.
    let mut bytes = 2_048_usize;
    if let Some(cache) = checkpoint.cache() {
        bytes = bytes
            .saturating_add(cache.policy_version().len())
            .saturating_add(cache.capability_version().len())
            .saturating_add(cache.pricing_version().map_or(0, str::len))
            .saturating_add(cache.resource().map_or(0, |id| id.as_str().len()))
            .saturating_add(512);
    }
    for (action, resolution, _) in checkpoint.pending().entries() {
        bytes = bytes.saturating_add(1_024);
        if let Some(call) = action.call() {
            bytes = bytes
                .saturating_add(call.id.as_str().len())
                .saturating_add(call.name.len())
                .saturating_add(call.args.as_str().len());
        }
        if let PendingAction::HumanInput(human) = action {
            bytes = bytes.saturating_add(human.question().len());
        }
        if let Some(resolution) = resolution {
            bytes = bytes.saturating_add(retained_resolution_bytes(resolution));
        }
    }
    for invocation in checkpoint.invocations() {
        bytes = bytes
            .saturating_add(1_024)
            .saturating_add(invocation.call().id.as_str().len())
            .saturating_add(invocation.call().name.len())
            .saturating_add(invocation.call().args.as_str().len())
            .saturating_add(
                invocation
                    .idempotency_key()
                    .map_or(0, |key| key.as_str().len()),
            );
        if let InvocationState::Finished { output, .. } = invocation.state() {
            bytes = bytes.saturating_add(retained_output_bytes(output));
        }
    }
    bytes
}

fn retained_resolution_bytes(resolution: &ActionResolution) -> usize {
    match resolution {
        ActionResolution::ExternalTool(output) => retained_output_bytes(output),
        ActionResolution::HumanInput(answer) => answer.len(),
        ActionResolution::Approval(_) | ActionResolution::Cancelled => 64,
    }
}

fn retained_output_bytes(output: &ToolOutput) -> usize {
    output.attachments().iter().fold(
        output.text().len().saturating_add(256),
        |bytes, attachment| {
            bytes
                .saturating_add(256)
                .saturating_add(attachment.path.len())
                .saturating_add(attachment.media_type.len())
        },
    )
}

struct BoundedDocument {
    bytes: Vec<u8>,
    full: bool,
}

impl BoundedDocument {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(16 * 1_024),
            full: false,
        }
    }
}

impl io::Write for BoundedDocument {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > MAX_CHECKPOINT_BYTES {
            self.full = true;
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "execution checkpoint reached its encoded limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn decode(bytes: &[u8]) -> Result<ExecutionCheckpoint, CheckpointError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| CheckpointError::Unreadable)?;
    if value.get("format").and_then(Value::as_u64) != Some(CHECKPOINT_FORMAT) {
        return Err(CheckpointError::Unreadable);
    }
    let id = text(&value, "checkpoint")?.parse::<CheckpointId>()?;
    let ancestry = decode_ancestry(field(&value, "ancestry")?)?;
    let scope = decode_scope(field(&value, "scope")?)?;
    let cache = match field(&value, "cache")? {
        Value::Null => None,
        cache => Some(decode_cache(cache)?),
    };
    let created_at = number(&value, "created_at")?;
    let expires_at = number(&value, "expires_at")?;
    let mut checkpoint =
        ExecutionCheckpoint::new(id, ancestry, scope, cache, created_at, expires_at)?;

    let mut pending = PendingActions::new();
    for record in array(&value, "pending")? {
        let action = decode_action(field(record, "action")?)?;
        let resolution = match field(record, "resolution")? {
            Value::Null => None,
            resolution => Some(decode_resolution(resolution)?),
        };
        pending.restore(action, resolution, boolean(record, "completed")?)?;
    }
    checkpoint.set_pending(pending);
    for invocation in array(&value, "invocations")? {
        checkpoint.add_invocation(decode_invocation(invocation)?)?;
    }
    Ok(checkpoint)
}

fn encode_action(action: &PendingAction) -> Value {
    let common = json!({
        "id": action.id().to_string(),
        "ancestry": encode_ancestry(action.ancestry()),
        "expires_at": action.expires_at(),
    });
    let mut object = common.as_object().cloned().unwrap_or_default();
    match action {
        PendingAction::Approval(approval) => {
            object.insert("kind".to_owned(), json!("approval"));
            object.insert(
                "invocation".to_owned(),
                json!(approval.invocation().to_string()),
            );
            object.insert("call".to_owned(), encode_call(approval.call()));
        }
        PendingAction::ExternalTool(external) => {
            object.insert("kind".to_owned(), json!("external_tool"));
            object.insert(
                "invocation".to_owned(),
                json!(external.invocation().to_string()),
            );
            object.insert("call".to_owned(), encode_call(external.call()));
        }
        PendingAction::HumanInput(human) => {
            object.insert("kind".to_owned(), json!("human_input"));
            object.insert("question".to_owned(), json!(human.question()));
        }
    }
    Value::Object(object)
}

fn decode_action(value: &Value) -> Result<PendingAction, CheckpointError> {
    let id = text(value, "id")?.parse::<ActionId>()?;
    let ancestry = decode_ancestry(field(value, "ancestry")?)?;
    let expires_at = number(value, "expires_at")?;
    match text(value, "kind")? {
        "approval" => Ok(PendingAction::Approval(PendingApproval::restore(
            id,
            text(value, "invocation")?.parse::<InvocationId>()?,
            decode_call(field(value, "call")?)?,
            ancestry,
            expires_at,
        ))),
        "external_tool" => Ok(PendingAction::ExternalTool(PendingExternalTool::restore(
            id,
            text(value, "invocation")?.parse::<InvocationId>()?,
            decode_call(field(value, "call")?)?,
            ancestry,
            expires_at,
        ))),
        "human_input" => Ok(PendingAction::HumanInput(PendingHumanInput::restore(
            id,
            text(value, "question")?,
            ancestry,
            expires_at,
        )?)),
        _ => Err(CheckpointError::Unreadable),
    }
}

fn encode_resolution(resolution: &ActionResolution) -> Value {
    match resolution {
        ActionResolution::Approval(ApprovalDecision::Approved) => {
            json!({ "kind": "approval", "decision": "approved" })
        }
        ActionResolution::Approval(ApprovalDecision::Rejected) => {
            json!({ "kind": "approval", "decision": "rejected" })
        }
        ActionResolution::ExternalTool(output) => {
            json!({ "kind": "external_tool", "output": encode_output(output) })
        }
        ActionResolution::HumanInput(answer) => {
            json!({ "kind": "human_input", "answer": answer.as_ref() })
        }
        ActionResolution::Cancelled => json!({ "kind": "cancelled" }),
    }
}

fn decode_resolution(value: &Value) -> Result<ActionResolution, CheckpointError> {
    match text(value, "kind")? {
        "approval" => match text(value, "decision")? {
            "approved" => Ok(ActionResolution::Approval(ApprovalDecision::Approved)),
            "rejected" => Ok(ActionResolution::Approval(ApprovalDecision::Rejected)),
            _ => Err(CheckpointError::Unreadable),
        },
        "external_tool" => Ok(ActionResolution::ExternalTool(decode_output(field(
            value, "output",
        )?)?)),
        "human_input" => {
            let answer = text(value, "answer")?;
            if answer.is_empty() || answer.len() > MAX_HUMAN_INPUT_BYTES {
                return Err(CheckpointError::Unreadable);
            }
            Ok(ActionResolution::HumanInput(answer.into()))
        }
        "cancelled" => Ok(ActionResolution::Cancelled),
        _ => Err(CheckpointError::Unreadable),
    }
}

fn encode_invocation(invocation: &InvocationRecord) -> Value {
    json!({
        "id": invocation.id().to_string(),
        "call": encode_call(invocation.call()),
        "ancestry": encode_ancestry(invocation.ancestry()),
        "effect": effect(invocation.effect()),
        "idempotency_key": invocation.idempotency_key().map(IdempotencyKey::as_str),
        "state": match invocation.state() {
            InvocationState::Prepared => json!({ "kind": "prepared" }),
            InvocationState::Started => json!({ "kind": "started" }),
            InvocationState::Finished { outcome, output } => json!({
                "kind": "finished",
                "outcome": outcome_word(*outcome),
                "output": encode_output(output),
            }),
        },
    })
}

fn decode_invocation(value: &Value) -> Result<InvocationRecord, CheckpointError> {
    let state = field(value, "state")?;
    let state = match text(state, "kind")? {
        "prepared" => InvocationState::Prepared,
        "started" => InvocationState::Started,
        "finished" => InvocationState::Finished {
            outcome: parse_outcome(text(state, "outcome")?)?,
            output: decode_output(field(state, "output")?)?,
        },
        _ => return Err(CheckpointError::Unreadable),
    };
    let key = nullable_text(value, "idempotency_key")?
        .map(IdempotencyKey::new)
        .transpose()?;
    Ok(InvocationRecord::restore(
        text(value, "id")?.parse::<InvocationId>()?,
        decode_call(field(value, "call")?)?,
        decode_ancestry(field(value, "ancestry")?)?,
        parse_effect(text(value, "effect")?)?,
        key,
        state,
    ))
}

fn encode_call(call: &ToolCall) -> Value {
    json!({
        "id": call.id.as_str(),
        "name": call.name.as_ref(),
        "args": call.args.as_str(),
    })
}

fn decode_call(value: &Value) -> Result<ToolCall, CheckpointError> {
    Ok(ToolCall {
        id: ToolId::new(text(value, "id")?),
        name: text(value, "name")?.into(),
        args: ToolArgs::new(text(value, "args")?),
    })
}

fn encode_output(output: &ToolOutput) -> Value {
    json!({
        "text": output.text(),
        "failed": output.is_failed(),
        "changed": output.changed().map(|change| json!({
            "added": change.added(),
            "removed": change.removed(),
        })),
        "attachments": output.attachments().iter().map(|attachment| json!({
            "path": attachment.path.as_ref(),
            "modality": attachment.modality.as_str(),
            "media_type": attachment.media_type.as_ref(),
            "hash": hex(&attachment.hash),
        })).collect::<Vec<_>>(),
    })
}

fn decode_output(value: &Value) -> Result<ToolOutput, CheckpointError> {
    let mut output = if boolean(value, "failed")? {
        ToolOutput::failed(text(value, "text")?)
    } else {
        ToolOutput::ok(text(value, "text")?)
    };
    let mut retention_probe = output.clone();
    if retention_probe.limit_encoded(TOOL_RESULT_BYTES).omitted() > 0 {
        return Err(CheckpointError::TooLarge);
    }
    if let Some(changed) = nullable(value, "changed")? {
        output = output.counting(crucible_core::Changed::new(
            usize::try_from(number(changed, "added")?).map_err(|_| CheckpointError::Unreadable)?,
            usize::try_from(number(changed, "removed")?)
                .map_err(|_| CheckpointError::Unreadable)?,
        ));
    }
    let attachments = array(value, "attachments")?
        .iter()
        .map(|attachment| {
            Ok(crucible_core::Attachment {
                path: text(attachment, "path")?.into(),
                modality: Modality::from_str(text(attachment, "modality")?)
                    .map_err(|_| CheckpointError::Unreadable)?,
                media_type: text(attachment, "media_type")?.into(),
                hash: parse_hex(text(attachment, "hash")?)?,
            })
        })
        .collect::<Result<Vec<_>, CheckpointError>>()?;
    Ok(crate::session::restored_output(output, attachments))
}

fn encode_ancestry(ancestry: Ancestry) -> Value {
    json!({
        "run": ancestry.run().to_string(),
        "parent": ancestry.parent().map(|id| id.to_string()),
        "root": ancestry.root().to_string(),
        "depth": ancestry.depth(),
    })
}

fn decode_ancestry(value: &Value) -> Result<Ancestry, CheckpointError> {
    let run = RunId::parse(text(value, "run")?).map_err(|_| CheckpointError::Unreadable)?;
    let parent = nullable_text(value, "parent")?
        .map(RunId::parse)
        .transpose()
        .map_err(|_| CheckpointError::Unreadable)?;
    let root = RunId::parse(text(value, "root")?).map_err(|_| CheckpointError::Unreadable)?;
    let depth = u16::try_from(number(value, "depth")?).map_err(|_| CheckpointError::Unreadable)?;
    Ancestry::restore(run, parent, root, depth).map_err(|_| CheckpointError::Unreadable)
}

fn encode_scope(scope: ResumeScope) -> Value {
    json!({
        "endpoint": hex(&scope.endpoint().bytes()),
        "model": hex(&scope.model().bytes()),
        "credential": hex(&scope.credential().bytes()),
        "authority": hex(&scope.authority().bytes()),
    })
}

fn decode_scope(value: &Value) -> Result<ResumeScope, CheckpointError> {
    Ok(ResumeScope::new(
        ResumeDigest::new(parse_hex(text(value, "endpoint")?)?),
        ResumeDigest::new(parse_hex(text(value, "model")?)?),
        ResumeDigest::new(parse_hex(text(value, "credential")?)?),
        ResumeDigest::new(parse_hex(text(value, "authority")?)?),
    ))
}

fn encode_cache(cache: &CacheCheckpoint) -> Value {
    json!({
        "policy_version": cache.policy_version(),
        "capability_version": cache.capability_version(),
        "pricing_version": cache.pricing_version(),
        "scope": hex(&cache.scope().bytes()),
        "prefix": hex(&cache.prefix().bytes()),
        "attempt": cache.attempt().map(|id| id.to_string()),
        "resource": cache.resource().map(PromptCacheResourceId::as_str),
        "expires_at": cache.expires_at(),
        "reconcile": cache.requires_reconciliation(),
    })
}

fn decode_cache(value: &Value) -> Result<CacheCheckpoint, CheckpointError> {
    let pricing: Option<Box<str>> = nullable_text(value, "pricing_version")?.map(Into::into);
    let attempt = nullable_text(value, "attempt")?
        .map(ProviderAttemptId::parse)
        .transpose()
        .map_err(|_| CheckpointError::Unreadable)?;
    let resource = nullable_text(value, "resource")?
        .map(PromptCacheResourceId::parse)
        .transpose()
        .map_err(|_| CheckpointError::Unreadable)?;
    CacheCheckpoint::new(
        text(value, "policy_version")?,
        text(value, "capability_version")?,
        pricing,
        PromptCacheScopeDigest::new(parse_hex(text(value, "scope")?)?),
        PromptCacheFingerprint::new(parse_hex(text(value, "prefix")?)?),
        attempt,
        resource,
        nullable_number(value, "expires_at")?,
        boolean(value, "reconcile")?,
    )
    .map_err(Into::into)
}

fn effect(effect: ToolEffect) -> &'static str {
    match effect {
        ToolEffect::ReadOnly => "read_only",
        ToolEffect::Idempotent => "idempotent",
        ToolEffect::NonIdempotent => "non_idempotent",
    }
}

fn parse_effect(value: &str) -> Result<ToolEffect, CheckpointError> {
    match value {
        "read_only" => Ok(ToolEffect::ReadOnly),
        "idempotent" => Ok(ToolEffect::Idempotent),
        "non_idempotent" => Ok(ToolEffect::NonIdempotent),
        _ => Err(CheckpointError::Unreadable),
    }
}

fn outcome_word(outcome: ToolOutcome) -> &'static str {
    match outcome {
        ToolOutcome::Succeeded => "succeeded",
        ToolOutcome::Failed => "failed",
        ToolOutcome::Forbidden => "forbidden",
        ToolOutcome::Refused => "refused",
        ToolOutcome::Cancelled => "cancelled",
        ToolOutcome::TimedOut => "timed_out",
        ToolOutcome::Rejected => "rejected",
        ToolOutcome::NotRun => "not_run",
        ToolOutcome::OutputLimit => "output_limit",
        ToolOutcome::Panicked => "panicked",
    }
}

fn parse_outcome(value: &str) -> Result<ToolOutcome, CheckpointError> {
    match value {
        "succeeded" => Ok(ToolOutcome::Succeeded),
        "failed" => Ok(ToolOutcome::Failed),
        "forbidden" => Ok(ToolOutcome::Forbidden),
        "refused" => Ok(ToolOutcome::Refused),
        "cancelled" => Ok(ToolOutcome::Cancelled),
        "timed_out" => Ok(ToolOutcome::TimedOut),
        "rejected" => Ok(ToolOutcome::Rejected),
        "not_run" => Ok(ToolOutcome::NotRun),
        "output_limit" => Ok(ToolOutcome::OutputLimit),
        "panicked" => Ok(ToolOutcome::Panicked),
        _ => Err(CheckpointError::Unreadable),
    }
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a Value, CheckpointError> {
    value.get(name).ok_or(CheckpointError::Unreadable)
}

fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str, CheckpointError> {
    field(value, name)?
        .as_str()
        .ok_or(CheckpointError::Unreadable)
}

fn number(value: &Value, name: &str) -> Result<u64, CheckpointError> {
    field(value, name)?
        .as_u64()
        .ok_or(CheckpointError::Unreadable)
}

fn boolean(value: &Value, name: &str) -> Result<bool, CheckpointError> {
    field(value, name)?
        .as_bool()
        .ok_or(CheckpointError::Unreadable)
}

fn array<'a>(value: &'a Value, name: &str) -> Result<&'a [Value], CheckpointError> {
    field(value, name)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or(CheckpointError::Unreadable)
}

fn nullable<'a>(value: &'a Value, name: &str) -> Result<Option<&'a Value>, CheckpointError> {
    match field(value, name)? {
        Value::Null => Ok(None),
        present => Ok(Some(present)),
    }
}

fn nullable_text<'a>(value: &'a Value, name: &str) -> Result<Option<&'a str>, CheckpointError> {
    nullable(value, name)?
        .map(|present| present.as_str().ok_or(CheckpointError::Unreadable))
        .transpose()
}

fn nullable_number(value: &Value, name: &str) -> Result<Option<u64>, CheckpointError> {
    nullable(value, name)?
        .map(|present| present.as_u64().ok_or(CheckpointError::Unreadable))
        .transpose()
}

fn hex(bytes: &[u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn parse_hex(value: &str) -> Result<[u8; 32], CheckpointError> {
    if value.len() != 64 {
        return Err(CheckpointError::Unreadable);
    }
    let mut output = [0_u8; 32];
    let bytes = value.as_bytes();
    for (index, slot) in output.iter_mut().enumerate() {
        let at = index * 2;
        let high = nibble(*bytes.get(at).ok_or(CheckpointError::Unreadable)?)?;
        let low = nibble(*bytes.get(at + 1).ok_or(CheckpointError::Unreadable)?)?;
        *slot = (high << 4) | low;
    }
    Ok(output)
}

fn nibble(value: u8) -> Result<u8, CheckpointError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(CheckpointError::Unreadable),
    }
}

fn at(action: &'static str) -> impl FnOnce(io::Error) -> CheckpointError {
    move |source| CheckpointError::Io { action, source }
}

/// One exclusively created replacement removed unless its rename lands.
#[derive(Debug)]
struct Temporary {
    path: PathBuf,
    file: Option<File>,
    landed: bool,
}

impl Temporary {
    fn new(directory: &Path) -> Result<Self, CheckpointError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..32 {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(
                ".checkpoint.{}.{sequence}.writing",
                std::process::id()
            ));
            match crucible_privacy::create_write(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                        landed: false,
                    });
                }
                Err(problem) if problem.kind() == io::ErrorKind::AlreadyExists => {}
                Err(problem) => {
                    return Err(at("create a replacement")(problem.into_io()));
                }
            }
        }
        Err(at("create a replacement")(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no free sibling checkpoint name",
        )))
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.landed {
            let _ = fs::remove_file(&self.path);
        }
    }
}
