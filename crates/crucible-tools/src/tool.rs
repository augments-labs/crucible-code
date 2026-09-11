//! What a tool is, from the runner's side.
//!
//! Tools are an open set: adding one must not edit this crate. The runner
//! dispatches over `dyn Tool` and never names `read`, `grep` or `bash`.
//!
//! Arguments arrive from the model as JSON text and stay text until the tool
//! that owns them parses them. That keeps this crate free of every tool's
//! argument shape, and it means an argument is validated exactly once, by the
//! code that knows what it means.

use std::fmt;
use std::sync::Mutex;
use std::time::Instant;

use crucible_runtime::Cancel;
use crucible_sandbox::{SandboxAudit, SandboxAuditError, SandboxAuditRecord, SandboxError};
use crucible_storage::{
    CallResultKey, CallResultReceipt, CallResultStoreError, IdempotencyKey, InvocationId,
};
use crucible_types::output::{CaptureElision, limit_encoded};
use crucible_types::{
    Ancestry, Attachment, Changed, Diff, RecordedToolOutput, RunId, ToolArgs, ToolId,
    ToolOutputRetention,
};

use crate::permissions::{Approved, Sensitivity};

/// Why a tool call did not produce a result.
///
/// A tool that ran and decided the answer is "no such file" returns a failed
/// [`ToolOutput`] instead — that is a result the model should see and act on,
/// not a breakdown of the mechanism.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// The model asked for a tool that is not registered.
    #[error("no tool named {0}")]
    Unknown(Box<str>),

    /// The arguments were not the shape this tool takes.
    #[error("{tool}: {problem}")]
    Arguments {
        /// Which tool rejected them.
        tool: Box<str>,
        /// What was wrong, in words the model can act on.
        problem: Box<str>,
    },

    /// The operating system refused.
    #[error("{tool}: {problem}")]
    Io {
        /// Which tool was running.
        tool: Box<str>,
        /// What failed, without the underlying path if it is sensitive.
        problem: Box<str>,
        /// What the operating system reported.
        source: std::io::Error,
    },

    /// The user cancelled while the tool was running.
    #[error("{0} cancelled")]
    Cancelled(Box<str>),

    /// The provider returned a call whose retained identity or arguments were
    /// unusable at invocation admission.
    #[error("invalid tool call {field}: {actual} bytes; the maximum is {maximum}")]
    InvalidCall {
        /// Which call field crossed its boundary.
        field: &'static str,
        /// The retained boundary.
        maximum: usize,
        /// What the call supplied.
        actual: usize,
    },

    /// An admission from one immutable generation was presented to another.
    #[error("tool {tool} is not reachable in the admitted generation")]
    StaleGeneration {
        /// The provider-visible name, without either generation identity.
        tool: Box<str>,
    },
}

/// What a call is about, in the words the transcript shows beside the tool's
/// name.
///
/// A type of its own rather than a `String`, for the reason [`ToolArgs`] is
/// one: it is made out of a call's arguments, and a `bash` call's arguments are
/// a command line somebody may have typed a token into. Redacting the arguments
/// and then carrying a copy of part of them under another name would be no
/// redaction at all.
#[derive(Clone, PartialEq, Eq)]
pub struct Summary(Box<str>);

impl Summary {
    /// Takes the words a tool worked out from its own arguments.
    #[must_use]
    pub fn new(said: impl Into<Box<str>>) -> Self {
        Self(said.into())
    }

    /// The words, for whatever is drawing the row.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the call said nothing that could be summarised.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Summary([redacted])")
    }
}

/// What a call that only looked around was looking at.
///
/// The transcript folds a run of these into one line, because a reader
/// scrolling past twelve rows that each say a file was read learns the same
/// thing from one row saying twelve files were read, and keeps the screen for
/// the work. A call that changed something is never one of these: a change is
/// the part of a turn a reader is entitled to see go by.
///
/// Distinct cases rather than a count, because the line names what was done —
/// searching, reading, listing, running and web research — a bare count would
/// name nothing. Closed, because a new kind of looking must make every
/// reader of this decide how to say it rather than fall into a default that
/// silently spells it wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Looking {
    /// A search for a pattern through the workspace's text.
    Pattern,
    /// A file read.
    File,
    /// A walk over what a directory holds.
    Directory,
    /// A command run that only reported what it found.
    Command,
    /// A query sent to a web search source.
    WebSearch,
    /// One web page requested from a fetch source.
    WebPage,
}

/// A file a call touched, volunteered for a compaction to remember.
///
/// The model's memory of a session is rebuilt from a recap, and the thing a
/// compacted session most needs back is *which files it was working in*. The
/// call's tool is the only code that knows which argument is the path — the
/// runner never parses arguments, for the reason [`Tool::summary`] gives — so
/// the tool says it here. A newtype rather than a `String`, for the reason
/// [`ToolArgs`] is one: it is made out of a call's arguments, which may name a
/// file somebody did not mean to share.
#[derive(Clone, PartialEq, Eq)]
pub struct Remembered {
    /// The path, as the call spelled it.
    path: Box<str>,
    /// Whether the call changed the file rather than only read it. A modified
    /// file is one a later turn may need to know the state of; a read one is
    /// context the model may simply want back.
    modified: bool,
}

