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

use crucible_storage::{PromptCacheResourceStore, SessionOwner, SessionStore};
use crucible_types::{
    Calibration, Compacted, ContextError, ContextPatch, ContextSnapshot, Message,
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
        let said = match message {
            Message::User { text, .. } | Message::Agent { text, .. } => format!("said {text}"),
            Message::Context(_) | Message::ToolResults(_) => "said nothing".to_owned(),
        };
        self.write(said)
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
