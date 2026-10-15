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
//! **A conversation runs on one backend at a time.** Its own runner is
//! assembled at startup whatever the route, and kept: while the conversation
//! is on an installed agent's route the runner holds no session and no
//! transcript and runs nothing, the agent's turns run through
//! [`crate::outside`], and a return to this program's own models continues on
//! the runner as it was assembled. Each read of the conversation decides what
//! an installed agent's route answers: what such a route cannot report reads
//! as absent, and what it cannot do is refused with a sentence, never handed
//! to the runner.
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
use crucible_execution::Failure;
use crucible_models::{
    Effort, FastForm, PromptCacheAttempt, PromptCacheCapabilities, Served, Speed,
};
use crucible_runner::{
    Breakdown, Post, PromptCacheCleanup, RunContext, RunPolicy, Runner, Totals, TurnError, Turned,
};
use crucible_runtime::{Aside, Cancel, Steer};
use crucible_session::{FilePromptCacheResourceStore, Session, SessionError};
use crucible_tools::{Looking, Mode, Summary};
use crucible_types::{
    Attachment, Compacting, Modalities, Modality, PlanWindows, PromptCachePolicy,
    PromptCacheResourceError, PromptCacheResourceRecord, SessionId, Spend, ToolCall, Transcript,
};
use crucible_workspace::Workspace;

use crate::backends::{REGISTRY, Registry};
use crate::client::Asker;
use crate::outside::{self, Installed, Outside};

