//! Owned tool descriptions and immutable advertised rosters.
//!
//! A tool executor is behavior. Its name, provider schema, source, and
//! scheduling controls are configuration, so they travel separately in a
//! [`ToolDescriptor`]. The split lets a descriptor come from a file, process,
//! or other runtime source without leaking strings to obtain a `'static`
//! lifetime, and lets one [`Tool`] executor be shared by several snapshots.
//!
//! A [`ToolSnapshot`] is the one roster a provider request was admitted
//! against. It owns no executor outright: descriptors and executors are shared
//! through [`Arc`], while the ordered slice that says which ones are visible
//! is immutable. A returned call is resolved against that same value, never a
//! live registry that may have refreshed while the provider was answering.

use std::fmt;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{
    Ancestry, Approved, Cancel, RunId, Sensitivity, TOOL_RESULT_MIN_BYTES, Tool, ToolArgs,
    ToolCall, ToolEffect, ToolError, ToolOutput, ToolOutputRetention, ToolSchema,
};

/// The most bytes a provider-visible tool name may retain.
///
/// Equal to the existing inbound call-name boundary. A descriptor the provider
/// can be shown but whose returned name the runner would refuse is not a usable
/// descriptor.
pub const TOOL_NAME_BYTES: usize = 4 * 1024;

/// The most bytes retained for one provider tool-call identifier.
pub const TOOL_CALL_ID_BYTES: usize = 16 * 1024;

/// The most bytes retained for one tool call's argument text.
pub const TOOL_ARGUMENT_BYTES: usize = 1024 * 1024;

/// The most bytes one exact provider schema may retain.
pub const TOOL_SCHEMA_BYTES: usize = 1024 * 1024;

/// The most bytes in a stable, receipt-safe source identifier.
pub const TOOL_SOURCE_ID_BYTES: usize = crate::registry::SOURCE_ID_BYTES;

/// The most bytes in the diagnostic spelling of a source.
pub const TOOL_SOURCE_LABEL_BYTES: usize = crate::registry::SOURCE_LABEL_BYTES;

/// The most bytes in one resource-exclusion key.
pub const TOOL_RESOURCE_KEY_BYTES: usize = 4 * 1024;

/// The most visible/reachable entries one immutable snapshot retains.
pub const TOOL_SNAPSHOT_ENTRIES: usize = 1024;

/// The aggregate descriptor bytes one immutable snapshot retains.
pub const TOOL_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

/// The run-scoped capabilities a live toolset may use while materializing.
///
/// Deliberately narrower than the runner's context: discovery and cleanup can
/// observe cancellation and a monotonic deadline, but cannot emit arbitrary
/// run events, steer the agent, mutate permission state, or reach a session.
#[derive(Clone)]
pub struct ToolsetContext {
    ancestry: Ancestry,
    cancel: Cancel,
    deadline: Option<Instant>,
}

impl ToolsetContext {
    /// Builds the context for one toolset lifecycle.
    #[must_use]
    pub fn new(ancestry: Ancestry, cancel: Cancel, deadline: Option<Instant>) -> Self {
        Self {
            ancestry,
            cancel,
            deadline,
        }
    }

    /// The run this materialization belongs to.
    #[must_use]
    pub const fn run(&self) -> RunId {
        self.ancestry.run()
    }

    /// The run and its parentage.
    #[must_use]
    pub const fn ancestry(&self) -> Ancestry {
        self.ancestry
    }

    /// A cancellation handle scoped to this lifecycle.
    #[must_use]
    pub const fn cancel(&self) -> &Cancel {
        &self.cancel
    }

    /// The monotonic time by which preparation or refresh should finish.
    #[must_use]
    pub const fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

impl fmt::Debug for ToolsetContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolsetContext")
            .field("ancestry", &self.ancestry)
            .field("cancelled", &self.cancel.requested())
            .field("deadline", &self.deadline)
            .finish()
    }
}

/// A live, reusable source of immutable tool generations.
///
/// One lifecycle is prepared for a run, snapshotted for its first provider
/// admission, optionally refreshed only between later admissions, and then
/// disposed. Implementations must make [`Toolset::dispose`] idempotent,
/// including when preparation or refresh failed. A later run may prepare the
/// same toolset again after disposal.
pub trait Toolset: Send + Sync {
    /// Acquires the resources this run needs before its first snapshot.
    ///
    /// # Errors
    ///
    /// [`ToolsetError`] when the source cannot be prepared. The caller still
    /// invokes [`Toolset::dispose`].
    fn prepare(&self, context: &ToolsetContext) -> Result<(), ToolsetError>;

