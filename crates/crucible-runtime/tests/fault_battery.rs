//! The faults owned work has to survive, driven through the public group.
//!
//! Each test starts work shaped the way every service contract hands it back —
//! a boxed `Send` future — under a [`Group`], opens and holds it with barriers
//! on a paused clock, and asserts what the owner is left with once shutdown
//! returns: an end for every task it admitted, and nothing any task held still
//! alive. Neither is a claim about how long anything took, so no test here
//! decides by a timeout.
//!
//! A barrier is a [`Cancel`] the test raises: it is already what a task looks
//! at to learn that something outside it has happened.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crucible_runtime::{BoxFuture, Cancel, Ended, Full, Group};

/// Long enough that a task returning inside it is a fact about the code rather
/// than about the machine. The clock is paused, so it costs no wall time.
const GRACE: Duration = Duration::from_millis(500);

/// How long a task waits between looks at a barrier: a real await point, so
/// the runtime goes idle and the paused clock can advance.
const TICK: Duration = Duration::from_millis(10);

/// The most ticks a test gives its tasks to reach a state before deciding
/// they never will.
const TURNS: usize = 1_000;

/// What every task in one test has done, counted from outside them.
#[derive(Default)]
struct Ledger {
    /// Futures built and not yet dropped, whether or not they ever ran.
    live: Arc<AtomicUsize>,
    /// Futures polled at least once.
    started: Arc<AtomicUsize>,
    /// Resources given back through their own asynchronous release.
    cleaned: Arc<AtomicUsize>,
}

impl Ledger {
    fn live(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    fn started(&self) -> usize {
        self.started.load(Ordering::SeqCst)
    }

    fn cleaned(&self) -> usize {
        self.cleaned.load(Ordering::SeqCst)
    }
}

/// Held by a task's future from the moment it is built until it is dropped.
struct Alive(Arc<AtomicUsize>);

impl Alive {
    fn new(ledger: &Ledger) -> Self {
        ledger.live.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(&ledger.live))
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Returns once `barrier` is raised, looking once a tick.
async fn raised(barrier: &Cancel) {
    while !barrier.requested() {
        tokio::time::sleep(TICK).await;
    }
}

/// Lets the runtime run until `ready` holds, a tick at a time, and says
/// whether it came to.
async fn until(ready: impl Fn() -> bool) -> bool {
    for _ in 0..TURNS {
        if ready() {
            return true;
        }
        tokio::time::sleep(TICK).await;
    }
    ready()
}

/// A call that returns as soon as its token is raised.
fn heeding(ledger: &Ledger, cancel: Cancel) -> BoxFuture<'static, &'static str> {
    let alive = Alive::new(ledger);
    let started = Arc::clone(&ledger.started);
    Box::pin(async move {
        let _alive = alive;
        started.fetch_add(1, Ordering::SeqCst);
        raised(&cancel).await;
        "heeded"
    })
}

/// A call that waits on `barrier` alone and never looks at its token.
fn waiting(ledger: &Ledger, barrier: Cancel) -> BoxFuture<'static, &'static str> {
    let alive = Alive::new(ledger);
    let started = Arc::clone(&ledger.started);
    Box::pin(async move {
        let _alive = alive;
        started.fetch_add(1, Ordering::SeqCst);
        raised(&barrier).await;
        "opened"
    })
}

/// Something a call holds that takes time to give back, as a process does.
struct Resource {
    cleaned: Arc<AtomicUsize>,
    _alive: Alive,
}

impl Resource {
    fn release(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            tokio::time::sleep(TICK * 3).await;
            self.cleaned.fetch_add(1, Ordering::SeqCst);
        })
    }
}

/// A call that, once its token is raised, gives back what it holds before it
/// returns.
fn releasing(ledger: &Ledger, cancel: Cancel) -> BoxFuture<'static, &'static str> {
    let mut resource = Resource {
        cleaned: Arc::clone(&ledger.cleaned),
        _alive: Alive::new(ledger),
    };
    let started = Arc::clone(&ledger.started);
    Box::pin(async move {
        started.fetch_add(1, Ordering::SeqCst);
        raised(&cancel).await;
        resource.release().await;
        "released"
    })
}

#[tokio::test(start_paused = true)]
async fn a_shutdown_accounts_for_every_call_and_leaves_none_alive() {
    let ledger = Ledger::default();
    let never = Cancel::new();
    let mut group = Group::new(4, &Cancel::new());
    for _ in 0..3 {
        assert!(
            group
                .spawn(heeding(&ledger, group.cancel().clone()))
                .is_ok()
        );
    }
    assert!(group.spawn(waiting(&ledger, never.clone())).is_ok());
    assert!(
        until(|| ledger.started() == 4).await,
        "the calls never got going"
    );

    let ends = group.shutdown(GRACE).await;

    let heeded = ends
        .iter()
        .filter(|end| **end == Ended::Done("heeded"))
        .count();
    let stopped = ends.iter().filter(|end| **end == Ended::Stopped).count();
    assert_eq!(
        (ends.len(), heeded, stopped),
        (4, 3, 1),
        "the owner was not handed one end for each call it started: {ends:?}"
    );
    assert_eq!(
        ledger.live(),
        0,
        "a call was still alive when its owner's shutdown returned"
    );
    assert!(!never.requested(), "the barrier nobody raised was raised");
}

#[tokio::test(start_paused = true)]
async fn a_call_refused_at_the_bound_never_runs_and_is_let_go_at_once() {
    let ledger = Ledger::default();
    let barrier = Cancel::new();
    let mut group = Group::new(2, &Cancel::new());
    assert!(group.spawn(waiting(&ledger, barrier.clone())).is_ok());
    assert!(group.spawn(waiting(&ledger, barrier.clone())).is_ok());
    assert!(
        until(|| ledger.started() == 2).await,
        "the admitted calls never got going"
    );

    assert_eq!(
        group.spawn(waiting(&ledger, barrier.clone())),
        Err(Full),
        "a call was admitted past the bound"
    );
    assert_eq!(
        ledger.live(),
        2,
        "the refused call was kept somewhere rather than let go"
    );

    barrier.request();
    let ends = group.shutdown(GRACE).await;

    assert_eq!(
        ends,
        vec![Ended::Done("opened"); 2],
        "the owner was handed something other than the two calls it admitted"
    );
    assert_eq!(ledger.started(), 2, "the refused call ran anyway");
    assert_eq!(
        ledger.live(),
        0,
        "a call was still alive when its owner's shutdown returned"
    );
}

#[tokio::test(start_paused = true)]
async fn every_call_gives_back_what_it_held_before_the_shutdown_returns() {
    let ledger = Ledger::default();
    let mut group = Group::new(3, &Cancel::new());
    for _ in 0..3 {
        assert!(
            group
                .spawn(releasing(&ledger, group.cancel().clone()))
                .is_ok()
        );
    }
    assert!(
        until(|| ledger.started() == 3).await,
        "the calls never got going"
    );

    let ends = group.shutdown(GRACE).await;

    assert_eq!(
        ends,
        vec![Ended::Done("released"); 3],
        "a call was taken away before it gave back what it held"
    );
    assert_eq!(
        ledger.cleaned(),
        3,
        "a release was cut short, or never began"
    );
    assert_eq!(
        ledger.live(),
        0,
        "a call was still alive when its owner's shutdown returned"
    );
}
