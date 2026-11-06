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
/// The manual poll this test drives it with belongs here, in a file the
/// workspace's hand-poll check reads as a test rather than as shipped source,
/// and not beside the trait: a caller in shipped code awaits the future instead
/// of building a waker by hand.
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

/// Two questions as one `ask_user` call asks them: one taking a single answer
/// and recommending its first, one taking several.
fn asked(heading: &str) -> Vec<Question> {
    vec![
        Question::new(
            heading,
            "Which language should the examples use?",
            [
                Answer::new("Rust").recommending(),
                Answer::new("Python").saying("the scripts are"),
            ],
        ),
        Question::new(
            "Checks",
            "Which checks should run?",
            [
                Answer::new("lint"),
                Answer::new("test").showing(["cargo test"]),
            ],
        )
        .several(),
    ]
}

/// What a refusal is expected to be.
type Expected = fn(&Unsettled) -> bool;

/// The heading of the first question in the `at`th batch.
fn heading(batches: &[Batch], at: usize) -> Option<&str> {
    batches.get(at)?.questions().first().map(Question::heading)
}

/// The one thing in `taken`.
fn only(taken: Vec<Delivered>) -> Delivered {
    let mut taken = taken.into_iter();
    let one = taken.next().unwrap();
    assert!(taken.next().is_none());
    one
}

fn chose(names: &[&str]) -> Reply {
    Reply::Answered(Answered::new(names.iter().copied()))
}

fn noted(names: &[&str], note: &str) -> Reply {
    Reply::Answered(Answered::new(names.iter().copied()).noting(note))
}

#[test]
fn a_registered_batch_waits_and_settling_it_moves_it_to_settled_once() {
    let deferred = Deferred::default();
    let before = deferred.revision();

    let id = deferred.register(asked("Language")).unwrap();
    let waiting = deferred.waiting();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting.first().map(Batch::id), Some(id));
    assert_eq!(heading(&waiting, 0), Some("Language"));
    assert!(!deferred.any_settled());
    assert_ne!(deferred.revision(), before);

    let registered = deferred.revision();
    deferred
        .settle(id, vec![chose(&["Rust"]), chose(&["lint", "test"])])
        .unwrap();
    assert!(deferred.waiting().is_empty());
    assert!(deferred.any_settled());
    assert_ne!(deferred.revision(), registered);

    let settled = deferred.revision();
    assert!(matches!(
        deferred.settle(id, vec![chose(&["Python"]), chose(&[])]),
        Err(Unsettled::Stale(stale)) if stale == id
    ));
    assert_eq!(deferred.revision(), settled, "a refusal changes nothing");
}

#[test]
fn ids_are_unique_within_a_registry_and_shared_by_its_clones() {
    let deferred = Deferred::default();
    let other = deferred.clone();

    let first = deferred.register(asked("One")).unwrap();
    let second = other.register(asked("Two")).unwrap();
    deferred.clear();
    let third = deferred.register(asked("Three")).unwrap();

    assert_ne!(first, second);
    assert_ne!(first, third);
    assert_ne!(second, third);
    assert_eq!(other.waiting().len(), 1, "a clone shares the batches");
}

