//! Stand-ins for the collaborators a turn needs.
//!
//! The runner names nothing concrete, so a test can hand it a provider that
//! answers from a list and tools that answer from a field. What is exercised is
//! the loop itself: what it sends, what it records, and when it stops.

use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::sync::{Arc, Mutex};

use crucible_core::{
    Approved, Ask, Cancel, CredentialScopeId, Delta, DeltaStream, DescribeTool, Diff, Effort,
    Fragment, Message, Modalities, Modality, PriceRate, PricingCurrency, PricingDate, PricingError,
    PricingUnit, PromptCacheCapabilities, PromptCacheEncoding, PromptCachePricing,
    PromptCacheRates, PromptCacheResourceCreate, PromptCacheResourceCreated,
    PromptCacheResourceDeadline, PromptCacheResourceError, PromptCacheResourceLifecycle,
    PromptCacheResourceRecord, PromptCacheResourceRemote, PromptCacheResourceState,
    PromptCacheRetentionClass, PromptCacheRoute, Provider, ProviderError, Remember, Request,
    Sensitivity, Steer, Summary, Target, Tool, ToolArgs, ToolCall, ToolContext, ToolError,
    ToolOutput, UsageRate, Verdict, Wrote,
};

/// The name a scripted provider answers to.
const SCRIPT: &str = "script";

/// Every request a provider was given, shared with the test that made it.
pub(crate) type Sent = Arc<Mutex<Vec<SentRequest>>>;

/// Fixed-size request evidence retained after its borrowed view is gone.
#[derive(Debug)]
pub(crate) struct SentRequest {
    pub(crate) transcript_len: usize,
    pub(crate) context: Vec<Fragment>,
    agent_text: Vec<u64>,
    pub(crate) tools: Vec<SentToolSchema>,
    pub(crate) max_tokens: u32,
    pub(crate) effort: Option<Effort>,
    /// Whether this request carried a system prompt at all.
    ///
    /// The text is not kept because no test here reads it. What is worth
    /// recording is the yes or no: one request a turn deliberately sends none,
    /// and nothing else could tell that request apart from the ordinary ones.
    pub(crate) had_system: bool,
    pub(crate) cache_attempt: Option<crucible_core::ProviderAttemptId>,
    pub(crate) cache_identity: Option<crucible_core::PromptCacheIdentity>,
    pub(crate) cache_selection: Option<crucible_core::PromptCacheSelection>,
    pub(crate) cache_resource: bool,
}

/// Owned evidence of one borrowed provider projection.
#[derive(Debug)]
pub(crate) struct SentToolSchema {
    pub(crate) name: Box<str>,
}

impl SentRequest {
    /// Whether the request carried an agent answer with exactly this text.
    pub(crate) fn carried(&self, text: &str) -> bool {
        self.agent_text.contains(&fingerprint(text))
    }
}

fn fingerprint(text: &str) -> u64 {
    let mut hash = DefaultHasher::new();
    text.hash(&mut hash);
    hash.finish()
}

/// A provider that answers from a script, one round per request.
pub(crate) struct Script {
    credential_scope: CredentialScopeId,
    rounds: Mutex<VecDeque<Vec<Delta>>>,
    sent: Sent,
    refuses: Option<u16>,
    breaks: bool,
    /// How many more requests go away before they have said anything.
    drops: Mutex<usize>,
    /// Optional provider usage reported by each otherwise empty dropped request.
    drop_usage: Option<crucible_core::ProviderUsage>,
    /// A line typed into this queue as the first request goes out.
    types: Mutex<Option<(Steer, Box<str>)>>,

    /// Whether every request is refused for not fitting the window.
    ///
    /// Separate from `refuses`, which carries a status: this refusal has none
    /// to carry. It is the one a provider gives before it has read anything,
    /// and the only refusal the loop answers by making room rather than by
    /// handing back.
    over_window: bool,
    /// Whether this fixture exposes one exact pricing record.
    cache: CacheFixture,
    /// Result of an explicit persistent-resource deletion.
    resource_delete: ResourceDelete,
}