    /// Captures the current ordered, visible, and reachable generation.
    ///
    /// # Errors
    ///
    /// [`ToolsetError`] when no bounded coherent snapshot can be produced.
    fn snapshot(&self, context: &ToolsetContext) -> Result<ToolSnapshot, ToolsetError>;

    /// Refreshes external state between admissions and captures its new view.
    ///
    /// An earlier [`ToolSnapshot`] remains immutable and valid for calls it
    /// already admitted; this result governs only later admissions.
    ///
    /// # Errors
    ///
    /// [`ToolsetError`] when refresh or materialization fails.
    fn refresh(&self, context: &ToolsetContext) -> Result<ToolSnapshot, ToolsetError>;

    /// Releases every resource acquired for the lifecycle.
    ///
    /// This must be safe to call after any earlier failure and more than once.
    ///
    /// # Errors
    ///
    /// [`ToolsetError`] when cleanup could not be completed. A repeated call
    /// must not repeat an effect merely to reproduce the error.
    fn dispose(&self, context: &ToolsetContext) -> Result<(), ToolsetError>;
}

impl fmt::Debug for dyn Toolset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Toolset([live source])")
    }
}

/// Where a tool registration came from: the registry's shared source kind.
pub type ToolSourceKind = crate::registry::SourceKind;

/// The receipt-safe projection of a tool registration source.
pub type ToolSourceReceipt = crate::registry::SourceReceipt;

/// A tool registration's bounded source identity and diagnostic spelling.
///
/// Tools were the first registry; every later contribution kind shares this
/// record, so it is the registry's [`Provenance`](crate::Provenance) and keeps
/// its tool name here.
pub type ToolProvenance = crate::registry::Provenance;

/// A bounded key whose calls may not overlap.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolResourceKey(Box<str>);

impl ToolResourceKey {
    /// Takes one resource name.
    ///
    /// # Errors
    ///
    /// [`ToolDescriptorError`] when the key is empty or too large.
    pub fn new(key: impl Into<Box<str>>) -> Result<Self, ToolDescriptorError> {
        let key = key.into();
        bounded("tool resource key", &key, TOOL_RESOURCE_KEY_BYTES)?;
        Ok(Self(key))
    }

    /// The exact resource name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ToolResourceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ToolResourceKey([redacted])")
    }
}

/// Whether and with what other calls one invocation may execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionMode {
    /// A barrier: every preceding call finishes before this starts, and every
    /// following call waits for it.
    Sequential,
    /// May overlap other admitted parallel work within the run ceiling.
    Parallel,
    /// May overlap except with another call holding the same key.
    Exclusive(ToolResourceKey),
}

/// Static, owned configuration for one tool executor.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolDescriptor {
    name: Box<str>,
    schema: Box<str>,
    provenance: ToolProvenance,
    execution: ToolExecutionMode,
    effect: ToolEffect,
    timeout: Option<Duration>,
    result_bytes: Option<NonZeroUsize>,
}

impl ToolDescriptor {
    /// Builds the smallest descriptor, executing sequentially with no
    /// descriptor-local timeout or result ceiling.
    ///
    /// # Errors
    ///
    /// [`ToolDescriptorError`] when the name or schema is empty or beyond its
    /// retained boundary.
    pub fn new(
        name: impl Into<Box<str>>,
        schema: impl Into<Box<str>>,
        provenance: ToolProvenance,
    ) -> Result<Self, ToolDescriptorError> {
        let name = name.into();
        bounded("tool name", &name, TOOL_NAME_BYTES)?;
        let schema = schema.into();
        bounded("tool schema", &schema, TOOL_SCHEMA_BYTES)?;

        Ok(Self {
            name,
            schema,
            provenance,
            execution: ToolExecutionMode::Sequential,
            effect: ToolEffect::NonIdempotent,
            timeout: None,
            result_bytes: None,
        })
    }

    /// Sets the call scheduling mode.
    #[must_use]
    pub fn executing(mut self, execution: ToolExecutionMode) -> Self {
        self.execution = execution;
        self
    }

