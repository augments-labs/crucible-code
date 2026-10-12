//! The runner and the session it records into, held together for as long as
//! the run lasts.
//!
//! The runner writes through a storage contract and never learns what is
//! behind it: closing the log, browsing it, reporting on it and swapping it for
//! another are the application's, so the application keeps the session it
//! built beside the runner that records into it, and the name of the provider
//! that runner is asking beside both.
//!
//! Every change a front end makes to a conversation is a method here, or in
//! [`crate::switching`], with no terminal in the signature: a prompt goes in as
//! text and attachments, a switch goes in as the provider and model wanted, and
//! what came of either comes back as a value the front end decides how to
//! show. The runner itself is never handed out: what a front end reads of it
//! comes through [`Settled`], a read of its state between turns, so the
//! session it records into and the provider it is said to be asking cannot be
//! moved from outside without the other half moving with them, and what is
//! read names what any backend a conversation runs on would have to answer.
//!
//! **A turn is awaited by whoever asks for it.** The runner's turn and
//! compaction are asynchronous, and so are [`Conversation::turn`] and
//! [`Conversation::compact`]: a front end runs each as a task on the
//! application's runtime, or awaits it inside one, and is answered when it
//! ends. A stop is the turn's own to answer, through the cancel on its run, so
//! it ends the way a stopped turn ends rather than as a future dropped at a
//! step, which could leave a call recorded with no result.
//!
//! **So are explicit prompt-cache operations**, and what picking a session up
//! or changing vendor owes the session. Each clears from the transcript what
//! the vendor being asked may not be sent, and the runner owes the session the
//! lines saying so; the conversation awaits the session taking them after each
//! pick-up and each change of vendor, and a turn or compaction writes any
//! still owed before anything of its own.

use std::path::Path;
use std::sync::Arc;

use crucible_context::Room;
use crucible_models::{
    Effort, FastForm, PromptCacheAttempt, PromptCacheCapabilities, Served, Speed,
};
use crucible_runner::{
    Breakdown, Post, PromptCacheCleanup, RunContext, RunPolicy, Runner, Totals, TurnError, Turned,
};
use crucible_runtime::{Aside, Cancel, Steer};
use crucible_session::{FilePromptCacheResourceStore, Session, SessionError};
use crucible_tools::{Ask, Looking, Mode, Summary};
use crucible_types::{
    Attachment, Compacting, Modalities, PlanWindows, PromptCachePolicy, PromptCacheResourceError,
    PromptCacheResourceRecord, SessionId, Spend, ToolCall, Transcript,
};
use crucible_workspace::Workspace;

/// What a conversation runs on.
///
/// Closed, and matched where it is read, so a backend added here makes every
/// read of the conversation decide what that backend answers.
#[derive(Debug)]
enum Backed {
    /// This program's own runner.
    Native(Runner),
}

/// A runner, the session it records into, and the provider it is asking.
#[derive(Debug)]
pub struct Conversation {
    backed: Backed,
    session: Arc<Session>,
    /// The registry name of the provider being asked, or `None` where nobody
    /// is. Not the runner's own word for it: a provider names itself for the
    /// session log, and the registry names it for `/model`, the settings file
    /// and the credential store, which is the name every switch is decided by.
    pub(crate) serving: Option<&'static str>,
    /// Whether some provider could be reached when nobody was chosen: set
    /// where the provider standing in for nobody is put in place, from the
    /// same credentials the sentence it refuses with was chosen from, read
    /// again after every `/login` and `/logout` that leaves nobody chosen, and
    /// read only while [`Self::serving`] is `None`. The store as this
    /// conversation last read it, so a change another process makes in
    /// between is not seen until then.
    pub(crate) reachable: bool,
    /// The yes given to each route whose vendor uses what is sent, where the
    /// run holds one: a turn on such a route with no yes is asked about
    /// before anything is sent.
    consent: Option<crate::content_use::Consent>,
    /// What the two web tools answer through: built for the provider and model
    /// being asked, and built again by each switch of either.
    pub(crate) web: crate::following::Following,
    /// When the plan behind each credential was last asked how much of its
    /// limits is used, and which credentials are not to be asked again.
    pub(crate) asks: crate::client::asking::Asks,
}

impl Conversation {
    /// A conversation recording into `session`, over the runner `build` makes
    /// of it, asking the provider the registry calls `serving`.
    ///
    /// `build` is handed the session as the store its runner is to record
    /// into, which is how the two come to be the same one. That much is
    /// trusted rather than checked: a runner does not say what it writes to,
    /// so a `build` that set the session aside and recorded somewhere else
    /// would go unnoticed here. `serving` is trusted the same way — it is the
    /// name the caller resolved the runner's provider under.
    #[must_use]
    pub fn recording(
        session: Arc<Session>,
        serving: Option<&'static str>,
        build: impl FnOnce(Arc<Session>) -> Runner,
    ) -> Self {
        Self {
            backed: Backed::Native(build(Arc::clone(&session))),
            session,
            serving,
            reachable: false,
            consent: None,
            web: crate::following::Following::default(),
            asks: crate::client::asking::Asks::default(),
        }
    }