#[derive(Debug, Clone, Copy)]
enum ResourceDelete {
    Deleted,
    StillReady,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, Default)]
struct CacheFixture {
    priced: bool,
    persistent: bool,
    encoding_failure: bool,
}

impl Script {
    /// Answers each request with the next round, and with nothing once the
    /// rounds run out.
    pub(crate) fn new(rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            credential_scope: CredentialScopeId::new(),
            rounds: Mutex::new(rounds.into()),
            sent: Sent::default(),
            refuses: None,
            breaks: false,
            drops: Mutex::new(0),
            drop_usage: None,
            types: Mutex::new(None),
            over_window: false,
            cache: CacheFixture::default(),
            resource_delete: ResourceDelete::Deleted,
        }
    }

    /// Reconstructs this fixture under one durable credential identity.
    #[cfg(test)]
    pub(crate) const fn with_credential_scope(mut self, scope: CredentialScopeId) -> Self {
        self.credential_scope = scope;
        self
    }

    /// A provider that refuses every request for not fitting the window.
    ///
    /// The refusal that arrives instead of an answer, rather than the stop
    /// reason that arrives inside one. They are two different rails through
    /// the loop and only this one is a [`ProviderError`].
    pub(crate) fn over_window() -> Self {
        Self {
            over_window: true,
            ..Self::new(Vec::new())
        }
    }

    /// A provider that types `line` into `steer` as its first request goes
    /// out, and answers from the script.
    ///
    /// The moment a reader actually types: while an answer is arriving, which
    /// is after the pass drained the queue and before the tools it asks for
    /// run. [`Typing`] covers the other one — typed while a call is out — and
    /// between them they are the two places a line can appear inside a pass.
    pub(crate) fn typing(steer: Steer, line: &str, rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            types: Mutex::new(Some((steer, line.into()))),
            ..Self::new(rounds)
        }
    }

    /// A provider that refuses every request, with a status nothing recovers
    /// from.
    pub(crate) fn failing() -> Self {
        Self::refusing(401)
    }

    /// A provider that refuses every request with `status`.
    pub(crate) fn refusing(status: u16) -> Self {
        Self {
            refuses: Some(status),
            ..Self::new(Vec::new())
        }
    }

    /// A provider whose first `drops` requests go away before they have said
    /// anything, and which answers from the script after that.
    ///
    /// The connection a provider closed while the tools ran: the request is
    /// accepted and the stream produces nothing at all, which is the one shape
    /// the loop may ask for again.
    pub(crate) fn dropping(drops: usize, rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            drops: Mutex::new(drops),
            ..Self::new(rounds)
        }
    }

    /// A first transport-ambiguous request reports usage before disappearing.
    pub(crate) fn dropping_with_usage(
        usage: crucible_core::ProviderUsage,
        rounds: Vec<Vec<Delta>>,
    ) -> Self {
        Self {
            drops: Mutex::new(1),
            drop_usage: Some(usage),
            ..Self::new(rounds)
        }
    }

    /// A provider whose connection breaks once the round's deltas have been
    /// handed over.
    ///
    /// The failure the loop cannot treat as an ending: the deltas were posted,
    /// so the user has already read them, and nothing the provider sends after
    /// them says how the answer was meant to finish.
    pub(crate) fn breaking(rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            breaks: true,
            ..Self::new(rounds)
        }
    }

    /// A handle on what it was asked, kept by the test before it hands the
    /// provider over.
    pub(crate) fn sent(&self) -> Sent {
        Arc::clone(&self.sent)
    }

    /// Exposes a deterministic USD pricing fixture for normalized cost tests.
    pub(crate) fn priced(mut self) -> Self {
        self.cache.priced = true;
        self
    }

    /// Exposes a deterministic persistent-resource lifecycle.
    pub(crate) fn persistent(mut self) -> Self {
        self.cache.persistent = true;
        self
    }

    /// Exposes a reviewed mechanism whose selected control cannot be lowered.
    pub(crate) fn failing_cache_encoding(mut self) -> Self {
        self.cache.encoding_failure = true;
        self
    }

    /// Exposes a lifecycle whose deletion returns a conclusive non-deleted state.
    pub(crate) fn surviving_delete(mut self) -> Self {
        self.cache.persistent = true;
        self.resource_delete = ResourceDelete::StillReady;
        self
    }

    /// Exposes a lifecycle whose deletion may have reached the provider.
    pub(crate) fn ambiguous_delete(mut self) -> Self {
        self.cache.persistent = true;
        self.resource_delete = ResourceDelete::Ambiguous;
        self
    }
}