#[test]
fn an_invalid_member_refuses_the_whole_submission_and_the_batch_still_waits() {
    let deferred = Deferred::default();
    let id = deferred.register(asked("Language")).unwrap();
    let revision = deferred.revision();

    let long = "x".repeat(MOST_REPLY_BYTES + 1);
    let refused: Vec<(Vec<Reply>, Expected)> = vec![
        (vec![chose(&["Rust"])], |e| {
            matches!(
                e,
                Unsettled::Count {
                    asked: 2,
                    replied: 1
                }
            )
        }),
        (
            vec![chose(&["Rust"]), chose(&["lint"]), Reply::Declined],
            |e| {
                matches!(
                    e,
                    Unsettled::Count {
                        asked: 2,
                        replied: 3
                    }
                )
            },
        ),
        (vec![chose(&["Rust", "Python"]), chose(&["lint"])], |e| {
            matches!(
                e,
                Unsettled::OneAnswer {
                    question: 1,
                    given: 2
                }
            )
        }),
        (vec![chose(&[]), chose(&["lint"])], |e| {
            matches!(
                e,
                Unsettled::OneAnswer {
                    question: 1,
                    given: 0
                }
            )
        }),
        (vec![chose(&["Rust"]), chose(&["lint", "lint"])], |e| {
            matches!(e, Unsettled::Repeated { question: 2 })
        }),
        (
            vec![chose(&["Rust"]), chose(&["my own", "and another"])],
            |e| matches!(e, Unsettled::WrittenTwice { question: 2 }),
        ),
        (
            vec![chose(&["Rust"]), noted(&["lint"], &long)],
            |e| matches!(e, Unsettled::TooLong { question: 2, part: Part::Note, bytes } if *bytes == MOST_REPLY_BYTES + 1),
        ),
        (vec![chose(&[&long]), chose(&["lint"])], |e| {
            matches!(
                e,
                Unsettled::TooLong {
                    question: 1,
                    part: Part::WrittenIn,
                    ..
                }
            )
        }),
    ];

    for (replies, expected) in refused {
        // The first member is valid wherever the second is not: one bad member
        // is enough to refuse every one.
        let error = deferred.settle(id, replies).unwrap_err();
        assert!(expected(&error), "{error:?}");
        assert!(!error.to_string().is_empty());
        assert_eq!(deferred.waiting().len(), 1, "{error}");
        assert!(!deferred.any_settled(), "{error}");
        assert_eq!(deferred.revision(), revision, "{error}");
    }

    // A written-in answer is a choice of its own, alone or beside offered ones.
    deferred
        .settle(id, vec![chose(&["Go"]), chose(&["lint", "clippy"])])
        .unwrap();
    assert!(deferred.any_settled());
}

#[test]
fn the_bound_on_a_reply_is_stated_where_it_refuses() {
    let deferred = Deferred::default();
    let id = deferred.register(asked("Language")).unwrap();

    let note = "x".repeat(MOST_REPLY_BYTES + 3);
    let error = deferred
        .settle(id, vec![chose(&["Rust"]), noted(&["lint"], &note)])
        .unwrap_err();

    let said = error.to_string();
    assert!(said.contains(&MOST_REPLY_BYTES.to_string()), "{said}");
    assert!(said.contains("3 bytes over"), "{said}");
    assert!(
        !said.contains("xxx"),
        "a refusal never repeats the reply: {said}"
    );
}

#[test]
fn a_ninth_batch_and_a_batch_over_the_byte_total_are_refused_before_storing() {
    let deferred = Deferred::default();
    for at in 0..MOST_HELD {
        deferred.register(asked(&format!("Batch {at}"))).unwrap();
    }
    let revision = deferred.revision();

    let error = deferred.register(asked("Ninth")).unwrap_err();
    assert!(
        matches!(error, Full::Crowded(Crowded::Batches { .. })),
        "{error:?}"
    );
    let said = error.to_string();
    assert!(
        said.contains(&format!("{} question batches", MOST_HELD + 1))
            && said.contains(&format!("at most {MOST_HELD}")),
        "{said}"
    );
    assert_eq!(deferred.waiting().len(), MOST_HELD);
    assert_eq!(deferred.revision(), revision);

    // A settled batch still holds its place until it is taken.
    let first = deferred.waiting().first().unwrap().id();
    deferred
        .settle(first, vec![chose(&["Rust"]), chose(&[])])
        .unwrap();
    assert!(matches!(
        deferred.register(asked("Ninth")),
        Err(Full::Crowded(Crowded::Batches { .. }))
    ));
    assert_eq!(deferred.take_settled().len(), 1);
    deferred.register(asked("Ninth")).unwrap();

    let deferred = Deferred::default();
    let half = MOST_HELD_BYTES / 2;
    let big = |text: &str| {
        vec![Question::new(
            "Big",
            text,
            [Answer::new("yes"), Answer::new("no")],
        )]
    };
    deferred.register(big(&"a".repeat(half))).unwrap();
    let revision = deferred.revision();

    let error = deferred.register(big(&"b".repeat(half))).unwrap_err();
    let Full::Crowded(Crowded::Bytes { bytes, held }) = error else {
        panic!("{error:?}");
    };
    assert!(bytes > half && held > half, "{bytes} {held}");
    let said = error.to_string();
    assert!(said.contains(&MOST_HELD_BYTES.to_string()), "{said}");
    assert!(
        said.contains(&format!("{} bytes over", bytes + held - MOST_HELD_BYTES)),
        "{said}"
    );
    assert_eq!(deferred.waiting().len(), 1);
    assert_eq!(deferred.revision(), revision);
}

