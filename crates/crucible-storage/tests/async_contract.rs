//! Stores written outside this crate, driven through trait objects.
//!
//! This file is a crate of its own and sees only what `crucible-storage`
//! exports. It names the future a store hands back by spelling the type out,
//! as an implementation with no runtime of its own would, and asks each future
//! itself: this crate names no workspace crate but `crucible-storage` and
//! `crucible-types`, and a store written against it need name no more.

use std::fmt;
use std::future::{self, Future};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crucible_storage::{
    CheckpointId, CheckpointStore, ExecutionCheckpoint, JournalStore, PromptCacheResourceStore,
    ResumeDigest, ResumeScope, RunItem, SessionOwner, SessionStore,
};
use crucible_types::{
    Ancestry, Calibration, Compacted, ContextError, ContextPatch, ContextSnapshot, Message,
    PromptCacheFingerprint, PromptCacheIsolation, PromptCachePolicyDigest,
    PromptCacheResourceBinding, PromptCacheResourceError, PromptCacheResourceId,
    PromptCacheResourceOwner, PromptCacheResourceRecord, PromptCacheScopeDigest, SessionId, ToolId,
};

/// What a waiting store method hands back, spelled out.
type Written<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Asks a future once and evaluates to its answer.
///
/// Every store here keeps what it is told in memory, so each answers the
/// first time it is asked; one that would have had to wait fails the test at
/// the line that asked, rather than hanging.
#[allow(clippy::panic)] // A store that would wait is a test failure.
#[track_caller]
fn at_once<T>(mut future: Written<'_, T>) -> T {
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(answer) => answer,
        Poll::Pending => panic!("an in-memory store answers when it is first asked"),
    }
}

/// Every write one session records, in the order it was told of them.
#[derive(Default)]
struct Told {
    writes: Mutex<Vec<String>>,
}

/// What one recorded message says, in the fewest words that tell it apart.
fn said_words(message: &Message) -> String {
    match message {
        Message::User { text, .. } | Message::Agent { text, .. } => text.to_string(),
        Message::Context(_) | Message::ToolResults(_) => "nothing".to_owned(),
    }
}

#[allow(clippy::expect_used)] // A poisoned log is a test that already failed.
impl Told {
    fn write(&self, what: String) -> Written<'_, ()> {
        Box::pin(async move {
            self.writes
                .lock()
                .expect("nothing panics holding the log")
                .push(what);
        })
    }

    fn writes(&self) -> Vec<String> {
        self.writes
            .lock()
            .expect("nothing panics holding the log")
            .clone()
    }
}

impl SessionStore for Told {
    fn session_id(&self) -> Option<SessionId> {
        None
    }

    fn owner(&self) -> Option<SessionOwner> {
        None
    }

    fn append_message<'a>(&'a self, message: &'a Message) -> Written<'a, ()> {
        self.write(format!("said {}", said_words(message)))
    }

    fn context_snapshot(&self) -> Option<ContextSnapshot> {
        None
    }

    fn contextual<'a>(&'a self, _patch: &'a ContextPatch) -> Written<'a, Result<(), ContextError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn compacted<'a>(&'a self, replaced: usize, recap: &'a str) -> Written<'a, ()> {
        self.write(format!("compacted {replaced} into {recap}"))
    }

    fn display_compacted(&self, _compacted: Compacted, _pruned: bool) -> Written<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn pruned<'a>(&'a self, freed: usize, results: &'a [ToolId]) -> Written<'a, ()> {
        self.write(format!("pruned {} for {freed}", results.len()))
    }

    fn restricted<'a>(
        &'a self,
        freed: usize,
        results: &'a [ToolId],
        notice: &'a str,
    ) -> Written<'a, ()> {
        self.write(format!(
            "restricted {} for {freed}: {notice}",
            results.len()
        ))
    }

    fn measured<'a>(&'a self, calibration: &'a Calibration) -> Written<'a, ()> {
        self.write(format!("measured {}", calibration.sent))
    }

    fn calibrated(&self) -> Option<Calibration> {
        None
    }
}

impl JournalStore for Told {
    fn append_run_item<'a>(&'a self, item: &'a RunItem) -> Written<'a, ()> {
        let said = match item.model_message() {
            Some(message) => format!("said {}", said_words(message)),
            None => "journaled a framework record".to_owned(),
        };
        self.write(said)
    }

    fn settle_call_results(&self) -> Written<'_, ()> {
        self.write("settled the results".to_owned())
    }
}