/// What a conversation runs on.
///
/// Closed, and matched where it is read, so a backend added here makes every
/// read of the conversation decide what that backend answers.
#[derive(Debug, Clone, Copy)]
enum Backed<'a> {
    /// This program's own runner.
    Native(&'a Runner),
    /// An installed agent.
    External(&'a Outside),
}

/// A runner, the session it records into, and the provider it is asking.
#[derive(Debug)]
pub struct Conversation {
    /// This program's own runner, kept whatever the route: what a credential
    /// or a pace sets is set on it, for the conversation's return to it.
    pub(crate) runner: Runner,
    /// The installed agent the conversation is on, where it is on one.
    outside: Option<Box<Outside>>,
    session: Arc<Session>,
    /// The registry name of the provider being asked, or `None` where nobody
    /// is. Not the runner's own word for it: a provider names itself for the
    /// session log, and the registry names it for `/model`, the settings file
    /// and the credential store, which is the name every switch is decided by.
    pub(crate) serving: Option<&'static str>,
    /// What kind of credential set up the provider [`Self::serving`] names:
    /// set in the same step as it, and read only while it is `Some`.
    pub(crate) source: Option<crate::providers::CredentialSource>,
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
    /// The installed agents this conversation's start was settled against.
    agents: &'static Registry,
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
            runner: build(Arc::clone(&session)),
            outside: None,
            session,
            serving,
            source: None,
            reachable: false,
            consent: None,
            agents: &REGISTRY,
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

    /// The same conversation, recording `agents` as the installed agents its
    /// start was settled against. Only startup sets it, and a test build's
    /// [`proof::agents`](crate::proof), so what it records is what the start
    /// was settled against.
    #[must_use]
    pub(crate) fn registry(self, agents: &'static Registry) -> Self {
        Self { agents, ..self }
    }

    /// The same conversation, the provider it is asking set up by a
    /// credential of the kind `source` says, where one was.
    #[must_use]
    pub(crate) fn credited(self, source: Option<crate::providers::CredentialSource>) -> Self {
        Self { source, ..self }
    }

    /// The installed agents this conversation's start was settled against.
    #[must_use]
    pub fn agents(&self) -> &'static Registry {
        self.agents
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
        Self {
            runner: self
                .runner
                .with_prompt_cache_store(FilePromptCacheResourceStore::in_home(home)),
            ..self
        }
    }

    /// What is settled between turns: the model, the mode, the transcript.
    /// What changes any of it is a method of this type.
    #[must_use]
    pub fn settled(&self) -> Settled<'_> {
        Settled(self.backed())
    }

    /// What the conversation runs on now.
    fn backed(&self) -> Backed<'_> {
        self.outside
            .as_deref()
            .map_or(Backed::Native(&self.runner), Backed::External)
    }

    /// The runner itself, for a test outside this crate that reads what no
    /// front end does. Compiled only with the `proof` feature, which nothing
    /// that ships turns on.
    #[cfg(feature = "proof")]
    #[must_use]
    pub fn native_runner(&self) -> Option<&Runner> {
        self.native()
    }

    /// The runner this conversation drives, or `None` while it is on an
    /// installed agent's route.
    pub(crate) fn native(&self) -> Option<&Runner> {
        self.outside.is_none().then_some(&self.runner)
    }

    /// The runner this conversation drives, to change, or `None` while it is
    /// on an installed agent's route.
    pub(crate) fn native_mut(&mut self) -> Option<&mut Runner> {
        self.outside.is_none().then_some(&mut self.runner)
    }

    /// Moves the conversation onto the route of the agent `installed`:
    /// `session` is recorded into from here on, bound to that route,
    /// `transcript` is what was seen of it so far, and the agent's run is
    /// replaced through its relaunch once it is spent. The kept runner is
    /// handed a session that keeps nothing and no transcript, so it holds no
    /// claim on a log and no second copy while the agent runs the
    /// conversation. The session left is returned.
    ///
    /// # Errors
    ///
    /// [`Failure`] where the agent has no launch in the conversation's mode:
    /// nothing moves, any run handed over is closed, and `session` is let go.
    pub async fn external(
        &mut self,
        session: Arc<Session>,
        transcript: Transcript,
        installed: Installed,
    ) -> Result<Arc<Session>, Failure> {
        let mode = self.settled().mode();
        if let Err(refused) = installed.relaunch.admits(mode) {
            if let Some(backend) = installed.backend {
                let _closed = backend.close().await;
            }
            return Err(refused);
        }
        if let Some(mut outside) = self.outside.take() {
            outside.retire().await;
        }
        Ok(self.moved(session, transcript, installed, mode))
    }

    /// [`Conversation::external`] for a conversation just assembled, which
    /// holds no run of an agent to retire: `installed` hands over none either,
    /// its first turn launching it, so nothing is awaited.
    ///
    /// # Errors
    ///
    /// [`Failure`] where the agent has no launch in the conversation's mode:
    /// nothing moves, and `session` is let go.
    pub(crate) fn opened_outside(
        &mut self,
        session: Arc<Session>,
        transcript: Transcript,
        installed: Installed,
    ) -> Result<(), Failure> {
        let mode = self.settled().mode();
        installed.relaunch.admits(mode)?;
        drop(self.moved(session, transcript, installed, mode));
        Ok(())
    }

    /// Records into `session` on the route of the agent `installed`, launched
    /// under `mode`, as [`Conversation::external`] says, and returns the
    /// session left.
    fn moved(
        &mut self,
        session: Arc<Session>,
        transcript: Transcript,
        installed: Installed,
        mode: Mode,
    ) -> Arc<Session> {
        let named = session.external_ref().cloned();
        self.runner
            .pick_up(Arc::new(Session::nowhere()), Transcript::new());
        self.outside = Some(Box::new(Outside::new(installed, transcript, mode, named)));
        std::mem::replace(&mut self.session, session)
    }

    /// Moves the conversation back onto this program's own models, closing
    /// the installed agent's run, and continues on the kept runner recording
    /// into `session` with `transcript`. The session left is returned.
    pub async fn inside(&mut self, session: Arc<Session>, transcript: Transcript) -> Arc<Session> {
        if let Some(mut outside) = self.outside.take() {
            outside.retire().await;
        }
        self.pick_up(session, transcript).await
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
        match self.backed() {
            Backed::Native(runner) => runner.starting(events, cancel, steer, aside),
            Backed::External(_) => RunContext::outside(events, cancel, steer, aside),
        }
    }

    /// The session being recorded into.
    #[must_use]
    pub const fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// The registry name of the provider being asked, or `None` where a run
    /// started with nobody to ask or signed out of the one it had, or is on an
    /// installed agent's route, which no provider serves.
    #[must_use]
    pub fn serving(&self) -> Option<&'static str> {
        self.serving.filter(|_| self.outside.is_none())
    }

    /// What kind of credential set up the provider being asked, where one is
    /// being asked and the conversation is not on an installed agent's route.
    #[must_use]
    pub(crate) fn credential(&self) -> Option<&crate::providers::CredentialSource> {
        self.serving()?;
        self.source.as_ref()
    }

    /// The configured instance of the installed agent the conversation is on,
    /// where it is on one.
    #[must_use]
    pub(crate) fn instance(&self) -> Option<&str> {
        self.outside
            .as_deref()
            .map(|outside| outside.route().instance())
    }

    /// Stops the conversation's work on the installed agent `instance`, where
    /// it is on that one: no turn launches it again, and the run held is
    /// closed. `None` where the conversation is not on it, and otherwise
    /// whether the run is known to have stopped.
    pub(crate) async fn disconnect(&mut self, instance: &str) -> Option<bool> {
        let outside = self
            .outside
            .as_deref_mut()
            .filter(|outside| outside.route().instance() == instance)?;
        Some(outside.disconnect().await)
    }

    /// What a turn of this conversation would be missing, where nothing can
    /// answer one: no model named, or a provider standing in for nobody.
    ///
    /// Asked before a turn and not inside it, because a turn with no model is
    /// not a turn: the prompt would be recorded, and a request naming nothing
    /// would go out, or be refused by the provider standing in only after the
    /// prompt was written down as said to a model nobody asked.
    ///
    /// An installed agent chooses its own model and is never missing one:
    /// what refuses its turn is its launch.
    #[must_use]
    pub fn missing(&self) -> Option<crate::providers::Missing> {
        let runner = self.native()?;
        let answerable = !runner.model().is_empty() && runner.provider().reaches_a_model();
        (!answerable).then(|| crate::providers::missing(self.serving, self.reachable))
    }

    /// Steps to the next permission mode, and says which it is. An installed
    /// agent steps past a mode it cannot be launched in.
    pub fn cycle(&mut self) -> Mode {
        match self.outside.as_deref_mut() {
            Some(outside) => outside.cycle(),
            None => self.runner.cycle(),
        }
    }

    /// Asks under `mode` from the next tool call on, and says the mode in
    /// force. An installed agent is launched under it from its next turn,
    /// and keeps the mode it has where it cannot be launched under `mode`.
    pub fn switch(&mut self, mode: Mode) -> Mode {
        if let Some(outside) = self.outside.as_deref_mut() {
            outside.switch(mode)
        } else {
            self.runner.switch(mode);
            mode
        }
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
        let Some(runner) = self.native_mut() else {
            return Err(TurnError::Refused(
                "a recap of an installed agent's conversation, which the agent keeps".into(),
            ));
        };
        let asked = runner.speed();
        let made = runner.compact(why, run, spent).await;
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
        match self.native_mut() {
            Some(runner) if runner.prompt_cache_resources_configured() => {
                runner.prompt_cache_resources().await
            }
            Some(_) | None => Ok(Vec::new()),
        }
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
        match self.native_mut() {
            Some(runner) if runner.prompt_cache_resources_configured() => {
                runner.clean_prompt_cache(cancel).await
            }
            Some(_) | None => Ok(PromptCacheCleanup::default()),
        }
    }

    /// Retires the persistent prompt-cache resources this conversation owns
    /// before its provider identity changes.
    pub(crate) async fn retire_prompt_cache(
        &mut self,
        cancel: &Cancel,
    ) -> Result<PromptCacheCleanup, PromptCacheResourceError> {
        match self.native_mut() {
            Some(runner) if runner.prompt_cache_retirement_pending() => {
                runner.retire_prompt_cache(cancel).await
            }
            Some(_) | None => Ok(PromptCacheCleanup::default()),
        }
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
    ///
    /// On an installed agent's route the turn is the agent's, as
    /// [`crate::outside`] says, and each leave it asks for goes to `ask`.
    pub async fn turn(
        &mut self,
        prompt: &str,
        attached: Box<[Attachment]>,
        ask: &mut dyn Asker,
        run: &RunContext<'_>,
    ) -> Result<Turned, TurnError> {
        if let Some(outside) = self.outside.as_deref_mut() {
            outside::unattached(&attached)?;
            return outside.turn(prompt, ask, run, &self.session).await;
        }
        let asked = self.runner.speed();
        let turned = self.runner.turn(prompt, attached, ask, run).await;
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
    ///
    /// On an installed agent's route the new log is bound to the same route,
    /// and the agent's run is closed, so that the next turn starts the agent
    /// on a session of its own as new as the one shown.
    pub async fn clear(
        &mut self,
        sessions: &Path,
        workspace: &Workspace,
        branch: Option<&str>,
    ) -> Result<Arc<Session>, SessionError> {
        if let Some(outside) = self.outside.as_deref_mut() {
            let session = Arc::new(Session::start_routed(
                sessions,
                workspace,
                branch,
                outside.route(),
            )?);
            outside.restart(Transcript::new(), None).await;
            return Ok(std::mem::replace(&mut self.session, session));
        }
        let session = Arc::new(Session::start(sessions, workspace, branch)?);
        Ok(self.pick_up(session, Transcript::new()).await)
    }

    /// Picks the session `id` names back up, with everything it already holds.
    ///
    /// # Errors
    ///
    /// [`SessionError`] where no session of this workspace answers to `id`, or
    /// where its log could not be read; the session in hand is then untouched.
    ///
    /// On an installed agent's route only a session of the same route is
    /// picked up, and the agent's next turn resumes the agent's own session
    /// by the name the log recorded.
    pub async fn resume(
        &mut self,
        sessions: &Path,
        workspace: &Workspace,
        id: &SessionId,
    ) -> Result<Arc<Session>, SessionError> {
        if let Some(outside) = self.outside.as_deref_mut() {
            let (session, transcript) =
                Session::reopen_routed(sessions, workspace, id, outside.route())?;
            let named = session.external_ref().cloned();
            outside.restart(transcript, named).await;
            return Ok(std::mem::replace(&mut self.session, Arc::new(session)));
        }
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
        self.runner.pick_up(session, transcript);
        self.clearings_recorded().await;
        left
    }

    /// Awaits the sessions owed lines by picking a session up or changing
    /// vendor taking them, as the module says.
    pub(crate) async fn clearings_recorded(&mut self) {
        if let Some(runner) = self.native_mut() {
            runner.record_clearings().await;
        }
    }
}

/// A conversation's state between turns, to be read and never changed.
///
/// On an installed agent's route it reads what the agent's turns reported and
/// what was seen of them, and what the agent does not report reads as absent.
#[derive(Debug, Clone, Copy)]
pub struct Settled<'a>(Backed<'a>);

