//! A real session log, watched at the moment its accepted results are settled.
//!
//! Settling removes the create-once copy of a result that was accepted before
//! the turn wrote it down. The log is the only other place the result lives,
//! so what matters is what the file holds at that instant, and only something
//! standing between the runner and the session can look then.

use std::fs;
use std::sync::{Arc, Mutex};

use crucible_core::{
    Ancestry, Calibration, CallResultKey, CallResultReceipt, CallResultStoreError, Compacted,
    ContextError, ContextPatch, ContextSnapshot, InvocationId, JournalStore, Message,
    RecordedToolOutput, RunItem, SessionId, SessionOwner, SessionStore, ToolId, ToolResult,
};
use crucible_session::Session;

/// What the file held each time the runner asked for a settle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Settle {
    /// A result was waiting beside the log when the settle was asked for.
    pub(crate) waiting: bool,
    /// The log on disk already held a results line once the settle returned.
    pub(crate) written: bool,
    /// Nothing was left beside the log afterwards.
    pub(crate) cleared: bool,
}

/// A session every call is passed straight through to.
///
/// It adds one thing a background tool would: a result accepted as durable the
/// moment its call is recorded, before the turn has written any answer down.
pub(crate) struct Watched {
    session: Arc<Session>,
    settles: Mutex<Vec<Settle>>,
}

impl Watched {
    pub(crate) fn over(session: Arc<Session>) -> Arc<Self> {
        Arc::new(Self {
            session,
            settles: Mutex::default(),
        })
    }

    pub(crate) fn settles(&self) -> Vec<Settle> {
        self.settles.lock().expect("valid fixture").clone()
    }

    fn beside(&self) -> std::path::PathBuf {
        self.session.path().with_extension("results")
    }
}

impl SessionStore for Watched {
    fn session_id(&self) -> Option<SessionId> {
        self.session.session_id()
    }

    fn owner(&self) -> Option<SessionOwner> {
        SessionStore::owner(&*self.session)
    }

    fn append_message(&self, message: &Message) {
        self.session.append_message(message);
        if let Message::Agent { calls, .. } = message {
            for call in calls {
                let accepted = ToolResult {
                    id: call.id.clone(),
                    output: RecordedToolOutput::ok("accepted in the background"),
                };
                let key = CallResultKey::derive(Ancestry::new(), InvocationId::new(), &call.id);
                self.session
                    .put_call_result(key, &accepted)
                    .expect("a recorded session accepts a result");
            }
        }
    }

    fn context_snapshot(&self) -> Option<ContextSnapshot> {
        self.session.context_snapshot()
    }

    fn contextual(&self, patch: &ContextPatch) -> Result<(), ContextError> {
        self.session.contextual(patch)
    }

    fn compacted(&self, replaced: usize, recap: &str) {
        SessionStore::compacted(&*self.session, replaced, recap);
    }

    fn display_compacted(&self, compacted: Compacted, pruned: bool) {
        SessionStore::display_compacted(&*self.session, compacted, pruned);
    }

    fn pruned(&self, freed: usize, results: &[ToolId]) {
        SessionStore::pruned(&*self.session, freed, results);
    }

    fn restricted(&self, freed: usize, results: &[ToolId], notice: &str) {
        SessionStore::restricted(&*self.session, freed, results, notice);
    }

    fn measured(&self, calibration: &Calibration) {
        SessionStore::measured(&*self.session, calibration);
    }

    fn calibrated(&self) -> Option<Calibration> {
        SessionStore::calibrated(&*self.session)
    }
}

impl JournalStore for Watched {
    fn append_run_item(&self, item: &RunItem) {
        self.session.append_run_item(item);
    }

    fn put_call_result(
        &self,
        key: CallResultKey,
        result: &ToolResult,
    ) -> Result<CallResultReceipt, CallResultStoreError> {
        self.session.put_call_result(key, result)
    }

    fn settle_call_results(&self) {
        let waiting = self.beside().exists();
        self.session.settle_call_results();
        // Read as bytes on disk and through no door of the session's, so
        // nothing here can flush a line the settle did not wait for.
        let written = fs::read_to_string(self.session.path())
            .expect("the log being written")
            .lines()
            .any(|line| line.contains("\"results\":[{"));
        self.settles.lock().expect("valid fixture").push(Settle {
            waiting,
            written,
            cleared: !self.beside().exists(),
        });
    }
}