impl Remembered {
    /// A file the call read.
    #[must_use]
    pub fn read(path: impl Into<Box<str>>) -> Self {
        Self {
            path: path.into(),
            modified: false,
        }
    }

    /// A file the call changed.
    #[must_use]
    pub fn modified(path: impl Into<Box<str>>) -> Self {
        Self {
            path: path.into(),
            modified: true,
        }
    }

    /// The path, as the call spelled it.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Whether the call changed the file rather than only read it.
    #[must_use]
    pub fn is_modified(&self) -> bool {
        self.modified
    }
}

impl fmt::Debug for Remembered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Remembered([redacted])")
    }
}

/// What the model said this call is for, on its way to whoever decides it.
///
/// The panel a call waits behind is drawn by a thread with no provider behind
/// it: it sends the question and blocks on the answer, so it cannot go back and
/// ask what the command was for. What the model says about a call therefore
/// arrives *with* the call, as an argument the schema invites and the tool
/// itself never reads.
///
/// Empty is the ordinary case and not a failure. A tool whose schema invites no
/// account, and a call that declined to give one, both come through here, and
/// the panel they get is the panel there was before this existed.
///
/// A [`Summary`] is the neighbouring type and answers a different question:
/// that one is *what* the call is, taken out of the arguments the tool acts on,
/// and it goes in the transcript. This is what the model says *about* the call,
/// in its own words, and it is shown only while somebody is deciding.
#[derive(Clone, PartialEq, Eq)]
pub struct Account {
    description: Box<str>,
    explanation: Box<[Box<str>]>,
}

impl Account {
    /// Takes the one line a call gave about itself.
    #[must_use]
    pub fn new(description: impl Into<Box<str>>) -> Self {
        Self {
            description: description.into(),
            explanation: Box::new([]),
        }
    }

    /// Takes the line and the paragraphs behind it.
    ///
    /// Two things rather than one because they are shown at different times: the
    /// line is on the panel from the moment it opens, and the paragraphs are
    /// behind a key somebody has to press. A call that wrote only the first is
    /// the ordinary one.
    #[must_use]
    pub fn explained(
        description: impl Into<Box<str>>,
        explanation: impl IntoIterator<Item = impl Into<Box<str>>>,
    ) -> Self {
        Self {
            explanation: explanation.into_iter().map(Into::into).collect(),
            ..Self::new(description)
        }
    }

    /// A call that said nothing about itself.
    #[must_use]
    pub fn none() -> Self {
        Self::new("")
    }

    /// The line, for whatever is drawing the question.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The paragraphs, in the order they were written.
    ///
    /// Empty is the ordinary case and means the panel opens with no prose behind
    /// its key — which is the panel there was before any of this existed.
    pub fn explanation(&self) -> impl ExactSizeIterator<Item = &str> {
        self.explanation.iter().map(AsRef::as_ref)
    }
}

impl fmt::Debug for Account {
    /// Redacted for the reason [`Summary`] is: this is the model's prose about
    /// arguments this crate refuses to show, and prose about a command line
    /// quotes it as often as not.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Account([redacted])")
    }
}

/// One piece of what a running tool has printed.
///
/// Redacted by `Debug` for the reason [`ToolOutput`] is, and it is the same
/// material: a command's own output, which is how a model reads a file and how
/// it runs `env`. A key printed once is a key in every `{:?}` this value reaches.
///
/// `Delta` is the neighbouring type and is deliberately not redacted, which is
/// the distinction worth keeping: that is the model's prose, written to be put on
/// screen, and this is whatever a program on this machine happened to print.
#[derive(Clone, PartialEq, Eq)]
pub struct Wrote(Box<str>);

impl Wrote {
    /// Takes what a tool has produced since it last said anything.
    #[must_use]
    pub fn new(text: impl Into<Box<str>>) -> Self {
        Self(text.into())
    }

    /// The text, for whatever is drawing it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Wrote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Wrote([redacted])")
    }
}

/// Where a tool reports what it has printed, while it is still running.
///
/// A trait for the reason `Post` is one, and narrower than it on
/// purpose: `Event` can say that a turn finished, and a tool has no
/// business saying so. The one sentence a running tool can utter is *here is
/// more of my output*, and this is that sentence.
///
/// It carries no identifier. Which call wrote this is attached by the layer that
/// dispatched the call, which is the layer that knows — so a tool cannot post
/// output under another call's name, in the same way it cannot obtain an
/// [`Approved`] for a call it was not given.
pub trait Watch: Send + Sync {
    /// Reports what has been produced since the last time this was called.
    ///
    /// Cannot fail, for the reason `Post::post` cannot: a tool that
    /// stopped to handle nobody listening would be stopping for the one
    /// condition that means nobody is waiting for it either.
    fn wrote(&self, text: Wrote);
}

/// Nobody is reading.
///
/// What a caller with nothing to draw hands to [`Tool::run`] — a test, and any
/// front end that does not put a running command on a screen. A tool writes into
/// it exactly as it writes into a channel, so there is no absence for a tool to
/// check for and no second path through it to get wrong.
#[derive(Debug, Clone, Copy)]
pub struct Unwatched;

