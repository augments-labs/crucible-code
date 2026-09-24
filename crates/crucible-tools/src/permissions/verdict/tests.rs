use std::future::poll_fn;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crucible_types::{ToolArgs, ToolId};

use super::*;
use crate::permissions::Target;

/// An [`Ask`] whose verdict is not there yet: a stand-in for the panel while
/// nobody has answered the permission question.
///
/// `poll_fn` rather than an `async` block: what this proves is that polling
/// it does not block, so the future has to answer `Pending` on its own the
/// moment it is asked, with no wait inside the poll that produces it. Unlike
/// an `async` block awaiting a real channel, this fake has to save the waker
/// itself and wake it by hand once [`Decides::decide`] is called — which is
/// also what proves an implementation that stores a waker and calls it, the
/// way a real front end's channel does, really does wake a future left
/// pending on it rather than depending on being polled again by luck.
///
/// [`Ask::ask`] takes `&mut self` for the length of the future, and a test
/// that also had to reach the answer through `self` could not hand it over
/// while the future it is answering is still alive — so [`Stalled::decides`]
/// hands out a separate handle onto the same state before `ask` is ever
/// called, the way [`crate::ask::tests::Stalled`] takes its own handle to
/// the slot it answers.
///
/// The manual poll this test drives it with belongs here, in a file this
/// crate's own bridge-ledger check reads as a test rather than as shipped
/// source, and not beside the trait: a synchronous caller in shipped code
/// crosses through a `Bridge` instead of building a waker by hand.
struct Stalled {
    decided: Arc<Mutex<Option<(Verdict, Remember)>>>,
    waker: Arc<Mutex<Option<Waker>>>,
}

/// A handle onto a [`Stalled`]'s state that does not borrow it, so a test can
/// hold one across the length of the future [`Ask::ask`] hands back.
struct Decides {
    decided: Arc<Mutex<Option<(Verdict, Remember)>>>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Decides {
    /// Gives the verdict, and wakes whatever waker the last `Pending` poll
    /// left behind — the same two steps a real front end's channel takes
    /// once a person decides.
    fn decide(&self, verdict: Verdict, remember: Remember) {
        *self.decided.lock().expect("no poisoned test lock") = Some((verdict, remember));
        if let Some(waker) = self.waker.lock().expect("no poisoned test lock").take() {
            waker.wake();
        }
    }
}

impl Stalled {
    fn new() -> Self {
        Self {
            decided: Arc::new(Mutex::new(None)),
            waker: Arc::new(Mutex::new(None)),
        }
    }

    fn decides(&self) -> Decides {
        Decides {
            decided: Arc::clone(&self.decided),
            waker: Arc::clone(&self.waker),
        }
    }
}

impl Ask for Stalled {
    fn ask<'a>(
        &'a mut self,
        _call: &'a ToolCall,
        _sensitivity: &'a Sensitivity,
    ) -> crucible_runtime::BoxFuture<'a, (Verdict, Remember)> {
        let decided = Arc::clone(&self.decided);
        let waker = Arc::clone(&self.waker);
        Box::pin(poll_fn(move |cx| {
            if let Some(decision) = decided.lock().expect("no poisoned test lock").take() {
                Poll::Ready(decision)
            } else {
                *waker.lock().expect("no poisoned test lock") = Some(cx.waker().clone());
                Poll::Pending
            }
        }))
    }
}

#[test]
fn a_delayed_verdict_leaves_ask_pending_and_completes_once_the_panel_decides() {
    let mut stalled = Stalled::new();
    let call = ToolCall {
        id: ToolId::new("call-1"),
        name: "bash".into(),
        args: ToolArgs::new("{}"),
    };
    let sensitivity = Sensitivity::ReadOnly {
        target: Target::at("/w/a", Some("a")),
    };
    let decides = stalled.decides();

    let mut future = pin!(stalled.ask(&call, &sensitivity));
    let mut cx = Context::from_waker(Waker::noop());

    // The call itself never blocks: this poll returns at once, whether or not
    // anybody has decided, because the wait lives in the future's own
    // `Pending` rather than inside the call that asks it.
    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));

    decides.decide(Verdict::Allow, Remember::Never);

    match future.as_mut().poll(&mut cx) {
        Poll::Ready((verdict, remember)) => {
            assert_eq!(verdict, Verdict::Allow);
            assert_eq!(remember, Remember::Never);
        }
        Poll::Pending => panic!("expected the verdict once it was decided"),
    }
}

/// The completion above is shown with a waker that does nothing, so a real
/// executor asleep on this future would never be told to poll it again. This
/// proves the other half: the waker a real poll left behind is the one
/// [`Stalled::decide`] wakes, the way a conforming `Ask` must.
#[test]
fn a_delayed_verdict_wakes_the_waker_the_pending_poll_left_behind() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    /// A waker that only counts how many times it was asked to wake its
    /// task, so a test can tell whether waking happened without needing a
    /// real executor to observe it happening.
    struct Counting(AtomicUsize);

    impl Wake for Counting {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let mut stalled = Stalled::new();
    let call = ToolCall {
        id: ToolId::new("call-1"),
        name: "bash".into(),
        args: ToolArgs::new("{}"),
    };
    let sensitivity = Sensitivity::ReadOnly {
        target: Target::at("/w/a", Some("a")),
    };
    let decides = stalled.decides();
    let counted = Arc::new(Counting(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&counted));
    let mut cx = Context::from_waker(&waker);

    let mut future = pin!(stalled.ask(&call, &sensitivity));
    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
    assert_eq!(
        counted.0.load(Ordering::SeqCst),
        0,
        "nothing should wake a future nobody has decided yet"
    );

    decides.decide(Verdict::Deny, Remember::Never);
    assert_eq!(
        counted.0.load(Ordering::SeqCst),
        1,
        "the decision did not wake the waker the pending poll left behind"
    );

    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Ready(_)));
}
