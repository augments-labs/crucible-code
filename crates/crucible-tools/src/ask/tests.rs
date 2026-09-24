use crucible_types::Answer;

use super::*;

#[test]
fn what_answers_a_question_is_reached_as_a_trait_and_may_answer_nobody() {
    struct Nobody;
    impl Put for Nobody {
        fn put<'a>(&'a self, _questions: &'a [Question]) -> BoxFuture<'a, Option<Vec<Answered>>> {
            Box::pin(async { None })
        }
    }

    struct Always(&'static str);
    impl Put for Always {
        fn put<'a>(&'a self, questions: &'a [Question]) -> BoxFuture<'a, Option<Vec<Answered>>> {
            Box::pin(
                async move { Some(questions.iter().map(|_| Answered::new([self.0])).collect()) },
            )
        }
    }

    let asked = [
        Question::new("One", "Which?", [Answer::new("Rust")]),
        Question::new("Two", "And?", [Answer::new("Python")]),
    ];

    let nobody: &dyn Put = &Nobody;
    assert!(crucible_runtime::answered!(nobody.put(&asked)).is_none());

    let always: &dyn Put = &Always("Rust");
    let given =
        crucible_runtime::answered!(always.put(&asked)).expect("an answer to every question");
    assert_eq!(given.len(), 2);
    assert_eq!(
        given
            .iter()
            .map(|one| one.chosen().collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        [["Rust"], ["Rust"]]
    );
}

/// A [`Put`] whose answer is not there yet: a stand-in for the panel while
/// nobody has pressed a key.
///
/// A hand-written future rather than an `async` block: what this proves is
/// that polling it does not block, so the future has to answer `Pending` on
/// its own the moment it is asked, with no wait inside the poll that produces
/// it. Unlike an `async` block awaiting a real channel, this fake has to save
/// the waker itself and wake it by hand once [`Stalled::answer`] is called —
/// which is also what proves an implementation that stores a waker and calls
/// it, the way a real front end's channel does, really does wake a future
/// left pending on it rather than depending on being polled again by luck.
///
/// The manual poll this test drives it with belongs here, in a file this
/// crate's own bridge-ledger check reads as a test rather than as shipped
/// source, and not beside the trait: a synchronous caller in shipped code
/// crosses through a `Bridge` instead of building a waker by hand.
struct Stalled {
    answer: std::sync::Mutex<Option<Vec<Answered>>>,
    waker: std::sync::Mutex<Option<std::task::Waker>>,
}

impl Stalled {
    fn new() -> Self {
        Self {
            answer: std::sync::Mutex::new(None),
            waker: std::sync::Mutex::new(None),
        }
    }

    /// Gives the answer, and wakes whatever waker the last `Pending` poll
    /// left behind — the same two steps a real front end's channel takes
    /// once a person decides.
    fn answer(&self, given: Vec<Answered>) {
        *self.answer.lock().expect("no poisoned test lock") = Some(given);
        if let Some(waker) = self.waker.lock().expect("no poisoned test lock").take() {
            waker.wake();
        }
    }
}

impl Put for Stalled {
    fn put<'a>(
        &'a self,
        _questions: &'a [Question],
    ) -> crucible_runtime::BoxFuture<'a, Option<Vec<Answered>>> {
        struct Waiting<'a> {
            answer: &'a std::sync::Mutex<Option<Vec<Answered>>>,
            waker: &'a std::sync::Mutex<Option<std::task::Waker>>,
        }

        impl std::future::Future for Waiting<'_> {
            type Output = Option<Vec<Answered>>;

            fn poll(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                if let Some(answer) = self.answer.lock().expect("no poisoned test lock").take() {
                    std::task::Poll::Ready(Some(answer))
                } else {
                    *self.waker.lock().expect("no poisoned test lock") = Some(cx.waker().clone());
                    std::task::Poll::Pending
                }
            }
        }

        Box::pin(Waiting {
            answer: &self.answer,
            waker: &self.waker,
        })
    }
}

#[test]
fn a_delayed_answer_leaves_put_pending_and_completes_once_the_panel_gives_it() {
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    let stalled = Stalled::new();
    let asked = [Question::new("One", "Which?", [Answer::new("Rust")])];

    let mut future = pin!(stalled.put(&asked));
    let mut cx = Context::from_waker(Waker::noop());

    // The call itself never blocks: this poll returns at once, whether or not
    // anybody has answered, because the wait lives in the future's own
    // `Pending` rather than inside the call that asks it.
    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));

    stalled.answer(vec![Answered::new(["Rust"])]);

    match future.as_mut().poll(&mut cx) {
        Poll::Ready(Some(given)) => {
            assert_eq!(
                given.first().map(|one| one.chosen().collect::<Vec<_>>()),
                Some(vec!["Rust"])
            );
        }
        other => panic!("expected the answer once it was given, got {other:?}"),
    }
}

/// The completion above is shown with a waker that does nothing, so a real
/// executor asleep on this future would never be told to poll it again. This
/// proves the other half: the waker a real poll left behind is the one
/// [`Stalled::answer`] wakes, the way a conforming `Put` must.
#[test]
fn a_delayed_answer_wakes_the_waker_the_pending_poll_left_behind() {
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};

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

    let stalled = Stalled::new();
    let asked = [Question::new("One", "Which?", [Answer::new("Rust")])];
    let counted = Arc::new(Counting(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&counted));
    let mut cx = Context::from_waker(&waker);

    let mut future = pin!(stalled.put(&asked));
    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
    assert_eq!(
        counted.0.load(Ordering::SeqCst),
        0,
        "nothing should wake a future nobody has answered yet"
    );

    stalled.answer(vec![Answered::new(["Rust"])]);
    assert_eq!(
        counted.0.load(Ordering::SeqCst),
        1,
        "the answer did not wake the waker the pending poll left behind"
    );

    assert!(matches!(
        future.as_mut().poll(&mut cx),
        Poll::Ready(Some(_))
    ));
}
