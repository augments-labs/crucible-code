//! A store that keeps a session in memory.
//!
//! Every runner test needs somewhere for a turn to be recorded, and what it is
//! testing is never the file. This is the whole of [`JournalStore`] — and so
//! the whole of [`SessionStore`] under it — written against nothing but the
//! contracts, which is the point: if a turn can be run and picked up again
//! through this, the loop above it names no store in particular.
//!
//! [`Recording::reopened`] is the half that makes it worth having. It replays
//! what was recorded into the transcript a later run would be asked with,
//! obeying the same records a durable store's replay obeys — room made,
//! results cleared, a vendor's results withdrawn, the reading the last request
//! carried. A store that only appended could not answer it.

use std::sync::{Arc, Mutex, PoisonError};

use sha2::{Digest as _, Sha256};

use crucible_core::{
    Calibration, CallResultKey, CallResultReceipt, CallResultStoreError, Compacted, ContextError,
    ContextPatch, ContextSnapshot, JournalStore, Message, RunItem, SessionId, SessionOwner,
    SessionStore, ToolId, ToolResult, Transcript,
};
use crucible_runtime::BoxFuture;

/// Domain separator for the receipt this store answers with.
const RECEIPT_DOMAIN: &[u8] = b"crucible:in-memory-call-result:v1\0";

/// One thing a store was told, in the order it was told.
///
/// The conversation and the framework history are both here, in one order,
/// because that is the one thing a record has that a pair of collections does
/// not: a test that asks whether the line came after the message it is about
/// can only ask it of a single sequence.
#[derive(Debug, Clone)]
pub(crate) enum Kept {
    /// A closed conversation message.
    Said(Message),
    /// A framework record, which is not a conversation message.
    Journaled(RunItem),
    /// Room made: what the notes replace, and what they say.
    Compacted { replaced: usize, recap: Box<str> },
    /// The notice a reader was shown for a completed compaction.
    Shown { compacted: Compacted, pruned: bool },
    /// Old tool results cleared to make room.
    Pruned { freed: usize, results: Vec<ToolId> },
    /// Results cleared because the vendor that produced them restricts them.
    Restricted {
        freed: usize,
        results: Vec<ToolId>,
        notice: Box<str>,
    },
    /// What the request behind the message above carried.
    Measured(Calibration),
    /// The results held beside the record were made durable.
    ///
    /// In the same sequence as the messages so that when it happened is
    /// readable: a settle recorded before the results it settles would be a
    /// claim of durability made about something not yet written down.
    Settled,
    /// One per-pass merge patch of typed model-visible state.
    Contextual(ContextPatch),
}

/// Everything one session was told, held in memory.
#[derive(Debug)]
pub(crate) struct Recording {
    /// Which session this is, absent where nothing is being recorded.
    id: Option<SessionId>,
    /// Whose records these are. Two stores under different names are two
    /// principals, which is all anything above compares them for.
    owner: Option<SessionOwner>,
    /// What was recorded, in order. Behind a lock because a turn writes from
    /// the thread it runs on while the test reads the same store.
    kept: Mutex<Vec<Kept>>,
    /// The typed state the patches above have advanced.
    context: Mutex<Option<ContextSnapshot>>,
    /// What the record this session was picked up from last said it carried.
    /// Settled at pick-up and never written again, for the reason
    /// [`SessionStore::calibrated`] gives.
    carried: Option<Calibration>,
    /// One result per identity, with the receipt that identity was answered by.
    results: Mutex<Vec<(CallResultKey, ToolResult, CallResultReceipt)>>,
}

impl Recording {
    /// A store nothing is kept in: a run asked not to be recorded.
    pub(crate) fn nowhere() -> Arc<Self> {
        Arc::new(Self::held(None, None, Vec::new(), None))
    }

    /// A session recorded under a name of its own, in `owner`.
    pub(crate) fn started(owner: &str) -> Arc<Self> {
        Arc::new(Self::held(
            Some(SessionId::new()),
            SessionOwner::new(owner),
            Vec::new(),
            None,
        ))
    }