#[test]
fn more_simultaneous_registrations_than_the_limit_admit_at_most_the_limit() {
    let deferred = Deferred::default();
    let start = std::sync::Barrier::new(MOST_HELD * 3);

    let admitted = std::thread::scope(|scope| {
        let racing: Vec<_> = (0..MOST_HELD * 3)
            .map(|at| {
                let deferred = deferred.clone();
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    deferred.register(asked(&format!("Racer {at}"))).ok()
                })
            })
            .collect();
        racing
            .into_iter()
            .filter_map(|racer| racer.join().unwrap())
            .collect::<Vec<_>>()
    });

    assert_eq!(admitted.len(), MOST_HELD);
    assert_eq!(deferred.waiting().len(), MOST_HELD);
    let mut unique = admitted.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), admitted.len());
}

#[test]
fn one_of_two_simultaneous_settlements_wins_and_the_other_is_stale() {
    for _ in 0..32 {
        let deferred = Deferred::default();
        let id = deferred.register(asked("Language")).unwrap();
        let start = std::sync::Barrier::new(2);

        let outcomes: Vec<_> = std::thread::scope(|scope| {
            let racing: Vec<_> = ["Rust", "Python"]
                .into_iter()
                .map(|answer| {
                    let deferred = deferred.clone();
                    let start = &start;
                    scope.spawn(move || {
                        start.wait();
                        deferred.settle(id, vec![chose(&[answer]), chose(&[])])
                    })
                })
                .collect();
            racing
                .into_iter()
                .map(|racer| racer.join().unwrap())
                .collect()
        });

        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        assert!(
            outcomes
                .iter()
                .any(|outcome| matches!(outcome, Err(Unsettled::Stale(stale)) if *stale == id))
        );
        assert_eq!(deferred.take_settled().len(), 1);
    }
}

#[test]
fn take_settled_hands_each_settled_batch_out_exactly_once() {
    let deferred = Deferred::default();
    let first = deferred.register(asked("First")).unwrap();
    let second = deferred.register(asked("Second")).unwrap();
    let third = deferred.register(asked("Third")).unwrap();

    deferred
        .settle(second, vec![chose(&["Rust"]), chose(&[])])
        .unwrap();
    deferred
        .settle(first, vec![chose(&["Python"]), chose(&["test"])])
        .unwrap();

    let revision = deferred.revision();
    let taken: Vec<QuestionId> = deferred.take_settled().iter().map(Delivered::id).collect();
    assert_eq!(taken, [second, first], "in the order they were settled");
    assert_ne!(deferred.revision(), revision);
    assert!(!deferred.any_settled());

    let revision = deferred.revision();
    assert!(deferred.take_settled().is_empty());
    assert_eq!(
        deferred.revision(),
        revision,
        "taking nothing changes nothing"
    );
    assert_eq!(deferred.waiting().len(), 1);
    assert_eq!(deferred.waiting().first().map(Batch::id), Some(third));
}

#[test]
fn what_an_answer_is_said_as_names_the_batch_each_heading_and_its_reply() {
    let deferred = Deferred::default();
    let id = deferred.register(asked("Language")).unwrap();
    deferred
        .settle(
            id,
            vec![
                noted(&["Python"], "the scripts are already Python"),
                chose(&[]),
            ],
        )
        .unwrap();

    let delivered = deferred.take_settled();
    let said = delivered.first().unwrap().said();
    assert!(said.contains(&id.to_string()), "{said}");
    assert!(said.contains("Language"), "{said}");
    assert!(said.contains("Python"), "{said}");
    assert!(said.contains("the scripts are already Python"), "{said}");
    assert!(said.contains("Checks"), "{said}");
    assert!(said.contains("nothing chosen"), "{said}");
}

#[test]
fn a_declined_member_reads_as_unresolved_and_names_no_offered_answer() {
    let deferred = Deferred::default();
    let id = deferred.register(asked("Language")).unwrap();
    deferred
        .settle(id, vec![Reply::Declined, chose(&["test"])])
        .unwrap();

    let said = only(deferred.take_settled()).said();
    let declined = said
        .lines()
        .find(|line| line.contains("Language"))
        .unwrap_or_else(|| panic!("{said}"));
    assert!(declined.contains("unresolved"), "{said}");
    for offered in ["Rust", "Python", "recommend"] {
        assert!(!said.contains(offered), "{said}");
    }
    assert!(said.contains("test"), "{said}");
}

