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
//! **A turn is waited for on the thread that asks for it.** The runner's turn
//! and compaction are asynchronous, and the front ends that ask for them are
//! not yet: [`Conversation::turn`] and [`Conversation::compact`] each cross
//! into the runner through [`Bridge::AppTurn`], which polls the whole turn on
//! the calling thread, entered into the application's runtime, and never
//! spawns it. Whatever the turn awaits therefore runs where it ran when the
//! turn was synchronous, on the thread the front end took the turn on. The
//! crossing waits under a cancel nothing raises: a stop is the turn's own to
//! answer, through the cancel on its run, so it ends the way a stopped turn
//! ends rather than as a future dropped at a step, which could leave a call
//! recorded with no result. The runtime is the one the application owns,
//! handed over by [`Conversation::on`]; a conversation never handed one
//! refuses every turn and compaction with [`TurnError::Unwaited`].
//!
//! **So are explicit prompt-cache operations.** Cache inspection, cleanup and
//! retirement arrive asynchronously from the runner but remain synchronous
//! front-end commands. The conversation waits for each on the application
//! runtime through that same application boundary. A conversation with no
//! runtime, or one asked from a runtime thread, reports the local cache
//! operation as failed without beginning it.
//!
//! **So is what picking a session up or changing vendor owes the session.**
//! Each clears from the transcript what the vendor being asked may not be
//! sent, and the runner owes the session the lines saying so. The
//! conversation waits for the session to take them through the same crossing,
//! once it is handed its runtime, after each pick-up and each change of
//! vendor. Where that wait is refused, they stay owed, and a later wait, turn
//! or compaction writes them before anything of its own. The refusal is kept
//! as what the log of each session they are owed to is missing, since without
//! one they are never written, and withdrawn once that session's lines are
//! written, by whichever wait writes them; the log goes on working meanwhile.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use crucible_context::Room;
use crucible_runner::{PromptCacheCleanup, RunContext, Runner, TurnError, Turned};
use crucible_runtime::{Bridge, Cancel, Unwaited};
use crucible_session::{FilePromptCacheResourceStore, Session, SessionError};
use crucible_tools::{Ask, Mode};
use crucible_types::{
    Attachment, Compacting, PromptCacheResourceError, PromptCacheResourceRecord, SessionId, Spend,
    Transcript,
};
use crucible_workspace::Workspace;
use tokio::runtime::Handle;

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
    /// The runtime application-owned asynchronous work is waited for on, or
    /// `None` where none was handed over, which refuses it.
    runtime: Option<Handle>,
    /// The sessions told they are missing lines the runner still owes them,
    /// held until those lines are written and the report is withdrawn.
    missing: Vec<Arc<Session>>,
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
            runtime: None,
            missing: Vec::new(),
        }
    }

    /// The same conversation, waiting for application-owned asynchronous work
    /// on `runtime`, the application's own, once it has waited there for what
    /// picking its session up owes the session.
    #[must_use]
    pub fn on(self, runtime: Handle) -> Self {
        let mut on = Self {
            runtime: Some(runtime),
            ..self
        };
        on.clearings_recorded(None);
        on
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
    /// Waited for on the calling thread, as the module says.
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
    ///
    /// [`TurnError::Unwaited`] where the compaction could not be waited for at
    /// all: this conversation was handed no runtime, or the caller is on a
    /// thread a runtime runs. Nothing was asked or recorded.
    pub fn compact(
        &mut self,
        why: Compacting,
        run: &RunContext<'_>,
        spent: &mut Spend,
    ) -> Result<Room, TurnError> {
        let compacted = waited(self.runtime.as_ref(), self.runner.compact(why, run, spent))
            .unwrap_or_else(|refused| Err(refused.into()));
        self.made_good();
        compacted
    }

    /// The persistent prompt-cache resources this conversation remembers
    /// making, for a redacted listing.
    ///
    /// # Errors
    ///
    /// [`PromptCacheResourceError`] where the private store could not be read,
    /// [`PromptCacheResourceError::Local`] among them where this conversation
    /// could not wait for the listing on its application runtime.
    pub fn prompt_cache_resources(
        &mut self,
    ) -> Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError> {
        if !self.runner.prompt_cache_resources_configured() {
            return Ok(Vec::new());
        }
        cache_waited(
            self.runtime.as_ref(),
            "list",
            self.runner.prompt_cache_resources(),
        )
    }

    /// Deletes the persistent prompt-cache resources held with the provider
    /// being asked, and counts what was and was not removed.
    ///
    /// # Errors
    ///
    /// [`PromptCacheResourceError`] where the private store could not be read
    /// or durably updated, [`PromptCacheResourceError::Local`] among them
    /// where this conversation could not begin the pass on its application
    /// runtime, [`PromptCacheResourceError::Unsupported`] when a record is held
    /// with the provider being asked and the provider has no lifecycle to
    /// delete one through, and [`PromptCacheResourceError::Cancelled`] when the
    /// pass comes to such a record and finds `cancel` requested, which leaves
    /// that record and those after it as they were. Every store and provider
    /// step is awaited; a provider cancellation, deadline or explicitly
    /// ambiguous answer is counted as ambiguous, as
    /// [`Runner::clean_prompt_cache`] says.
    pub fn clean_prompt_cache(
        &mut self,
        cancel: &Cancel,
    ) -> Result<PromptCacheCleanup, PromptCacheResourceError> {
        if !self.runner.prompt_cache_resources_configured() {
            return Ok(PromptCacheCleanup::default());
        }
        cache_waited(
            self.runtime.as_ref(),
            "clean",
            self.runner.clean_prompt_cache(cancel),
        )
    }

    /// Retires the persistent prompt-cache resources this conversation owns
    /// before its provider identity changes.
    pub(crate) fn retire_prompt_cache(
        &mut self,
        cancel: &Cancel,
    ) -> Result<PromptCacheCleanup, PromptCacheResourceError> {
        if !self.runner.prompt_cache_retirement_pending() {
            return Ok(PromptCacheCleanup::default());
        }
        cache_waited(
            self.runtime.as_ref(),
            "retire",
            self.runner.retire_prompt_cache(cancel),
        )
    }

    /// Takes one turn: `prompt` and what is attached to it, answered by the
    /// model or refused before it is asked.
    ///
    /// Everything the turn reported on the way is on `run`; what comes back is
    /// how it ended. Waited for on the calling thread, as the module says. A
    /// refusal is an answer too — [`Turned::Rejected`] and
    /// [`Turned::Undecided`] name the guardrail and its reason so that whoever
    /// typed the prompt can be told why nothing was said.
    ///
    /// # Errors
    ///
    /// [`TurnError`] where the turn could not be taken at all, or was not
    /// finished, as [`Runner::turn`] says, which also says what each failure
    /// leaves. [`TurnError::Unready`] is a step that would have had to wait and
    /// was dropped, leaving what it began unconfirmed rather than undone. A
    /// tool source's own step that would have had to wait comes back as that
    /// source's failure.
    ///
    /// [`TurnError::Unwaited`] where the turn could not be waited for at all:
    /// this conversation was handed no runtime, or the caller is on a thread a
    /// runtime runs. Nothing of the turn was asked or recorded.
    pub fn turn(
        &mut self,
        prompt: &str,
        attached: Box<[Attachment]>,
        ask: &mut dyn Ask,
        run: &RunContext<'_>,
    ) -> Result<Turned, TurnError> {
        let turned = waited(
            self.runtime.as_ref(),
            self.runner.turn(prompt, attached, ask, run),
        )
        .unwrap_or_else(|refused| Err(refused.into()));
        self.made_good();
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
        self.clearings_recorded(Some(&left));
        left
    }

    /// Waits for the sessions owed lines by picking a session up or changing
    /// vendor to take them, as the module says; `left` is the session a
    /// pick-up has just left, if one has.
    ///
    /// Refused — no runtime was handed over, or the caller is on a thread a
    /// runtime runs — the lines stay owed, and a later wait, turn or
    /// compaction of this conversation writes them before anything of its
    /// own; with none, they are never written. So each session a line is
    /// still owed to is told, through [`Session::missing`], and the report is
    /// withdrawn once its lines are written. It is not the session's trouble:
    /// the log goes on working.
    pub(crate) fn clearings_recorded(&mut self, left: Option<&Arc<Session>>) {
        if self.runner.owes_clearings() {
            let runner = &mut self.runner;
            let written = waited(self.runtime.as_ref(), async {
                runner.record_clearings().await;
                Ok::<(), TurnError>(())
            });
            if written.is_err() {
                self.report_missing(left);
            }
        }
        self.made_good();
    }

    /// Tells each session the runner still owes a line that it is missing it.
    ///
    /// A line is owed only to a session this conversation has held: the one
    /// in hand, the one a pick-up has just left, or one already told, which
    /// is held until its lines are written.
    fn report_missing(&mut self, left: Option<&Arc<Session>>) {
        for session in std::iter::once(&self.session).chain(left) {
            let told = self
                .missing
                .iter()
                .any(|missing| Arc::ptr_eq(missing, session));
            if !told && self.runner.owes_clearings_to(&**session) {
                session.missing(UNWAITED_CLEARINGS);
                self.missing.push(Arc::clone(session));
            }
        }
    }

    /// Withdraws the report from each session told it is missing lines once
    /// the runner owes it none, whichever wait wrote them.
    fn made_good(&mut self) {
        let runner = &self.runner;
        self.missing.retain(|session| {
            let owed = runner.owes_clearings_to(&**session);
            if !owed {
                session.no_longer_missing();
            }
            owed
        });
    }
}