impl Provider for Script {
    fn name(&self) -> &'static str {
        SCRIPT
    }

    /// A stand-in spells what every real provider here spells today.
    ///
    /// It is not a wire protocol, so it has nothing of its own to declare; what
    /// it must not do is claim more, or a test would be exercising a capability
    /// no provider has. Pictures are in because all three wires write one; a
    /// PDF is not, because only two of them do.
    fn spells(&self) -> Modalities {
        Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
    }

    fn prompt_cache_capabilities(&self, model: &str) -> PromptCacheCapabilities {
        if self.cache.encoding_failure {
            return PromptCacheCapabilities::supported(
                "script-encoding-failure-v1",
                (model == "claude-test").then_some("script-revision-v1"),
                crucible_core::PromptCacheProvenance::new(
                    "https://provider.invalid/prompt-cache",
                    "2026-08-31",
                    "script-encoding-failure-v1",
                ),
                crucible_core::StatefulTransportCapability::Unsupported,
                &[
                    crucible_core::PromptCacheMechanismCapability::explicit_breakpoints(
                        0,
                        1,
                        &[crucible_core::PromptCacheBoundary::AfterSystem],
                        &[crucible_core::PromptCacheContent::Text],
                    ),
                ],
                crucible_core::PromptCacheUsageReporting::ReadAndWriteTokens,
            );
        }
        if !self.cache.persistent {
            return PromptCacheCapabilities::unknown("script-fixture-v1");
        }
        PromptCacheCapabilities::supported(
            "script-persistent-v1",
            (model == "claude-test").then_some("script-revision-v1"),
            crucible_core::PromptCacheProvenance::new(
                "https://provider.invalid/persistent-cache",
                "2026-08-31",
                "script-persistent-v1",
            ),
            crucible_core::StatefulTransportCapability::Unsupported,
            &[
                crucible_core::PromptCacheMechanismCapability::persistent_content(
                    0,
                    &[crucible_core::PromptCacheContent::Text],
                ),
            ],
            crucible_core::PromptCacheUsageReporting::ReadAndWriteTokens,
        )
    }

    fn prompt_cache_resources(&self) -> Option<&dyn PromptCacheResourceLifecycle> {
        self.cache.persistent.then_some(self)
    }

    fn prompt_cache_pricing(
        &self,
        model: &str,
        revision: Option<&str>,
        input_tokens: Option<u64>,
        retention: PromptCacheRetentionClass,
        at: PricingDate,
    ) -> Result<Option<PromptCachePricing>, PricingError> {
        if !self.cache.priced
            || model != "claude-test"
            || revision.is_some()
            || input_tokens.is_none()
            || retention != PromptCacheRetentionClass::ProviderDefault
            || at < PricingDate::new(2026, 8, 31)
        {
            return Ok(None);
        }
        Ok(Some(PromptCachePricing::new(
            SCRIPT,
            SCRIPT,
            "claude-test",
            None,
            PricingDate::new(2026, 8, 31),
            "script-pricing-v1",
            "https://provider.invalid/pricing",
            PricingCurrency::new("USD"),
            PricingUnit::MillionTokens,
            PromptCacheRates {
                uncached_input: UsageRate::priced(PriceRate::per_million(1_000_000_000)),
                cache_read: UsageRate::priced(PriceRate::per_million(100_000_000)),
                cache_write_or_creation: UsageRate::NotApplicable,
                output: UsageRate::priced(PriceRate::per_million(5_000_000_000)),
                reasoning: UsageRate::NotApplicable,
                storage: UsageRate::NotApplicable,
                other: UsageRate::NotApplicable,
            },
        )))
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: SCRIPT,
            endpoint: SCRIPT,
            custom_endpoint: true,
            credential_scope: self.credential_scope,
            account: None,
            project: None,
            request_shape_version: "script-fixture-v1",
        }
    }

    fn prompt_cache_encoding(&self, request: &Request<'_>) -> PromptCacheEncoding {
        if self.cache.encoding_failure
            && request
                .prompt_cache
                .and_then(|cache| cache.selection.selected())
                .is_some()
        {
            return PromptCacheEncoding::Failed(
                crucible_core::PromptCacheIneligibleReason::UnsupportedBoundary,
            );
        }
        if request
            .prompt_cache
            .and_then(|cache| cache.resource)
            .is_some()
        {
            PromptCacheEncoding::PersistentResourceReferenced
        } else {
            PromptCacheEncoding::NoControlIntended
        }
    }

    fn stream(
        &self,
        request: Request<'_>,
        _cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        self.sent.lock().unwrap().push(SentRequest {
            transcript_len: request.transcript.len(),
            context: request
                .transcript
                .messages()
                .iter()
                .filter_map(|message| match message {
                    Message::Context(fragment) => Some(fragment.clone()),
                    Message::User { .. } | Message::Agent { .. } | Message::ToolResults(_) => None,
                })
                .collect(),
            agent_text: request
                .transcript
                .messages()
                .iter()
                .filter_map(|message| match message {
                    Message::Agent { text, .. } => Some(fingerprint(text)),
                    Message::Context(_) | Message::User { .. } | Message::ToolResults(_) => None,
                })
                .collect(),
            tools: request
                .tools
                .iter()
                .map(|tool| SentToolSchema {
                    name: tool.name.into(),
                })
                .collect(),
            max_tokens: request.max_tokens,
            effort: request.effort,
            had_system: request.system.is_some(),
            cache_attempt: request.prompt_cache.map(|cache| cache.attempt),
            cache_identity: request.prompt_cache.map(|cache| cache.identity),
            cache_selection: request.prompt_cache.map(|cache| cache.selection),
            cache_resource: request
                .prompt_cache
                .and_then(|cache| cache.resource)
                .is_some(),
        });

        // Before anything is answered: the line is meant to arrive while the
        // request is out, not once it has been read.
        if let Some((steer, line)) = self.types.lock().unwrap().take() {
            steer.say(line.into());
        }

        if self.over_window {
            return Err(ProviderError::WindowExceeded { provider: SCRIPT });
        }

        if let Some(status) = self.refuses {
            return Err(ProviderError::Refused {
                provider: SCRIPT,
                status,
                message: "no".into(),
            });
        }

        // Before a round is taken, because a response that went away said
        // nothing and cost the script nothing: the answer it was going to give
        // is still the next one.
        let mut drops = self.drops.lock().unwrap();
        if *drops > 0 {
            *drops -= 1;
            return Ok(Box::new(Recited {
                deltas: self
                    .drop_usage
                    .clone()
                    .map(Delta::Usage)
                    .into_iter()
                    .collect(),
                breaks: true,
            }));
        }
        drop(drops);

        let round = self.rounds.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::new(Recited {
            deltas: round.into(),
            breaks: self.breaks,
        }))
    }
}