    /// Classifies what an ambiguous started invocation may have changed.
    ///
    /// The conservative default is [`ToolEffect::NonIdempotent`]. A tool opts
    /// into retry only when its executor contract can justify it.
    #[must_use]
    pub const fn causing(mut self, effect: ToolEffect) -> Self {
        self.effect = effect;
        self
    }

    /// Applies a cooperative per-call timeout.
    ///
    /// # Errors
    ///
    /// [`ToolDescriptorError::ZeroTimeout`] for a zero interval, which would
    /// admit a call only to cancel it before it can observe its context.
    pub fn timing_out_after(mut self, timeout: Duration) -> Result<Self, ToolDescriptorError> {
        if timeout.is_zero() {
            return Err(ToolDescriptorError::ZeroTimeout);
        }
        self.timeout = Some(timeout);
        Ok(self)
    }

    /// Applies a descriptor-local retained-result ceiling.
    ///
    /// # Errors
    ///
    /// [`ToolDescriptorError::ResultBytesTooSmall`] when the allowance cannot
    /// retain the model-visible elision note every bounded result promises.
    pub fn limiting_result_to(mut self, bytes: usize) -> Result<Self, ToolDescriptorError> {
        if bytes < TOOL_RESULT_MIN_BYTES {
            return Err(ToolDescriptorError::ResultBytesTooSmall {
                minimum: TOOL_RESULT_MIN_BYTES,
                actual: bytes,
            });
        }
        self.result_bytes = NonZeroUsize::new(bytes);
        Ok(self)
    }

    /// The exact provider-visible name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The exact raw schema the provider adapter projects.
    #[must_use]
    pub fn schema(&self) -> &str {
        &self.schema
    }

    /// Where this registration came from.
    #[must_use]
    pub const fn provenance(&self) -> &ToolProvenance {
        &self.provenance
    }

    /// How calls may overlap.
    #[must_use]
    pub const fn execution(&self) -> &ToolExecutionMode {
        &self.execution
    }

    /// Crash-recovery effect classification.
    #[must_use]
    pub const fn effect(&self) -> ToolEffect {
        self.effect
    }

    /// Its cooperative timeout, where one was set.
    #[must_use]
    pub const fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// Its retained-result ceiling, where one was set.
    #[must_use]
    pub const fn result_bytes(&self) -> Option<usize> {
        match self.result_bytes {
            Some(bytes) => Some(bytes.get()),
            None => None,
        }
    }

    /// The bytes this descriptor keeps alive, for a roster's ceiling.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.name
            .len()
            .saturating_add(self.schema.len())
            .saturating_add(self.provenance.retained_bytes())
            .saturating_add(match &self.execution {
                ToolExecutionMode::Exclusive(key) => key.as_str().len(),
                ToolExecutionMode::Sequential | ToolExecutionMode::Parallel => 0,
            })
    }

    /// The borrowed projection a provider serializes.
    #[must_use]
    pub fn advertised(&self) -> ToolSchema<'_> {
        ToolSchema {
            name: self.name(),
            schema: self.schema(),
        }
    }
}

impl fmt::Debug for ToolDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolDescriptor")
            .field("name", &self.name)
            .field("schema", &"[redacted]")
            .field("provenance", &self.provenance)
            .field("execution", &self.execution)
            .field("effect", &self.effect)
            .field("timeout", &self.timeout)
            .field("result_bytes", &self.result_bytes)
            .finish()
    }
}

/// Static metadata supplied separately from a built-in executor.
///
/// Runtime toolsets may construct a [`ToolDescriptor`] directly. This trait is
/// the convenience for compiled tools whose names and schemas are constants:
/// it keeps those methods off [`Tool`], so a snapshot never has to ask an
/// executor what was registered beside it.
pub trait DescribeTool {
    /// The name placed into an owned descriptor at registration.
    fn name(&self) -> &str;

    /// The raw schema placed into an owned descriptor at registration.
    fn schema(&self) -> &str;

    /// Crash-recovery effect class for this executor contract.
    fn effect(&self) -> ToolEffect {
        ToolEffect::NonIdempotent
    }

    /// Builds the owned descriptor for `provenance`.
    ///
    /// # Errors
    ///
    /// [`ToolDescriptorError`] when any bounded field is invalid.
    fn descriptor(
        &self,
        provenance: ToolProvenance,
    ) -> Result<ToolDescriptor, ToolDescriptorError> {
        ToolDescriptor::new(self.name(), self.schema(), provenance)
            .map(|descriptor| descriptor.causing(self.effect()))
    }
}