    /// A session recorded before typed model-visible state existed.
    ///
    /// It answers unknown vintage rather than a known empty snapshot, which is
    /// the distinction a run picking it up has to state defensively instead of
    /// assuming: nothing here says what the model was last told.
    pub(crate) fn pre_context(owner: &str) -> Arc<Self> {
        let store = Self::held(
            Some(SessionId::new()),
            SessionOwner::new(owner),
            Vec::new(),
            None,
        );
        *store.context.lock().unwrap_or_else(PoisonError::into_inner) = None;
        Arc::new(store)
    }

    /// The same session picked up again, and the transcript a later run would
    /// be asked with.
    ///
    /// Which session it is and whose carry over as they stand, the way a log
    /// keeps its name across a reopen. Of what a later run is asked with,
    /// nothing but the log's own vintage is read off the store's own fields:
    /// the transcript is rebuilt from the records, the way a durable store
    /// rebuilds one from its file, so a recording that wrote the wrong line
    /// answers the wrong transcript here rather than passing on what it
    /// happened to be holding.
    pub(crate) fn reopened(&self) -> (Arc<Self>, Transcript) {
        let kept = self.kept();
        let mut transcript = Transcript::new();
        // Whether there is typed model-visible state at all is the log's
        // vintage rather than anything a record carries, and a reopen does not
        // get to invent one: a store that opened knowing nothing of it and was
        // never told any answers unknown again, as a log written before that
        // state existed does. What the state *is* is rebuilt below.
        let mut context = self.context_snapshot().is_some().then(ContextSnapshot::new);
        let mut calibration = None;

        for one in &kept {
            match one {
                Kept::Said(message) => {
                    let _ = transcript.push(message.clone());
                    calibration = None;
                }
                Kept::Compacted { replaced, recap } => {
                    transcript.compacted(*replaced, recap.as_ref());
                    calibration = None;
                }
                Kept::Pruned { results, .. } => {
                    transcript.prune(results);
                    calibration = None;
                }
                Kept::Restricted {
                    results, notice, ..
                } => {
                    transcript.clear_tool_outputs(results, notice);
                    calibration = None;
                }
                Kept::Contextual(patch) => {
                    context = patch.apply(&context.unwrap_or_default()).ok();
                    calibration = None;
                }
                // What the request behind the answer above carried. Kept only
                // while nothing follows it, which is what makes it usable.
                Kept::Measured(measured) => calibration = Some(*measured),
                // Framework history and the reader's own notice change neither
                // the transcript nor the reading.
                Kept::Journaled(_) | Kept::Shown { .. } | Kept::Settled => {}
            }
        }

        let picked = Self::held(self.id.clone(), self.owner.clone(), kept, calibration);
        *picked
            .context
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = context;
        (Arc::new(picked), transcript)
    }

    /// Everything recorded, in the order it was recorded.
    pub(crate) fn kept(&self) -> Vec<Kept> {
        self.kept
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Every conversation message recorded, in order.
    pub(crate) fn said(&self) -> Vec<Message> {
        self.kept()
            .into_iter()
            .filter_map(|one| match one {
                Kept::Said(message) => Some(message),
                _ => None,
            })
            .collect()
    }

    /// Every framework record written, in order.
    pub(crate) fn journaled(&self) -> Vec<RunItem> {
        self.kept()
            .into_iter()
            .filter_map(|one| match one {
                Kept::Journaled(item) => Some(item),
                _ => None,
            })
            .collect()
    }

    /// How many times the results held beside the record were settled.
    pub(crate) fn settled(&self) -> usize {
        self.kept()
            .iter()
            .filter(|one| matches!(one, Kept::Settled))
            .count()
    }

    /// The name a session recorded here answers to.
    pub(crate) fn id(&self) -> Option<SessionId> {
        self.id.clone()
    }

    fn held(
        id: Option<SessionId>,
        owner: Option<SessionOwner>,
        kept: Vec<Kept>,
        carried: Option<Calibration>,
    ) -> Self {
        Self {
            id,
            owner,
            kept: Mutex::new(kept),
            context: Mutex::new(Some(ContextSnapshot::new())),
            carried,
            results: Mutex::new(Vec::new()),
        }
    }

    fn record(&self, one: Kept) {
        self.kept
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(one);
    }
}

impl SessionStore for Recording {
    fn session_id(&self) -> Option<SessionId> {
        self.id.clone()
    }

