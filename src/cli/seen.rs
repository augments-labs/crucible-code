//! One receiver for the two things the drawing thread has to answer.
//!
//! A turn runs on its own thread and reports through [`Post`]; it also stops
//! mid-flight to ask a question through a [`Front`]. The thread that draws is
//! parked in `recv`, and a channel has no `select`, so both have to arrive on
//! the same one. That is all [`Seen`] is: the union of what can turn up.
//!
//! The alternative — a second thread forwarding events into the first — buys
//! nothing and adds a hop to every delta.
//!
//! A question is put under the identity the application minted for it, and an
//! answer goes back as a [`Decision`] naming that identity: this thread is one
//! front end of [`crucible_app::client`], and is held to what any other is. The
//! name is stamped where the answer is heard rather than carried through the
//! thread that draws, and that is sound for a reason particular to this
//! thread. [`Asking`] reads from a channel of its own, and is put a question
//! through `&mut self` and blocks until the answer arrives — so the value that
//! asked is the value that reads, and it cannot have a second question
//! outstanding while it waits. The thread that draws answers each exactly once:
//! on the path that drew it, or on the path that has stopped drawing.
//!
//! Two turns cannot overlap either, but that is not what this rests on. A
//! second asker would be a second [`Asking`] with a channel of its own, and
//! what it would want is the drawing thread learning which channel to answer
//! on, which is the reply end travelling with the question rather than being
//! held for the turn. One asker is why it is held for the turn.

use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crucible_app::Conversation;
use crucible_app::client::{self, Ended, Front, Shown};
use crucible_client_api::bounds::SAID_BYTES;
use crucible_client_api::{
    Capabilities, Command, Decision, Lasting, Pending, Picked, Refusal, Ruling, Said,
};
use crucible_core::{
    Answered, Attachment, Put, Question, Remember, Sensitivity, ToolCall, Verdict, Wrote,
};
use crucible_runner::{Event, EventEnvelope, Post, RunContext};

use super::client::Client;

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

/// What comes back when an ask is answered: one answer per question, or nobody
/// answered at all.
pub(crate) type Given = Option<Vec<Answered>>;

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

    /// A tool is waiting on a person. Put the questions, then answer.
    Asked {
        /// What to put, in the order it should be answered.
        questions: Vec<Question>,
    },
}

/// A worker's events, on their way to the thread that draws.
#[derive(Debug)]
pub(crate) struct Relay {
    to: SyncSender<Seen>,
    putting: Putting,
}

impl Relay {
    /// Takes the sending end. One per turn, dropped when the turn ends — which
    /// is how the drawing thread learns the turn is over.
    ///
    /// It carries the ask handle for exactly that reason: this value's death is
    /// already the signal, so it is the right thing to give the ends back.
    pub(crate) fn new(to: SyncSender<Seen>, putting: Putting) -> Self {
        Self { to, putting }
    }
}

impl Drop for Relay {
    /// Gives back the ends the turn lent, before its own sender goes.
    ///
    /// A sender parked in the ask handle would outlive the turn, and the loop
    /// that draws learns a turn is over by watching that channel close — so one
    /// left behind is a loop that waits for ever. Done here rather than at the
    /// end of the worker's body, because a worker that panicked would skip that
    /// and hang the session; a drop runs either way.
    fn drop(&mut self) {
        self.putting.close();
    }
}

impl Post for Relay {
    /// Where the attribution stops.
    ///
    /// Nothing on screen is drawn per run, so the loop that draws has nothing
    /// to tell apart: every `match` in it asks what happened rather than whose
    /// it was. Dropped here, in one named place, so that the day the screen has
    /// to tell two runs apart it is this line that has to change rather than
    /// every pattern downstream of it.
    fn post(&self, reported: EventEnvelope) {
        drop(self.to.send(Seen::Turn(reported.into_event())));
    }
}