    /// The same conversation, its web tools answering through `web`, which a
    /// switch of model or provider builds again.
    #[must_use]
    pub(crate) fn following(self, web: crate::following::Following) -> Self {
        Self { web, ..self }
    }

    /// The same conversation, asking `consent` before a turn goes on a route
    /// whose vendor uses what is sent.
    #[must_use]
    pub fn consenting(self, consent: crate::content_use::Consent) -> Self {
        Self {
            consent: Some(consent),
            ..self
        }
    }

    /// What a turn on a route whose vendor uses what is sent is asked about
    /// against, where the run holds one.
    #[must_use]
    pub fn consent(&self) -> Option<&crate::content_use::Consent> {
        self.consent.as_ref()
    }

    /// The same conversation, remembering the persistent prompt-cache
    /// resources it is authorized to make in a file under `home`.
    ///
    /// Nothing is opened here: the store touches the disk only once a policy
    /// and a model's capabilities together select a persistent mechanism.
    #[must_use]
    pub fn remembering_caches_in(self, home: &Path) -> Self {
        let Backed::Native(runner) = self.backed;
        Self {
            backed: Backed::Native(
                runner.with_prompt_cache_store(FilePromptCacheResourceStore::in_home(home)),
            ),
            ..self
        }
    }

    /// What is settled between turns: the model, the mode, the transcript.
    /// What changes any of it is a method of this type.
    #[must_use]
    pub const fn settled(&self) -> Settled<'_> {
        Settled(self.native())
    }

    /// The runner itself, for a test outside this crate that reads what no
    /// front end does. Compiled only with the `proof` feature, which nothing
    /// that ships turns on.
    #[cfg(feature = "proof")]
    #[must_use]
    pub const fn native_runner(&self) -> Option<&Runner> {
        Some(self.native())
    }

    /// The runner this conversation drives.
    pub(crate) const fn native(&self) -> &Runner {
        let Backed::Native(runner) = &self.backed;
        runner
    }

    /// The runner this conversation drives, to change.
    pub(crate) fn native_mut(&mut self) -> &mut Runner {
        let Backed::Native(runner) = &mut self.backed;
        runner
    }

    /// The handle a turn or a compaction runs under: what it reports goes to
    /// `events`, and it stops, steers and takes asides through the rest.
    #[must_use]
    pub fn starting<'a>(
        &self,
        events: &'a dyn Post,
        cancel: &'a Cancel,
        steer: &'a Steer,
        aside: &'a Aside,
    ) -> RunContext<'a> {
        self.native().starting(events, cancel, steer, aside)
    }

    /// The session being recorded into.
    #[must_use]
    pub const fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// The registry name of the provider being asked, or `None` where a run
    /// started with nobody to ask or signed out of the one it had.
    #[must_use]
    pub const fn serving(&self) -> Option<&'static str> {
        self.serving
    }

    /// What a turn of this conversation would be missing, where nothing can
    /// answer one: no model named, or a provider standing in for nobody.
    ///
    /// Asked before a turn and not inside it, because a turn with no model is
    /// not a turn: the prompt would be recorded, and a request naming nothing
    /// would go out, or be refused by the provider standing in only after the
    /// prompt was written down as said to a model nobody asked.
    #[must_use]
    pub fn missing(&self) -> Option<crate::providers::Missing> {
        let runner = self.native();
        let answerable = !runner.model().is_empty() && runner.provider().reaches_a_model();
        (!answerable).then(|| crate::providers::missing(self.serving, self.reachable))
    }

    /// Steps to the next permission mode, and says which it is.
    pub fn cycle(&mut self) -> Mode {
        self.native_mut().cycle()
    }

    /// Asks under `mode` from the next tool call on.
    pub fn switch(&mut self, mode: Mode) {
        self.native_mut().switch(mode);
    }

    /// Makes room in the transcript by asking for a recap of it.
    ///
    /// # Errors
    ///
    /// [`TurnError`] where [`Runner::compact`] says, which also says what each
    /// failure leaves. A failed request for the recap replaces nothing, so the
    /// transcript is as it was but for any pruning before it.
    ///
    /// Every prompt-cache step of the recap request is awaited. A cache failure
    /// replaces nothing, as a failed request does, and a changing operation
    /// whose answer remains uncertain is recorded as ambiguous for
    /// reconciliation. The compaction's session lines are awaited.
    pub async fn compact(
        &mut self,
        why: Compacting,
        run: &RunContext<'_>,
        spent: &mut Spend,
    ) -> Result<Room, TurnError> {
        let asked = self.native().speed();
        let made = self.native_mut().compact(why, run, spent).await;
        self.refusal_written(asked);
        made
    }

    /// The persistent prompt-cache resources this conversation remembers
    /// making, for a redacted listing.
    ///
    /// # Errors
    ///
    /// [`PromptCacheResourceError`] where the private store could not be read.
    pub async fn prompt_cache_resources(
        &mut self,
    ) -> Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError> {
        if !self.native().prompt_cache_resources_configured() {
            return Ok(Vec::new());
        }
        self.native_mut().prompt_cache_resources().await
    }

    /// Deletes the persistent prompt-cache resources held with the provider
    /// being asked, and counts what was and was not removed.
    ///
    /// # Errors
    ///
    /// [`PromptCacheResourceError`] where the private store could not be read
    /// or durably updated, [`PromptCacheResourceError::Unsupported`] when a
    /// record is held
    /// with the provider being asked and the provider has no lifecycle to
    /// delete one through, and [`PromptCacheResourceError::Cancelled`] when the
    /// pass comes to such a record and finds `cancel` requested, which leaves
    /// that record and those after it as they were. Every store and provider
    /// step is awaited; a provider cancellation, deadline or explicitly
    /// ambiguous answer is counted as ambiguous, as
    /// [`Runner::clean_prompt_cache`] says.
    pub async fn clean_prompt_cache(
        &mut self,
        cancel: &Cancel,
    ) -> Result<PromptCacheCleanup, PromptCacheResourceError> {
        if !self.native().prompt_cache_resources_configured() {
            return Ok(PromptCacheCleanup::default());
        }
        self.native_mut().clean_prompt_cache(cancel).await
    }

    /// Retires the persistent prompt-cache resources this conversation owns
    /// before its provider identity changes.
    pub(crate) async fn retire_prompt_cache(
        &mut self,
        cancel: &Cancel,
    ) -> Result<PromptCacheCleanup, PromptCacheResourceError> {
        if !self.native().prompt_cache_retirement_pending() {
            return Ok(PromptCacheCleanup::default());
        }
        self.native_mut().retire_prompt_cache(cancel).await
    }

    /// Takes one turn: `prompt` and what is attached to it, answered by the
    /// model or refused before it is asked.
    ///
    /// Everything the turn reported on the way is on `run`; what comes back is
    /// how it ended. A refusal is an answer too — [`Turned::Rejected`] and
    /// [`Turned::Undecided`] name the guardrail and its reason so that whoever
    /// typed the prompt can be told why nothing was said.
    ///
    /// # Errors
    ///
    /// [`TurnError`] where the turn could not be taken at all, or was not
    /// finished, as [`Runner::turn`] says, which also says what each failure
    /// leaves. A tool source's own step that gave up comes back as that
    /// source's failure.
    pub async fn turn(
        &mut self,
        prompt: &str,
        attached: Box<[Attachment]>,
        ask: &mut dyn Ask,
        run: &RunContext<'_>,
    ) -> Result<Turned, TurnError> {
        let asked = self.native().speed();
        let turned = self.native_mut().turn(prompt, attached, ask, run).await;
        self.refusal_written(asked);
        turned
    }

    /// Starts a new session and records into it from here on, with nothing
    /// carried over from the one being left.
    ///
    /// The session left is returned rather than finished here: it is the
    /// caller's to report on, and its log stays on the disk where a resume can
    /// find it.
    ///
    /// # Errors
    ///
    /// [`SessionError`] where the new log could not be started; the session in
    /// hand is then untouched and still being recorded into.
    pub async fn clear(
        &mut self,
        sessions: &Path,
        workspace: &Workspace,
        branch: Option<&str>,
    ) -> Result<Arc<Session>, SessionError> {
        let session = Arc::new(Session::start(sessions, workspace, branch)?);
        Ok(self.pick_up(session, Transcript::new()).await)
    }

    /// Picks the session `id` names back up, with everything it already holds.
    ///
    /// # Errors
    ///
    /// [`SessionError`] where no session of this workspace answers to `id`, or
    /// where its log could not be read; the session in hand is then untouched.
    pub async fn resume(
        &mut self,
        sessions: &Path,
        workspace: &Workspace,
        id: &SessionId,
    ) -> Result<Arc<Session>, SessionError> {
        let (session, transcript) = Session::reopen(sessions, workspace, id)?;
        Ok(self.pick_up(Arc::new(session), transcript).await)
    }

    /// Records into `session` from here on, and returns the one left.
    ///
    /// The session is swapped first and the runner handed the new one second,
    /// so that nothing the runner writes in between lands in a log that has
    /// been left.
    async fn pick_up(&mut self, session: Arc<Session>, transcript: Transcript) -> Arc<Session> {
        let left = std::mem::replace(&mut self.session, Arc::clone(&session));
        self.native_mut().pick_up(session, transcript);
        self.clearings_recorded().await;
        left
    }

    /// Awaits the sessions owed lines by picking a session up or changing
    /// vendor taking them, as the module says.
    pub(crate) async fn clearings_recorded(&mut self) {
        self.native_mut().record_clearings().await;
    }
}

