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
//! front end of [`crucible_app::client`], and is held to what any other is.
//! Each [`Seen::Question`] and [`Seen::Asked`] carries its own one-shot reply
//! channel, made fresh when the question is and travelling with it rather
//! than living on [`Asking`] or [`Putting`] for the length of a turn: the
//! channel is the identity. A stray reply cannot settle a question it was not
//! made for, because there is no shared channel left for it to arrive on —
//! the receiver it would have to reach was dropped with the question it
//! answered, or with the question that was never answered before something
//! else asked again.
//!
//! Awaiting rather than blocking is what frees the thread polling the turn
//! for the length of a human-length wait: the answer travels over an
//! asynchronous channel, so the wait is the future's own `Pending` and never
//! a block inside a poll. The channel is bounded by construction — a
//! one-shot carries exactly one value — and it is dropped with the pending
//! action: a dropped ask drops its receiver, so a reply sent after that finds
//! nobody rather than queuing for whatever asks next.

use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::oneshot;

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
use crucible_runtime::BoxFuture;

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
        /// Where the verdict goes — made fresh for this question alone, so
        /// answering it can never reach a different one.
        reply: oneshot::Sender<Answer>,
    },

    /// A tool is waiting on a person. Put the questions, then answer.
    Asked {
        /// What to put, in the order it should be answered.
        questions: Vec<Question>,
        /// Where the answers go — made fresh for this ask alone, for the
        /// reason [`Seen::Question`]'s `reply` is.
        reply: oneshot::Sender<Given>,
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

/// Puts a question to the drawing thread and awaits the answer.
#[derive(Debug)]
pub(crate) struct Asking {
    to: SyncSender<Seen>,
    client: Client,
}

impl Asking {
    /// Takes where questions go and what a request is made with: the client
    /// that numbers requests. Where an answer arrives is made fresh for each
    /// question, not held here — see the module documentation. What a
    /// pending action is named from is the application's and is not handed
    /// in.
    pub(crate) const fn new(to: SyncSender<Seen>, client: Client) -> Self {
        Self { to, client }
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

        self.client
            .answered(&request, conversation, || ended.outcome());

        ended
    }
}

impl Front for Asking {
    /// Awaits the turn until someone answers.
    ///
    /// Silence is a refusal, and the application is what makes it one: a
    /// channel that will not carry the question, or that closes before an
    /// answer comes back, means nobody is left to consent — and running a tool
    /// nobody agreed to is the one outcome worth avoiding more than stopping.
    /// A human's answer is human-length, so this hands back a future that
    /// answers `Pending` the moment it is asked and wakes once the drawing
    /// thread has sent one — never a wait inside the poll that produces it.
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Decision>> {
        Box::pin(async move {
            // A model's questions come through the tool that asks them, which
            // is lent its own ends; a turn stops here on a call and nothing
            // else.
            let Shown::Call { call, sensitivity } = shown else {
                return None;
            };

            self.client.put(pending);

            let (reply, hear) = oneshot::channel();
            let question = Seen::Question {
                call: call.clone(),
                sensitivity: sensitivity.clone(),
                reply,
            };
            self.to.send(question).ok()?;
            let (verdict, remember) = hear.await.ok()?;

            let decision = Decision::Ruled {
                id: pending.id(),
                ruling: match verdict {
                    Verdict::Allow => Ruling::Allow,
                    Verdict::Deny => Ruling::Deny,
                },
                lasting: match remember {
                    Remember::Never => Lasting::Once,
                    // The prompt offers nothing that outlasts the process, and
                    // the engine keeps the two alike: for the rest of this
                    // session.
                    Remember::Session | Remember::Always => Lasting::Session,
                },
            };

            self.client.decided(&decision);

            Some(decision)
        })
    }

    /// Nothing to say: every decision made above names the action it was put
    /// and rules on it, so the application has none of them to turn away.
    fn refused(&mut self, _: Refusal) {}

    /// The panel is drawn from the call and what it would do, as they stand
    /// on the host, and never from the cut words a pending action carries.
    fn draws_whole(&self) -> bool {
        true
    }
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
mod tests;

/// The ends a turn lent, as one ask reaches them.
///
/// It holds the place the ends are lent into, never a copy of them, and
/// writes nothing back there: each question it puts reads the sending end
/// lent there, sends, and lets go of it before waiting for the answer. So an
/// ask still waiting when its turn ends holds nothing of that turn — the loop
/// that draws sees the turn's channel close as it always does. A question is
/// put down whatever ends are lent when it is sent: none once they were taken
/// back, and those of the next turn once that turn lends its own — which a
/// question never meets, because every run a turn starts is awaited within
/// that turn. The end that would carry an answer back is never here (see the
/// module documentation): each question makes its own, so an answer only
/// ever settles the question it was made for, and a second ask in the same
/// turn cannot take anything from a first one still outstanding.
struct Lent<'a> {
    ends: &'a Mutex<Option<SyncSender<Seen>>>,
}

/// Where a tool's questions go.
///
/// Held by the tool from the moment it is built and lent its ends one turn at a
/// time, which is the difference between this and [`Asking`]: a verdict is
/// asked for by the loop, which can be handed a fresh value per turn, and this
/// is asked for by a tool that was built once and never rebuilt.
///
/// **Nobody there is an answer.** A turn that has lent no ends and a channel
/// that will not carry the questions both mean the same thing — there is no
/// one to ask — and the tool turns that into a result the turn survives. That
/// is the whole of what makes it different from the verdict beside it, whose
/// silence has to be a refusal because running a tool nobody agreed to is
/// worse than stopping. Here nothing runs either way.
#[derive(Debug, Clone, Default)]
pub(crate) struct Putting {
    ends: Arc<Mutex<Option<SyncSender<Seen>>>>,
}

impl Putting {
    /// A handle with no turn behind it yet.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Lends the ends of this turn's channel.
    pub(crate) fn open(&self, to: SyncSender<Seen>) {
        if let Ok(mut held) = self.ends.lock() {
            *held = Some(to);
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
    /// Awaits the turn until somebody answers.
    ///
    /// A human's answer is human-length, so this hands back a future that
    /// answers `Pending` the moment it is asked and wakes once the drawing
    /// thread has sent one — never a wait inside the poll that produces it.
    ///
    /// What waits for the answer is [`Lent`], which holds none of the turn's
    /// ends across the wait and never puts any back, so a put its turn
    /// outlived cannot keep that turn's channel open, or leave a stale end
    /// where the next turn's are lent.
    fn put<'a>(&'a self, questions: &'a [Question]) -> BoxFuture<'a, Option<Vec<Answered>>> {
        Box::pin(async move {
            if self.ends.lock().ok()?.is_none() {
                return None;
            }
            let mut lent = Lent { ends: &self.ends };
            client::questions(Capabilities::every(), &mut lent, questions).await
        })
    }
}

impl Front for Lent<'_> {
    /// Puts `questions` down a fresh one-shot made for this ask alone, and
    /// awaits the far end of it — see the module documentation for why the
    /// channel is not a field of `Lent` or [`Putting`]. The turn's sending end
    /// is read as the question is sent and let go of before the wait.
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Decision>> {
        Box::pin(async move {
            let Shown::Questions(questions) = shown else {
                return None;
            };

            let (reply, hear) = oneshot::channel();
            let to = self.ends.lock().ok()?.clone()?;
            to.send(Seen::Asked {
                questions: questions.to_vec(),
                reply,
            })
            .ok()?;
            drop(to);

            let id = pending.id();
            let answers = hear
                .await
                .ok()?
                .and_then(|answered| answered.iter().map(picked).collect());
            Some(match answers {
                Some(answers) => Decision::Answered { id, answers },
                None => Decision::Declined { id },
            })
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