#[test]
fn restoring_its_state_round_trips_the_waiting_batches_and_drops_nothing_silently() {
    let deferred = Deferred::default();
    let first = deferred.register(asked("First")).unwrap();
    let second = deferred.register(asked("Second")).unwrap();
    let settled = deferred.register(asked("Settled")).unwrap();
    deferred
        .settle(settled, vec![chose(&["Rust"]), chose(&[])])
        .unwrap();

    let state = deferred.state();
    let resumed = Deferred::default();
    resumed.restore(&state).unwrap();

    let waiting = resumed.waiting();
    assert_eq!(
        waiting.iter().map(Batch::id).collect::<Vec<_>>(),
        [first, second]
    );
    assert_eq!(resumed.state(), state);
    let questions = waiting.first().unwrap().questions();
    let question = questions.first().unwrap();
    assert_eq!(question.heading(), "First");
    assert_eq!(
        question.question(),
        "Which language should the examples use?"
    );
    assert!(!question.takes_several());
    let answers: Vec<_> = question.answers().collect();
    assert!(answers.first().unwrap().is_recommended());
    assert_eq!(answers.get(1).unwrap().says(), "the scripts are");
    let checks = questions.get(1).unwrap();
    assert!(checks.takes_several());
    let test = checks.answers().nth(1).unwrap();
    assert_eq!(test.shows().collect::<Vec<_>>(), ["cargo test"]);

    // An id restored is never handed out again.
    let next = resumed.register(asked("Next")).unwrap();
    assert!(![first, second, settled].contains(&next));

    // A state it cannot take whole is refused whole.
    let before = resumed.state();
    let mut broken = state.clone();
    *broken
        .pointer_mut("/waiting/1/questions/0/heading")
        .unwrap() = serde_json::json!(7);
    assert!(resumed.restore(&broken).is_err());
    assert!(
        resumed
            .restore(&serde_json::json!({"waiting": "no"}))
            .is_err()
    );
    let mut repeated = state.clone();
    let first_id = repeated.pointer("/waiting/0/id").unwrap().clone();
    *repeated.pointer_mut("/waiting/1/id").unwrap() = first_id;
    assert!(resumed.restore(&repeated).is_err());
    assert_eq!(resumed.state(), before);

    // And one past the bounds is refused rather than cut down to them.
    let crowded = Deferred::default();
    let many = serde_json::json!({
        "next": MOST_HELD + 1,
        "waiting": (0..=MOST_HELD as u64)
            .map(|at| {
                let mut batch = state.pointer("/waiting/0").unwrap().clone();
                *batch.pointer_mut("/id").unwrap() =
                    serde_json::json!(QuestionId::numbered(at).to_string());
                batch
            })
            .collect::<Vec<_>>()
    });
    let error = crowded.restore(&many).unwrap_err();
    assert!(
        matches!(error, Unrestored::Crowded(Crowded::Batches { .. })),
        "{error:?}"
    );
    // Nothing is held, so the refusal does not say there is.
    let said = error.to_string();
    assert!(
        said.contains(&format!("{} question batches", MOST_HELD + 1)) && !said.contains("already"),
        "{said}"
    );
    assert!(crowded.waiting().is_empty());

    // Crowded is the one refusal both a batch and a restored state meet.
    let mut heavy = state.pointer("/waiting/0").unwrap().clone();
    *heavy.pointer_mut("/questions/0/question").unwrap() =
        serde_json::json!("q".repeat(MOST_HELD_BYTES));
    let heavy = serde_json::json!({ "next": 1, "waiting": [heavy] });
    assert!(matches!(
        crowded.restore(&heavy),
        Err(Unrestored::Crowded(Crowded::Bytes { .. }))
    ));
    assert!(crowded.waiting().is_empty());
}

#[test]
fn control_characters_are_refused_and_a_reply_at_its_bound_is_kept_whole() {
    let deferred = Deferred::default();
    let id = deferred.register(asked("Language")).unwrap();

    for control in ["\u{1b}[2J", "a\nb", "tab\there", "\u{7}", "\u{85}"] {
        assert!(
            matches!(
                deferred.settle(id, vec![chose(&["Rust"]), noted(&[], control)]),
                Err(Unsettled::Control {
                    question: 2,
                    part: Part::Note
                })
            ),
            "{control:?}"
        );
        assert!(
            matches!(
                deferred.settle(id, vec![chose(&[control]), chose(&[])]),
                Err(Unsettled::Control {
                    question: 1,
                    part: Part::WrittenIn
                })
            ),
            "{control:?}"
        );
    }

    // Two-byte characters filling the bound exactly, and one byte past it.
    let exact = "é".repeat(MOST_REPLY_BYTES / 2);
    let over = format!("{exact}é");
    assert!(matches!(
        deferred.settle(id, vec![chose(&["Rust"]), noted(&[], &over)]),
        Err(Unsettled::TooLong { bytes, .. }) if bytes == MOST_REPLY_BYTES + 2
    ));
    deferred
        .settle(id, vec![chose(&[&exact]), noted(&[], &exact)])
        .unwrap();

    let said = only(deferred.take_settled()).said();
    assert_eq!(said.matches(exact.as_str()).count(), 2, "kept whole");
}