/// What a session is told where the lines clearing what a vendor may not be
/// sent could not be waited for.
const UNWAITED_CLEARINGS: &str = "lines clearing results a vendor may not be sent could not be \
                                  waited for, and reach this log only if a later wait, turn or \
                                  compaction of the conversation writes them";

/// Waits on the calling thread for one prompt-cache operation on `runtime`.
///
/// The front end still carries cache inspection, cleanup and retirement as
/// synchronous commands. The work itself is asynchronous, so it is awaited on
/// the application runtime through the same application boundary as a turn.
/// A conversation without a runtime, or one asked from a runtime thread, keeps
/// the cache error surface and reports the failed local operation.
fn cache_waited<T>(
    runtime: Option<&Handle>,
    operation: &'static str,
    work: impl Future<Output = Result<T, PromptCacheResourceError>>,
) -> Result<T, PromptCacheResourceError> {
    match waited(runtime, work) {
        Ok(result) => result,
        Err(refused) => Err(PromptCacheResourceError::Local {
            operation,
            source: std::io::Error::other(refused),
        }),
    }
}

/// Waits on the calling thread for `work` on `runtime`.
///
/// A turn, a compaction, the lines a pick-up or vendor change owes the
/// session, and explicit prompt-cache inspection, cleanup or retirement all
/// cross the same application boundary. Under a cancel nothing raises, so the
/// crossing never drops the work part way: a stop reaches a turn through the
/// cancel on its own run, and cache work carries the cancel its caller asked
/// for.
fn waited<T, E>(
    runtime: Option<&Handle>,
    work: impl Future<Output = Result<T, E>>,
) -> Result<Result<T, E>, Unwaited> {
    Bridge::AppTurn.wait(runtime, &Cancel::new(), work)
}
