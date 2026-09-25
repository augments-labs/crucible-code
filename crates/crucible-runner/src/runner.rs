//! The turn loop.
//!
//! Ask the model, run what it asked for, tell it what happened, ask again —
//! until it yields, the user stops it, or something goes wrong.
//!
//! A response that failed for a moment, before it said a word, is asked for
//! again, up to the policy's attempts, rather than counted as the thing that
//! went wrong. The socket a provider closed while the tools ran is the usual
//! reason, and it is safe to ask again for exactly the reason it is worth
//! doing: nothing arrived, so nothing has been drawn that a second answer
//! could contradict.
//!
//! Progress leaves through events, because the thread that draws is not this
//! one. The outcome leaves through the return value, because the caller is
//! what decides whether the session continues.
//!
//! A turn is asynchronous. It awaits the provider's stream and each read of
//! it, every prompt-cache step, every call's run, a background result's
//! acceptance and the toolset's preparation, listing, refreshing and
//! disposal, so a step that has to wait for its answer is waited for rather
//! than refused. It starts no runtime: whoever awaits it polls it, on that
//! caller's own thread, and the one thing it spawns is its calls' runs, onto
//! the runtime it is polled in, at most `TOOL_RUNS` at once and each awaited
//! before the pass goes on — so a turn is polled inside a runtime, as the
//! application's wait for one is. It hands its [`Cancel`] to every step it
//! awaits and looks at it between them, so a stop ends it as it always has,
//! and how soon an awaited step heeds that stop is the step's own contract.
//! No step on the turn's path is still reached through a bridge that asks
//! once: the bridges that remain — `BashSandbox`, `LocalBackend`,
//! `SandboxReport` and `SandboxPanel` — are crossed outside the turn, and a
//! step that would have had to wait there is refused where it is crossed,
//! naming its bridge.
//!
//! The loop's own body lives in [`passes`], because it lasts one turn and this
//! does not. What stays here is the session it is taken against — the provider,
//! the transcript, what the user has already allowed, the store — together with
//! the one request, the recap and the retry that a pass reaches for. A caller
//! never sees the split: [`Runner::turn`] is still the whole of the way in.
//!
//! Explicit prompt-cache resource cleanup lives in [`cleanup`] for the other
//! reason: it is not part of a turn at all. It is asked for, walks the store
//! once and stops, and [`Runner::clean_prompt_cache`] is still the way in.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crucible_core::{
    Aside, Ask, Attachment, Cancel, Compacting, Content, ContextSection, Delta, DeltaStream,
    Effort, JournalStore, Looking, Message, Modalities, Mode, Permission, PermissionsSection,
    PromptCacheAttempt, PromptCacheEncoding, PromptCacheFact, PromptCacheOutcome,
    PromptCachePreparationError, PromptCacheRequestDisposition, PromptCacheRequestFact,
    PromptCacheResourceError, PromptCacheResourceRecord, PromptCacheRetentionClass,
    PromptCacheUsageFact, PromptCacheUsageReporting, Provider, ProviderError, ProviderUsage,
    Request, Room, RunItem, SandboxAuditRegistry, Spend, Steer, StopReason, Summary, ToolCall,
    ToolEntry, ToolError, ToolGeneration, ToolSchema, ToolSnapshot, Toolset, ToolsetContext,
    Transcript, TurnId, UsageCost,
};

use crucible_context::ContextInputs;

use crucible_agents::{Agent, AgentContext, Decision, GuardrailError, Model};

use crate::context::RunContext;
use crate::outcome::{RunResult, Turned};
use crate::policy::{Compaction, RunPolicy};
use crate::prompt_cache::{self, ScopeInputs};
use crate::tools::Tools;

use crate::{Event, Post, Reporter, TurnError};
mod answer;
mod assembly;
pub mod attachments;
mod cleanup;
mod clearing;
mod compaction;
mod load;
mod passes;
mod record;
mod state;
mod work;

use answer::Answer;
pub use cleanup::PromptCacheCleanup;
use load::{Counting, Load};
use passes::AgentLoop;
use state::Judged;
pub use state::RunState;
use work::{Went, Work};

/// How many compactions one turn may run without getting anywhere.
///
/// A compaction that frees nothing and is asked for again is the one way this
/// loop can spin without the transcript growing, so it is the one thing still
/// counted. Two, because the first may legitimately free little on a session
/// that is mostly one enormous turn, and a third has proved the point.
const COMPACTIONS_WITHOUT_PROGRESS: u8 = 2;

/// How long a pause holds before it looks at the cancel again.
const CANCEL_SLICE: Duration = Duration::from_millis(25);

/// Maximum wall-clock time for one explicit persistent-resource operation.
const PROMPT_CACHE_RESOURCE_DEADLINE: Duration = Duration::from_secs(15);

/// What making room left the turn able to do.
///
/// Three answers because the loop does three different things with them, and
/// the one that used to be missing is the one a reader asked for: a stop is not
/// a compaction that got nowhere, it is somebody saying leave the session
/// alone, and a turn that asked again afterwards would be spending a request
/// they had just refused to pay for.
enum After {
    /// Ask again, against a transcript that is smaller or has another go in it.
    Carry,
    /// Two goes in a row freed nothing. Each caller reached here for its own
    /// reason and says so in its own words.
    Stuck,
    /// Somebody stopped the recap. The turn ends the way a stopped request
    /// ends it, and nothing was replaced.
    Stopped,
}

/// Provider-controlled work retained during one turn.
#[derive(Default)]
struct TurnBounds {
    retained: usize,
    tool_output: usize,
}

/// Immutable cache-reporting dimensions bound to one provider attempt.
#[derive(Clone, Copy)]
struct CacheObservation {
    attempt: crucible_core::ProviderAttemptId,
    reporting: PromptCacheUsageReporting,
    model_revision: Option<&'static str>,
    retention: PromptCacheRetentionClass,
    pricing_date: crucible_core::PricingDate,
}

/// The state one provider request reads and updates together.
struct Listening<'a> {
    /// The run the request is part of: where its progress goes, whether it has
    /// been stopped, and how many goes it gets.
    run: &'a RunContext<'a>,
    advertised: &'a [ToolSchema<'a>],
    generation: &'a ToolGeneration,
    counting: &'a mut Counting,
}

impl TurnBounds {
    fn heard(&mut self, answer: &Answer) {
        self.retained = self.retained.saturating_add(answer.retained());
    }
}

/// Drives turns to completion.
///
/// Holds three kinds of thing, and holds them apart. The *definition* is what
/// the agent is — shared, never written to, and replaced whole when a session
/// is asked to change model, effort or instructions, so that a request already
/// out and a run beside this one keep the one they started under. The *state*
/// is what this run has accumulated: the transcript, the turn count, the load,
/// the roster in force. Everything else is a *service or a setting* the run was
/// given — the provider, the toolset source, the permission memory, the log,
/// the policy — which is neither a fact about the agent nor something the run
/// builds up.
#[derive(Debug)]
pub struct Runner {
    provider: Box<dyn Provider>,
    toolset: Arc<dyn Toolset>,
    /// The definition in force. Shared rather than owned, so that replacing it
    /// cannot reach a request already out under the last one.
    agent: Arc<Agent>,
    /// What this run has accumulated, and no other run has.
    state: RunState,
    context: ContextInputs,
    permission: Permission,
    /// Where the turn is recorded, as a contract rather than a file.
    ///
    /// Shared rather than owned: whoever opened the store still holds it —
    /// closing it, browsing it and reporting on it are theirs — and this crate
    /// writes to it without ever learning what it writes to.
    store: Arc<dyn JournalStore>,
    policy: RunPolicy,
    prompt_cache_store: Option<Box<dyn crucible_core::PromptCacheResourceStore>>,
    sandbox_audits: SandboxAuditRegistry,
}

/// What `agent` would be advertised out of `tools`, between turns.
///
/// A pass narrows the roster it admits and leaves it behind narrowed, so after
/// the first one this hands the roster straight back. Before it, the run is
/// still holding everything it was wired with, and both things read here
/// between turns — the names under the box and the size of the request the
/// next turn would send — are about what the definition declares rather than
/// about what the wiring installed.
///
/// A function over the two fields rather than a method, so a caller can hold
/// the load it is about to write while it asks.
fn advertising<'a>(agent: &Agent, tools: &'a ToolSnapshot) -> Vec<ToolSchema<'a>> {
    tools
        .advertised()
        .into_iter()
        .filter(|schema| agent.availability().offers(schema.name))
        .collect()
}

struct Tooling {
    source: Arc<dyn Toolset>,
    snapshot: ToolSnapshot,
}