#[test]
fn nothing_held_or_delivered_is_ever_shown_in_debug() {
    let deferred = Deferred::default();
    let id = deferred
        .register(vec![Question::new(
            "heading-canary",
            "question-canary",
            [Answer::new("answer-canary"), Answer::new("other-canary")],
        )])
        .unwrap();
    deferred.register(asked("waiting-canary")).unwrap();
    let held = format!("{deferred:?}");

    deferred
        .settle(id, vec![noted(&["answer-canary"], "note-canary")])
        .unwrap();
    let settled = format!("{deferred:?}");
    let delivered = format!("{:?}", deferred.take_settled());

    for shown in [&held, &settled, &delivered] {
        for canary in [
            "heading-canary",
            "question-canary",
            "answer-canary",
            "other-canary",
            "waiting-canary",
            "note-canary",
        ] {
            assert!(!shown.contains(canary), "{shown}");
        }
    }
    assert!(held.contains("2 waiting"), "{held}");
    assert!(settled.contains("1 settled"), "{settled}");
}

#[test]
fn ids_never_repeat_even_when_the_numbers_run_out() {
    // The last number is never handed out, so a batch bearing it was never
    // saved.
    let saved = Deferred::default();
    saved.register(asked("Saved")).unwrap();
    let mut at_the_last = saved.state();
    *at_the_last.pointer_mut("/waiting/0/id").unwrap() =
        serde_json::json!(QuestionId::numbered(u64::MAX).to_string());
    let deferred = Deferred::default();
    assert!(matches!(
        deferred.restore(&at_the_last),
        Err(Unrestored::Malformed(_))
    ));

    deferred
        .restore(&serde_json::json!({ "next": u64::MAX - 1, "waiting": [] }))
        .unwrap();
    let id = deferred.register(asked("Last")).unwrap();
    assert_eq!(id, QuestionId::numbered(u64::MAX - 1));

    let revision = deferred.revision();
    let error = deferred.register(asked("Past the last")).unwrap_err();
    assert!(matches!(error, Full::Ids), "{error:?}");
    assert!(!error.to_string().is_empty());
    assert_eq!(deferred.waiting().len(), 1);
    assert_eq!(deferred.revision(), revision, "a refusal changes nothing");

    // What it saves once they have run out reads back as run out.
    let resumed = Deferred::default();
    resumed.restore(&deferred.state()).unwrap();
    assert!(matches!(
        resumed.register(asked("After a resume")),
        Err(Full::Ids)
    ));
}