impl Watch for Unwatched {
    fn wrote(&self, _text: Wrote) {}
}

/// Owned completion half of one runner-finalized durable call result.
///
/// A background executor registers this only after an application registry owns
/// its cleanup scope. The runner calls it after output hooks and both encoded
/// output bounds have selected the exact result written to durable storage.
pub trait CallResultAcceptance: Send {
    /// Binds the durable result receipt into the executor's lifecycle record.
    ///
    /// # Errors
    ///
    /// The executor could not durably close its acceptance transition.
    fn accept(self: Box<Self>, receipt: CallResultReceipt) -> Result<(), SandboxError>;
}

/// One source-qualified result waiting for runner-owned finalization.
pub struct PendingCallResult {
    key: CallResultKey,
    acceptance: Box<dyn CallResultAcceptance>,
}

impl PendingCallResult {
    /// The fixed durable identity derived before the tool ran.
    #[must_use]
    pub const fn key(&self) -> CallResultKey {
        self.key
    }

    /// Completes the executor lifecycle after the result sink returned a receipt.
    ///
    /// # Errors
    ///
    /// The executor could not durably close its acceptance transition.
    pub fn accept(self, receipt: CallResultReceipt) -> Result<(), SandboxError> {
        self.acceptance.accept(receipt)
    }
}

impl fmt::Debug for PendingCallResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingCallResult")
            .field("key", &"[redacted]")
            .finish_non_exhaustive()
    }
}

/// The run-scoped capabilities one admitted tool call receives.
///
/// Narrow by design: a tool can identify its run and call, observe its own
/// child cancellation/deadline, and stream output under that call. It cannot
/// emit arbitrary events, mint approval, steer the agent, or reach a session.
pub struct ToolContext<'a> {
    ancestry: Ancestry,
    call: ToolId,
    cancel: Cancel,
    deadline: Option<Instant>,
    watch: &'a dyn Watch,
    sandbox: SandboxAudit,
    call_result: Option<CallResultKey>,
    pending_result: Mutex<Option<PendingCallResult>>,
}

impl<'a> ToolContext<'a> {
    /// Builds a per-call context under `parent` cancellation.
    #[must_use]
    pub fn new(
        ancestry: Ancestry,
        call: ToolId,
        parent: &Cancel,
        deadline: Option<Instant>,
        watch: &'a dyn Watch,
    ) -> Self {
        let sandbox = SandboxAudit::new(ancestry, call.clone());
        Self {
            ancestry,
            call,
            cancel: parent.child_until(deadline),
            deadline,
            watch,
            sandbox,
            call_result: None,
            pending_result: Mutex::new(None),
        }
    }

    /// Binds the source-qualified identity this call's durable result is kept under.
    #[must_use]
    pub fn with_invocation(mut self, invocation: InvocationId) -> Self {
        self.call_result = Some(CallResultKey::derive(self.ancestry, invocation, &self.call));
        self
    }

    /// Builds a context around a host-created collector that may outlive a
    /// panicking tool frame.
    ///
    /// # Errors
    ///
    /// The collector was created for another ancestry or call.
    pub fn with_sandbox_audit(mut self, sandbox: SandboxAudit) -> Result<Self, SandboxAuditError> {
        if !sandbox.belongs_to(self.ancestry, &self.call) {
            return Err(SandboxAuditError::AttributionMismatch);
        }
        self.sandbox = sandbox;
        Ok(self)
    }

    /// The run this call belongs to.
    #[must_use]
    pub const fn run(&self) -> RunId {
        self.ancestry.run()
    }

    /// The run and its parentage.
    #[must_use]
    pub const fn ancestry(&self) -> Ancestry {
        self.ancestry
    }

    /// The provider call this context belongs to.
    #[must_use]
    pub const fn call(&self) -> &ToolId {
        &self.call
    }

    /// Source-qualified key fixed before this call began.
    #[must_use]
    pub fn call_result_key(&self) -> Option<CallResultKey> {
        self.call_result
    }

    /// Transfers one background executor into runner-owned result finalization.
    ///
    /// # Errors
    ///
    /// The context has no durable result capability, already owns a pending
    /// result, or its bounded ownership slot became unavailable.
    pub fn defer_call_result(
        &self,
        acceptance: Box<dyn CallResultAcceptance>,
    ) -> Result<(), CallResultStoreError> {
        let key = self.call_result.ok_or(CallResultStoreError::Unavailable)?;
        let mut pending = self
            .pending_result
            .lock()
            .map_err(|_| CallResultStoreError::Storage)?;
        if pending.is_some() {
            return Err(CallResultStoreError::Conflict);
        }
        *pending = Some(PendingCallResult { key, acceptance });
        Ok(())
    }

    /// Takes the executor half of a result for runner-owned finalization.
    ///
    /// # Errors
    ///
    /// The bounded ownership slot became unavailable.
    pub fn take_call_result(&self) -> Result<Option<PendingCallResult>, CallResultStoreError> {
        self.pending_result
            .lock()
            .map(|mut pending| pending.take())
            .map_err(|_| CallResultStoreError::Storage)
    }

