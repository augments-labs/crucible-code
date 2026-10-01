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
//! show. The runner itself is handed out to be read and never to be changed,
//! so the session it records into and the provider it is said to be asking
//! cannot be moved from outside without the other half moving with them.
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
use crucible_runner::{PromptCacheCleanup, RunContext, Runner, TurnError, Turned};
use crucible_runtime::Cancel;
use crucible_session::{FilePromptCacheResourceStore, Session, SessionError};
use crucible_tools::{Ask, Mode};
use crucible_types::{
    Attachment, Compacting, PromptCacheResourceError, PromptCacheResourceRecord, SessionId, Spend,
    Transcript,
};
use crucible_workspace::Workspace;

/// A runner, the session it records into, and the provider it is asking.
#[derive(Debug)]
pub struct Conversation {
    pub(crate) runner: Runner,
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
            session,
            serving,
            reachable: false,
            consent: None,
            web: crate::following::Following::default(),
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
        Self {
            runner: self
                .runner
                .with_prompt_cache_store(FilePromptCacheResourceStore::in_home(home)),
            ..self
        }
    }

    /// The runner, for what is read off it: its model, its mode, its
    /// transcript. What changes it is a method of this type.
    #[must_use]
    pub const fn runner(&self) -> &Runner {
        &self.runner
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
        let answerable =
            !self.runner.model().is_empty() && self.runner.provider().reaches_a_model();
        (!answerable).then(|| crate::providers::missing(self.serving, self.reachable))
    }

    /// Steps to the next permission mode, and says which it is.
    pub fn cycle(&mut self) -> Mode {
        self.runner.cycle()
    }

    /// Asks under `mode` from the next tool call on.
    pub fn switch(&mut self, mode: Mode) {
        self.runner.switch(mode);
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
        let asked = self.runner.speed();
        let made = self.runner.compact(why, run, spent).await;
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
        if !self.runner.prompt_cache_resources_configured() {
            return Ok(Vec::new());
        }
        self.runner.prompt_cache_resources().await
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
        if !self.runner.prompt_cache_resources_configured() {
            return Ok(PromptCacheCleanup::default());
        }
        self.runner.clean_prompt_cache(cancel).await
    }

    /// Retires the persistent prompt-cache resources this conversation owns
    /// before its provider identity changes.
    pub(crate) async fn retire_prompt_cache(
        &mut self,
        cancel: &Cancel,
    ) -> Result<PromptCacheCleanup, PromptCacheResourceError> {
        if !self.runner.prompt_cache_retirement_pending() {
            return Ok(PromptCacheCleanup::default());
        }
        self.runner.retire_prompt_cache(cancel).await
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
        self.runner.pick_up(session, transcript);
        self.clearings_recorded().await;
        left
    }

    /// Awaits the sessions owed lines by picking a session up or changing
    /// vendor taking them, as the module says.
    pub(crate) async fn clearings_recorded(&mut self) {
        self.runner.record_clearings().await;
    }
}