/// One checkpoint kept, under the identity it was saved with.
struct Kept {
    checkpoint: Option<ExecutionCheckpoint>,
}

/// What a store written outside this crate refuses with.
#[derive(Debug)]
struct Refused(&'static str);

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Refused {}

impl CheckpointStore for Kept {
    type Error = Refused;

    fn save<'a>(
        &'a mut self,
        checkpoint: &'a ExecutionCheckpoint,
    ) -> Written<'a, Result<(), Self::Error>> {
        Box::pin(async move {
            self.checkpoint = Some(checkpoint.clone());
            Ok(())
        })
    }

    fn load(
        &self,
        id: CheckpointId,
    ) -> Written<'_, Result<Option<ExecutionCheckpoint>, Self::Error>> {
        Box::pin(async move { Ok(self.checkpoint.clone().filter(|kept| kept.id() == id)) })
    }

    fn remove(&mut self, id: CheckpointId) -> Written<'_, Result<(), Self::Error>> {
        Box::pin(async move {
            if self.checkpoint.as_ref().is_some_and(|kept| kept.id() == id) {
                self.checkpoint = None;
            }
            Ok(())
        })
    }
}

/// The authority a resume is taken under, in the four digests it is made of.
fn resume_scope(fill: u8) -> ResumeScope {
    ResumeScope::new(
        ResumeDigest::new([fill; 32]),
        ResumeDigest::new([fill.wrapping_add(1); 32]),
        ResumeDigest::new([fill.wrapping_add(2); 32]),
        ResumeDigest::new([fill.wrapping_add(3); 32]),
    )
}

/// One checkpoint, bounded and holding nothing else.
#[allow(clippy::expect_used)] // Every word this fixture is built from is valid.
fn checkpoint(id: CheckpointId) -> ExecutionCheckpoint {
    ExecutionCheckpoint::new(id, Ancestry::new(), resume_scope(1), None, 1_000, 5_000)
        .expect("a bounded checkpoint")
}

/// Resource records kept in memory, up to a fixed count.
struct Held {
    records: Vec<PromptCacheResourceRecord>,
    limit: usize,
}

impl fmt::Debug for Held {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Held({} of {})", self.records.len(), self.limit)
    }
}

impl PromptCacheResourceStore for Held {
    fn matching<'a>(
        &'a mut self,
        binding: &'a PromptCacheResourceBinding,
    ) -> Written<'a, Result<Option<PromptCacheResourceRecord>, PromptCacheResourceError>> {
        Box::pin(async move {
            Ok(self
                .records
                .iter()
                .rev()
                .find(|record| record.binding() == binding)
                .cloned())
        })
    }

    fn put<'a>(
        &'a mut self,
        record: &'a PromptCacheResourceRecord,
    ) -> Written<'a, Result<(), PromptCacheResourceError>> {
        Box::pin(async move {
            self.records.retain(|kept| kept.id() != record.id());
            if self.records.len() >= self.limit {
                return Err(PromptCacheResourceError::StoreFull);
            }
            self.records.push(record.clone());
            Ok(())
        })
    }

    fn remove<'a>(
        &'a mut self,
        id: &'a PromptCacheResourceId,
    ) -> Written<'a, Result<(), PromptCacheResourceError>> {
        Box::pin(async move {
            self.records.retain(|kept| kept.id() != id);
            Ok(())
        })
    }

    fn inspect(
        &mut self,
        maximum: usize,
    ) -> Written<'_, Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError>> {
        Box::pin(async move { Ok(self.records.iter().take(maximum).cloned().collect()) })
    }
}

#[allow(clippy::expect_used)] // Every word the binding is given is valid.
fn record(model: &str) -> PromptCacheResourceRecord {
    let binding = PromptCacheResourceBinding::new(
        PromptCacheScopeDigest::new([1; 32]),
        PromptCacheScopeDigest::new([4; 32]),
        PromptCacheScopeDigest::new([5; 32]),
        PromptCacheFingerprint::new([2; 32]),
        PromptCachePolicyDigest::new([3; 32]),
        PromptCacheResourceOwner::new(PromptCacheIsolation::Session, true),
        "fixture",
        model,
        None,
    )
    .expect("a valid binding");
    PromptCacheResourceRecord::creating(PromptCacheResourceId::new(), binding, 100)
}