    /// Cancellation local to this call and inherited from its run.
    #[must_use]
    pub const fn cancel(&self) -> &Cancel {
        &self.cancel
    }

    /// The monotonic deadline, where the descriptor set one.
    #[must_use]
    pub const fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Whether this call's own deadline has passed.
    #[must_use]
    pub fn timed_out(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// Reports incremental output under this call's identity.
    pub fn wrote(&self, text: Wrote) {
        self.watch.wrote(text);
    }

    /// Fixed-attribution collector for host-owned sandbox services.
    #[must_use]
    pub fn sandbox_audit(&self) -> SandboxAudit {
        self.sandbox.clone()
    }

    /// Facts accumulated by sandbox lifecycles in this call.
    ///
    /// # Errors
    ///
    /// The bounded collector became unavailable.
    pub fn sandbox_facts(&self) -> Result<Box<[SandboxAuditRecord]>, SandboxAuditError> {
        self.sandbox.records()
    }

    /// Takes accumulated sandbox facts exactly once for event/journal delivery.
    ///
    /// # Errors
    ///
    /// The bounded collector became unavailable.
    pub fn take_sandbox_facts(&self) -> Result<Box<[SandboxAuditRecord]>, SandboxAuditError> {
        self.sandbox.take_records()
    }
}

impl fmt::Debug for ToolContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolContext")
            .field("ancestry", &self.ancestry)
            .field("call", &"[redacted]")
            .field("cancelled", &self.cancel.requested())
            .field("deadline", &self.deadline)
            .finish()
    }
}

impl Watch for ToolContext<'_> {
    fn wrote(&self, text: Wrote) {
        ToolContext::wrote(self, text);
    }
}

/// What a tool produced, on its way back to the model.
///
/// And on its way to the reader, which is not the same journey. The text is
/// what both are shown; a [`Diff`] is what only the reader is, and it comes off
/// at [`ToolOutput::into_recorded`] where the two copies part company. What
/// survives that is [`Changed`], two integers naming no file, because the header
/// a reader was shown has to be drawable again from the copy that was kept.
/// Files go the other way: they are for the model to look at, so they stay on
/// the copy the transcript keeps and are absent from the row that is drawn.
///
/// This is the live value, and it is the only one a tool may return. It holds
/// the files an [`Approved`] admitted, so it is the value permission is about;
/// [`RecordedToolOutput`] is what survives into a log, and the crossing between
/// them runs one way. Nothing reconstructs this from a record.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolOutput {
    text: Box<str>,
    failed: bool,
    capture: Option<CaptureElision>,
    diff: Option<Diff>,
    changed: Option<Changed>,
    attachments: Box<[Attachment]>,
}

impl fmt::Debug for ToolOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolOutput")
            .field("text", &"[redacted]")
            .field("failed", &self.failed)
            .field("capture", &self.capture)
            .field("diff", &self.diff)
            .field("changed", &self.changed)
            .field("attachments", &self.attachments)
            .finish()
    }
}

impl ToolOutput {
    /// A successful result.
    #[must_use]
    pub fn ok(text: impl Into<Box<str>>) -> Self {
        Self {
            text: text.into(),
            failed: false,
            capture: None,
            diff: None,
            changed: None,
            attachments: Box::new([]),
        }
    }

    /// A result the model should treat as a failure it can react to — a
    /// missing file, a non-zero exit status, a pattern that matched nothing.
    #[must_use]
    pub fn failed(text: impl Into<Box<str>>) -> Self {
        Self {
            text: text.into(),
            failed: true,
            capture: None,
            diff: None,
            changed: None,
            attachments: Box::new([]),
        }
    }

    /// The same result, with the change it made for the reader to look at.
    ///
    /// What a tool that rewrote a file adds on its way out. It is the one thing
    /// here the model is not sent, so it carries no meaning the text does not
    /// also carry — a result whose words depended on it would say different
    /// things to the two readers of the same call.
    #[must_use]
    pub fn showing(mut self, diff: Diff) -> Self {
        self.diff = Some(diff);
        self
    }

    /// The same result, with the header a reader was shown and no lines under
    /// it.
    ///
    /// What [`ToolOutput::into_recorded`] leaves behind, said outright.
    #[must_use]
    pub fn counting(mut self, changed: Changed) -> Self {
        self.changed = Some(changed);
        self
    }

    /// The same result, with files the tool found for the model to look at.
    ///
    /// Paths rather than bytes, exactly as at the prompt: the runner is the one
    /// thing that opens a file, once, for the one request that carries it.
    ///
    /// A file the user named at the prompt is a file the user chose. A file a
    /// tool chose is not, so this takes the proof that the call was permitted —
    /// the same verdict that let the tool read in the first place, and no
    /// verdict kind of its own. The argument is never read. Requiring one is
    /// the point: an [`Approved`] cannot be minted outside the permission
    /// engine, so a tool that has not been permitted cannot reach this at all.
    ///
    /// ```compile_fail,E0061
    /// use crucible_tools::ToolOutput;
    ///
    /// let output = ToolOutput::ok("one match").with_attachments(Vec::new());
    /// ```
    #[must_use]
    pub fn with_attachments(
        mut self,
        _approved: &Approved,
        attachments: impl Into<Box<[Attachment]>>,
    ) -> Self {
        self.attachments = attachments.into();
        self
    }