impl Runner {
    /// A session that has not said anything yet.
    #[must_use]
    pub fn new(
        provider: Box<dyn Provider>,
        tools: Tools,
        agent: Agent,
        context: ContextInputs,
        store: Arc<dyn JournalStore>,
    ) -> Self {
        let snapshot = tools.snapshot().unwrap_or_default();
        Self::from_parts(
            provider,
            Tooling {
                source: Arc::new(tools),
                snapshot,
            },
            agent,
            context,
            store,
        )
    }

    /// A session backed by an arbitrary live toolset.
    ///
    /// Its first exact generation is prepared at turn admission. Until then,
    /// the between-turn descriptive view is empty; after a pass it is the last
    /// immutable generation that pass admitted.
    #[must_use]
    pub fn with_toolset<T>(
        provider: Box<dyn Provider>,
        toolset: T,
        agent: Agent,
        context: ContextInputs,
        store: Arc<dyn JournalStore>,
    ) -> Self
    where
        T: Toolset + 'static,
    {
        Self::from_parts(
            provider,
            Tooling {
                source: Arc::new(toolset),
                snapshot: ToolSnapshot::empty(),
            },
            agent,
            context,
            store,
        )
    }

    fn from_parts(
        provider: Box<dyn Provider>,
        tooling: Tooling,
        agent: Agent,
        context: ContextInputs,
        store: Arc<dyn JournalStore>,
    ) -> Self {
        let Tooling {
            source: toolset,
            snapshot: tools,
        } = tooling;
        let mut runner = Self {
            provider,
            toolset,
            // The window the definition names is what this run starts out
            // believing. What a provider goes on to report is this run's own
            // evidence, so it is corrected here and never written back.
            state: RunState::offering(agent.model().window, tools),
            agent: Arc::new(agent),
            context,
            permission: Permission::new(),
            store,
            policy: RunPolicy::default(),
            prompt_cache_store: None,
            sandbox_audits: SandboxAuditRegistry::new(),
        };
        runner.state.load.requesting(
            runner.agent.instructions(),
            &advertising(&runner.agent, &runner.state.tools),
        );
        runner
    }

    /// Takes the engine configuration described, rather than the default one.
    ///
    /// Built by the wiring and handed over whole, so this crate never learns
    /// that a rule has a syntax or that a mode has a spelling. A session
    /// without this call is one where nothing was configured, which is the
    /// engine asking about every change and every command.
    #[must_use]
    pub fn permitting(mut self, permission: Permission) -> Self {
        self.permission = permission;
        self
    }

