//! One receiver for the two things the drawing thread has to answer.
//!
//! A turn runs on its own thread and reports through [`Post`]; it also stops
//! mid-flight to ask a question through [`Ask`]. The thread that draws is
//! parked in `recv`, and a channel has no `select`, so both have to arrive on
//! the same one. That is all [`Seen`] is: the union of what can turn up.
//!
//! The alternative — a second thread forwarding events into the first — buys
//! nothing and adds a hop to every delta.

use std::sync::mpsc::{Receiver, Sender};

use crucible_core::{Ask, Event, Post, Remember, Sensitivity, ToolCall, Verdict};

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
pub(crate) struct Relay(Sender<Seen>);

impl Relay {
    /// Takes the sending end. One per turn, dropped when the turn ends — which
    /// is how the drawing thread learns the turn is over.
    pub(crate) fn new(to: Sender<Seen>) -> Self {
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
    to: Sender<Seen>,
    answers: Receiver<Answer>,
}

impl Asking {
    /// Takes the two ends it needs: where questions go, where answers arrive.
    pub(crate) fn new(to: Sender<Seen>, answers: Receiver<Answer>) -> Self {
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use crucible_core::{Command, ToolArgs, ToolId};

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
            command: Command::Understood(Box::from([Box::from("ls")])),
        }
    }

    #[test]
    fn an_event_arrives_as_something_to_draw() {
        let (to, seen) = channel();
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
        let (to, seen) = channel();
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
        let (to, seen) = channel();
        let (reply, answers) = channel::<Answer>();
        let mut asking = Asking::new(to, answers);
        drop(reply);

        let answer = asking.ask(&call(), &running());

        assert_eq!(answer, (Verdict::Deny, Remember::Never));
        drop(seen);
    }

    #[test]
    fn a_question_that_cannot_be_delivered_is_a_refusal() {
        let (to, seen) = channel();
        let (_reply, answers) = channel::<Answer>();
        let mut asking = Asking::new(to, answers);
        drop(seen);

        assert_eq!(
            asking.ask(&call(), &running()),
            (Verdict::Deny, Remember::Never)
        );
    }
}