    /// The files this result asks the model to look at.
    #[must_use]
    pub fn attachments(&self) -> &[Attachment] {
        &self.attachments
    }

    /// The text the model sees.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The same text, out of the result and into whatever asked for it.
    #[must_use]
    pub fn into_text(self) -> Box<str> {
        self.text
    }

    /// Whether the provider should mark this result as an error.
    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.failed
    }

    /// Records capture-time process elision for the encoded limiter.
    ///
    /// Process tools already state this in their returned text. Retaining the
    /// counts here lets a later encoded-size pass repeat the fact if it must
    /// replace that middle marker with its own.
    #[must_use]
    pub const fn with_capture_elision(mut self, original: usize, omitted: usize) -> Self {
        self.capture = CaptureElision::new(original, omitted);
        self
    }

    /// Elides the middle until the JSON-encoded text fits `maximum`.
    ///
    /// Both ends survive: the head carries setup and the tail usually carries
    /// the failure or final status. When anything is removed, the inserted
    /// model-visible note states the original encoded size and the encoded
    /// bytes omitted. Callers must supply at least
    /// [`crucible_types::TOOL_RESULT_MIN_BYTES`], which descriptor construction
    /// enforces for local limits.
    #[must_use]
    pub fn limit_encoded(&mut self, maximum: usize) -> ToolOutputRetention {
        let (text, retention) = limit_encoded(&self.text, self.capture, maximum);
        if let Some(text) = text {
            self.text = text;
        }
        retention
    }

    /// What the call changed, where it changed a file and said so.
    #[must_use]
    pub fn diff(&self) -> Option<&Diff> {
        self.diff.as_ref()
    }

    /// How much it changed, where the lines are no longer here to be counted.
    ///
    /// Set as the lines are dropped rather than beside them, so a copy that
    /// still holds its [`Diff`] answers `None` here and is drawn from the
    /// lines. The transcript copy keeps this two-integer display header.
    #[must_use]
    pub fn changed(&self) -> Option<Changed> {
        self.changed
    }

    /// Drops preview lines before transcript retention, keeping display counts.
    ///
    /// Provider projections send result text and attachments, not these counts
    /// or preview lines. The protected display journal may retain the bounded
    /// preview separately, so resume restores it without expanding model context.
    /// Older logs retain just the header. Calling this again preserves the
    /// existing counts; there are no remaining lines to count a second time.
    pub fn forget_diff(&mut self) {
        if let Some(diff) = self.diff.take() {
            self.changed = Some(Changed::new(diff.added(), diff.removed()));
        }
    }

    /// Finishes this live result into the value persistence acknowledges.
    ///
    /// The crossing runs one way. The change lines come off here and their
    /// header stays, because the row a reader was shown has to be drawable
    /// again from the copy that was kept; the files stay, because they are what
    /// the model was told to look at and the approval that admitted them is
    /// what put them here. Nothing turns the result back into a live one, so a
    /// record that is read back — or edited — carries no authority to run
    /// anything. There is no reverse constructor to name, and a record is not
    /// the type a tool returns:
    ///
    /// ```compile_fail,E0308
    /// use crucible_tools::ToolOutput;
    /// use crucible_types::RecordedToolOutput;
    ///
    /// fn ran() -> ToolOutput {
    ///     RecordedToolOutput::ok("read back out of a log")
    /// }
    /// ```
    #[must_use]
    pub fn into_recorded(mut self) -> RecordedToolOutput {
        self.forget_diff();
        RecordedToolOutput::recorded(
            self.text,
            self.failed,
            self.capture,
            self.changed,
            self.attachments,
        )
    }
}

/// One tool the agent can call.
pub trait Tool: Send + Sync {
    /// Checks that `args` are a call this executor understands, without
    /// causing an effect.
    ///
    /// The invocation pipeline calls this before and after any argument
    /// transformation. It must be side-effect free.
    ///
    /// # Errors
    ///
    /// [`ToolError`] when the arguments are not one complete call this tool
    /// can execute.
    fn validate(&self, args: &ToolArgs) -> Result<(), ToolError>;

    /// How dangerous this particular call is.
    ///
    /// Takes the arguments because it is not a property of the tool: `bash`
    /// running `ls` and `bash` running `rm -rf` are the same tool.
    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity;

    /// Stable executor key that makes repeating this exact admitted call one
    /// effect, where the descriptor declares
    /// [`crucible_storage::ToolEffect::Idempotent`].
    ///
    /// The default is no such guarantee. A returned key is retained only in a
    /// protected execution checkpoint and redacted from ordinary diagnostics.
    fn idempotency_key(&self, _args: &ToolArgs) -> Option<IdempotencyKey> {
        None
    }

