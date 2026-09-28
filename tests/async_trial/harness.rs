//! The tools, checks, recorders and session a trial run is built from.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crucible_runner::{AgentContext, Decision, EventEnvelope, InputGuardrail, Post, Undecided};
use crucible_runtime::{BoxFuture, Cancel};
use crucible_session::Session;
use crucible_storage::{
    CallResultKey, CallResultReceipt, CallResultStoreError, InvocationState, JournalStore, RunItem,
    SessionOwner, SessionStore,
};
use crucible_tools::{
    Approved, Ask, Remember, Sensitivity, Summary, Target, Tool, ToolContext, ToolDescriptor,
    ToolError, ToolOutput, ToolProvenance, Verdict,
};
use crucible_types::{
    Calibration, Compacted, ContextError, ContextPatch, ContextSnapshot, Message, RunId, SessionId,
    ToolArgs, ToolCall, ToolId, ToolResult, Transcript,
};
use crucible_workspace::Workspace;

use crate::vendor::Gate;

/// The descriptor a trial tool is registered under.
pub(crate) fn described(name: &str) -> ToolDescriptor {
    ToolDescriptor::new(
        name,
        r#"{"type":"object"}"#,
        ToolProvenance::builtin(name).expect("a built-in name fits its own identity"),
    )
    .expect("a descriptor the trial wrote")
}

/// Whatever a trial tool reaches, which is nothing on disk.
fn nothing() -> Sensitivity {
    Sensitivity::ReadOnly {
        target: Target::unresolved(),
    }
}

/// A tool whose effect is counted: every run is one more stamp.
pub(crate) struct Stamp {
    name: &'static str,
    ran: Arc<AtomicUsize>,
    then: Option<Gate>,
}

impl Stamp {
    /// A stamp called `name`, and the count of its runs.
    pub(crate) fn new(name: &'static str) -> (Arc<Self>, Arc<AtomicUsize>) {
        let ran = Arc::new(AtomicUsize::new(0));
        let stamp = Arc::new(Self {
            name,
            ran: Arc::clone(&ran),
            then: None,
        });
        (stamp, ran)
    }

    /// A stamp that opens `gate` once it has run.
    pub(crate) fn opening(name: &'static str, gate: &Gate) -> (Arc<Self>, Arc<AtomicUsize>) {
        let ran = Arc::new(AtomicUsize::new(0));
        let stamp = Arc::new(Self {
            name,
            ran: Arc::clone(&ran),
            then: Some(gate.clone()),
        });
        (stamp, ran)
    }
}

impl Tool for Stamp {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        nothing()
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new(self.name)
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let stamped = self.ran.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(gate) = &self.then {
                gate.open();
            }
            Ok(ToolOutput::ok(format!("{} #{stamped}", self.name)))
        })
    }
}

/// How long [`Late`] is given before its deadline.
pub(crate) const LATE: Duration = Duration::from_millis(50);

/// A tool that waits for its own deadline and never answers before it.
pub(crate) struct Late;

impl Tool for Late {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        nothing()
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("late")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            context.cancel().race(std::future::pending::<()>()).await;
            Err(ToolError::Cancelled("the deadline passed first".into()))
        })
    }
}

/// A tool that panics every time it runs.
pub(crate) struct Falls;

impl Tool for Falls {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        nothing()
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("falls")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move { panic!("a trial tool falls over, as it was written to") })
    }
}

/// Every agent a [`Guard`] was asked about, with the words it was asked about.
pub(crate) type Heard = Arc<Mutex<Vec<(String, String)>>>;

/// An input check that remembers who it was asked about, and what.
#[derive(Debug)]
pub(crate) struct Guard {
    name: &'static str,
    heard: Heard,
}

impl Guard {
    /// A check called `name`, and what it will have heard.
    pub(crate) fn new(name: &'static str) -> (Arc<Self>, Heard) {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let guard = Arc::new(Self {
            name,
            heard: Arc::clone(&heard),
        });
        (guard, heard)
    }
}

impl InputGuardrail for Guard {
    fn name(&self) -> &str {
        self.name
    }

    fn checking(&self, context: &AgentContext<'_>) -> Result<Decision, Undecided> {
        self.heard.lock().expect("what the check heard").push((
            context.agent().as_str().to_owned(),
            context.said().to_owned(),
        ));
        Ok(Decision::Allowed)
    }
}

/// One event a run posted, as the trial reads it back.
pub(crate) struct Posted {
    /// The run it was posted under.
    pub run: RunId,
    /// The call it was about, where it was about one.
    pub call: Option<String>,
    /// Everything it said.
    pub said: String,
}

/// Every event one agent's runs posted.
#[derive(Clone, Default)]
pub(crate) struct Seen(Arc<Mutex<Vec<Posted>>>);

impl Seen {
    /// What was posted, in order.
    pub(crate) fn posted(&self) -> std::sync::MutexGuard<'_, Vec<Posted>> {
        self.0.lock().expect("the events")
    }
}

impl Post for Seen {
    fn post(&self, envelope: EventEnvelope) {
        use crucible_runner::Event;
        let call = match envelope.event() {
            Event::ToolRequested { call, .. } => Some(call.id.as_str().to_owned()),
            Event::ToolFinished { call, .. } => Some(call.as_str().to_owned()),
            _ => None,
        };
        self.posted().push(Posted {
            run: envelope.run(),
            call,
            said: format!("{envelope:?}"),
        });
    }
}

/// A person who allows every call, and remembers each one they were asked.
#[derive(Default)]
pub(crate) struct Permit(pub Vec<String>);