#[cfg(feature = "proof")]
impl<'a> From<&'a Runner> for Settled<'a> {
    /// What `runner` has settled, for a front end's test that builds a runner
    /// without a conversation around it.
    fn from(runner: &'a Runner) -> Self {
        Self(Backed::Native(runner))
    }
}

impl<'a> Settled<'a> {
    /// The model asked, as its provider spells it, or nothing where none is
    /// named, as on an installed agent's route, where the agent chooses.
    #[must_use]
    pub fn model(self) -> &'a str {
        match self.0 {
            Backed::Native(runner) => runner.model(),
            Backed::External(_) => "",
        }
    }

    /// How hard the model is asked to think, where a rung is set.
    #[must_use]
    pub fn effort(self) -> Option<Effort> {
        match self.0 {
            Backed::Native(runner) => runner.effort(),
            Backed::External(_) => None,
        }
    }

    /// The pace the next request asks for.
    #[must_use]
    pub fn speed(self) -> Speed {
        match self.0 {
            Backed::Native(runner) => runner.speed(),
            Backed::External(_) => Speed::default(),
        }
    }

    /// The pace the last answer says it was served at.
    #[must_use]
    pub const fn served(self) -> Served {
        match self.0 {
            Backed::Native(runner) => runner.served(),
            Backed::External(_) => Served::Unsaid,
        }
    }

    /// What asking for the fast pace looks like for the model asked.
    #[must_use]
    pub fn fast(self) -> FastForm {
        match self.0 {
            Backed::Native(runner) => runner.provider().fast(runner.model()),
            Backed::External(_) => FastForm::None,
        }
    }

    /// The permission mode a tool call is asked under, or an installed agent
    /// launched in.
    #[must_use]
    pub fn mode(self) -> Mode {
        match self.0 {
            Backed::Native(runner) => runner.mode(),
            Backed::External(outside) => outside.mode(),
        }
    }

    /// The vendor the next turn reaches, in the provider's own word for it,
    /// or the installed agent's program.
    #[must_use]
    pub fn serving(self) -> &'static str {
        match self.0 {
            Backed::Native(runner) => runner.serving(),
            Backed::External(outside) => outside.driver(),
        }
    }

    /// What the provider's requests have a shape for: text alone for an
    /// installed agent, which is handed nothing attached.
    #[must_use]
    pub fn spells(self) -> Modalities {
        match self.0 {
            Backed::Native(runner) => runner.provider().spells(),
            Backed::External(_) => Modalities::empty().insert(Modality::Text),
        }
    }

    /// What the model reads, where the session was told.
    #[must_use]
    pub fn reads(self) -> Option<Modalities> {
        match self.0 {
            Backed::Native(runner) => runner.reads(),
            Backed::External(_) => None,
        }
    }

    /// Everything said so far.
    #[must_use]
    pub fn transcript(self) -> &'a Transcript {
        match self.0 {
            Backed::Native(runner) => runner.transcript(),
            Backed::External(outside) => outside.transcript(),
        }
    }

    /// The tokens the next request would carry, which an installed agent
    /// does not say.
    #[must_use]
    pub fn carrying(self) -> u64 {
        match self.0 {
            Backed::Native(runner) => runner.carrying(),
            Backed::External(_) => 0,
        }
    }

    /// The share of the window left, in percent, where the window is known.
    #[must_use]
    pub fn left(self) -> Option<u8> {
        match self.0 {
            Backed::Native(runner) => runner.left(),
            Backed::External(_) => None,
        }
    }

    /// What the next request carries, by category.
    #[must_use]
    pub fn breakdown(self) -> Breakdown {
        match self.0 {
            Backed::Native(runner) => runner.breakdown(),
            Backed::External(_) => Breakdown::default(),
        }
    }

    /// What the session has used so far.
    #[must_use]
    pub fn totals(self) -> Totals {
        match self.0 {
            Backed::Native(runner) => runner.totals(),
            Backed::External(outside) => outside.totals(),
        }
    }

    /// The plan's limits, as the provider last reported them; an installed
    /// agent reports none.
    #[must_use]
    pub fn plan_limits(self) -> Option<PlanWindows> {
        match self.0 {
            Backed::Native(runner) => runner.plan_limits(),
            Backed::External(_) => None,
        }
    }

    /// The bounds a run is held to.
    #[must_use]
    pub fn policy(self) -> RunPolicy {
        match self.0 {
            Backed::Native(runner) => runner.policy(),
            Backed::External(_) => RunPolicy::default(),
        }
    }

    /// The line a recorded call is drawn as.
    #[must_use]
    pub fn about(self, call: &ToolCall) -> Summary {
        match self.0 {
            Backed::Native(runner) => runner.about(call),
            Backed::External(_) => Summary::new(""),
        }
    }

    /// How a recorded call folds when drawn, where it does.
    #[must_use]
    pub fn folds(self, call: &ToolCall) -> Option<Looking> {
        match self.0 {
            Backed::Native(runner) => runner.folds(call),
            Backed::External(_) => None,
        }
    }

    /// Whether the session is owed lines saying what was cleared from what
    /// the vendor is sent.
    #[must_use]
    pub fn owes_clearings(self) -> bool {
        match self.0 {
            Backed::Native(runner) => runner.owes_clearings(),
            Backed::External(_) => false,
        }
    }

    /// When the prompt cache is asked for.
    #[must_use]
    pub fn prompt_cache_policy(self) -> PromptCachePolicy {
        match self.0 {
            Backed::Native(runner) => runner.prompt_cache_policy(),
            Backed::External(_) => PromptCachePolicy::default(),
        }
    }

    /// What the provider and model can cache.
    #[must_use]
    pub fn prompt_cache_capabilities(self) -> PromptCacheCapabilities {
        match self.0 {
            Backed::Native(runner) => runner.prompt_cache_capabilities(),
            Backed::External(_) => {
                PromptCacheCapabilities::unknown("an installed agent keeps its own cache")
            }
        }
    }

    /// What the last request asked of the cache, where one did.
    #[must_use]
    pub const fn prompt_cache_attempt(self) -> Option<&'a PromptCacheAttempt> {
        match self.0 {
            Backed::Native(runner) => runner.prompt_cache_attempt(),
            Backed::External(_) => None,
        }
    }
}