/// The only pre-validation middleware: it may replace call arguments.
pub trait ArgumentTransform: Send + Sync {
    /// Produces the arguments that must be revalidated and authorized.
    ///
    /// # Errors
    ///
    /// [`ToolError`] when no safe transformed call can be produced.
    fn transform(&self, call: &ToolCall) -> Result<ToolArgs, ToolError>;
}

/// A guardrail over the final validated input and recomputed sensitivity.
pub trait InputGuard: Send + Sync {
    /// Refuses input before human approval or execution.
    ///
    /// # Errors
    ///
    /// [`ToolError`] with the model-facing reason for refusal.
    fn guard(&self, call: &ToolCall, sensitivity: &Sensitivity) -> Result<(), ToolError>;
}

/// A guardrail over executor output before any result is retained.
pub trait OutputGuard: Send + Sync {
    /// Returns the output safe to retain, or refuses it.
    ///
    /// # Errors
    ///
    /// [`ToolError`] with the model-facing reason for refusal.
    fn guard(&self, call: &ToolCall, output: ToolOutput) -> Result<ToolOutput, ToolError>;
}

/// The three supported invocation hook points for one registration.
#[derive(Clone, Default)]
pub struct ToolHooks {
    argument: Option<Arc<dyn ArgumentTransform>>,
    input: Option<Arc<dyn InputGuard>>,
    output: Option<Arc<dyn OutputGuard>>,
}

impl ToolHooks {
    /// No middleware or guardrails.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs the one argument-transform hook.
    #[must_use]
    pub fn transforming(mut self, transform: Arc<dyn ArgumentTransform>) -> Self {
        self.argument = Some(transform);
        self
    }

    /// Installs the one final-input guard.
    #[must_use]
    pub fn guarding_input(mut self, guard: Arc<dyn InputGuard>) -> Self {
        self.input = Some(guard);
        self
    }

    /// Installs the one pre-retention output guard.
    #[must_use]
    pub fn guarding_output(mut self, guard: Arc<dyn OutputGuard>) -> Self {
        self.output = Some(guard);
        self
    }

    /// The argument transform, where this registration has one.
    #[must_use]
    pub fn argument(&self) -> Option<&dyn ArgumentTransform> {
        self.argument.as_deref()
    }

    /// The input guard, where this registration has one.
    #[must_use]
    pub fn input(&self) -> Option<&dyn InputGuard> {
        self.input.as_deref()
    }

    /// The output guard, where this registration has one.
    #[must_use]
    pub fn output(&self) -> Option<&dyn OutputGuard> {
        self.output.as_deref()
    }
}

impl fmt::Debug for ToolHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolHooks")
            .field("argument", &self.argument.is_some())
            .field("input", &self.input.is_some())
            .field("output", &self.output.is_some())
            .finish()
    }
}

/// One descriptor bound to the executor it configures.
#[derive(Clone)]
pub struct ToolEntry {
    descriptor: Arc<ToolDescriptor>,
    tool: Arc<dyn Tool>,
    hooks: ToolHooks,
}

impl ToolEntry {
    /// Binds one owned descriptor to one shareable executor.
    #[must_use]
    pub fn new(descriptor: ToolDescriptor, tool: Arc<dyn Tool>) -> Self {
        Self::with_hooks(descriptor, tool, ToolHooks::new())
    }

    /// Binds configuration, executor, and the three invocation hook points.
    #[must_use]
    pub fn with_hooks(descriptor: ToolDescriptor, tool: Arc<dyn Tool>, hooks: ToolHooks) -> Self {
        Self {
            descriptor: Arc::new(descriptor),
            tool,
            hooks,
        }
    }

    /// The immutable configuration.
    #[must_use]
    pub fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    /// The executor the descriptor names.
    #[must_use]
    pub fn tool(&self) -> &dyn Tool {
        self.tool.as_ref()
    }

    /// A shared handle to the executor.
    #[must_use]
    pub fn shared_tool(&self) -> Arc<dyn Tool> {
        Arc::clone(&self.tool)
    }

    /// Middleware and guardrails bound to this exact registration.
    #[must_use]
    pub const fn hooks(&self) -> &ToolHooks {
        &self.hooks
    }
}

impl fmt::Debug for ToolEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolEntry")
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}