    /// What this call is about — the [`Summary`] a transcript row shows.
    ///
    /// A different question from [`Tool::sensitivity`], asked of the same
    /// arguments: that one answers what is at risk, this one answers what the
    /// reader is looking at. `grep` is where the two come apart — what is at
    /// risk is the directory about to be walked, and what the call is about is
    /// the pattern.
    ///
    /// Owned by the tool for the reason the arguments are text everywhere else
    /// in this crate: the tool is the only code that knows which field carries
    /// the answer. Read anywhere else, that field name would be a second reading
    /// of a schema this trait keeps opaque on purpose, and the two would drift
    /// apart the first time one of them was renamed.
    ///
    /// Empty where the arguments cannot be read at all: that call is refused by
    /// [`Tool::run`] a moment later, and words invented for it would describe
    /// something that never happened.
    fn summary(&self, args: &ToolArgs) -> Summary;

    /// Whether this call can be left running while the turn goes on.
    ///
    /// False by default because returning from [`Tool::run`] ordinarily means the
    /// operation is over. A tool that answers true owns the handoff which keeps
    /// the operation alive and makes its eventual completion observable; a front
    /// end may then honestly offer its background action while this call is out.
    ///
    /// Takes the arguments because this is a capability of the call rather than
    /// necessarily of every invocation of the tool. It is asked before the call
    /// runs and must agree with the execution path [`Tool::run`] will take.
    fn backgroundable(&self, _args: &ToolArgs) -> bool {
        false
    }

    /// What kind of looking-around this call is, where it is only that.
    ///
    /// `None` by default, and `None` is the honest answer for most tools: a
    /// call that writes a file, asks the reader a question or edits code is
    /// part of what happened, and folding it into a count would hide it. A tool
    /// answers here only when its call leaves the workspace exactly as it found
    /// it and the reader loses nothing by seeing it counted rather than named.
    ///
    /// Takes the arguments for the reason [`Tool::backgroundable`] does, and
    /// with more force: `bash` is only looking around when the particular
    /// command line is, so this is a property of the call and never of the
    /// tool. It is asked before the call runs, so a tool that cannot tell from
    /// the arguments answers `None` rather than guess — a wrongly folded change
    /// is a change the reader never saw.
    fn looking(&self, _args: &ToolArgs) -> Option<Looking> {
        None
    }

    /// What a compaction should remember of this call, where there is anything.
    ///
    /// `None` is the ordinary answer: most calls leave nothing a rebuilt session
    /// needs to find again. The ones that do are the file tools — `read`, `edit`
    /// and `write` — and what they volunteer is the path, read off the same
    /// argument [`Tool::summary`] reads for the reason it gives: the tool is the
    /// only code that knows which field is the path, and the runner must never
    /// parse the arguments to find out.
    ///
    /// A default rather than a method every tool must write, because tools are
    /// an open set and most have no file to name. A tool that says nothing here
    /// is simply not tracked, which is the right answer for `grep`, `bash` and
    /// the rest.
    fn remember(&self, _args: &ToolArgs) -> Option<Remembered> {
        None
    }

    /// Runs the call.
    ///
    /// An [`Approved`] cannot be constructed outside the permission engine, so
    /// a call site that has not obtained a verdict cannot reach this function.
    /// It carries the tool and the arguments as well as the proof, which is
    /// what makes the arguments a tool runs on *the* arguments a verdict was
    /// reached about, and this tool *the* tool it was reached about — a
    /// separate `args` parameter, and a handle found beside the call, both
    /// left that to the caller's care.
    ///
    /// `context` carries per-call cancellation/deadline and the only output
    /// sink this executor may use. Most tools report nothing while running;
    /// commands are the reason the sink is present.
    ///
    /// # Errors
    ///
    /// [`ToolError`] when the call could not be carried out at all. A result
    /// the model should see, including a failure, comes back as a failed
    /// [`ToolOutput`].
    fn run(&self, approved: Approved, context: &ToolContext<'_>) -> Result<ToolOutput, ToolError>;
}

impl fmt::Debug for dyn Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Tool([executor])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crucible_types::{Change, Line, Modality, ToolCall};

    use crate::permissions::{Ask, Permission, Remember, Settled, Target, Verdict};

    /// Nobody to ask. A read is settled without a question in every mode, so a
    /// test that reaches this has stopped testing what it meant to.
    struct Unasked;

