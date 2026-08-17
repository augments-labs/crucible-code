//! One receiver for the two things the drawing thread has to answer.
//!
//! A turn runs on its own thread and reports through [`Post`]; it also stops
//! mid-flight to ask a question through [`Ask`]. The thread that draws is
//! parked in `recv`, and a channel has no `select`, so both have to arrive on
//! the same one. That is all [`Seen`] is: the union of what can turn up.
//!
//! The alternative — a second thread forwarding events into the first — buys
//! nothing and adds a hop to every delta.
//!
//! An answer carries no name for the question it answers, and does not need
//! one. [`Asking`] reads from a channel of its own, and [`Ask::ask`] takes
//! `&mut self` and blocks until the answer arrives — so the value that asked is
//! the value that reads, and it cannot have a second question outstanding while
//! it waits. What an identifier would defend against is therefore only a
//! question answered twice, and the thread that draws answers each exactly
//! once: on the path that drew it, or on the path that has stopped drawing.
//!
//! Two turns cannot overlap either, but that is not what this rests on. A
//! second asker would be a second [`Asking`] with a channel of its own, and
//! what it would want is not a name for its question: it would want the drawing
//! thread to learn which channel to answer on, which is the reply end
//! travelling with the question rather than being held for the turn. One asker
//! is why it is held for the turn, and an identifier carried today would name a
//! question nothing is competing with.

use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::time::Duration;

use crucible_core::{Ask, Event, Post, Remember, Sensitivity, ToolCall, Verdict};

/// Events allowed to wait for the terminal.
///
/// Built-in providers reject a wire event above one MiB. Two slots therefore
/// retain at most two such deltas while a slow terminal applies backpressure to
/// the pull-based stream.
pub(crate) const CAPACITY: usize = 2;

/// Most adjacent delta text combined into one renderer call.
///
/// Without its own ceiling, draining a full slot as quickly as the producer
/// refills it could turn a count-bounded channel into one growing `String`.
const BATCH_BYTES: usize = 1024 * 1024;

/// What comes back when a question is answered: what was decided, and how long
/// it holds. Named because it travels down a channel, and a bare tuple in a
/// channel type says nothing about which half is which.
pub(crate) type Answer = (Verdict, Remember);

/// Something the drawing thread has to deal with.
#[derive(Debug)]
pub(crate) enum Seen {
    /// Something happened in the turn. Draw it.
    Turn(Event),

    /// A tool is waiting on a verdict. Ask, then answer.
    Question {
        /// What the model asked for.
        call: ToolCall,
        /// How much damage it could do.
        sensitivity: Sensitivity,
    },
}

/// A worker's events, on their way to the thread that draws.
#[derive(Debug)]
pub(crate) struct Relay(SyncSender<Seen>);

impl Relay {
    /// Takes the sending end. One per turn, dropped when the turn ends — which
    /// is how the drawing thread learns the turn is over.
    pub(crate) fn new(to: SyncSender<Seen>) -> Self {
        Self(to)
    }
}

impl Post for Relay {
    fn post(&self, event: Event) {
        drop(self.0.send(Seen::Turn(event)));
    }
}

/// Puts a question to the drawing thread and blocks on the answer.
#[derive(Debug)]
pub(crate) struct Asking {
    to: SyncSender<Seen>,
    answers: Receiver<Answer>,
}

impl Asking {
    /// Takes the two ends it needs: where questions go, where answers arrive.
    pub(crate) fn new(to: SyncSender<Seen>, answers: Receiver<Answer>) -> Self {
        Self { to, answers }
    }
}

impl Ask for Asking {
    /// Blocks the turn until someone answers.
    ///
    /// Silence is a refusal. A channel that will not carry the question, or
    /// that closes before an answer comes back, means nobody is left to consent
    /// — and running a tool nobody agreed to is the one outcome worth avoiding
    /// more than stopping.
    fn ask(&mut self, call: &ToolCall, sensitivity: &Sensitivity) -> Answer {
        let question = Seen::Question {
            call: call.clone(),
            sensitivity: sensitivity.clone(),
        };

        if self.to.send(question).is_err() {
            return refused();
        }

        self.answers.recv().unwrap_or_else(|_| refused())
    }
}