#[test]
fn an_external_session_store_is_written_through_a_trait_object() {
    let told = Arc::new(Told::default());
    let store: Arc<dyn SessionStore> = Arc::clone(&told) as Arc<dyn SessionStore>;
    let results = [ToolId::new("one"), ToolId::new("two")];
    let withheld = [ToolId::new("three")];
    let calibration = Calibration {
        sent: 42,
        ..Calibration::default()
    };

    at_once(store.compacted(3, "a recap"));
    at_once(store.pruned(10, &results));
    at_once(store.restricted(20, &withheld, "withheld"));
    at_once(store.measured(&calibration));

    assert_eq!(
        told.writes(),
        [
            "compacted 3 into a recap",
            "pruned 2 for 10",
            "restricted 1 for 20: withheld",
            "measured 42",
        ]
    );
}

#[test]
fn a_write_borrowing_its_message_is_asked_on_another_thread() {
    let told = Arc::new(Told::default());
    let store: Arc<dyn SessionStore> = Arc::clone(&told) as Arc<dyn SessionStore>;
    let message = Message::said("hello");
    let writing = store.append_message(&message);

    std::thread::scope(|scope| {
        scope
            .spawn(move || at_once(writing))
            .join()
            .expect("the write ends");
    });

    assert_eq!(told.writes(), ["said hello"]);
}

#[test]
fn an_external_journal_store_keeps_the_order_it_was_told_in() {
    let told = Arc::new(Told::default());
    let store: Arc<dyn JournalStore> = Arc::clone(&told) as Arc<dyn JournalStore>;
    let said = Message::said("hello");
    let journaled = RunItem::message(Ancestry::new(), said.clone()).expect("a bounded item");

    at_once(store.append_message(&said));
    at_once(store.append_run_item(&journaled));
    at_once(store.settle_call_results());

    // The message, then the record that is about it, then the settle. A settle
    // before the results it settles would claim a durability nothing has
    // written yet, and a journal line before the conversation line it is about
    // would leave a log cut between the two resuming without the message: a
    // resume reads the conversation, and the journal is not read back.
    assert_eq!(
        told.writes(),
        ["said hello", "said hello", "settled the results",]
    );
}

#[test]
fn an_external_checkpoint_store_holds_one_checkpoint_through_a_trait_object() {
    let mut store: Box<dyn CheckpointStore<Error = Refused>> = Box::new(Kept { checkpoint: None });
    let id = CheckpointId::new();
    let kept = checkpoint(id);
    let elsewhere = checkpoint(CheckpointId::new());

    at_once(store.save(&kept)).expect("the checkpoint is kept");
    assert_eq!(
        at_once(store.load(id))
            .expect("the store reads")
            .map(|held| held.id()),
        Some(id)
    );
    // A store holds the one it was given, and answers `None` for any other
    // identity rather than the checkpoint that happens to be there.
    assert!(
        at_once(store.load(elsewhere.id()))
            .expect("the store reads")
            .is_none()
    );

    at_once(store.remove(id)).expect("the checkpoint is gone");
    assert!(at_once(store.load(id)).expect("the store reads").is_none());
}

#[test]
fn a_checkpoint_write_borrowing_its_record_is_asked_on_another_thread() {
    let mut store: Box<dyn CheckpointStore<Error = Refused>> = Box::new(Kept { checkpoint: None });
    let id = CheckpointId::new();
    let kept = checkpoint(id);
    let writing = store.save(&kept);

    std::thread::scope(|scope| {
        scope
            .spawn(move || at_once(writing))
            .join()
            .expect("the write ends")
    })
    .expect("the checkpoint is kept");

    assert_eq!(
        at_once(store.load(id))
            .expect("the store reads")
            .map(|held| held.id()),
        Some(id)
    );
}

#[test]
fn an_external_resource_store_holds_records_through_a_trait_object() {
    let mut store: Box<dyn PromptCacheResourceStore> = Box::new(Held {
        records: Vec::new(),
        limit: 1,
    });
    let first = record("model-a");
    let second = record("model-b");

    assert!(at_once(store.put(&first)).is_ok());
    assert!(matches!(
        at_once(store.put(&second)),
        Err(PromptCacheResourceError::StoreFull)
    ));
    let found = at_once(store.matching(first.binding())).expect("the store reads");
    assert_eq!(
        found.map(|kept| kept.id().clone()),
        Some(first.id().clone())
    );

    assert!(at_once(store.remove(first.id())).is_ok());
    let left = at_once(store.inspect(10)).expect("the store reads");
    assert!(left.is_empty());
}