/// Puts a question to the drawing thread and blocks on the answer.
#[derive(Debug)]
pub(crate) struct Asking {
    to: SyncSender<Seen>,
    answers: Receiver<Answer>,
    client: Client,
}

impl Asking {
    /// Takes the two ends it needs — where questions go, where answers arrive
    /// — and what a request is made with: the client that numbers requests.
    /// What a pending action is named from is the application's and is not
    /// handed in.
    pub(crate) const fn new(
        to: SyncSender<Seen>,
        answers: Receiver<Answer>,
        client: Client,
    ) -> Self {
        Self {
            to,
            answers,
            client,
        }
    }

    /// Asks the application for the turn `command` names, answering from the
    /// drawing thread whatever it stops on.
    pub(crate) fn turn(
        &mut self,
        conversation: &mut Conversation,
        command: Command,
        attached: Box<[Attachment]>,
        run: &RunContext<'_>,
    ) -> Ended {
        let request = self.client.asking(command);
        let ended = client::turn(conversation, &request, attached, self, run);

        #[cfg(test)]
        self.client
            .answered(&request, conversation, ended.outcome());

        ended
    }
}

impl Front for Asking {
    /// Blocks the turn until someone answers.
    ///
    /// Silence is a refusal, and the application is what makes it one: a
    /// channel that will not carry the question, or that closes before an
    /// answer comes back, means nobody is left to consent — and running a tool
    /// nobody agreed to is the one outcome worth avoiding more than stopping.
    fn put(&mut self, pending: &Pending, shown: Shown<'_>) -> Option<Decision> {
        // A model's questions come through the tool that asks them, which is
        // lent its own ends; a turn stops here on a call and nothing else.
        let Shown::Call { call, sensitivity } = shown else {
            return None;
        };

        #[cfg(test)]
        self.client.put(pending);

        let question = Seen::Question {
            call: call.clone(),
            sensitivity: sensitivity.clone(),
        };
        self.to.send(question).ok()?;
        let (verdict, remember) = self.answers.recv().ok()?;

        let decision = Decision::Ruled {
            id: pending.id(),
            ruling: match verdict {
                Verdict::Allow => Ruling::Allow,
                Verdict::Deny => Ruling::Deny,
            },
            lasting: match remember {
                Remember::Never => Lasting::Once,
                // The prompt offers nothing that outlasts the process, and the
                // engine keeps the two alike: for the rest of this session.
                Remember::Session | Remember::Always => Lasting::Session,
            },
        };

        #[cfg(test)]
        self.client.decided(&decision);

        Some(decision)
    }

    /// Nothing to say: every decision made above names the action it was put
    /// and rules on it, so the application has none of them to turn away.
    fn refused(&mut self, _: Refusal) {}
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