impl Ask for Permit {
    fn ask<'a>(
        &'a mut self,
        call: &'a ToolCall,
        _sensitivity: &'a Sensitivity,
    ) -> BoxFuture<'a, (Verdict, Remember)> {
        self.0.push(call.id.as_str().to_owned());
        Box::pin(async { (Verdict::Allow, Remember::Never) })
    }
}

/// A directory of the trial's own, gone when the trial is.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    /// A new, empty one.
    pub(crate) fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "crucible-async-trial-{}",
            SessionId::new().as_str()
        ));
        fs::create_dir(&path).expect("a scratch directory");
        Self(path)
    }

    /// The workspace a run is aimed at.
    pub(crate) fn workspace(&self) -> Workspace {
        Workspace::open(&self.0).expect("the scratch directory as a workspace")
    }

    /// Where its sessions are logged.
    pub(crate) fn logs(&self) -> PathBuf {
        self.0.join("sessions")
    }

    /// Its path.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// A fresh session.
    pub(crate) fn session(&self) -> Session {
        Session::start(&self.logs(), &self.workspace(), None).expect("a session")
    }

    /// The newest session, continued, with what it holds.
    pub(crate) fn resumed(&self) -> (Session, Transcript) {
        Session::resume(&self.logs(), &self.workspace()).expect("a session to continue")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        drop(fs::remove_dir_all(&self.0));
    }
}

/// The moment in a call's life a [`Tripwire`] acts at.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Stage {
    /// Admitted, and recorded before anything ran.
    Prepared,
    /// Recorded as possibly having had its effect.
    Started,
    /// Recorded with its result.
    Finished,
}

impl Stage {
    fn is(self, state: &InvocationState) -> bool {
        matches!(
            (self, state),
            (Self::Prepared, InvocationState::Prepared)
                | (Self::Started, InvocationState::Started)
                | (Self::Finished, InvocationState::Finished { .. })
        )
    }
}

/// What a [`Tripwire`] does once its stage is recorded.
pub(crate) enum Trip {
    /// Asks the turn to stop, as a person pressing the key would.
    Cancel(Cancel),
    /// Opens the gate, and then never lets the turn past the record, as a
    /// process that died there would not.
    Park(Gate),
}

/// A session that acts once a call reaches `stage`, after recording it.
pub(crate) struct Tripwire {
    session: Session,
    stage: Stage,
    trip: Trip,
    tripped: AtomicBool,
}

impl Tripwire {
    /// `session`, tripping `trip` the first time a call reaches `stage`.
    pub(crate) fn new(session: Session, stage: Stage, trip: Trip) -> Self {
        Self {
            session,
            stage,
            trip,
            tripped: AtomicBool::new(false),
        }
    }
}

impl SessionStore for Tripwire {
    fn session_id(&self) -> Option<SessionId> {
        self.session.session_id()
    }

    fn owner(&self) -> Option<SessionOwner> {
        self.session.owner()
    }

    fn append_message<'a>(&'a self, message: &'a Message) -> BoxFuture<'a, ()> {
        SessionStore::append_message(&self.session, message)
    }

    fn context_snapshot(&self) -> Option<ContextSnapshot> {
        SessionStore::context_snapshot(&self.session)
    }

    fn contextual<'a>(
        &'a self,
        patch: &'a ContextPatch,
    ) -> BoxFuture<'a, Result<(), ContextError>> {
        SessionStore::contextual(&self.session, patch)
    }

    fn compacted<'a>(&'a self, replaced: usize, recap: &'a str) -> BoxFuture<'a, ()> {
        SessionStore::compacted(&self.session, replaced, recap)
    }

    fn display_compacted(&self, compacted: Compacted, pruned: bool) -> BoxFuture<'_, ()> {
        SessionStore::display_compacted(&self.session, compacted, pruned)
    }

    fn pruned<'a>(&'a self, freed: usize, results: &'a [ToolId]) -> BoxFuture<'a, ()> {
        SessionStore::pruned(&self.session, freed, results)
    }

    fn restricted<'a>(
        &'a self,
        freed: usize,
        results: &'a [ToolId],
        notice: &'a str,
    ) -> BoxFuture<'a, ()> {
        SessionStore::restricted(&self.session, freed, results, notice)
    }

    fn measured<'a>(&'a self, calibration: &'a Calibration) -> BoxFuture<'a, ()> {
        SessionStore::measured(&self.session, calibration)
    }

    fn calibrated(&self) -> Option<Calibration> {
        SessionStore::calibrated(&self.session)
    }
}

impl JournalStore for Tripwire {
    fn append_run_item<'a>(&'a self, item: &'a RunItem) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            JournalStore::append_run_item(&self.session, item).await;
            let RunItem::Invocation { record, .. } = item else {
                return;
            };
            if !self.stage.is(record.state()) || self.tripped.swap(true, Ordering::SeqCst) {
                return;
            }
            match &self.trip {
                Trip::Cancel(cancel) => cancel.request(),
                Trip::Park(gate) => {
                    gate.open();
                    std::future::pending::<()>().await;
                }
            }
        })
    }

    fn put_call_result<'a>(
        &'a self,
        key: CallResultKey,
        result: &'a ToolResult,
    ) -> BoxFuture<'a, Result<CallResultReceipt, CallResultStoreError>> {
        JournalStore::put_call_result(&self.session, key, result)
    }

    fn settle_call_results(&self) -> BoxFuture<'_, ()> {
        JournalStore::settle_call_results(&self.session)
    }
}