/// Equality identity and model-visible label for one materialized roster.
///
/// Equality stays pointer-based: the label is evidence a context section can
/// report, never an authorization token and never a second way to resolve an
/// admitted call. Its UUID only prevents a resumed process from accidentally
/// describing a different first generation with the same persisted label.
#[derive(Clone)]
pub struct ToolGeneration(Arc<Generation>);

struct Generation {
    context: Box<str>,
}

impl ToolGeneration {
    fn new() -> Self {
        Self(Arc::new(Generation {
            context: uuid::Uuid::now_v7().to_string().into(),
        }))
    }

    /// The opaque label safe for the model-visible tool advertisement.
    ///
    /// This is descriptive evidence only. Invocation still requires an
    /// admission and [`Approved`] proof bound to this value's pointer identity.
    #[must_use]
    pub fn context_id(&self) -> &str {
        &self.0.context
    }
}

/// A name admitted from one exact immutable generation.
///
/// Constructed only by [`ToolSnapshot::admit`]. It is intentionally not an
/// executor handle: resolving it through a snapshot rechecks the opaque
/// generation before any tool behavior becomes reachable.
#[derive(Debug, Clone)]
pub struct ToolAdmission {
    generation: ToolGeneration,
    index: usize,
    call: ToolCall,
}

impl ToolAdmission {
    /// The generation permission approval must remain bound to.
    #[must_use]
    pub const fn generation(&self) -> &ToolGeneration {
        &self.generation
    }

    /// The exact admitted provider-visible name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.call.name
    }

    pub(crate) const fn call(&self) -> &ToolCall {
        &self.call
    }
}

impl PartialEq for ToolGeneration {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ToolGeneration {}

impl fmt::Debug for ToolGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ToolGeneration([opaque])")
    }
}

/// The closed final state recorded for one tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutcome {
    /// The executor returned a successful result.
    Succeeded,
    /// The executor or one of its pipeline stages failed.
    Failed,
    /// Standing permission policy forbade the call.
    Forbidden,
    /// The user refused the call.
    Refused,
    /// The run was cancelled before the call could finish.
    Cancelled,
    /// The descriptor's cooperative deadline elapsed.
    TimedOut,
    /// The call was invalid, unknown, or stale before effects.
    Rejected,
    /// An earlier call ended the turn before this call could run.
    NotRun,
    /// The per-turn retained-output allowance was exhausted.
    OutputLimit,
    /// The executor panicked and was contained by the scheduler.
    Panicked,
}

/// Bounded audit and usage evidence emitted with one final result.
#[derive(Clone)]
pub struct ToolReceipt {
    generation: ToolGeneration,
    source: Option<ToolSourceReceipt>,
    input_bytes: usize,
    output: ToolOutputRetention,
    outcome: ToolOutcome,
}

impl ToolReceipt {
    /// Records the exact generation, safe source, retained sizes, and outcome.
    #[must_use]
    pub fn new(
        generation: ToolGeneration,
        source: Option<ToolSourceReceipt>,
        input_bytes: usize,
        output: ToolOutputRetention,
        outcome: ToolOutcome,
    ) -> Self {
        Self {
            generation,
            source,
            input_bytes,
            output,
            outcome,
        }
    }

    /// The immutable roster this call was answered against.
    #[must_use]
    pub const fn generation(&self) -> &ToolGeneration {
        &self.generation
    }

    /// The non-sensitive registration source, where the name resolved.
    #[must_use]
    pub const fn source(&self) -> Option<&ToolSourceReceipt> {
        self.source.as_ref()
    }

    /// Bytes in the final arguments authorized for execution.
    #[must_use]
    pub const fn input_bytes(&self) -> usize {
        self.input_bytes
    }

    /// Encoded output sizes before and after the per-result limiter.
    #[must_use]
    pub const fn output(&self) -> ToolOutputRetention {
        self.output
    }

    /// The one closed final state.
    #[must_use]
    pub const fn outcome(&self) -> ToolOutcome {
        self.outcome
    }
}

impl fmt::Debug for ToolReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolReceipt")
            .field("generation", &self.generation)
            .field("source", &self.source)
            .field("input_bytes", &self.input_bytes)
            .field("output", &self.output)
            .field("outcome", &self.outcome)
            .finish()
    }
}

/// One immutable, provider-ordered set of visible and reachable tools.
#[derive(Debug, Clone)]
pub struct ToolSnapshot {
    generation: ToolGeneration,
    entries: Arc<[ToolEntry]>,
}