    /// Runs under the policy described, rather than the default one.
    ///
    /// Handed over whole for the reason [`Runner::permitting`] is: this crate
    /// never learns that any of it has a spelling in a file. Whole rather than
    /// one family at a time because a run is under one policy — a builder per
    /// family would let a caller set half of one and leave the rest at figures
    /// nobody chose.
    #[must_use]
    pub const fn under(mut self, policy: RunPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Supplies the lazy private metadata store for explicitly authorized
    /// persistent prompt-cache resources.
    ///
    /// Constructing a store must not touch disk. The runner reaches it only
    /// after policy and exact adapter/model capabilities select a persistent
    /// mechanism.
    #[must_use]
    pub fn with_prompt_cache_store(
        mut self,
        store: impl crucible_core::PromptCacheResourceStore + 'static,
    ) -> Self {
        self.prompt_cache_store = Some(Box::new(store));
        self
    }

    /// Latest complete prompt-cache state known for one provider send.
    #[must_use]
    pub const fn prompt_cache_attempt(&self) -> Option<&PromptCacheAttempt> {
        self.state.prompt_cache_attempt.as_ref()
    }

    /// Effective cache policy applied to the next provider attempt.
    #[must_use]
    pub const fn prompt_cache_policy(&self) -> crucible_core::PromptCachePolicy {
        self.policy.prompt_cache
    }

    /// Exact declared cache capability for the current provider/model route.
    #[must_use]
    pub fn prompt_cache_capabilities(&self) -> crucible_core::PromptCacheCapabilities {
        self.provider
            .prompt_cache_capabilities(&self.agent.model().name)
    }

    /// Bounded private resource metadata for user-facing redacted inspection.
    ///
    /// # Errors
    ///
    /// [`PromptCacheResourceError`] when the private store cannot be read.
    pub async fn prompt_cache_resources(
        &mut self,
    ) -> Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError> {
        match self.prompt_cache_store.as_deref_mut() {
            Some(store) => {
                store
                    .inspect(crucible_core::MAX_PROMPT_CACHE_RESOURCES)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    /// Whether this run has a private prompt-cache resource store to inspect,
    /// clean, or retire.
    #[must_use]
    pub const fn prompt_cache_resources_configured(&self) -> bool {
        self.prompt_cache_store.is_some()
    }

    /// Whether this run owns persistent prompt-cache resources that a change of
    /// model, provider, or credential must retire first.
    #[must_use]
    pub const fn prompt_cache_retirement_pending(&self) -> bool {
        self.state.prompt_cache_owner_scope.is_some()
    }

    /// Picks up a transcript that already happened — what `--continue`
    /// replays.
    ///
    /// The turn count comes with it. Numbering the first continued turn `1`
    /// would tell the user this is a new session, which is exactly what
    /// they asked it not to be.
    ///
    /// What this run's vendor may not be sent is cleared from the transcript,
    /// and the lines saying so are owed to the session, as [`Runner::pick_up`]
    /// says.
    #[must_use]
    pub fn resuming(mut self, transcript: Transcript) -> Self {
        self.state.forget_checked();
        self.state.turn = Self::counting(&transcript);
        self.state.transcript = transcript;
        let reading = self.admit_restricted();
        self.recount(reading);
        self
    }

    /// Measures a transcript this runner did not build a message at a time.
    ///
    /// Walked once, where a session is picked up, rather than on any path a
    /// turn takes: what it costs is proportional to the transcript, and the one
    /// moment that is affordable is the one where the transcript was just read
    /// off a disk.
    ///
    /// The walk estimates, because a transcript is messages and messages do not
    /// say what they cost. Where the session picked up brought a reading back
    /// with it, that estimate is superseded by it and the session comes back
    /// knowing how much of the window it has left — which is the whole of why
    /// a log records one. The reading is handed in rather than read here: one
    /// taken before a clearing measured a request this run will never send, so
    /// a caller that just took results out hands in none, as replaying the
    /// clearing's line does.
    fn recount(&mut self, reading: Option<crucible_types::Calibration>) {
        self.state.load.replaced();
        for message in self.state.transcript.messages() {
            self.state.load.recounted(message);
        }
        self.state.load.requesting(
            self.agent.instructions(),
            &advertising(&self.agent, &self.state.tools),
        );

        // After the fixed content of this run's request is known, and never
        // before: what the log remembers is taken only where it still covers
        // the request this run would send.
        if let Some(calibration) = reading {
            self.state.load.measured(calibration);
        }
        self.state.load.resumed();
    }

    /// Puts this runner on a different session.
    ///
    /// What `/resume` runs. The store handed in is the one the caller opened
    /// and still holds: nothing is handed back, because closing the store this
    /// runner was writing to was never this crate's to do, and the caller that
    /// opened it is the one that reports what its last write came to.
    ///
    /// What the vendor being asked may not be sent is cleared from the
    /// transcript picked up here, and the lines saying so are owed to the
    /// session picked up until [`Runner::record_clearings`], or the next turn
    /// or compaction, writes them. A line still owed to the session left
    /// behind is written to that session.
    ///
    /// Everything about the session that was answered is answered again. The
    /// transcript, the store and the turn count come from the session picked
    /// up; what was allowed for the rest of the *last* session is forgotten,
    /// since that scope was the thing just left behind — and so is any input
    /// decision committed there, for the same reason. The mode is not an answer
    /// of that kind — it is where this process is being run, it is on screen at
    /// all times, and a session that quietly moved it would be the one place
    /// the row under the box could be wrong.
    pub fn pick_up(&mut self, store: Arc<dyn JournalStore>, transcript: Transcript) {
        self.permission.forget();
        self.state.forget_checked();
        self.state.turn = Self::counting(&transcript);
        self.state.transcript = transcript;

        // Before the recount rather than after it: what the session picked up
        // remembers about its own load is part of what is being recounted.
        self.store = store;
        let reading = self.admit_restricted();
        self.recount(reading);
    }

    /// The transcript so far.
    #[must_use]
    pub fn transcript(&self) -> &Transcript {
        &self.state.transcript
    }

    /// What a call is about, in the words of the tool that owns its arguments.
    ///
    /// The same answer that rides [`Event::ToolRequested`] while a call is out,
    /// asked for after the fact — a transcript keeps the call and not what was
    /// said about it, so a session put back on the screen has to ask again. Both
    /// go through here, because a call that read one way live and another way on
    /// the way back in is two calls as far as a reader is concerned.
    ///
    /// Asked of every tool registered, not only of those the model may call
    /// right now: a deferred tool looked up in an earlier turn is not one the
    /// model may call until it looks it up again, but the call it made then is
    /// still about what it was about. Nothing runs from the answer.
    ///
    /// Empty for a name no tool answers to, which is what a call that was
    /// refused looks like from here.
    #[must_use]
    pub fn about(&self, call: &ToolCall) -> Summary {
        self.describing(&call.name).map_or_else(
            || Summary::new(""),
            |entry| entry.tool().summary(&call.args),
        )
    }

    /// The tool that owns a call's arguments, for saying what the call was.
    ///
    /// The visible generation first, because that is the tool the call ran
    /// against where it ran this turn; then whatever the source has registered
    /// under the name, for a call made against a generation that has since
    /// closed -- a deferred tool the model looked up last time, say. Owned
    /// rather than borrowed because the second answer is minted by the source.
    fn describing(&self, name: &str) -> Option<ToolEntry> {
        self.state
            .tools
            .find(name)
            .cloned()
            .or_else(|| self.toolset.registered(name))
    }

    /// What kind of looking-around a call is, where it is only that.
    ///
    /// Beside [`Runner::about`] and asked for the same reason: the answer rides
    /// [`Event::ToolRequested`] while a call is out and is not kept in the
    /// transcript, so a session put back on the screen asks the tool again. A
    /// run of calls that folded into one line while it happened has to fold the
    /// same way on the way back in, or the session a reader resumes is not the
    /// one they left.
    ///
    /// `None` for a name no tool answers to, which is the right answer twice
    /// over: a refused call is named rather than counted.
    #[must_use]
    pub fn folds(&self, call: &ToolCall) -> Option<Looking> {
        self.describing(&call.name)
            .and_then(|entry| entry.tool().looking(&call.args))
    }

    /// What the next request would carry, in tokens.
    ///
    /// An estimate for the stretch nothing has reported on yet, and what the
    /// provider said for everything before it. Read by the wiring to decide
    /// whether a session picked up is worth asking about — the loop itself
    /// never asks, because a turn already running has nobody to ask.
    #[must_use]
    pub fn carrying(&self) -> u64 {
        self.state.load.tokens()
    }

    /// How much usable room remains before compaction, where a window is known.
    ///
    /// The between-turn read of the same prompt fact [`Event::Carried`]
    /// refreshes while a turn runs. The answer and tool-result reserve is not
    /// shown as room the transcript may still consume: zero is the safe
    /// compaction boundary, not the model's literal last token.
    #[must_use]
    pub fn left(&self) -> Option<u8> {
        self.left_under(self.policy.compaction)
    }

    /// The same reading, against the compaction answer given.
    ///
    /// One function so there is one reader: this session's own answer between
    /// turns, and the answer of the run in progress while a turn is running.
    /// Two of them, each doing its own arithmetic, is how a run that holds
    /// back less of the window than its session does came to be told the
    /// window was full.
    fn left_under(&self, compaction: Compaction) -> Option<u8> {
        self.state.load.left(
            self.state.window,
            self.reserve(compaction, self.state.window),
        )
    }

    /// Room that automatic compaction keeps free for an exchange in progress.
    ///
    /// The settings are handed in rather than read off the session, because
    /// the answer a turn runs under is the run's own: a run may keep back more
    /// of the window than its session does. Reading them here instead would
    /// measure one boundary against a figure the turn never agreed to, and the
    /// half that fires would be deciding for the half that never saw it.
    ///
    /// Three callers pass their own, and none of them is fixed to one answer.
    /// [`Runner::left_under`] takes whichever it is handed: [`Runner::left`]
    /// gives it the session's between turns, and [`Runner::compact`] gives it
    /// the run's while a turn is running. `exchange` passes the run's for the
    /// figure a turn starts on, and the pass loop passes it again for the
    /// figure the turn is re-measured against once a response has corrected
    /// the window.
    fn reserve(&self, compaction: Compaction, window: Option<u32>) -> u64 {
        if compaction.automatic {
            load::reserve(self.agent.model().max_tokens, window, compaction.reserve)
        } else {
            0
        }
    }

    /// Everything this session was told to hold a turn to.
    ///
    /// One reader for the whole answer rather than one per figure: a caller
    /// wanting a single field takes it off this, and the next field to be
    /// wanted needs no second accessor. Read-only and by value, because a
    /// session's ceiling is settled when it is assembled — [`Runner::turn`]
    /// and [`Runner::compact`] are the two entries a run is admitted through,
    /// and each holds it to this.
    #[must_use]
    pub const fn policy(&self) -> RunPolicy {
        self.policy
    }

    /// The stable operator instructions every turn is asked under, if any.
    ///
    /// Session facts are not included. They are assembled as typed transcript
    /// context once per pass so a changing fact does not rewrite this prefix.
    #[must_use]
    pub fn instructions(&self) -> Option<&str> {
        self.agent.instructions()
    }

    /// The permission mode this session is in, which the prompt shows at all
    /// times.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.permission.mode()
    }

    /// Steps that mode on to the next of the ring, and says which it is now.
    ///
    /// Reachable only between turns, because a turn owns the runner while it
    /// runs. That is not a rule this crate enforces so much as one the loop's
    /// shape already made true, and it is what leaves the engine needing no
    /// lock: nothing is deciding a call while the mode is being changed.
    pub fn cycle(&mut self) -> Mode {
        self.permission.cycle()
    }

    /// Puts the mode where the user named, rather than stepping to it.
    ///
    /// Reachable at the same moment [`Runner::cycle`] is and for the same
    /// reason.
    pub fn switch(&mut self, mode: Mode) {
        self.permission.switch(mode);
    }

    /// The model this session is asking, as the provider spells it.
    ///
    /// Empty where nothing has chosen one. That is a session that can do
    /// everything except take a turn, and the caller is what refuses the turn —
    /// this crate is handed a name and does not decide which names are real.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.agent.model().name
    }

    /// The tools this session is advertising, by name.
    ///
    /// Read from the exact immutable generation last admitted. The typed tools
    /// context section reports changes from that same generation; this accessor
    /// remains the between-turn view used by the terminal.
    ///
    /// Held to what the definition declares here rather than at the roster this
    /// is read from, because a session that has not taken a turn yet is holding
    /// the whole roster it was wired with: narrowing it in place would mean a
    /// definition deciding what a *run* admitted, and it is the pass that owns
    /// that. What the reader is shown is the same either way.
    #[must_use]
    pub fn offering(&self) -> Vec<String> {
        advertising(&self.agent, &self.state.tools)
            .into_iter()
            .map(|schema| schema.name.to_owned())
            .collect()
    }

    /// The maximum output carried with the next provider request.
    #[must_use]
    pub fn maximum_output(&self) -> u32 {
        self.agent.model().max_tokens
    }

    /// The context window used for proactive compaction, where known.
    #[must_use]
    pub fn context_window(&self) -> Option<u32> {
        self.state.window
    }

    /// What the model in force reads, where the caller said.
    ///
    /// The model's half alone: what may actually be put in front of it is this
    /// met with what the provider can spell, and the two are asked separately
    /// so a refusal can say which side of it said no.
    ///
    /// `None` is the caller never having said, which is a different thing from
    /// a model that reads nothing. It travels with the name through
    /// [`Runner::ask`], so this is the same answer the next request carries
    /// rather than a second lookup that could disagree with it.
    #[must_use]
    pub fn reads(&self) -> Option<Modalities> {
        self.agent.model().accepts
    }

    /// The provider this runner is asking, for the questions only it can answer.
    ///
    /// Handed out rather than answered here because what a caller wants of it
    /// is a fact about a protocol — what shapes it has a word for — and the
    /// runner drives `dyn Provider` precisely so it never has to know one.
    /// Borrowed, so nothing can take it or hold it past the turn.
    #[must_use]
    pub fn provider(&self) -> &dyn Provider {
        self.provider.as_ref()
    }

    /// The vendor this session is writing to, by the name it is asked for on
    /// the command line and written down under.
    ///
    /// Read off the provider rather than remembered beside it, so that
    /// [`Runner::serve`] cannot leave the two disagreeing: what a status row
    /// says is then the vendor the next turn actually reaches.
    #[must_use]
    pub fn serving(&self) -> &'static str {
        self.provider.name()
    }

    /// Asks a different model from the next turn on.
    ///
    /// The provider is not changed by this: which vendor is being written to was
    /// settled by which credential the wiring resolved, and a name that vendor
    /// does not serve comes back as its own refusal rather than as a silent
    /// redirection to one that does. [`Runner::serve`] is what changes it, and
    /// it costs a credential to call.
    ///
    /// Reachable between turns, where [`Runner::switch`] is and for the same
    /// reason: a turn owns the runner while it runs.
    ///
    /// The limits travel with the name. Usage reported by the previous model is
    /// not meaningful against either the new tokenizer or its window, so the
    /// transcript is recounted as a conservative estimate until the new model
    /// reports an exact request size of its own.
    pub fn ask(
        &mut self,
        model: &str,
        max_tokens: u32,
        window: Option<u32>,
        accepts: Option<Modalities>,
    ) {
        // A definition is not written to. The session selects another built
        // from this one under the same identity, so a request already out and
        // a run beside this one keep asking the model they started under.
        let aimed = Model {
            name: model.into(),
            max_tokens,
            window,
            accepts,
            effort: self.agent.model().effort,
        };
        self.agent = Arc::new(self.agent.aimed(aimed));
        self.state.window = window;
        self.state.load.reestimated();
    }

    /// Stands the session under different operator instructions from the next
    /// turn on.
    ///
    /// Model-visible session facts do not belong here. [`Runner::ask`],
    /// [`Runner::think`], permission changes, and toolset refreshes are observed
    /// by typed context assembly without rewriting these stable bytes.
    ///
    /// Reachable between turns, where [`Runner::ask`] is and for the same
    /// reason: a turn owns the runner while it runs.
    ///
    /// The empty string is nothing said, not a system field holding nothing.
    /// That reading belongs to the field rather than to this method — it is
    /// [`Agent::telling`] that applies it — and it is the reading a prompt key
    /// written empty already gets in the documents this text is built from.
    pub fn telling(&mut self, system: &str) {
        self.agent = Arc::new(self.agent.telling(system));
        self.state.load.requesting(
            self.agent.instructions(),
            &advertising(&self.agent, &self.state.tools),
        );
    }

    /// Writes to a different vendor from the next turn on.
    ///
    /// What a key given mid-session is for. Until there was one, the provider a
    /// run resolved at startup was the provider it died with, so a machine that
    /// had never logged in spent its whole first session refusing every turn —
    /// the key it was just given being read by a run that had already finished
    /// deciding.
    ///
    /// The transcript is kept across the swap, and that is deliberate rather
    /// than incidental: what was said is what the user said, and a vendor is who
    /// it gets sent to. What the new vendor may not be sent is cleared from it,
    /// and the lines saying so are owed to the session, as
    /// [`Runner::pick_up`] says. What does not carry is anything the old vendor knows
    /// about the old messages, which is nothing this program was ever told.
    ///
    /// Reachable between turns, where [`Runner::ask`] is and for the same
    /// reason: a turn owns the runner while it runs.
    pub fn serve(&mut self, provider: Box<dyn Provider>) {
        // Decided before the swap and against both vendors: a result recorded
        // before results said who answered them is judged by what the vendor
        // being left restricts, and by the next line there is nobody left to
        // ask. Staying with the same vendor moves nothing anywhere.
        let clearing = self.untransferable(0, provider.as_ref(), Some(self.provider.as_ref()));

        self.provider = provider;
        // As though a report had measured every message: the estimate is taken
        // again from the byte total below, and that total follows every message.
        self.clear_untransferable(&clearing, self.state.transcript.messages().len());
        // Cached-token and tokenizer semantics belong to the provider that
        // reported them. Keep the transcript, but not that provider's exact
        // reading of it.
        self.state.load.reestimated();
    }

    /// How hard this session is asking the model to think.
    ///
    /// `None` where nothing has said, which is not the middle rung: it is the
    /// field left off the request altogether, and what a vendor does with a
    /// request that does not carry one is the vendor's own default per model.
    #[must_use]
    pub fn effort(&self) -> Option<Effort> {
        self.agent.model().effort
    }

    /// Asks for a different rung from the next turn on.
    ///
    /// There is no way back to `None` from here, and that is the honest shape:
    /// a rung asked for is a rung the caller can see on the screen, and a
    /// session cannot un-see it by being handed the vendor's default again — a
    /// default this program is never told the name of.
    ///
    /// Reachable between turns, where [`Runner::ask`] is and for the same
    /// reason: a turn owns the runner while it runs.
    pub fn think(&mut self, effort: Effort) {
        let harder = Model {
            effort: Some(effort),
            ..self.agent.model().clone()
        };
        self.agent = Arc::new(self.agent.aimed(harder));
    }

    fn flush_sandbox_audits(&self, events: Reporter<'_>) -> Result<(), ToolError> {
        work::report_sandbox_registry(&self.sandbox_audits, events, &*self.store)
    }

    /// Writes one normalized cache fact to the durable framework journal and
    /// emits the same typed fact to the live event stream.
    fn report_prompt_cache(&self, run: &RunContext<'_>, fact: PromptCacheFact) {
        self.report_prompt_cache_to(&run.reporting(), fact);
    }

    fn report_prompt_cache_to(&self, events: &Reporter<'_>, fact: PromptCacheFact) {
        self.store
            .append_run_item(&RunItem::provider_attempt(events.ancestry(), fact.clone()));
        events.post(Event::PromptCache { fact });
    }

    /// A run against this session, under the policy the session was given.
    ///
    /// The only way in from outside: [`RunContext`] is minted in this crate,
    /// so the run a caller is handed is a root, and descending from it is this
    /// crate's. What that closes is the *context* — it does not close event
    /// attribution, because [`Ancestry`] and [`Reporter`] are public in
    /// `crucible-core` and three calls there will stamp an event with a run
    /// nothing started. Nothing shipped does: the one [`Post`] is the binary's
    /// relay, and the only [`Reporter`] outside tests comes from
    /// [`RunContext::reporting`]. So this is a run the caller cannot forge by
    /// accident, not one the types forbid forging.
    ///
    /// [`Ancestry`]: crucible_core::Ancestry
    ///
    /// A context carries no session either, so "against this session" is what
    /// the caller does and not something checked here: one context per unit of
    /// work, matching the run to the runner it was minted from. What *is*
    /// checked is the policy — [`Runner::turn`] and [`Runner::compact`] hold
    /// whatever they are handed to this session's ceiling before spending
    /// against it.
    ///
    /// Minting it out here rather than inside [`Runner::turn`] is what lets the
    /// caller report under the same run the work ran as — a [`TurnError`] is
    /// handed back rather than posted, and whoever asked for the work is the
    /// only one that can say it failed.
    ///
    /// The services are borrowed for as long as the run lasts and the session
    /// is not: the returned context does not hold this runner, so the same
    /// caller can go on to ask it for the turn.
    #[must_use]
    pub fn starting<'a>(
        &self,
        events: &'a dyn Post,
        cancel: &'a Cancel,
        steer: &'a Steer,
        aside: &'a Aside,
    ) -> RunContext<'a> {
        RunContext::new(self.policy, events, cancel, steer, aside)
    }

    /// Takes one turn: the prompt, and the exchange until the model yields.
    ///
    /// The run's cancel arrives cleared: [`Cancel::reset`] is the caller's to
    /// call, on the thread that reads the keyboard, before the thread this
    /// runs on exists. Clearing it here would clear a key pressed in between,
    /// so a flag found raised belongs to this turn and stops it — before the
    /// prompt is recorded and before a request goes out, whatever a given
    /// provider makes of being handed a cancel that is already up.
    ///
    /// One run covers the whole turn, including a turn refused on the way in:
    /// the pair of events that refusal posts is still something that happened,
    /// and an event with nothing to attribute it to is the one shape this path
    /// is not allowed to carry.
    ///
    /// # Errors
    ///
    /// [`TurnError`] wherever the turn could not be finished: the provider
    /// failed, the user refused a call, the turn produced more than a spend
    /// ceiling allowed, a compaction did not return a complete recap, the
    /// window had no room left and compacting it freed none, or tool results
    /// crossed the per-turn retained-output limit. Nothing a tool does is on
    /// that list: a call that could not be run at all, and one that ran and
    /// did not like what it found, both go back to the model as results it can
    /// work around.
    ///
    /// The turn itself never ends on [`TurnError::Unready`]: the provider's
    /// stream and each read of it, every prompt-cache step, every call's run,
    /// a background result's acceptance and the toolset's preparation,
    /// listing, refreshing and disposal are all awaited, so a step that would
    /// have had to wait is waited for rather than refused. A refusal still
    /// names its bridge — one of the crossings that remain outside the turn —
    /// and what the dropped step began is unconfirmed rather than undone.
    /// The turn's session writes are awaited, and a line the log could not
    /// keep is the session's to report rather than the turn's to end on.
    /// Before anything of the turn is recorded or sent, the lines picking a
    /// session up or changing vendor still owe the session are written, as
    /// [`Runner::record_clearings`] writes them.
    ///
    /// Every step the turn takes is awaited, so none ends the turn on a
    /// refusal. A stop ends it as it always has, through the cancel each
    /// awaited step was handed and that the turn looks at between them. A
    /// step of a compaction the turn made leaves what [`Runner::compact`]
    /// says it does.
    ///
    /// A tool source's own step that was dropped before it answered is the
    /// source's failure, which [`TurnError::Toolset`] or
    /// [`TurnError::ToolsetCleanup`] carries: as [`ToolsetError::Unready`]
    /// where the source crossed a bridge that could not wait, or in the
    /// source's own words as [`ToolsetError::Source`], which is how MCP
    /// hosting reports a step it gave up on; either way what that step began
    /// is unconfirmed rather than undone. A cleanup step of the source's that
    /// would have had to wait after another failure is reported as that
    /// failure, and named at most in its text.
    ///
    /// # Panics
    ///
    /// At the turn's first tool call where it is polled outside a Tokio
    /// runtime: every call's run is spawned onto the runtime the turn is
    /// polled in, and spawning outside one panics. The application waits for
    /// a turn inside its own runtime; a caller of its own polls the turn
    /// inside one, with a timer where a call has a deadline.
    ///
    /// [`ToolsetError::Unready`]: crucible_core::ToolsetError::Unready
    /// [`ToolsetError::Source`]: crucible_core::ToolsetError::Source
    pub async fn turn(
        &mut self,
        prompt: &str,
        attachments: Box<[Attachment]>,
        ask: &mut dyn Ask,
        run: &RunContext<'_>,
    ) -> Result<Turned, TurnError> {
        let turned = self.invoking(prompt, attachments, ask, run).await;

        // The invocation ends where the caller gets an answer it can act on,
        // and what the input checks made of these words ends with it. A
        // [`TurnError`] is not that: the turn did not finish, the caller may
        // try the same words again, and the decision committed for them still
        // covers the attempt.
        if turned.is_ok() {
            self.state.forget_checked();
        }
        turned
    }

    /// The turn itself, for [`Runner::turn`] to end the invocation around.
    ///
    /// # Errors
    ///
    /// [`TurnError`], exactly as [`Runner::turn`] describes.
    async fn invoking(
        &mut self,
        prompt: &str,
        attachments: Box<[Attachment]>,
        ask: &mut dyn Ask,
        run: &RunContext<'_>,
    ) -> Result<Turned, TurnError> {
        // Whatever the caller handed in, held to what this session allows.
        // See [`RunContext::held_to`]: the session's policy is the ceiling,
        // and a context that asks for more gets the session's figure.
        //
        // Here rather than in [`Runner::exchange`], which this is the only
        // shipped caller of. Putting it there would make a run whose
        // compaction rail differs from its session's unreachable through
        // `exchange`, and that difference is what
        // `a_pass_is_measured_against_the_room_its_own_run_holds` opposes to
        // tell a rail read off the run from one read off the session. The
        // ceiling belongs where a run is admitted, and the two admitting
        // entries are this and [`Runner::compact`].
        let run = &run.held_to(self.policy);

        // What picking a session up or changing vendor still owes the
        // session goes into its log before anything of this turn does.
        self.record_clearings().await;

        // The number this turn would have, worked out before it is known
        // whether the turn gets to take it. One expression rather than two, so
        // that the turn which runs and the turn which is stopped on the way in
        // cannot come to disagree about what the next one is called.
        let turn = if self.state.transcript.is_empty() {
            self.state.turn
        } else {
            self.state.turn.next()
        };

        let events = run.reporting();

        if run.cancel().requested() {
            return Ok(Turned::Ran(RunResult::new(
                run.run(),
                Self::stopped(turn, &events),
                Spend::NONE,
            )));
        }

        // Before the first request of this invocation, and before the turn is
        // announced. A refusal here is a turn that never happened: nothing is
        // recorded, nothing is sent, and no pair of events tells the reader a
        // turn began. A check reaching no decision is not a refusal and does
        // not say the prompt was rejected, because it did not say that.
        match self.checking_input(prompt, run) {
            Ok(Judged::Allowed) => {}
            Ok(Judged::Rejected(rejection)) => {
                return Ok(Turned::Rejected {
                    rejection,
                    stop: None,
                });
            }
            Err(problem) => {
                return Ok(Turned::Undecided {
                    problem,
                    stop: None,
                });
            }
        }

        self.state.turn = turn;
        events.post(Event::TurnStarted {
            turn: self.state.turn,
        });
        self.record(
            run.ancestry(),
            Message::User {
                text: prompt.into(),
                attachments,
            },
        )
        .await?;

        // Posted from here rather than from either place the exchange ends, so
        // that a turn cannot acquire a second way to finish without one. The
        // reason is what tells a truncated answer from a complete one, and it
        // has to reach the thread that draws — a return value never does.
        let turned = self.exchange(ask, run).await?;
        if let Some(stop) = turned.stop() {
            events.post(Event::TurnFinished {
                turn: self.state.turn,
                stop,
            });
        }

        Ok(turned)
    }

    /// What this invocation's input checks make of `prompt`.
    ///
    /// The first check that refuses is the answer: the rest have nothing left
    /// to decide, and asking them anyway would run whatever a check does for a
    /// living against words already on their way back to the caller.
    ///
    /// A session that declared none returns without copying the prompt or
    /// committing anything, which is every session that ships today and is the
    /// reason this costs one branch there rather than a prompt-sized write.
    ///
    /// The decision is committed for the length of one invocation. An
    /// invocation retried after it failed asks the same words again and is
    /// answered from what was committed rather than by running the checks a
    /// second time, so a check that has since changed its mind cannot overturn
    /// an invocation already under way; once the turn hands an answer back, the
    /// commit is dropped and the same words typed again are checked afresh. A
    /// check that could not decide commits nothing: `?` leaves before the
    /// commit, and the next attempt is a fresh one rather than one bound to a
    /// non-answer.
    ///
    /// # Errors
    ///
    /// [`GuardrailError`] where a check ran and could not reach a decision.
    fn checking_input(
        &mut self,
        prompt: &str,
        run: &RunContext<'_>,
    ) -> Result<Judged, GuardrailError> {
        if self.agent.input_guardrails().is_empty() {
            return Ok(Judged::Allowed);
        }
        if let Some(committed) = self.state.checked(prompt) {
            return Ok(committed.clone());
        }

        // The definition the checks are read off is held for the length of the
        // pass, so a check cannot be handed a list that something replaced
        // while it was being walked.
        let agent = Arc::clone(&self.agent);
        let context = AgentContext::new(run.run(), agent.id(), prompt);
        let mut decision = Judged::Allowed;
        for guard in agent.input_guardrails() {
            // The name is the one the check was declared under, never taken
            // from what it answered, whether it refused or could not say.
            match guard
                .check()
                .checking(&context)
                .map_err(|unsure| guard.unanswered(unsure.problem()))?
            {
                Decision::Allowed => {}
                Decision::Rejected(why) => {
                    decision = Judged::Rejected(guard.refused(&why));
                    break;
                }
            }
        }

        self.state.commit(prompt, decision.clone());
        Ok(decision)
    }

    /// What this agent's output checks make of `candidate`.
    ///
    /// Asked of the final candidate answer, before it is accepted and before
    /// it is written down. What streamed to the reader while it arrived is
    /// provisional and says so; what the transcript carries into the next
    /// request is not.
    ///
    /// Nothing is committed and nothing is retried: a check refuses a
    /// particular answer, and asking the model again for a different one is a
    /// decision for whoever asked the turn.
    ///
    /// # Errors
    ///
    /// [`GuardrailError`] where a check ran and could not reach a decision.
    fn vouching(&self, candidate: &str, run: &RunContext<'_>) -> Result<Judged, GuardrailError> {
        if self.agent.output_guardrails().is_empty() {
            return Ok(Judged::Allowed);
        }

        let context = AgentContext::new(run.run(), self.agent.id(), self.said());
        for guard in self.agent.output_guardrails() {
            match guard
                .check()
                .checking(&context, candidate)
                .map_err(|unsure| guard.unanswered(unsure.problem()))?
            {
                Decision::Allowed => {}
                Decision::Rejected(why) => {
                    return Ok(Judged::Rejected(guard.refused(&why)));
                }
            }
        }
        Ok(Judged::Allowed)
    }

    /// The last thing the caller said, which is what an invocation is about.
    ///
    /// Read back from the transcript rather than kept a second time beside it.
    /// A line typed while the turn ran is the caller speaking too, and a check
    /// that was shown the opening prompt after the reader had moved past it
    /// would be judging an invocation nobody is still taking.
    fn said(&self) -> &str {
        self.state
            .transcript
            .messages()
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::User { text, .. } => Some(&**text),
                _ => None,
            })
            .unwrap_or("")
    }

    /// Ends a turn the user stopped before it began, and says so twice.
    ///
    /// Nothing is recorded, and `turn` is not kept. The prompt was never sent,
    /// so the transcript has no half of an exchange to explain and the model is
    /// never told a question that was withdrawn a moment after it was asked —
    /// which is what recording it would come to, since every request afterwards
    /// carries it. The count follows the transcript, so a turn that adds
    /// nothing to it leaves the number free: this announces the number it would
    /// have had, and the next prompt is that turn, taken for real.
    ///
    /// Both events go out all the same, because the screen is the other record:
    /// a start with no finish leaves the turn looking as though it is still
    /// running, and a finish with no start is a shape nothing else here
    /// produces.
    fn stopped(turn: TurnId, events: &Reporter<'_>) -> StopReason {
        let stop = StopReason::Cancelled;

        events.post(Event::TurnStarted { turn });
        events.post(Event::TurnFinished { turn, stop });

        stop
    }

    /// Passes of asking and running, until something ends the turn.
    ///
    /// A failure returns instead, and the caller posts nothing: the failure is
    /// its own event, and a turn with two endings on screen has one too many.
    ///
    /// Everything the loop needs that is not the session arrives in `run`: who
    /// it is, how to stop it, what the reader typed at it, what finished behind
    /// it, where its progress goes, and what it may spend. A test that wants
    /// a turn to cross the tool-output ceiling lowers that figure in the run's
    /// policy rather than printing megabytes to get there.
    ///
    /// The permission prompt stays outside it, because asking is `&mut`.
    async fn exchange(
        &mut self,
        ask: &mut dyn Ask,
        run: &RunContext<'_>,
    ) -> Result<Turned, TurnError> {
        // Not held to the session here. [`Runner::turn`] does it, and is the
        // only caller that ships; a test reaching this directly is asking for
        // the run exactly as it wrote it. A second entry that reaches a
        // provider without passing through `turn` has to take the ceiling
        // itself, the way [`Runner::compact`] does.

        let toolsets = ToolsetContext::new(run.ancestry(), run.cancel().clone(), None)
            .with_sandbox_audits(self.sandbox_audits.clone());
        let prepared = self
            .toolset
            .prepare(&toolsets)
            .await
            .map_err(TurnError::from);
        let prepared = combine_sandbox_audit(prepared, self.flush_sandbox_audits(run.reporting()));
        let ran = match prepared {
            Ok(()) => {
                // The turn's own running totals. A bound only where somebody asked for
                // one: a turn that runs long because there is work in it is not a turn
                // to stop, and what a runaway one actually consumes is this.
                //
                // Held out here rather than inside the passes because the loop has
                // enough ways out that carrying the total back through each return
                // value would mean writing it at every one. What that buys is the
                // bookkeeping, not the reporting: the `?` below leaves on the failure
                // exits without a result, and a `TurnError` has nowhere to put a
                // figure, so only a turn that ended says what it spent.
                let mut counting = Counting {
                    spent: Spend::NONE,
                    load: self.state.load,
                    window: self.state.window,
                    reserve: self.reserve(run.policy().compaction, self.state.window),
                };

                AgentLoop::new(self, run, ask, &toolsets)
                    .drive(&mut counting)
                    .await
                    .map(|ending| ending.turned(run.run(), counting.spent))
            }
            Err(problem) => Err(problem),
        };

        let finished = match (ran, self.toolset.dispose(&toolsets).await) {
            (Ok(result), Ok(())) => Ok(result),
            (Ok(_), Err(cleanup)) => Err(TurnError::Toolset(cleanup)),
            (Err(primary), Ok(())) => Err(primary),
            (Err(primary), Err(cleanup)) => Err(TurnError::ToolsetCleanup {
                primary: Box::new(primary),
                cleanup,
            }),
        };
        combine_sandbox_audit(finished, self.flush_sandbox_audits(run.reporting()))
    }

    /// Makes room, and says what the turn may do next.
    ///
    /// [`After::Stuck`] where two goes in a row freed nothing, which is the one
    /// way this loop could spin without the transcript growing: everything else
    /// it does either adds to the transcript or ends the turn. The caller
    /// decides what to say about it, because the rails reached here for
    /// different reasons and owe the reader different sentences.
    ///
    /// # Errors
    ///
    /// [`TurnError`] wherever [`Runner::compact`] fails, which says what each
    /// failure leaves.
    async fn made_room(
        &mut self,
        why: Compacting,
        run: &RunContext<'_>,
        fruitless: &mut u8,
        spent: &mut Spend,
    ) -> Result<After, TurnError> {
        match self.compact(why, run, spent).await? {
            // Not counted against the goes this loop is allowed, because it was
            // not a go: nothing was replaced and nobody is going to ask again.
            Room::Stopped => return Ok(After::Stopped),
            Room::Made(compacted) if compacted.after < compacted.before => *fruitless = 0,
            Room::Made(_) | Room::Nothing => *fruitless += 1,
        }

        Ok(if *fruitless < COMPACTIONS_WITHOUT_PROGRESS {
            After::Carry
        } else {
            After::Stuck
        })
    }

    /// Whether the turn is over, and why — or `None` to run the calls.
    ///
    /// Every variant is named rather than caught by a rest pattern: a reason
    /// added to [`StopReason`] has to stop the build here, where the decision
    /// is whether a turn goes on, rather than be waved through as an ending.
    fn over(said: StopReason, calls: &[ToolCall]) -> Option<StopReason> {
        match said {
            StopReason::WantsTools if !calls.is_empty() => None,
            // Waiting on nothing is yielding, whatever the provider called it.
            // Believing it instead would re-send an unchanged transcript and
            // ask the same question until the user noticed.
            StopReason::WantsTools => Some(StopReason::Yielded),
            StopReason::Yielded
            | StopReason::OutOfTokens
            | StopReason::WindowExceeded
            | StopReason::Filtered
            | StopReason::Paused
            | StopReason::Cancelled
            | StopReason::Unknown => Some(said),
        }
    }

    /// One request, read to the end, and the reason it ended.
    ///
    /// An answer that breaks off part way is recorded before the failure
    /// leaves: those deltas were posted as they arrived, so the user has
    /// already read them, and a transcript that does not hold them is one the
    /// user and the model disagree about — the next prompt would follow the
    /// last one with nothing in between. Calls the model never finished asking
    /// for go no further, the same as when it stops early. What is recorded
    /// says the answer never reached an ending, which is what keeps the next
    /// request and a later replay from reading it as one that did. The line is
    /// awaited before the failure leaves, and the failure is what leaves.
    ///
    /// A stream that ends without saying why is that same failure: the socket
    /// went quiet, and quiet is what a finished response and a truncated one
    /// have in common.
    ///
    /// A failure that reached none of that is asked again instead, up to
    /// [`crate::Retry::attempts`] times. The one it exists for is a connection
    /// the provider closed while the tools ran — the turn's own pauses are
    /// exactly where a pooled connection goes stale, so the request that fails
    /// is the one after a tool pass rather than the first, and the discussion
    /// stops part way through. All three parts of the condition carry weight:
    /// only a failure [`ProviderError::transient`] calls a moment rather than
    /// a request, and only a response that has neither kept a word of what it
    /// said nor given a reason for stopping. Deltas are posted as
    /// they arrive, so re-asking after one would put an answer on screen twice
    /// and leave the transcript holding the half that was taken back.
    ///
    /// [`ProviderError::transient`]: crucible_core::ProviderError::transient
    async fn listen(
        &mut self,
        bounds: &TurnBounds,
        mut listening: Listening<'_>,
    ) -> Result<(Answer, StopReason), TurnError> {
        let mut left = listening.run.policy().retry.attempts;
        let mut pause = listening.run.policy().retry.first_pause;

        loop {
            let mut answer = Answer::within(
                self.provider.name(),
                bounds.retained,
                listening.run.policy().bounds.response_bytes,
            );

            let problem = match self.hearing(&mut answer, &mut listening).await {
                Ok(said) => return Ok((answer, said)),
                Err(problem) => problem,
            };

            if left > 0 && Self::again(&problem, &answer) {
                left -= 1;
                listening.run.reporting().post(Event::Retrying);

                // A pause the user sat through and then had to interrupt would
                // be this program keeping them waiting rather than the provider.
                if Self::pausing(pause, listening.run.cancel()) {
                    pause = pause.saturating_mul(2);
                    continue;
                }
            }

            let stop = answer.stop();
            let (text, _) = answer.finish();
            // The transcript refuses only a continuation, which this message
            // does not carry, and the session's line is awaited rather than
            // refused, so recording it meets nothing. Were it to, that would be
            // about the record rather than the request, and must not stand in
            // for the failure being recorded.
            let _recorded = self
                .record(
                    listening.run.ancestry(),
                    Message::Agent {
                        continuation: None,
                        text,
                        calls: Vec::new(),
                        stop,
                    },
                )
                .await;

            return Err(problem);
        }
    }

    /// What an attachment has to be for this request to carry its bytes.
    ///
    /// Both halves, and the narrower of the two decides: the model's, which the
    /// wiring above resolved from a table, and the provider's, which is what
    /// this build can actually write into a request. A model that reads video
    /// and a protocol module with no word for one leave nothing between them,
    /// and a set with nothing in it is the honest answer to that.
    fn carries(&self) -> Modalities {
        self.agent
            .model()
            .accepts
            .unwrap_or_else(Modalities::empty)
            .intersection(self.provider.spells())
    }

    /// One request, read to the end, recording nothing either way.
    ///
    /// Separate from [`Self::listen`] because what a failed response leaves in
    /// the transcript depends on whether it is going to be asked again, and that
    /// question is asked once rather than at each place the reading can fail.
    async fn hearing(
        &mut self,
        answer: &mut Answer,
        listening: &mut Listening<'_>,
    ) -> Result<StopReason, TurnError> {
        // Both locals are the request's whole hold on the bytes: `resolved`
        // owns them, `attached` is what the provider borrows, and the pass
        // returning drops the pair. Nothing read here survives one request.
        let resolved = attachments::resolve(&self.state.transcript, self.carries());
        let attached = resolved.attached();
        // What the ceiling let through, not what the transcript refers to. An
        // entry the pass aged out is a sentence by the time it gets here, and
        // a sentence is text at the rate text is already charged at.
        listening.counting.load.responding(
            attached
                .iter()
                .filter(|one| matches!(one.content, Content::Bytes(_)))
                .count(),
        );
        // Once per request rather than once per turn. Going out short is a
        // fact about the request rather than about the turn, and a retry sends
        // a second one — a reader watching that answer arrive is owed the same
        // sentence about it.
        let aged = resolved.aged(&self.state.transcript);
        if !aged.is_empty() {
            listening.run.reporting().post(Event::Aged { files: aged });
        }
        // Beside it rather than folded into it: a file the model does not read
        // stayed behind for a reason the reader answers differently, and a row
        // that said one thing about both would be wrong about one of them.
        let unread = resolved.unread(&self.state.transcript);
        if !unread.is_empty() {
            listening
                .run
                .reporting()
                .post(Event::Unread { files: unread });
        }

        let pricing_date = pricing_today();
        let (mut stream, cache_observation) = {
            // Built from disjoint fields here because persistent-resource
            // preparation may mutably update its dedicated store while the
            // provider request borrows transcript/spec data. A helper borrowing
            // the whole runner would falsely make those owners overlap.
            let request = Request {
                purpose: crucible_core::RequestPurpose::Turn,
                model: &self.agent.model().name,
                transcript: &self.state.transcript,
                tools: listening.advertised,
                max_tokens: self.agent.model().max_tokens,
                system: self.agent.instructions(),
                effort: self.agent.model().effort,
                attached: &attached,
                prompt_cache: None,
            };
            let capabilities = self
                .provider
                .prompt_cache_capabilities(&self.agent.model().name);
            let model_revision = capabilities.model_revision();
            let route = self.provider.prompt_cache_route();
            let authority = PermissionsSection::new(&self.permission)
                .snapshot()
                .to_string();
            let workspace = self.context.workspace();
            let user = self.store.owner();
            let session = self.store.session_id();
            let scope = ScopeInputs {
                route,
                policy: listening.run.policy().prompt_cache,
                model: &self.agent.model().name,
                model_revision,
                max_tokens: self.agent.model().max_tokens,
                effort: self.agent.model().effort,
                run: listening.run.run(),
                session: session.as_ref().map(crucible_core::SessionId::as_str),
                workspace,
                user: user
                    .as_ref()
                    .map_or(&[], crucible_core::SessionOwner::as_bytes),
                trust: b"local-workspace-authority-v1",
                authority: authority.as_bytes(),
                instructions: self.agent.instructions().unwrap_or_default().as_bytes(),
                tool_generation: listening.generation.context_id(),
            };
            self.state.prompt_cache_owner_scope = Some(prompt_cache::owner_scope(&scope));
            let mut resource_facts = Vec::new();
            let prepared = match (
                self.provider.prompt_cache_resources(),
                self.prompt_cache_store.as_deref_mut(),
            ) {
                (Some(lifecycle), Some(store)) => {
                    prompt_cache::prepare_with_resource_facts(
                        &request,
                        capabilities,
                        &scope,
                        prompt_cache::ResourceInputs {
                            store,
                            lifecycle,
                            cancel: listening.run.cancel(),
                            now: unix_now(),
                            deadline: std::time::Instant::now() + PROMPT_CACHE_RESOURCE_DEADLINE,
                        },
                        &mut resource_facts,
                    )
                    .await
                }
                _ => prompt_cache::prepare(&request, capabilities, &scope).await,
            };
            for fact in resource_facts {
                self.report_prompt_cache(listening.run, PromptCacheFact::ResourceChanged(fact));
            }
            let prepared = prepared?;
            let mut cache = prepared.request();
            self.state.prompt_cache_attempt = Some(PromptCacheAttempt {
                id: cache.attempt,
                capabilities: cache.capabilities.clone(),
                policy: cache.policy,
                selection: cache.selection,
                encoding: PromptCacheEncoding::NoControlIntended,
                disposition: PromptCacheRequestDisposition::NotSent,
                outcome: PromptCacheOutcome::Unreported,
                usage: None,
                cost: UsageCost::UNKNOWN,
            });
            self.report_prompt_cache(
                listening.run,
                PromptCacheFact::Planned(Box::new(cache.planned())),
            );
            let mut encoding = self.provider.prompt_cache_encoding(&Request {
                prompt_cache: Some(&cache),
                ..request
            });
            if let Some(attempt) = self
                .state
                .prompt_cache_attempt
                .as_mut()
                .filter(|attempt| attempt.id == cache.attempt)
            {
                attempt.encoding = encoding;
            }
            if let PromptCacheEncoding::Failed(reason) = encoding {
                self.report_prompt_cache(
                    listening.run,
                    PromptCacheFact::RequestEncoded(PromptCacheRequestFact {
                        attempt: cache.attempt,
                        encoding,
                        disposition: PromptCacheRequestDisposition::NotSent,
                    }),
                );
                cache = prepared
                    .fallback_request(reason)
                    .ok_or(PromptCachePreparationError::Encoding(reason))?;
                self.report_prompt_cache(
                    listening.run,
                    PromptCacheFact::Planned(Box::new(cache.planned())),
                );
                encoding = self.provider.prompt_cache_encoding(&Request {
                    prompt_cache: Some(&cache),
                    ..request
                });
                if let Some(attempt) = self
                    .state
                    .prompt_cache_attempt
                    .as_mut()
                    .filter(|attempt| attempt.id == cache.attempt)
                {
                    attempt.selection = cache.selection;
                    attempt.encoding = encoding;
                }
                if let PromptCacheEncoding::Failed(reason) = encoding {
                    return Err(PromptCachePreparationError::Encoding(reason).into());
                }
            }

            let request = Request {
                prompt_cache: Some(&cache),
                ..request
            };
            let streamed = self.provider.stream(request, listening.run.cancel()).await;
            // Recorded and reported before a failure ends the turn.
            let disposition = request_disposition(&streamed);
            if let Some(attempt) = self
                .state
                .prompt_cache_attempt
                .as_mut()
                .filter(|attempt| attempt.id == cache.attempt)
            {
                attempt.disposition = disposition;
            }
            self.report_prompt_cache(
                listening.run,
                PromptCacheFact::RequestEncoded(PromptCacheRequestFact {
                    attempt: cache.attempt,
                    encoding,
                    disposition,
                }),
            );
            (
                streamed?,
                CacheObservation {
                    attempt: cache.attempt,
                    reporting: cache.capabilities.usage(),
                    model_revision: cache.capabilities.model_revision(),
                    retention: cache
                        .selection
                        .selected()
                        .map_or(cache.policy.retention().class(), |selected| {
                            selected.retention()
                        }),
                    pricing_date,
                },
            )
        };

        self.hear(stream.as_mut(), answer, listening, cache_observation)
            .await?;
        // EOF is itself a read. Cancellation can arrive during that read even
        // when the stream returns no final delta, so check the run's authority
        // again before making any native state or tool call replayable.
        if listening.run.cancel().requested() {
            return Ok(StopReason::Cancelled);
        }
        answer.finalize().map_err(TurnError::from)
    }

    /// Whether this failure, on this much of an answer, is worth asking again.
    ///
    /// The stop is checked beside the bytes because a response can fail after
    /// one and hold nothing: what a stop reason has already told the turn is as
    /// much a thing not to say twice as a sentence the user has read.
    fn again(problem: &TurnError, answer: &Answer) -> bool {
        matches!(problem, TurnError::Provider(failure) if failure.transient())
            && answer.retained() == 0
            && !answer.has_progress()
            && answer.stop().is_none()
    }

    /// Waits out a pause, and says whether it ran to the end.
    ///
    /// In slices, because the thread this runs on is the one holding the turn:
    /// a user who presses Esc during a pause is answered at the next slice
    /// rather than when the provider would have been asked again.
    fn pausing(pause: Duration, cancel: &Cancel) -> bool {
        let mut left = pause;

        while !left.is_zero() {
            if cancel.requested() {
                return false;
            }

            let slice = left.min(CANCEL_SLICE);
            thread::sleep(slice);
            left -= slice;
        }

        !cancel.requested()
    }

    /// Reads deltas into `answer` until the stream ends.
    async fn hear(
        &mut self,
        stream: &mut dyn DeltaStream,
        answer: &mut Answer,
        listening: &mut Listening<'_>,
        cache_observation: CacheObservation,
    ) -> Result<(), TurnError> {
        let events = listening.run.reporting();
        let counting = &mut *listening.counting;
        // What the turn had spent before this response opened. Each reading a
        // provider sends is this response's total rather than an increment, so
        // it is added to that fixed number and not to the last reading — which
        // is also what makes a provider that sends one final figure and one
        // that counts up as it goes come out the same.
        let before = counting.spent;

        while let Some(delta) = stream.next().await {
            match delta? {
                Delta::Text(text) => {
                    let bytes = text.len();
                    answer.say(&text)?;
                    events.post(Event::Delta { text });
                    Self::output_grew(&events, counting, bytes);
                }
                Delta::ToolStarted { id, name } => {
                    let bytes = id.as_str().len().saturating_add(name.len());
                    answer.calling(id, name)?;
                    Self::output_grew(&events, counting, bytes);
                }
                Delta::ToolArgs(fragment) => {
                    let bytes = fragment.len();
                    answer.arguments(&fragment)?;
                    Self::output_grew(&events, counting, bytes);
                }
                Delta::Continuation(state) => {
                    answer.continuing(state, self.state.transcript.continuation_room())?;
                }
                Delta::Progress => answer.progressed()?,
                Delta::Usage(usage) => {
                    let usage = merge_usage(
                        self.state
                            .prompt_cache_attempt
                            .as_ref()
                            .filter(|cache| cache.id == cache_observation.attempt)
                            .and_then(|cache| cache.usage.as_ref()),
                        usage,
                        self.provider.name(),
                    )?;
                    if let Some(tokens) = usage.output {
                        let said = Spend::new(tokens);
                        counting.spent = before.and(said);
                        counting.load.spent(said);
                        events.post(Event::Spent {
                            spend: counting.spent,
                        });
                    }
                    if let Some(tokens) = usage.input.total {
                        let carried = crucible_core::Carried::new(tokens);
                        counting.load.carried(carried);
                        if counting
                            .window
                            .is_some_and(|window| carried.tokens() > u64::from(window))
                        {
                            counting.window = None;
                        }
                    }
                    let cost = self
                        .provider
                        .prompt_cache_pricing(
                            &self.agent.model().name,
                            cache_observation.model_revision,
                            usage.input.total,
                            cache_observation.retention,
                            cache_observation.pricing_date,
                        )
                        .ok()
                        .flatten()
                        .and_then(|pricing| pricing.cost(&usage).ok())
                        .unwrap_or(UsageCost::UNKNOWN);
                    let outcome = usage.input.outcome(cache_observation.reporting);
                    if let Some(cache) = self
                        .state
                        .prompt_cache_attempt
                        .as_mut()
                        .filter(|cache| cache.id == cache_observation.attempt)
                    {
                        cache.outcome = outcome;
                        cache.usage = Some(usage.clone());
                        cache.cost = cost;
                    }
                    self.report_prompt_cache(
                        listening.run,
                        PromptCacheFact::UsageReported(Box::new(PromptCacheUsageFact {
                            attempt: cache_observation.attempt,
                            outcome,
                            usage,
                            cost,
                        })),
                    );
                    events.post(Event::Carried {
                        left: counting.left(),
                    });
                }
                Delta::Spent(said) => {
                    counting.spent = before.and(said);
                    counting.load.spent(said);
                    events.post(Event::Spent {
                        spend: counting.spent,
                    });
                    // Output occupies the same context window as input. Report
                    // the percentage again as it grows rather than leaving the
                    // opening input count on screen for the whole response.
                    events.post(Event::Carried {
                        left: counting.left(),
                    });
                }
                // Not added to the spend beside it, and not accumulated at
                // all. What a request carried is a level rather than a total —
                // the transcript goes whole to the provider every time, so each
                // reading supersedes the last instead of extending it, and a
                // running sum of them would describe a session nobody had.
                Delta::Carried(carried) => {
                    counting.load.carried(carried);

                    // A request that carried more than this model was believed
                    // to accept is that belief disproved by the only authority
                    // there is. What it does not give is a replacement: the
                    // vendor has shown this much fits and nothing about how
                    // much more would have.
                    //
                    // So the size becomes unknown rather than becoming this
                    // number — which would say the window is exactly as large
                    // as the thing that just fitted in it, and pin the reading
                    // at nothing all over again. Unknown is a state everything
                    // here already handles: no reading is drawn, nothing
                    // compacts against a figure nobody has, and the provider
                    // refusing is what makes room. A request *smaller* than the
                    // window is evidence of nothing and changes none of it.
                    if counting
                        .window
                        .is_some_and(|window| carried.tokens() > u64::from(window))
                    {
                        counting.window = None;
                    }

                    events.post(Event::Carried {
                        left: counting.left(),
                    });
                }
                Delta::Stopped(stop) => answer.stopped(stop)?,
            }
        }

        Ok(())
    }

    /// Updates the reading when unreported response bytes cross a percentage.
    fn output_grew(events: &Reporter<'_>, counting: &mut Counting, bytes: usize) {
        let before = counting.left();
        counting.load.produced(bytes);
        let left = counting.left();
        if left != before {
            events.post(Event::Carried { left });
        }
    }

    /// Where a transcript that already happened leaves the count.
    ///
    /// The turn a continued session is *on*, not the one after it: the loop
    /// steps the count on its way into a turn, and numbering the first
    /// continued turn `1` would tell the user this is a new session, which is
    /// exactly what they asked it not to be.
    fn counting(transcript: &Transcript) -> TurnId {
        (1..transcript.turns()).fold(TurnId::FIRST, |turn, _| turn.next())
    }
}

