use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use crucible_app::client::Deciding;
use crucible_core::{Ancestry, Answer as Offered, Ask, Command, ToolArgs, ToolId, TurnId, Wrote};
use crucible_runner::Reporter;

use super::*;
use crate::cli::client::tests::Noted;
use crate::cli::fake::Awaited;

/// What the permission engine hears when it asks through `asking`, the way
/// a turn does.
fn asked(asking: &mut Asking) -> Answer {
    asked_about(asking, &call(), &running())
}

/// The same, about a call and sensitivity of the caller's choosing — so a
/// test can tell two questions on the same [`Asking`] apart.
fn asked_about(asking: &mut Asking, call: &ToolCall, sensitivity: &Sensitivity) -> Answer {
    Deciding::new(asking, Capabilities::every())
        .ask(call, sensitivity)
        .awaited()
}

fn call() -> ToolCall {
    ToolCall {
        id: ToolId::new("a"),
        name: "bash".into(),
        args: ToolArgs::new(r#"{"command":"ls"}"#),
    }
}

fn running() -> Sensitivity {
    Sensitivity::SpawnsProcess {
        command: Command::Understood {
            sent: "ls".into(),
            parts: Box::from([Box::from("ls")]),
        },
    }
}

/// A second, distinct call from [`call`]/[`running`], so a test can tell
/// which of two questions an answer was meant for.
fn other_call() -> ToolCall {
    ToolCall {
        id: ToolId::new("b"),
        name: "bash".into(),
        args: ToolArgs::new(r#"{"command":"rm -rf build"}"#),
    }
}

/// One question, freshly built each time it is asked for — so a test that
/// needs it more than once never fights the borrow checker over a value one
/// call already consumed.
fn one_question() -> [Question; 1] {
    [Question::new(
        "Words",
        "What should it say?",
        [Offered::new("these"), Offered::new("those")],
    )]
}

#[test]
fn a_stale_reply_after_a_dropped_ask_never_settles_the_next_question() {
    // Each question owns its own one-shot reply channel, made when the
    // question is and travelling with it, so a reply meant for one question
    // can never be read as the answer to another: there is no channel shared
    // between them for a stale reply to arrive on.
    let (to, seen) = sync_channel(4);
    let mut asking = Asking::new(to, Client::new());

    // The first question is asked and dropped before anybody answers it.
    let first = call();
    let sensitivity = running();
    {
        let mut deciding = Deciding::new(&mut asking, Capabilities::every());
        let mut future = std::pin::pin!(deciding.ask(&first, &sensitivity));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
    } // the future, and the one-shot receiver inside it, drop here.

    let Seen::Question { reply: stale, .. } = seen.recv().unwrap() else {
        panic!("the first question was not put");
    };

    // The human's Allow for that dropped question arrives late: there is
    // nobody left on the other end of its own channel to receive it.
    assert!(
        stale.send((Verdict::Allow, Remember::Session)).is_err(),
        "a reply meant for a dropped question still found a receiver"
    );

    // A second, different call is asked about on the same `Asking`, and
    // answered on the fresh channel that question made for itself.
    let waiting = std::thread::spawn(move || asked_about(&mut asking, &other_call(), &running()));
    let Seen::Question { reply: fresh, .. } = seen.recv().unwrap() else {
        panic!("the second question was not put");
    };
    fresh.send((Verdict::Deny, Remember::Never)).unwrap();

    assert_eq!(
        waiting.join().unwrap(),
        (Verdict::Deny, Remember::Never),
        "the stale Allow meant for the dropped question settled a different one"
    );
}

#[test]
fn an_event_arrives_as_something_to_draw() {
    let (to, seen) = sync_channel(2);
    let relay = Relay::new(to, Putting::new());

    Reporter::new(Ancestry::new(), &relay).post(Event::Delta {
        text: "hello".into(),
    });

    assert!(matches!(
        seen.recv().unwrap(),
        Seen::Turn(Event::Delta { text }) if &*text == "hello"
    ));
}

#[test]
fn a_question_waits_for_the_answer_it_is_given() {
    let (to, seen) = sync_channel(2);
    let mut asking = Asking::new(to, Client::new());

    let waiting = std::thread::spawn(move || asked(&mut asking));

    let Seen::Question { reply, .. } = seen.recv().unwrap() else {
        panic!("a question was not put");
    };
    reply.send((Verdict::Allow, Remember::Session)).unwrap();

    assert_eq!(waiting.join().unwrap(), (Verdict::Allow, Remember::Session));
}

#[test]
fn nobody_left_to_ask_is_a_refusal() {
    // Not a deadlock and not an allow: the process is leaving, and a tool
    // that ran on the way out ran without consent.
    let (to, seen) = sync_channel(2);
    let (client, journal) = Client::noting();
    let mut asking = Asking::new(to, client);

    let waiting = std::thread::spawn(move || asked(&mut asking));

    let Seen::Question { reply, .. } = seen.recv().unwrap() else {
        panic!("a question was not put");
    };
    drop(reply); // the question was put; nobody answers it.

    assert_eq!(waiting.join().unwrap(), (Verdict::Deny, Remember::Never));

    // Accounted for rather than timed: the question was put once, nothing
    // was decided about it, and the no is the application's own.
    assert!(
        matches!(journal.noted().as_slice(), [Noted::Put(_)]),
        "{:?}",
        journal.noted()
    );
}

#[test]
fn a_question_that_cannot_be_delivered_is_a_refusal() {
    let (to, seen) = sync_channel(2);
    let (client, journal) = Client::noting();
    let mut asking = Asking::new(to, client);
    drop(seen);

    assert_eq!(asked(&mut asking), (Verdict::Deny, Remember::Never));
    assert!(
        matches!(journal.noted().as_slice(), [Noted::Put(_)]),
        "{:?}",
        journal.noted()
    );
}

#[test]
fn an_answer_as_long_as_the_panel_lets_one_be_reaches_the_tool_that_asked_whole() {
    // Longer than any line drawn for a person is cut to, and far shorter
    // than the panel's own editor stops at: words somebody can paste today.
    let pasted = "\u{e9}".repeat(40 * 1024);
    let beside = "n".repeat(20 * 1024);
    let given = vec![Answered::new([pasted.clone()]).noting(beside.clone())];

    let (to, seen) = sync_channel(CAPACITY);
    let putting = Putting::new();
    putting.open(to);

    let drawing = std::thread::spawn(move || {
        let put = seen.recv();
        let Ok(Seen::Asked { reply, .. }) = put else {
            panic!("{put:?}");
        };
        reply.send(Some(given)).expect("the tool is waiting");
    });
    let heard = putting
        .put(&one_question())
        .awaited()
        .expect("somebody answered");
    drawing.join().expect("the drawing side ran");

    let answer = heard.first().expect("one answer for one question");
    let chosen: Vec<&str> = answer.chosen().collect();
    assert_eq!(chosen.len(), 1);
    assert_eq!(chosen.first().map(|words| words.len()), Some(pasted.len()));
    assert!(chosen.first() == Some(&pasted.as_str()), "the words differ");
    assert_eq!(answer.note().len(), beside.len());
    assert!(answer.note() == beside, "the note differs");
}

#[test]
fn adjacent_deltas_merge_without_crossing_a_turn_event() {
    let (to, from) = sync_channel(8);
    to.send(Seen::Turn(Event::Delta { text: "one".into() }))
        .unwrap();
    to.send(Seen::Turn(Event::Delta {
        text: " two".into(),
    }))
    .unwrap();
    to.send(Seen::Turn(Event::TurnStarted {
        turn: TurnId::FIRST,
    }))
    .unwrap();
    to.send(Seen::Turn(Event::Delta {
        text: "three".into(),
    }))
    .unwrap();

    let mut inbox = Inbox::new(from);
    assert!(matches!(
        inbox.recv_timeout(Duration::ZERO).unwrap(),
        Seen::Turn(Event::Delta { text }) if &*text == "one two"
    ));
    assert!(matches!(
        inbox.recv_timeout(Duration::ZERO).unwrap(),
        Seen::Turn(Event::TurnStarted { .. })
    ));
    assert!(matches!(
        inbox.recv_timeout(Duration::ZERO).unwrap(),
        Seen::Turn(Event::Delta { text }) if &*text == "three"
    ));
}

#[test]
fn what_one_call_wrote_is_drawn_together_and_never_joined_to_another_call() {
    // The reason the event carries a call at all. Calls run one at a time
    // today, so the second half of this is a rule about a future rather than
    // a bug being fixed — but it is the rule the id exists to make statable,
    // and it costs one comparison.
    let (to, from) = sync_channel(8);
    let piece = |call: &str, text: &str| {
        Seen::Turn(Event::Wrote {
            call: ToolId::new(call),
            text: Wrote::new(text),
        })
    };

    to.send(piece("a", "Compiling one\n")).unwrap();
    to.send(piece("a", "Compiling two\n")).unwrap();
    to.send(piece("b", "elsewhere\n")).unwrap();

    let mut inbox = Inbox::new(from);

    let Seen::Turn(Event::Wrote { call, text }) = inbox.recv_timeout(Duration::ZERO).unwrap()
    else {
        panic!("what arrived was not what one call wrote");
    };
    assert_eq!(call, ToolId::new("a"));
    assert_eq!(text.as_str(), "Compiling one\nCompiling two\n");

    let Seen::Turn(Event::Wrote { call, text }) = inbox.recv_timeout(Duration::ZERO).unwrap()
    else {
        panic!("the second call's output was swallowed by the first");
    };
    assert_eq!(call, ToolId::new("b"));
    assert_eq!(text.as_str(), "elsewhere\n");
}

#[test]
fn output_is_never_drawn_across_the_event_that_ended_the_call() {
    let (to, from) = sync_channel(8);
    to.send(Seen::Turn(Event::Wrote {
        call: ToolId::new("a"),
        text: Wrote::new("last line\n"),
    }))
    .unwrap();
    to.send(Seen::Turn(Event::TurnFinished {
        turn: TurnId::FIRST,
        stop: crucible_core::StopReason::Yielded,
    }))
    .unwrap();

    let mut inbox = Inbox::new(from);
    assert!(matches!(
        inbox.recv_timeout(Duration::ZERO).unwrap(),
        Seen::Turn(Event::Wrote { text, .. }) if text.as_str() == "last line\n"
    ));
    assert!(matches!(
        inbox.recv_timeout(Duration::ZERO).unwrap(),
        Seen::Turn(Event::TurnFinished { .. })
    ));
}

#[test]
fn a_slow_renderer_bounds_and_coalesces_a_delta_flood() {
    const POSTED: usize = 10_000;

    let (to, from) = sync_channel(2);
    let finished = Arc::new(AtomicBool::new(false));
    let done = finished.clone();
    let flooding = std::thread::spawn(move || {
        let relay = Relay::new(to, Putting::new());
        for _ in 0..POSTED {
            Reporter::new(Ancestry::new(), &relay).post(Event::Delta { text: "x".into() });
        }
        done.store(true, Ordering::Release);
    });

    // With nobody rendering, the third delta must meet backpressure rather
    // than making the queue grow with the provider's output.
    std::thread::sleep(Duration::from_millis(20));
    assert!(!finished.load(Ordering::Acquire));

    let mut inbox = Inbox::new(from);
    let mut deltas = 0;
    let mut bytes = 0;
    while !finished.load(Ordering::Acquire) || bytes < POSTED {
        let seen = inbox.recv_timeout(Duration::from_secs(1)).unwrap();
        if let Seen::Turn(Event::Delta { text }) = seen {
            deltas += 1;
            bytes += text.len();
        }
        std::thread::sleep(Duration::from_micros(10));
    }

    flooding.join().unwrap();
    assert_eq!(bytes, POSTED);
    assert!(deltas < POSTED, "the flood was not coalesced: {deltas}");
}

#[test]
fn maximum_wire_sized_deltas_neither_overfill_nor_make_an_unbounded_batch() {
    const POSTED: usize = CAPACITY + 2;

    let (to, from) = sync_channel(CAPACITY);
    let finished = Arc::new(AtomicBool::new(false));
    let done = finished.clone();
    let flooding = std::thread::spawn(move || {
        let relay = Relay::new(to, Putting::new());
        for _ in 0..POSTED {
            Reporter::new(Ancestry::new(), &relay).post(Event::Delta {
                text: "x".repeat(BATCH_BYTES).into(),
            });
        }
        done.store(true, Ordering::Release);
    });

    std::thread::sleep(Duration::from_millis(20));
    assert!(!finished.load(Ordering::Acquire));

    let mut inbox = Inbox::new(from);
    let mut received = 0;
    for _ in 0..POSTED {
        let Seen::Turn(Event::Delta { text }) = inbox
            .recv_timeout(Duration::from_secs(1))
            .expect("a bounded delta")
        else {
            panic!("a non-delta crossed the test bridge");
        };
        assert_eq!(text.len(), BATCH_BYTES);
        received += 1;
    }

    flooding.join().unwrap();
    assert_eq!(received, POSTED);
}

/// Proves that a dropped [`Front::put`] on [`Asking`] leaves no waiter
/// behind: nothing is holding the answer's receiver once the future that
/// awaited it is gone, so a late reply finds nobody rather than blocking or
/// queuing for whatever asks next.
///
/// The one-shot's own drop is the whole of the cleanup here: it is a local
/// of the future's own async block, so dropping the future — mid-wait,
/// before an answer arrived — drops the receiving half with it.
#[test]
fn a_dropped_permission_ask_leaves_no_waiter_and_a_late_answer_finds_nobody() {
    let (to, seen) = sync_channel(2);
    let mut asking = Asking::new(to, Client::new());
    let call = call();
    let running = running();

    {
        let mut deciding = Deciding::new(&mut asking, Capabilities::every());
        let mut future = std::pin::pin!(deciding.ask(&call, &running));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
    } // the future, and the one-shot receiver inside it, drop here.

    let Seen::Question { reply, .. } = seen.recv().unwrap() else {
        panic!("the question was not put");
    };
    assert!(reply.send((Verdict::Allow, Remember::Never)).is_err());
}

/// The same proof for [`Putting::put`]: dropping the future mid-wait leaves
/// nobody on the other end of the one-shot it made for that ask, and the
/// same, long-lived `Putting` still answers a second, unrelated ask normally
/// right after — its channel belongs to the question, not to the turn, so
/// proving a dropped ask leaves no waiter never requires dropping `Putting`
/// itself.
#[test]
fn a_dropped_put_leaves_no_waiter_and_the_next_put_is_unaffected() {
    let (to, seen) = sync_channel(CAPACITY);
    let putting = Putting::new();
    putting.open(to);

    let asked = one_question();
    {
        let mut future = std::pin::pin!(putting.put(&asked));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
    } // the future, and the one-shot receiver inside it, drop here.

    let Seen::Asked { reply: stale, .. } = seen.recv().unwrap() else {
        panic!("the ask was not put");
    };
    assert!(
        stale.send(Some(vec![Answered::new(["these"])])).is_err(),
        "a reply meant for a dropped ask still found a receiver"
    );

    let again = putting.clone();
    let waiting = std::thread::spawn(move || again.put(&one_question()).awaited());
    let Seen::Asked { reply: fresh, .. } = seen.recv().unwrap() else {
        panic!("the second ask was not put");
    };
    fresh.send(None).expect("the tool is waiting");

    assert!(
        waiting.join().unwrap().is_none(),
        "the stale answer meant for the dropped ask settled the next one"
    );
}