impl ToolSnapshot {
    /// An empty generation.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            generation: ToolGeneration::new(),
            entries: Arc::new([]),
        }
    }

    /// Materializes one bounded generation.
    ///
    /// # Errors
    ///
    /// [`ToolsetError`] if the snapshot is too large or contains two entries
    /// whose provider-visible names are one name to a permission rule, which
    /// reads them without case.
    pub fn new(entries: impl IntoIterator<Item = ToolEntry>) -> Result<Self, ToolsetError> {
        let entries: Vec<ToolEntry> = entries.into_iter().collect();
        if entries.len() > TOOL_SNAPSHOT_ENTRIES {
            return Err(ToolsetError::Entries {
                maximum: TOOL_SNAPSHOT_ENTRIES,
                actual: entries.len(),
            });
        }

        let mut retained = 0_usize;
        for (at, entry) in entries.iter().enumerate() {
            retained = retained.saturating_add(entry.descriptor().retained_bytes());
            if retained > TOOL_SNAPSHOT_BYTES {
                return Err(ToolsetError::Bytes {
                    maximum: TOOL_SNAPSHOT_BYTES,
                    actual: retained,
                });
            }

            // Ignoring case, because that is how a permission rule reads a
            // name: `Rule::names` matches without it, so two tools spelled
            // apart only by case are one tool to every rule anybody could write
            // about them — and a verdict given for the first would be spent on
            // the second without ever having been asked about it. A roster that
            // cannot be written rules about is refused instead.
            if let Some(first) = entries.iter().take(at).find(|first| {
                first
                    .descriptor()
                    .name()
                    .eq_ignore_ascii_case(entry.descriptor().name())
            }) {
                return Err(ToolsetError::Duplicate {
                    name: entry.descriptor().name().into(),
                    first: first.descriptor().provenance().clone(),
                    second: entry.descriptor().provenance().clone(),
                });
            }
        }

        Ok(Self {
            generation: ToolGeneration::new(),
            entries: entries.into(),
        })
    }

    /// Which exact materialization this is.
    #[must_use]
    pub const fn generation(&self) -> &ToolGeneration {
        &self.generation
    }

    /// The visible/reachable entry with `name`.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&ToolEntry> {
        self.entries
            .iter()
            .find(|entry| entry.descriptor().name() == name)
    }

    /// Validates and admits one call from this generation.
    ///
    /// # Errors
    ///
    /// [`ToolError::InvalidCall`] when a retained field crosses its boundary,
    /// or [`ToolError::Unknown`] when no visible and reachable descriptor in
    /// this generation owns the name.
    pub fn admit(&self, call: &ToolCall) -> Result<ToolAdmission, ToolError> {
        call_bound("identifier", call.id.as_str(), TOOL_CALL_ID_BYTES)?;
        call_bound("name", &call.name, TOOL_NAME_BYTES)?;
        if call.args.as_str().len() > TOOL_ARGUMENT_BYTES {
            return Err(ToolError::InvalidCall {
                field: "arguments",
                maximum: TOOL_ARGUMENT_BYTES,
                actual: call.args.as_str().len(),
            });
        }

        let Some((index, _entry)) = self
            .entries
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.descriptor().name() == &*call.name)
        else {
            return Err(ToolError::Unknown(call.name.clone()));
        };

        Ok(ToolAdmission {
            generation: self.generation.clone(),
            index,
            call: call.clone(),
        })
    }

    /// Resolves an admission only through the generation that minted it.
    ///
    /// # Errors
    ///
    /// [`ToolError::StaleGeneration`] for a different generation or an
    /// internally inconsistent handle. Both fail before an executor is
    /// returned.
    pub fn resolve(&self, admission: &ToolAdmission) -> Result<&ToolEntry, ToolError> {
        if admission.generation != self.generation {
            return Err(stale(admission.name()));
        }

        self.entries
            .get(admission.index)
            .filter(|entry| entry.descriptor().name() == admission.name())
            .ok_or_else(|| stale(admission.name()))
    }

    /// Resolves an approved call only through its admitted generation.
    ///
    /// # Errors
    ///
    /// [`ToolError::StaleGeneration`] when the approval was minted without an
    /// admission, belongs to another generation, or names no current entry.
    pub fn resolve_approved(&self, approved: &Approved) -> Result<&ToolEntry, ToolError> {
        if approved.generation() != Some(&self.generation) {
            return Err(stale(approved.tool()));
        }

        self.find(approved.tool())
            .ok_or_else(|| stale(approved.tool()))
    }

    /// The provider projections, in materialized order.
    #[must_use]
    pub fn advertised(&self) -> Vec<ToolSchema<'_>> {
        self.entries
            .iter()
            .map(|entry| entry.descriptor().advertised())
            .collect()
    }

    /// The entries in provider order.
    #[must_use]
    pub fn entries(&self) -> &[ToolEntry] {
        &self.entries
    }
}