#[test]
fn a_batch_out_of_shape_is_refused_whether_registered_or_restored() {
    type Shaped = fn(&Misshapen) -> bool;
    let offering = |count: usize| -> Vec<Answer> {
        (0..count).map(|at| Answer::new(format!("a{at}"))).collect()
    };
    let one = |answers: Vec<Answer>| Question::new("H", "Which?", answers);
    let asking = |count: usize| -> Vec<Question> {
        (0..count)
            .map(|at| {
                Question::new(
                    "H",
                    format!("Which {at}?"),
                    [Answer::new("a"), Answer::new("b")],
                )
            })
            .collect()
    };

    let refused: Vec<(Vec<Question>, Shaped)> = vec![
        (Vec::new(), |e| matches!(e, Misshapen::Empty)),
        (
            asking(MOST_QUESTIONS + 1),
            |e| matches!(e, Misshapen::Questions(given) if *given == MOST_QUESTIONS + 1),
        ),
        (
            vec![one(offering(FEWEST_ANSWERS - 1))],
            |e| matches!(e, Misshapen::Answers { question: 1, given } if *given == FEWEST_ANSWERS - 1),
        ),
        (
            vec![one(offering(2)), one(offering(MOST_ANSWERS + 1))],
            |e| matches!(e, Misshapen::Answers { question: 2, given } if *given == MOST_ANSWERS + 1),
        ),
        (
            vec![one(vec![
                Answer::new("a"),
                Answer::new("b").showing(vec![""; MOST_SHOWS + 1]),
            ])],
            |e| matches!(e, Misshapen::Shows { question: 1, given } if *given == MOST_SHOWS + 1),
        ),
        (vec![one(vec![Answer::new(""), Answer::new("")])], |e| {
            matches!(e, Misshapen::RepeatedAnswer { question: 1 })
        }),
        (vec![one(offering(2)), one(offering(3))], |e| {
            matches!(e, Misshapen::RepeatedQuestion { question: 2 })
        }),
    ];

    let deferred = Deferred::default();
    for (questions, expected) in refused {
        let revision = deferred.revision();
        let error = deferred.register(questions).unwrap_err();
        let Full::Shape(shape) = &error else {
            panic!("{error:?}");
        };
        assert!(expected(shape), "{error:?}");
        assert!(!error.to_string().is_empty());
        assert!(deferred.waiting().is_empty(), "{error}");
        assert_eq!(deferred.revision(), revision, "{error}");
    }
    // At the bounds, not past them.
    deferred.register(asking(MOST_QUESTIONS)).unwrap();
    deferred
        .register(vec![one(vec![
            Answer::new("a"),
            Answer::new("b").showing(vec![""; MOST_SHOWS]),
        ])])
        .unwrap();
    deferred
        .register(vec![one(offering(MOST_ANSWERS))])
        .unwrap();

    // What a saved state could never hold is refused on the way back in too.
    let saved = Deferred::default();
    saved.register(asked("Saved")).unwrap();
    let state = saved.state();
    let resumed = Deferred::default();

    let mut empty = state.clone();
    *empty.pointer_mut("/waiting/0/questions").unwrap() = serde_json::json!([]);
    assert!(matches!(
        resumed.restore(&empty),
        Err(Unrestored::Shape(Misshapen::Empty))
    ));
    let mut lone = state.clone();
    lone.pointer_mut("/waiting/0/questions/0/answers")
        .and_then(serde_json::Value::as_array_mut)
        .unwrap()
        .truncate(1);
    assert!(matches!(
        resumed.restore(&lone),
        Err(Unrestored::Shape(Misshapen::Answers {
            question: 1,
            given: 1
        }))
    ));
    // A mark below the first is not one `state` writes, so it is not quietly
    // moved or dropped.
    let mut marked = state.clone();
    *marked
        .pointer_mut("/waiting/0/questions/0/answers/1/recommended")
        .unwrap() = serde_json::json!(true);
    assert!(matches!(
        resumed.restore(&marked),
        Err(Unrestored::Malformed(_))
    ));
    assert!(resumed.waiting().is_empty());
}

#[test]
fn line_and_paragraph_separators_are_refused_as_line_breaks() {
    let deferred = Deferred::default();
    let id = deferred.register(asked("Language")).unwrap();

    for separator in ["a\u{2028}- Checks: lint", "a\u{2029}b"] {
        assert!(
            matches!(
                deferred.settle(id, vec![chose(&["Rust"]), noted(&[], separator)]),
                Err(Unsettled::Control {
                    question: 2,
                    part: Part::Note
                })
            ),
            "{separator:?}"
        );
        assert!(
            matches!(
                deferred.settle(id, vec![chose(&[separator]), chose(&[])]),
                Err(Unsettled::Control {
                    question: 1,
                    part: Part::WrittenIn
                })
            ),
            "{separator:?}"
        );
    }
    assert_eq!(deferred.waiting().len(), 1);
}

#[test]
fn a_written_in_answer_is_said_apart_from_the_answers_offered() {
    let deferred = Deferred::default();
    let id = deferred.register(asked("Language")).unwrap();
    deferred
        .settle(
            id,
            vec![chose(&["Rust, Python"]), chose(&["lint", "clippy"])],
        )
        .unwrap();

    let said = only(deferred.take_settled()).said();
    let lines: Vec<&str> = said.lines().collect();
    let at = |prefix: &str| {
        lines
            .iter()
            .position(|line| line.starts_with(prefix))
            .unwrap_or_else(|| panic!("{said}"))
    };

    let language = at("- Language:");
    assert_eq!(
        lines.get(language),
        Some(&"- Language: none of the answers offered"),
        "{said}"
    );
    assert_eq!(
        lines.get(language + 1),
        Some(&"  their own answer: Rust, Python"),
        "{said}"
    );
    let checks = at("- Checks:");
    assert_eq!(lines.get(checks), Some(&"- Checks: lint"), "{said}");
    assert_eq!(
        lines.get(checks + 1),
        Some(&"  their own answer: clippy"),
        "{said}"
    );
}