/// A conversation's state between turns, to be read and never changed.
#[derive(Debug, Clone, Copy)]
pub struct Settled<'a>(&'a Runner);

impl<'a> From<&'a Runner> for Settled<'a> {
    /// What `runner` has settled, for a front end's test that builds a runner
    /// without a conversation around it.
    fn from(runner: &'a Runner) -> Self {
        Self(runner)
    }
}

impl<'a> Settled<'a> {
    /// The model asked, as its provider spells it, or nothing where none is
    /// named.
    #[must_use]
    pub fn model(self) -> &'a str {
        self.0.model()
    }

    /// How hard the model is asked to think, where a rung is set.
    #[must_use]
    pub fn effort(self) -> Option<Effort> {
        self.0.effort()
    }

    /// The pace the next request asks for.
    #[must_use]
    pub fn speed(self) -> Speed {
        self.0.speed()
    }

    /// The pace the last answer says it was served at.
    #[must_use]
    pub const fn served(self) -> Served {
        self.0.served()
    }

    /// What asking for the fast pace looks like for the model asked.
    #[must_use]
    pub fn fast(self) -> FastForm {
        self.0.provider().fast(self.0.model())
    }

    /// The permission mode a tool call is asked under.
    #[must_use]
    pub fn mode(self) -> Mode {
        self.0.mode()
    }

    /// The vendor the next turn reaches, in the provider's own word for it.
    #[must_use]
    pub fn serving(self) -> &'static str {
        self.0.serving()
    }

    /// What the provider's requests have a shape for.
    #[must_use]
    pub fn spells(self) -> Modalities {
        self.0.provider().spells()
    }

    /// What the model reads, where the session was told.
    #[must_use]
    pub fn reads(self) -> Option<Modalities> {
        self.0.reads()
    }

    /// Everything said so far.
    #[must_use]
    pub fn transcript(self) -> &'a Transcript {
        self.0.transcript()
    }

    /// The tokens the next request would carry.
    #[must_use]
    pub fn carrying(self) -> u64 {
        self.0.carrying()
    }

    /// The share of the window left, in percent, where the window is known.
    #[must_use]
    pub fn left(self) -> Option<u8> {
        self.0.left()
    }

    /// What the next request carries, by category.
    #[must_use]
    pub fn breakdown(self) -> Breakdown {
        self.0.breakdown()
    }

    /// What the session has used so far.
    #[must_use]
    pub const fn totals(self) -> Totals {
        self.0.totals()
    }

    /// The plan's limits, as the provider last reported them.
    #[must_use]
    pub fn plan_limits(self) -> Option<PlanWindows> {
        self.0.plan_limits()
    }

    /// The bounds a run is held to.
    #[must_use]
    pub const fn policy(self) -> RunPolicy {
        self.0.policy()
    }

    /// The line a recorded call is drawn as.
    #[must_use]
    pub fn about(self, call: &ToolCall) -> Summary {
        self.0.about(call)
    }

    /// How a recorded call folds when drawn, where it does.
    #[must_use]
    pub fn folds(self, call: &ToolCall) -> Option<Looking> {
        self.0.folds(call)
    }

    /// Whether the session is owed lines saying what was cleared from what
    /// the vendor is sent.
    #[must_use]
    pub fn owes_clearings(self) -> bool {
        self.0.owes_clearings()
    }

    /// When the prompt cache is asked for.
    #[must_use]
    pub const fn prompt_cache_policy(self) -> PromptCachePolicy {
        self.0.prompt_cache_policy()
    }

    /// What the provider and model can cache.
    #[must_use]
    pub fn prompt_cache_capabilities(self) -> PromptCacheCapabilities {
        self.0.prompt_cache_capabilities()
    }

    /// What the last request asked of the cache, where one did.
    #[must_use]
    pub const fn prompt_cache_attempt(self) -> Option<&'a PromptCacheAttempt> {
        self.0.prompt_cache_attempt()
    }
}