fn request_disposition<T>(result: &Result<T, ProviderError>) -> PromptCacheRequestDisposition {
    match result {
        Ok(_) => PromptCacheRequestDisposition::Accepted,
        Err(
            ProviderError::Cancelled(_)
            | ProviderError::Credential { .. }
            | ProviderError::Unconfigured(_),
        ) => PromptCacheRequestDisposition::NotSent,
        Err(ProviderError::Refused { .. } | ProviderError::WindowExceeded { .. }) => {
            PromptCacheRequestDisposition::Rejected
        }
        Err(
            ProviderError::Limit { .. }
            | ProviderError::Transport { .. }
            | ProviderError::Upstream { .. }
            | ProviderError::Protocol { .. },
        ) => PromptCacheRequestDisposition::Unknown,
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn pricing_today() -> crucible_core::PricingDate {
    crucible_core::PricingDate::from_unix_seconds(unix_now())
}

/// Joins cumulative/partial usage fields belonging to one provider attempt.
fn merge_usage(
    previous: Option<&ProviderUsage>,
    newer: ProviderUsage,
    provider: &'static str,
) -> Result<ProviderUsage, ProviderError> {
    let merged = match previous {
        Some(previous) => previous.merged(&newer),
        None => Ok(newer),
    };
    merged.map_err(|problem| ProviderError::Protocol {
        provider,
        problem: format!("invalid cumulative usage accounting: {problem}").into(),
    })
}

/// A failed audit cannot replace the primary failure or turn a successful
/// operation into an apparently successful, unaudited lifecycle.
fn combine_sandbox_audit<T>(
    result: Result<T, TurnError>,
    audit: Result<(), ToolError>,
) -> Result<T, TurnError> {
    match (result, audit) {
        (result, Ok(())) => result,
        (Ok(_), Err(audit)) => Err(TurnError::Tool(audit)),
        (Err(primary), Err(audit)) => Err(TurnError::SandboxAudit {
            primary: Box::new(primary),
            audit,
        }),
    }
}

#[cfg(test)]
mod tests;