        // Two kinds of text arrive in runs and are drawn in one call each: the
        // model's prose, and what a running command has printed. Nothing else
        // merges, and neither of these merges into the other — a delta and a
        // command's output are different things on screen and the renderer draws
        // them differently.
        match first {
            Seen::Turn(Event::Delta { text }) => {
                let joined = self.gather(&text, |seen| match seen {
                    Seen::Turn(Event::Delta { text }) => Some(text),
                    _ => None,
                });

                Ok(Seen::Turn(Event::Delta {
                    text: joined.into(),
                }))
            }

            // Only with what the same call wrote. Calls run one at a time today,
            // so nothing yet produces two runs to keep apart; the comparison is
            // what makes "one call's output is drawn together" a rule that can be
            // stated rather than a coincidence of the dispatch order.
            Seen::Turn(Event::Wrote { call, text }) => {
                let joined = self.gather(text.as_str(), |seen| match seen {
                    Seen::Turn(Event::Wrote { call: from, text }) if *from == call => {
                        Some(text.as_str())
                    }
                    _ => None,
                });

                Ok(Seen::Turn(Event::Wrote {
                    call,
                    text: Wrote::new(joined),
                }))
            }

            other => Ok(other),
        }
    }

    /// Takes `first` and everything already waiting that `mergeable` accepts.
    ///
    /// One place, because the rule is one rule: adjacent text merges up to a
    /// ceiling, and the first thing that does not merge is put back rather than
    /// dropped. What differs between the two callers is only which events count
    /// as adjacent text, which is exactly what the closure answers.
    fn gather(&mut self, first: &str, mergeable: impl Fn(&Seen) -> Option<&str>) -> String {
        let mut text = String::from(first);

        while text.len() < BATCH_BYTES {
            let Ok(next) = self.from.try_recv() else {
                break;
            };

            match mergeable(&next) {
                Some(more) if text.len().saturating_add(more.len()) <= BATCH_BYTES => {
                    text.push_str(more);
                }
                // Either something else entirely, or more of the same that would
                // cross the ceiling. Held either way: the next call takes it, and
                // the terminal still sees what the runner reported in order.
                Some(_) | None => {
                    self.held = Some(next);
                    break;
                }
            }
        }

        text
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, sync_channel};
    use std::time::Duration;

    use crucible_app::client::Deciding;
    use crucible_core::{
        Ancestry, Answer as Offered, Ask, Command, ToolArgs, ToolId, TurnId, Wrote,
    };
    use crucible_runner::Reporter;

    use super::*;
    use crate::cli::client::tests::Noted;

    /// What the permission engine hears when it asks through `asking`, the way
    /// a turn does.
    fn asked(asking: &mut Asking) -> Answer {
        Deciding::new(asking, Capabilities::every()).ask(&call(), &running())
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
        let (reply, answers) = channel();
        let mut asking = Asking::new(to, answers, Client::new());

        let waiting = std::thread::spawn(move || asked(&mut asking));

        assert!(matches!(seen.recv().unwrap(), Seen::Question { .. }));
        reply.send((Verdict::Allow, Remember::Session)).unwrap();

        assert_eq!(waiting.join().unwrap(), (Verdict::Allow, Remember::Session));
    }

    #[test]
    fn nobody_left_to_ask_is_a_refusal() {
        // Not a deadlock and not an allow: the process is leaving, and a tool
        // that ran on the way out ran without consent.
        let (to, seen) = sync_channel(2);
        let (reply, answers) = channel::<Answer>();
        let client = Client::new();
        let mut asking = Asking::new(to, answers, client.clone());
        drop(reply);

        let answer = asked(&mut asking);

        assert_eq!(answer, (Verdict::Deny, Remember::Never));
        drop(seen);

        // Accounted for rather than timed: the question was put once, nothing
        // was decided about it, and the no is the application's own.
        assert!(
            matches!(client.noted().as_slice(), [Noted::Put(_)]),
            "{:?}",
            client.noted()
        );
    }

    #[test]
    fn a_question_that_cannot_be_delivered_is_a_refusal() {
        let (to, seen) = sync_channel(2);
        let (_reply, answers) = channel::<Answer>();
        let client = Client::new();
        let mut asking = Asking::new(to, answers, client.clone());
        drop(seen);

        assert_eq!(asked(&mut asking), (Verdict::Deny, Remember::Never));
        assert!(
            matches!(client.noted().as_slice(), [Noted::Put(_)]),
            "{:?}",
            client.noted()
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
        let (reply, answers) = channel::<Given>();
        let putting = Putting::new();
        putting.open(to, answers);

        let drawing = std::thread::spawn(move || {
            let put = seen.recv();
            assert!(matches!(put, Ok(Seen::Asked { .. })), "{put:?}");
            reply.send(Some(given)).expect("the tool is waiting");
        });
        let question = Question::new(
            "Words",
            "What should it say?",
            [Offered::new("these"), Offered::new("those")],
        );
        let heard = putting.put(&[question]).expect("somebody answered");
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
}

/// The two ends a turn lends whoever is asking.
///
/// Made fresh for each turn, for the reason the verdict channel is: an answer
/// that outlived the turn it was meant for is an answer to a question nobody
/// asked.
#[derive(Debug)]
struct Ends {
    to: SyncSender<Seen>,
    answers: Receiver<Given>,
}

/// Where a tool's questions go, and where the answers come back.
///
/// Held by the tool from the moment it is built and lent its ends one turn at a
/// time, which is the difference between this and [`Asking`]: a verdict is
/// asked for by the loop, which can be handed a fresh value per turn, and this
/// is asked for by a tool that was built once and never rebuilt.
///
/// **Nobody there is an answer.** A turn that has lent no ends, a channel that
/// will not carry the questions, and a reply channel that closed first all mean
/// the same thing — there is no one to ask — and the tool turns that into a
/// result the turn survives. That is the whole of what makes it different from
/// the verdict beside it, whose silence has to be a refusal because running a
/// tool nobody agreed to is worse than stopping. Here nothing runs either way.
#[derive(Debug, Clone, Default)]
pub(crate) struct Putting {
    ends: Arc<Mutex<Option<Ends>>>,
}

impl Putting {
    /// A handle with no turn behind it yet.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Lends the ends of this turn's channels.
    pub(crate) fn open(&self, to: SyncSender<Seen>, answers: Receiver<Given>) {
        if let Ok(mut held) = self.ends.lock() {
            *held = Some(Ends { to, answers });
        }
    }

    /// Takes them back, so a question put after the turn ended finds nobody.
    ///
    /// [`Relay`]'s drop is what calls it, and why is written there.
    pub(crate) fn close(&self) {
        if let Ok(mut held) = self.ends.lock() {
            *held = None;
        }
    }
}

impl Put for Putting {
    /// Blocks the turn until somebody answers.
    ///
    /// The lock is held across the wait on purpose: two asks outstanding at once
    /// would be two questions on one screen with one reply channel between them,
    /// and there is no shape of that which is right. Tools run one at a time, so
    /// what this makes impossible is not something anything does today — it is
    /// something a later change cannot start doing by accident.
    fn put(&self, questions: &[Question]) -> Option<Vec<Answered>> {
        let held = self.ends.lock().ok()?;
        let mut ends = held.as_ref()?;

        client::questions(Capabilities::every(), &mut ends, questions)
    }
}

impl Front for &Ends {
    fn put(&mut self, pending: &Pending, shown: Shown<'_>) -> Option<Decision> {
        let Shown::Questions(questions) = shown else {
            return None;
        };

        self.to
            .send(Seen::Asked {
                questions: questions.to_vec(),
            })
            .ok()?;

        let id = pending.id();
        let answers = self
            .answers
            .recv()
            .ok()?
            .and_then(|answered| answered.iter().map(picked).collect());
        Some(match answers {
            Some(answers) => Decision::Answered { id, answers },
            None => Decision::Declined { id },
        })
    }

    /// Nothing to say, for the reason [`Asking`] has nothing to: what is
    /// answered above is one answer per question put, or nobody answering.
    fn refused(&mut self, _: Refusal) {}
}

/// What a person writes on the panel fits what a decision carries: the panel's
/// editors stop at the one ceiling, and a decision's words go up to the other.
const _: () = assert!(crucible_tui::Editor::MAX_BYTES <= SAID_BYTES);

/// One answer, as a decision carries it: every word of it, or no answer.
///
/// Nothing is cut here, because the tool that asked acts on what comes back.
/// `None` is an answer longer than a decision carries, which the assertion
/// above keeps the panel from producing; the questions are then declined rather
/// than answered with words nobody wrote.
fn picked(answered: &Answered) -> Option<Picked> {
    Some(Picked {
        chosen: answered
            .chosen()
            .map(|words| Said::new(words).ok())
            .collect::<Option<_>>()?,
        note: Said::new(answered.note()).ok()?,
    })
}