impl PromptCacheResourceLifecycle for Script {
    fn create(
        &self,
        request: PromptCacheResourceCreate<'_>,
        cancel: &Cancel,
    ) -> Result<PromptCacheResourceCreated, PromptCacheResourceError> {
        if cancel.requested() {
            return Err(PromptCacheResourceError::Cancelled);
        }
        if request.deadline.expired() {
            return Err(PromptCacheResourceError::Deadline);
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Ok(PromptCacheResourceCreated {
            handle: crucible_core::PromptCacheResourceHandle::new("script-remote-resource")
                .expect("bounded fixture handle"),
            expires_at: now.saturating_add(600),
        })
    }

    fn resolve(
        &self,
        record: &PromptCacheResourceRecord,
        deadline: PromptCacheResourceDeadline,
        cancel: &Cancel,
    ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
        if cancel.requested() {
            return Err(PromptCacheResourceError::Cancelled);
        }
        if deadline.expired() {
            return Err(PromptCacheResourceError::Deadline);
        }
        Ok(PromptCacheResourceRemote {
            handle: record.handle().cloned(),
            state: PromptCacheResourceState::Ready,
            expires_at: record.expires_at(),
        })
    }

    fn renew(
        &self,
        record: &PromptCacheResourceRecord,
        _retention: crucible_core::PromptCacheRetention,
        deadline: PromptCacheResourceDeadline,
        cancel: &Cancel,
    ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
        self.resolve(record, deadline, cancel)
    }

    fn delete(
        &self,
        record: &PromptCacheResourceRecord,
        _deadline: PromptCacheResourceDeadline,
        cancel: &Cancel,
    ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
        if cancel.requested() {
            return Err(PromptCacheResourceError::Cancelled);
        }
        match self.resource_delete {
            ResourceDelete::Deleted => Ok(PromptCacheResourceRemote {
                handle: None,
                state: PromptCacheResourceState::Deleted,
                expires_at: None,
            }),
            ResourceDelete::StillReady => Ok(PromptCacheResourceRemote {
                handle: record.handle().cloned(),
                state: PromptCacheResourceState::Ready,
                expires_at: record.expires_at(),
            }),
            ResourceDelete::Ambiguous => Err(PromptCacheResourceError::Ambiguous(
                crucible_core::PromptCacheResourceOperation::Delete,
            )),
        }
    }

    fn reconcile(
        &self,
        record: &PromptCacheResourceRecord,
        deadline: PromptCacheResourceDeadline,
        cancel: &Cancel,
    ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
        if record.pending() == Some(crucible_core::PromptCacheResourceOperation::Delete) {
            return self.delete(record, deadline, cancel);
        }
        self.resolve(record, deadline, cancel)
    }

    fn inspect(
        &self,
        record: &PromptCacheResourceRecord,
        deadline: PromptCacheResourceDeadline,
        cancel: &Cancel,
    ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError> {
        self.resolve(record, deadline, cancel)
    }
}

/// A round already in memory, handed out one delta at a time.
struct Recited {
    deltas: VecDeque<Delta>,
    /// Whether running out of deltas is a broken connection rather than the end
    /// of the answer.
    breaks: bool,
}

impl DeltaStream for Recited {
    fn next(&mut self) -> Option<Result<Delta, ProviderError>> {
        if let Some(delta) = self.deltas.pop_front() {
            return Some(Ok(delta));
        }

        self.breaks.then(|| {
            self.breaks = false;
            Err(ProviderError::Transport {
                provider: SCRIPT,
                problem: "the connection went away".into(),
            })
        })
    }
}

/// A tool whose answer is decided before it runs.
pub(crate) struct Fixed {
    name: &'static str,
    answer: Box<str>,
    problem: Option<Box<str>>,
    cancels: bool,
    sensitivity: Sensitivity,
    diff: Option<Diff>,
    writes: Vec<Box<str>>,
    backgroundable: bool,
}

impl Fixed {
    /// A read-only tool that succeeds.
    pub(crate) fn new(name: &'static str) -> Self {
        Self {
            name,
            answer: "done".into(),
            problem: None,
            cancels: false,
            sensitivity: Sensitivity::ReadOnly {
                target: Target::unresolved(),
            },
            diff: None,
            writes: Vec::new(),
            backgroundable: false,
        }
    }

    /// What it prints, one piece at a time, before it answers.
    pub(crate) fn writing(mut self, pieces: &[&str]) -> Self {
        self.writes = pieces.iter().map(|piece| (*piece).into()).collect();
        self
    }

    /// What it produces when it succeeds.
    pub(crate) fn answering(mut self, text: &str) -> Self {
        self.answer = text.into();
        self
    }

    /// Makes it report that it could not carry the call out.
    pub(crate) fn breaking(mut self, problem: &str) -> Self {
        self.problem = Some(problem.into());
        self
    }

    /// Makes it notice a cancellation part way through its work.
    pub(crate) fn cancelling(mut self) -> Self {
        self.cancels = true;
        self
    }

    /// How dangerous it claims to be, which is what decides whether the user
    /// is asked.
    pub(crate) fn risking(mut self, sensitivity: Sensitivity) -> Self {
        self.sensitivity = sensitivity;
        self
    }

    /// Makes it a tool whose calls can be left running.
    pub(crate) fn detachable(mut self) -> Self {
        self.backgroundable = true;
        self
    }

    /// Makes it a tool that rewrote a file and has the change to show for it.
    pub(crate) fn showing(mut self, diff: Diff) -> Self {
        self.diff = Some(diff);
        self
    }
}

impl DescribeTool for Fixed {
    fn name(&self) -> &str {
        self.name
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Fixed {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        self.sensitivity.clone()
    }

    /// The arguments as they arrived. A real tool names one field of them;
    /// what a test needs is to see that whatever the tool said reached the
    /// other end, so this says something no other value could be mistaken for.
    fn summary(&self, args: &ToolArgs) -> Summary {
        Summary::new(args.as_str())
    }

    fn backgroundable(&self, _args: &ToolArgs) -> bool {
        self.backgroundable
    }

    fn run(&self, _approved: Approved, context: &ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        for piece in &self.writes {
            context.wrote(Wrote::new(piece.clone()));
        }

        if self.cancels {
            return Err(ToolError::Cancelled(self.name.into()));
        }

        match &self.problem {
            Some(problem) => Err(ToolError::Arguments {
                tool: self.name.into(),
                problem: problem.clone(),
            }),
            None => Ok(match &self.diff {
                Some(diff) => ToolOutput::ok(self.answer.clone()).showing(diff.clone()),
                None => ToolOutput::ok(self.answer.clone()),
            }),
        }
    }
}

/// A tool that types a line into the reader's queue while it runs.
///
/// The one moment a steered line can arrive between a call and its answer is
/// while that call is out, and nothing else here can reach it: the queue is
/// pushed to from the thread that reads the keyboard, and a test has only the
/// thread the turn runs on.
pub(crate) struct Typing {
    name: &'static str,
    steer: Steer,
    line: Box<str>,
}

impl Typing {
    /// A read-only tool that says `line` as the reader would, then answers.
    pub(crate) fn new(name: &'static str, steer: Steer, line: &str) -> Self {
        Self {
            name,
            steer,
            line: line.into(),
        }
    }
}

impl DescribeTool for Typing {
    fn name(&self) -> &str {
        self.name
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Typing {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        Summary::new(args.as_str())
    }

    fn run(
        &self,
        _approved: Approved,
        _context: &ToolContext<'_>,
    ) -> Result<ToolOutput, ToolError> {
        self.steer.say(self.line.to_string());
        Ok(ToolOutput::ok("done"))
    }
}

/// A call that gets the user asked: a change to a file, which no mode waves
/// through except `fullAccess`.
///
/// The target is one nothing resolved, so no rule written about a path matches
/// it and what the tests here exercise is the loop rather than the matcher.
pub(crate) fn changing() -> Sensitivity {
    Sensitivity::MutatesFile {
        target: Target::unresolved(),
    }
}

/// A user who answers every question the same way, and counts them.
pub(crate) struct Says {
    verdict: Verdict,
    remember: Remember,
    /// How often the user was put to the question.
    pub(crate) asked: usize,
}

impl Says {
    /// Answers `verdict`, for this call only.
    pub(crate) fn new(verdict: Verdict) -> Self {
        Self {
            verdict,
            remember: Remember::Never,
            asked: 0,
        }
    }

    /// Answers the same way, and asks for it to hold until the session ends.
    pub(crate) fn for_the_session() -> Self {
        Self {
            verdict: Verdict::Allow,
            remember: Remember::Session,
            asked: 0,
        }
    }
}

impl Ask for Says {
    fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
        self.asked += 1;
        (self.verdict, self.remember)
    }
}