impl Default for ToolSnapshot {
    fn default() -> Self {
        Self::empty()
    }
}

/// Why owned tool metadata or one of its controls was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ToolDescriptorError {
    /// The tool's source could not be described.
    #[error(transparent)]
    Provenance(#[from] crate::registry::ProvenanceError),
    /// A retained string was empty.
    #[error("{field} must not be empty")]
    Empty {
        /// Which descriptor field.
        field: &'static str,
    },
    /// A retained string crossed its boundary.
    #[error("{field} is {actual} bytes; the maximum is {maximum}")]
    TooLong {
        /// Which descriptor field.
        field: &'static str,
        /// Its boundary.
        maximum: usize,
        /// What was supplied.
        actual: usize,
    },
    /// A timeout of no time at all.
    #[error("a tool timeout must be greater than zero")]
    ZeroTimeout,
    /// An explicit result allowance too small to state an elision.
    #[error("a tool result limit is {actual} bytes; the minimum is {minimum}")]
    ResultBytesTooSmall {
        /// The smallest usable encoded allowance.
        minimum: usize,
        /// What was supplied.
        actual: usize,
    },
}

/// Why a roster could not be registered or materialized.
#[derive(Debug, thiserror::Error)]
pub enum ToolsetError {
    /// One descriptor was not bounded or usable.
    #[error(transparent)]
    Descriptor(#[from] ToolDescriptorError),
    /// One registration's source could not be described.
    #[error(transparent)]
    Provenance(#[from] crate::registry::ProvenanceError),
    /// A live source could not be started, read, or released.
    ///
    /// The static roster cannot reach this: it registers what the binary
    /// compiled in, and nothing outside the process can refuse that. Every
    /// other implementation of [`Toolset`] starts somebody else's program or
    /// reads somebody else's file, and without this its only way to report a
    /// refusal would be to invent a descriptor error about a descriptor that
    /// was never built.
    #[error("tool source {id}: {problem}")]
    Source {
        /// Which source, by the stable spelling its provenance carries.
        ///
        /// Not spelled `source`, which `thiserror` reads as the error this one
        /// wraps: what refused here is a program or a file rather than another
        /// error, and there is nothing underneath to chain to.
        id: Box<str>,
        /// What went wrong, in the words the source's own failure used.
        problem: Box<str>,
    },
    /// Two distinct registrations answer to one provider-visible name.
    #[error("tool {name} is registered by both {first} and {second}")]
    Duplicate {
        /// The colliding name.
        name: Box<str>,
        /// The registration already present.
        first: ToolProvenance,
        /// The registration that was refused.
        second: ToolProvenance,
    },
    /// A snapshot held too many tools.
    #[error("a tool snapshot has {actual} entries; the maximum is {maximum}")]
    Entries {
        /// The ceiling.
        maximum: usize,
        /// The requested count.
        actual: usize,
    },
    /// Aggregate descriptor data crossed its boundary.
    #[error("a tool snapshot retains {actual} descriptor bytes; the maximum is {maximum}")]
    Bytes {
        /// The ceiling.
        maximum: usize,
        /// The requested bytes.
        actual: usize,
    },
    /// The tool registry refused a transaction for a reason of its own.
    #[error("the tool registry refused the change: {0}")]
    Registry(#[source] crate::registry::RegistryError),
}

impl From<crate::registry::RegistryError> for ToolsetError {
    fn from(error: crate::registry::RegistryError) -> Self {
        use crate::registry::RegistryError;
        match error {
            RegistryError::Provenance(error) => Self::Provenance(error),
            RegistryError::Duplicate { id, first, second } => Self::Duplicate {
                name: id,
                first,
                second,
            },
            RegistryError::Entries { maximum, actual } => Self::Entries { maximum, actual },
            RegistryError::Bytes { maximum, actual } => Self::Bytes { maximum, actual },
            other @ (RegistryError::Unknown { .. }
            | RegistryError::Superseded
            | RegistryError::Stale) => Self::Registry(other),
        }
    }
}

fn bounded(field: &'static str, value: &str, maximum: usize) -> Result<(), ToolDescriptorError> {
    if value.is_empty() {
        return Err(ToolDescriptorError::Empty { field });
    }
    if value.len() > maximum {
        return Err(ToolDescriptorError::TooLong {
            field,
            maximum,
            actual: value.len(),
        });
    }
    Ok(())
}

fn call_bound(field: &'static str, value: &str, maximum: usize) -> Result<(), ToolError> {
    if value.is_empty() || value.len() > maximum {
        return Err(ToolError::InvalidCall {
            field,
            maximum,
            actual: value.len(),
        });
    }
    Ok(())
}

fn stale(tool: &str) -> ToolError {
    ToolError::StaleGeneration { tool: tool.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(name: &str) -> ToolProvenance {
        ToolProvenance::new(ToolSourceKind::User, format!("test:{name}"), name).unwrap()
    }

    #[test]
    fn every_owned_field_is_bounded_before_it_is_retained() {
        let long = "x".repeat(TOOL_NAME_BYTES + 1);
        let problem = ToolDescriptor::new(long, "{}", source("too-long")).unwrap_err();

        assert!(matches!(
            problem,
            ToolDescriptorError::TooLong {
                field: "tool name",
                maximum: TOOL_NAME_BYTES,
                ..
            }
        ));
    }

    #[test]
    fn effect_classification_defaults_closed_and_descriptor_metadata_can_narrow_it() {
        struct ReadOnly;
        impl DescribeTool for ReadOnly {
            fn name(&self) -> &'static str {
                "read-only"
            }

            fn schema(&self) -> &'static str {
                "{}"
            }

            fn effect(&self) -> ToolEffect {
                ToolEffect::ReadOnly
            }
        }

        let conservative = ToolDescriptor::new("unknown", "{}", source("unknown")).unwrap();
        let read_only = ReadOnly.descriptor(source("read-only")).unwrap();

        assert_eq!(conservative.effect(), ToolEffect::NonIdempotent);
        assert_eq!(read_only.effect(), ToolEffect::ReadOnly);
    }

    #[test]
    fn two_names_one_permission_rule_reads_alike_are_one_name_to_a_snapshot() {
        // A rule matches a tool name without case, so `search` and `Search` are
        // one name to every rule anybody could write. A snapshot holding both
        // would spend a verdict given for one on the other, so it is refused —
        // here rather than only where a particular source reads its own names,
        // because every source arrives through this door.
        use crate::{Summary, ToolContext};

        // Named and never asked anything: the roster is refused while it is
        // being built, so nothing here is ever reached.
        struct Inert;
        impl Tool for Inert {
            fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
                panic!("named, never called")
            }

            fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
                panic!("named, never called")
            }

            fn summary(&self, _args: &ToolArgs) -> Summary {
                panic!("named, never called")
            }

            fn run(
                &self,
                _approved: Approved,
                _context: &ToolContext<'_>,
            ) -> Result<ToolOutput, ToolError> {
                panic!("named, never called")
            }
        }

        let entry = |name: &str| {
            ToolEntry::new(
                ToolDescriptor::new(name, "{}", source(name)).unwrap(),
                Arc::new(Inert) as Arc<dyn Tool>,
            )
        };

        let refused = ToolSnapshot::new([entry("search"), entry("Search")])
            .expect_err("one name to every rule");

        assert!(
            matches!(&refused, ToolsetError::Duplicate { name, .. } if &**name == "Search"),
            "{refused:?}"
        );
    }

    #[test]
    fn generation_identity_is_equality_only_and_shared_by_a_clone() {
        let snapshot = ToolSnapshot::new(Vec::new()).unwrap();
        let same = snapshot.clone();
        let other = ToolSnapshot::new(Vec::new()).unwrap();

        assert_eq!(snapshot.generation(), same.generation());
        assert_ne!(snapshot.generation(), other.generation());
        assert_eq!(
            snapshot.generation().context_id(),
            same.generation().context_id()
        );
        assert_ne!(
            snapshot.generation().context_id(),
            other.generation().context_id()
        );
    }
}