    impl Ask for Unasked {
        fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
            (Verdict::Deny, Remember::Never)
        }
    }

    /// A verdict about the very read that found the file.
    fn permitted() -> Approved {
        let call = ToolCall {
            id: ToolId::new("call-1"),
            name: "read".into(),
            args: ToolArgs::new("{}"),
        };
        let settled = Permission::new().decide(
            &call,
            &Sensitivity::ReadOnly {
                target: Target::at("/w/pictures/holiday.png", Some("pictures/holiday.png")),
            },
            &mut Unasked,
        );

        let Settled::Approved(approved) = settled else {
            panic!("a read is allowed without a question")
        };
        approved
    }

    fn holiday() -> Attachment {
        Attachment {
            path: "pictures/holiday.png".into(),
            modality: Modality::Image,
            media_type: "image/png".into(),
            hash: [0xab; 32],
        }
    }

    #[test]
    fn clearing_a_result_takes_the_files_it_showed_with_it() {
        let mut output = ToolOutput::ok("m".repeat(RecordedToolOutput::MIN_PRUNE_BYTES))
            .with_attachments(&permitted(), [holiday()])
            .into_recorded();

        assert!(output.prune() > 0);
        assert!(
            output.attachments().is_empty(),
            "a result the model will never read again is not still sending a picture"
        );
    }

    #[test]
    fn a_tool_context_has_call_identity_and_child_cancellation() {
        let parent = Cancel::new();
        let context = ToolContext::new(
            Ancestry::new(),
            ToolId::new("call-context"),
            &parent,
            None,
            &Unwatched,
        );

        assert_eq!(context.call().as_str(), "call-context");
        assert_eq!(context.run(), context.ancestry().run());
        context.cancel().request();
        assert!(context.cancel().requested());
        assert!(!parent.requested());
    }

    struct Accepted(Arc<Mutex<Option<CallResultReceipt>>>);

    impl CallResultAcceptance for Accepted {
        fn accept(self: Box<Self>, receipt: CallResultReceipt) -> Result<(), SandboxError> {
            *self.0.lock().unwrap() = Some(receipt);
            Ok(())
        }
    }

    #[test]
    fn a_context_bound_to_no_invocation_has_no_result_to_defer() {
        let accepted = Arc::new(Mutex::new(None));
        let context = ToolContext::new(
            Ancestry::new(),
            ToolId::new("call-unbound"),
            &Cancel::new(),
            None,
            &Unwatched,
        );

        assert!(context.call_result_key().is_none());
        assert!(matches!(
            context.defer_call_result(Box::new(Accepted(Arc::clone(&accepted)))),
            Err(CallResultStoreError::Unavailable)
        ));
        assert!(context.take_call_result().unwrap().is_none());
        assert!(accepted.lock().unwrap().is_none());
    }

    #[test]
    fn a_tool_context_transfers_one_pending_result_to_runner_finalization() {
        let accepted = Arc::new(Mutex::new(None));
        let context = ToolContext::new(
            Ancestry::new(),
            ToolId::new("call-context"),
            &Cancel::new(),
            None,
            &Unwatched,
        )
        .with_invocation(InvocationId::new());
        context
            .defer_call_result(Box::new(Accepted(Arc::clone(&accepted))))
            .unwrap();
        assert!(
            context
                .defer_call_result(Box::new(Accepted(Arc::clone(&accepted))))
                .is_err(),
            "a second owner replaced the pending result"
        );

        let pending = context.take_call_result().unwrap().expect("pending result");
        let receipt = CallResultReceipt::from_digest([0x5a; 32]);
        pending.accept(receipt).unwrap();

        assert_eq!(*accepted.lock().unwrap(), Some(receipt));
        assert!(context.take_call_result().unwrap().is_none());
    }

    /// The lines a call changed, whose text nothing outside the reader may see.
    fn rewrote() -> Diff {
        Diff::new([
            Line::new(1, Change::Removed, "let key = \"hunter2\";"),
            Line::new(1, Change::Added, "let key = read_key()?;"),
            Line::new(2, Change::Added, "audit(&key);"),
        ])
    }

    #[test]
    fn the_lines_are_dropped_and_the_count_they_added_up_to_is_not() {
        let mut output = ToolOutput::ok("rewrote main.rs").showing(rewrote());
        assert_eq!(output.changed(), None, "the lines are still here to count");

        output.forget_diff();

        assert!(output.diff().is_none(), "a line survived the copy parting");
        assert_eq!(output.changed(), Some(Changed::new(2, 1)));
    }

    #[test]
    fn forgetting_a_diff_that_is_already_gone_keeps_what_it_left() {
        let mut output = ToolOutput::ok("rewrote main.rs").showing(rewrote());
        output.forget_diff();
        output.forget_diff();

        assert_eq!(output.changed(), Some(Changed::new(2, 1)));
    }

    #[test]
    fn a_call_that_showed_no_change_has_no_count_to_keep() {
        let mut output = ToolOutput::ok("no such pattern");
        output.forget_diff();

        assert_eq!(output.changed(), None);
    }

    #[test]
    fn a_count_is_all_that_can_be_read_off_a_result_that_changed_a_file() {
        // The counts go where a diff may not, so what a `Debug` of one prints
        // decides whether that is safe: an integer says how many lines moved
        // and a line says what was in the file, and only one of those is fit
        // for a log. Asserted on both sides of the moment the lines are
        // dropped, because the lines are present for one of them.
        let mut output = ToolOutput::ok("rewrote main.rs").showing(rewrote());
        let showing = format!("{output:?}");

        output.forget_diff();
        let counting = format!("{output:?}");

        for said in [&showing, &counting] {
            assert!(
                !said.contains("hunter2"),
                "a line body reached a log: {said}"
            );
            assert!(
                !said.contains("read_key"),
                "a line body reached a log: {said}"
            );
        }
        assert!(
            counting.contains("added: 2"),
            "the count is unreadable: {counting}"
        );
        assert!(
            counting.contains("removed: 1"),
            "the count is unreadable: {counting}"
        );
    }

    #[test]
    fn a_tool_answers_with_the_file_it_found() {
        let output = ToolOutput::ok("one match").with_attachments(&permitted(), [holiday()]);

        assert_eq!(output.text(), "one match");
        let [only] = output.attachments() else {
            panic!("one file was attached")
        };
        assert_eq!(only.path.as_ref(), "pictures/holiday.png");
    }

    #[test]
    fn a_result_that_attached_nothing_carries_nothing() {
        assert!(ToolOutput::ok("done").attachments().is_empty());
        assert!(ToolOutput::failed("no such file").attachments().is_empty());
    }

    #[test]
    fn output_carries_whether_the_model_should_treat_it_as_a_failure() {
        assert!(!ToolOutput::ok("done").is_failed());
        assert!(ToolOutput::failed("no such file").is_failed());
        assert_eq!(ToolOutput::failed("no such file").text(), "no such file");
    }

    #[test]
    fn a_call_carries_the_paragraphs_it_would_be_explained_in() {
        let account = Account::explained(
            "run the suite",
            ["It builds every crate.", "It takes about two minutes."],
        );

        assert_eq!(account.description(), "run the suite");
        assert_eq!(
            account.explanation().collect::<Vec<_>>(),
            ["It builds every crate.", "It takes about two minutes."]
        );
    }

    #[test]
    fn a_call_that_said_only_what_it_was_for_did_not_explain_itself() {
        // The two travel together and stay separate the whole way down: the
        // line is a caption drawn with the panel, and the paragraphs are prose
        // behind a key. A call that wrote one and not the other gets that.
        assert_eq!(Account::new("lists the files").explanation().len(), 0);
        assert_eq!(Account::none().explanation().len(), 0);
        assert!(Account::none().description().is_empty());
    }

    #[test]
    fn account_debug_never_shows_what_the_model_wrote() {
        let account = Account::explained("description-canary", ["explanation-canary"]);
        let shown = format!("{account:?}");

        for canary in ["description-canary", "explanation-canary"] {
            assert!(
                !shown.contains(canary),
                "Account Debug exposed model-written text"
            );
        }
        assert!(shown.contains("redacted"));
    }

    #[test]
    fn output_debug_never_shows_workspace_content() {
        let output = ToolOutput::ok("output-debug-canary").showing(Diff::new([Line::new(
            1,
            Change::Added,
            "diff-debug-canary",
        )]));
        let shown = format!("{output:?}");
        for canary in ["output-debug-canary", "diff-debug-canary"] {
            assert!(!shown.contains(canary), "{shown}");
        }
        assert!(shown.contains("redacted"));
    }

    #[test]
    fn a_result_shown_a_diff_hands_it_over_until_it_is_asked_to_forget_it() {
        // The two ends of the one journey that is not shared. Everything else on
        // a result goes to the model and the reader alike; this goes to the
        // reader, and the same value carries it as far as the split and no
        // further.
        let mut output = ToolOutput::ok("changed 1, 1 replacements")
            .showing(Diff::new([Line::new(315, Change::Added, "budgets:")]));

        assert_eq!(output.diff().map(Diff::added), Some(1));

        output.forget_diff();

        assert!(output.diff().is_none());
        // And what the model was told is untouched, because it never depended on
        // the diff in the first place.
        assert_eq!(output.text(), "changed 1, 1 replacements");
    }

    #[test]
    fn a_result_that_changed_nothing_carries_no_diff_to_begin_with() {
        assert!(ToolOutput::ok("done").diff().is_none());
        assert!(ToolOutput::failed("no such file").diff().is_none());
    }

    #[test]
    fn what_a_command_printed_is_never_shown_by_debug() {
        let wrote = Wrote::new("wrote-debug-canary");
        let shown = format!("{wrote:?}");

        assert!(!shown.contains("wrote-debug-canary"), "{shown}");
        assert!(shown.contains("redacted"));
    }

    #[test]
    fn a_watcher_is_told_what_was_written_in_the_order_it_was_written() {
        #[derive(Default)]
        struct Recorded(std::sync::Mutex<Vec<String>>);

        impl Watch for Recorded {
            fn wrote(&self, text: Wrote) {
                if let Ok(mut said) = self.0.lock() {
                    said.push(text.as_str().to_owned());
                }
            }
        }

        let recorded = Recorded::default();
        recorded.wrote(Wrote::new("first"));
        recorded.wrote(Wrote::new("second"));

        assert_eq!(
            recorded.0.into_inner().unwrap(),
            ["first".to_owned(), "second".to_owned()]
        );
    }

    #[test]
    fn a_tool_nobody_is_watching_still_runs() {
        // What a caller with nothing to draw hands over. It is not a failure
        // and it is not an absence the tool has to check for: a tool writes
        // into it exactly as it writes into a channel, and the words go
        // nowhere.
        let nobody = Unwatched;
        nobody.wrote(Wrote::new("nobody is reading this"));
    }

    #[test]
    fn an_unknown_tool_names_the_tool_the_model_asked_for() {
        let err = ToolError::Unknown("frobnicate".into());
        assert_eq!(err.to_string(), "no tool named frobnicate");
    }
}