/// What silence means. A duration is still needed alongside it, and the only
/// honest one is that this answer covers nothing beyond the call it refused.
fn refused() -> Answer {
    (Verdict::Deny, Remember::Never)
}

/// The bounded event receiver, merging only adjacent deltas already waiting.
///
/// The channel owns the memory bound; this owns the recovery path after a slow
/// frame. One renderer call can consume a run of deltas, but never crosses a
/// tool or turn event, so the terminal still observes the runner's order.
#[derive(Debug)]
pub(crate) struct Inbox {
    from: Receiver<Seen>,
    held: Option<Seen>,
}

impl Inbox {
    pub(crate) fn new(from: Receiver<Seen>) -> Self {
        Self { from, held: None }
    }

    pub(crate) fn recv_timeout(&mut self, wait: Duration) -> Result<Seen, RecvTimeoutError> {
        let first = if let Some(held) = self.held.take() {
            held
        } else {
            self.from.recv_timeout(wait)?
        };

        let Seen::Turn(Event::Delta { text }) = first else {
            return Ok(first);
        };
        let mut text = String::from(text);

        while text.len() < BATCH_BYTES {
            match self.from.try_recv() {
                Ok(Seen::Turn(Event::Delta { text: next }))
                    if text.len().saturating_add(next.len()) <= BATCH_BYTES =>
                {
                    text.push_str(&next);
                }
                Ok(next @ Seen::Turn(Event::Delta { .. })) => {
                    self.held = Some(next);
                    break;
                }
                Ok(other) => {
                    self.held = Some(other);
                    break;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }

        Ok(Seen::Turn(Event::Delta { text: text.into() }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, sync_channel};
    use std::time::Duration;

    use crucible_core::{Command, ToolArgs, ToolId, TurnId};

    use super::*;

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

    #[test]
    fn an_event_arrives_as_something_to_draw() {
        let (to, seen) = sync_channel(2);
        let relay = Relay::new(to);

        relay.post(Event::Delta {
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
        let (reply, answers) = channel();
        let mut asking = Asking::new(to, answers);

        let asked = std::thread::spawn(move || asking.ask(&call(), &running()));

        assert!(matches!(seen.recv().unwrap(), Seen::Question { .. }));
        reply.send((Verdict::Allow, Remember::Session)).unwrap();

        assert_eq!(asked.join().unwrap(), (Verdict::Allow, Remember::Session));
    }

    #[test]
    fn nobody_left_to_ask_is_a_refusal() {
        // Not a deadlock and not an allow: the process is leaving, and a tool
        // that ran on the way out ran without consent.
        let (to, seen) = sync_channel(2);
        let (reply, answers) = channel::<Answer>();
        let mut asking = Asking::new(to, answers);
        drop(reply);

        let answer = asking.ask(&call(), &running());

        assert_eq!(answer, (Verdict::Deny, Remember::Never));
        drop(seen);
    }

    #[test]
    fn a_question_that_cannot_be_delivered_is_a_refusal() {
        let (to, seen) = sync_channel(2);
        let (_reply, answers) = channel::<Answer>();
        let mut asking = Asking::new(to, answers);
        drop(seen);

        assert_eq!(
            asking.ask(&call(), &running()),
            (Verdict::Deny, Remember::Never)
        );
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
    fn a_slow_renderer_bounds_and_coalesces_a_delta_flood() {
        const POSTED: usize = 10_000;

        let (to, from) = sync_channel(2);
        let finished = Arc::new(AtomicBool::new(false));
        let done = finished.clone();
        let flooding = std::thread::spawn(move || {
            let relay = Relay::new(to);
            for _ in 0..POSTED {
                relay.post(Event::Delta { text: "x".into() });
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
            let relay = Relay::new(to);
            for _ in 0..POSTED {
                relay.post(Event::Delta {
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
}