    fn owner(&self) -> Option<SessionOwner> {
        self.owner.clone()
    }

    fn append_message<'a>(&'a self, message: &'a Message) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.record(Kept::Said(message.clone()));
        })
    }

    fn context_snapshot(&self) -> Option<ContextSnapshot> {
        self.context
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn contextual<'a>(
        &'a self,
        patch: &'a ContextPatch,
    ) -> BoxFuture<'a, Result<(), ContextError>> {
        Box::pin(async move {
            let mut held = self.context.lock().unwrap_or_else(PoisonError::into_inner);
            let advanced = patch.apply(&held.clone().unwrap_or_default())?;
            *held = Some(advanced);
            drop(held);
            self.record(Kept::Contextual(patch.clone()));
            Ok(())
        })
    }

    fn compacted<'a>(&'a self, replaced: usize, recap: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.record(Kept::Compacted {
                replaced,
                recap: recap.into(),
            });
        })
    }

    fn display_compacted(&self, compacted: Compacted, pruned: bool) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.record(Kept::Shown { compacted, pruned });
        })
    }

    fn pruned<'a>(&'a self, freed: usize, results: &'a [ToolId]) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.record(Kept::Pruned {
                freed,
                results: results.to_vec(),
            });
        })
    }

    fn restricted<'a>(
        &'a self,
        freed: usize,
        results: &'a [ToolId],
        notice: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.record(Kept::Restricted {
                freed,
                results: results.to_vec(),
                notice: notice.into(),
            });
        })
    }

    fn measured<'a>(&'a self, calibration: &'a Calibration) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.record(Kept::Measured(*calibration));
        })
    }

    fn calibrated(&self) -> Option<Calibration> {
        self.carried
    }
}

impl JournalStore for Recording {
    fn append_run_item<'a>(&'a self, item: &'a RunItem) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.record(Kept::Journaled(item.clone()));
        })
    }

    /// Answers the same receipt for the same content and refuses different
    /// content under a key already taken.
    ///
    /// A store with nowhere to keep anything says so instead: a receipt from a
    /// store that kept nothing is what would let a background acceptance claim
    /// durability nobody has.
    fn put_call_result<'a>(
        &'a self,
        key: CallResultKey,
        result: &'a ToolResult,
    ) -> BoxFuture<'a, Result<CallResultReceipt, CallResultStoreError>> {
        Box::pin(async move {
            if self.id.is_none() {
                return Err(CallResultStoreError::Unavailable);
            }

            let receipt = receipt(key, result);
            let mut held = self.results.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((_, _, already)) = held.iter().find(|(taken, _, _)| *taken == key) {
                return if *already == receipt {
                    Ok(receipt)
                } else {
                    Err(CallResultStoreError::Conflict)
                };
            }

            held.push((key, result.clone(), receipt));
            Ok(receipt)
        })
    }

    fn settle_call_results(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.record(Kept::Settled);
        })
    }
}

/// The digest one identity and one exact result are answered by.
fn receipt(key: CallResultKey, result: &ToolResult) -> CallResultReceipt {
    let mut digest = Sha256::new();
    digest.update(RECEIPT_DOMAIN);
    digest.update(key.bytes());
    digest.update(result.id.as_str().as_bytes());
    digest.update(result.output.text().as_bytes());
    CallResultReceipt::from_digest(digest.finalize().into())
}
