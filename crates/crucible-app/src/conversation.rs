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
        }
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
            session: self.session,
            serving: self.serving,
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
    /// Three refusals come back as [`TurnError::Unready`], even when the
    /// compaction is being stopped: a refusal outranks a stop. A line the
    /// session would not take between turns, still held, is refused before
    /// anything is recorded or sent, and the transcript is as it was. A step of
    /// the recap request that would have had to wait replaces nothing, as a
    /// failed request does; what the step began is unconfirmed, and its
    /// prompt-cache attempt, or the cache step refused, is recorded as
    /// [`Runner::compact`] says. A session write of the compaction's own that
    /// would have had to wait leaves standing what came before it: a refused
    /// line about the pruning leaves the pruning, a refused line recording the
    /// recap leaves the pruning without the replacement, which comes after that
    /// line, and a refused line reporting what the recap freed leaves both.
    /// Whether the log kept a refused line is not known.
    pub fn compact(
        &mut self,
        why: Compacting,
        run: &RunContext<'_>,
        spent: &mut Spend,
    ) -> Result<Room, TurnError> {
        self.runner.compact(why, run, spent)
    }

    /// The persistent prompt-cache resources this conversation remembers
    /// making, for a redacted listing.
    ///
    /// # Errors
    ///
    /// [`PromptCacheResourceError`] where the private store could not be read,
    /// [`PromptCacheResourceError::Local`] carrying the refusal among them
    /// where reading it would have had to wait and was dropped.
    pub fn prompt_cache_resources(
        &mut self,
    ) -> Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError> {
        self.runner.prompt_cache_resources()
    }

    /// Deletes the persistent prompt-cache resources held with the provider
    /// being asked, and counts what was and was not removed.
    ///
    /// # Errors
    ///
    /// [`PromptCacheResourceError`] where the private store could not be read
    /// or durably updated, [`PromptCacheResourceError::Local`] carrying the
    /// refusal among them where a step on it would have had to wait, which may
    /// or may not have acted, [`PromptCacheResourceError::Unsupported`] when a
    /// record is held with the provider being asked and the provider has no
    /// lifecycle to delete one through, and
    /// [`PromptCacheResourceError::Cancelled`] when the pass comes to such a
    /// record and finds `cancel` requested, which leaves that record and those
    /// after it as they were. A provider step that would have had to wait is
    /// counted rather than returned, as [`Runner::clean_prompt_cache`] says.
    pub fn clean_prompt_cache(
        &mut self,
        cancel: &Cancel,
    ) -> Result<PromptCacheCleanup, PromptCacheResourceError> {
        self.runner.clean_prompt_cache(cancel)
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
    /// leaves. [`TurnError::Unready`] is a step that would have had to wait and
    /// was dropped, leaving what it began unconfirmed rather than undone, or a
    /// line the session would not take between turns, still held, which ends
    /// the turn before anything of it is recorded or sent. A tool source's own
    /// step that would have had to wait comes back as that source's failure.
    pub fn turn(
        &mut self,
        prompt: &str,
        attached: Box<[Attachment]>,
        ask: &mut dyn Ask,
        run: &RunContext<'_>,
    ) -> Result<Turned, TurnError> {
        self.runner.turn(prompt, attached, ask, run)
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
    pub fn clear(
        &mut self,
        sessions: &Path,
        workspace: &Workspace,
        branch: Option<&str>,
    ) -> Result<Arc<Session>, SessionError> {
        let session = Arc::new(Session::start(sessions, workspace, branch)?);
        Ok(self.pick_up(session, Transcript::new()))
    }

    /// Picks the session `id` names back up, with everything it already holds.
    ///
    /// # Errors
    ///
    /// [`SessionError`] where no session of this workspace answers to `id`, or
    /// where its log could not be read; the session in hand is then untouched.
    pub fn resume(
        &mut self,
        sessions: &Path,
        workspace: &Workspace,
        id: &SessionId,
    ) -> Result<Arc<Session>, SessionError> {
        let (session, transcript) = Session::reopen(sessions, workspace, id)?;
        Ok(self.pick_up(Arc::new(session), transcript))
    }

    /// Records into `session` from here on, and returns the one left.
    ///
    /// The session is swapped first and the runner handed the new one second,
    /// so that nothing the runner writes in between lands in a log that has
    /// been left.
    fn pick_up(&mut self, session: Arc<Session>, transcript: Transcript) -> Arc<Session> {
        let left = std::mem::replace(&mut self.session, Arc::clone(&session));
        self.runner.pick_up(session, transcript);
        left
    }
}
