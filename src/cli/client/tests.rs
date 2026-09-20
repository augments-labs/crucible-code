//! The terminal and a client with no terminal, asked the same things.
//!
//! Each pair below runs one script twice over conversations built alike. Once
//! through the whole loop, typed into the way a person types: what the loop
//! asked of the application, what it was put and what it decided are noted as
//! they cross. Once with no terminal at all: every request and every decision
//! is written to the bytes it would travel as and read back before the
//! application sees it, and every answer is read back off its bytes too. What
//! is then held side by side is what the application was asked and answered,
//! in order, where the conversation stood after each answer, and whether the
//! tool ran — never a byte the terminal drew.

use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use crucible_app::Conversation;
use crucible_app::client::{self, Front, Minting, Shown, snapshot};
use crucible_client_api::{
    Capabilities, Command, Correlation, Decision, ErrorCode, Lasting, Outcome, Pending, Prompt,
    Refusal, Request, Response, ResumeOutcome, Ruling, Snapshot, Stop, TurnOutcome,
};
use crucible_core::{
    Approved, Aside, Cancel, Delta, DescribeTool, Message, Mode, Permission, Rules, Sensitivity,
    SessionId, Steer, StopReason, Summary, Tool, ToolArgs, ToolContext, ToolError, ToolId,
    ToolOutput,
};
use crucible_runner::{EventEnvelope, Tools};
use crucible_session::Session;
use crucible_tui::{Editor, Recording, Renderer};

use super::Client;
use crate::cli::converse::tests::{opening, paired, plain, scripted};
use crate::cli::converse::{Terms, converse};
use crate::cli::fake::{Script, changing};
use crate::cli::sample::Sample;

/// One thing that passed between the terminal and the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Noted {
    /// A command was answered, and where the conversation stood afterwards.
    Answered {
        /// The command.
        asked: Command,
        /// What the application answered.
        outcome: Outcome,
        /// Where the conversation stood once it had.
        standing: Snapshot,
    },
    /// A turn stopped on this, and the front end was put it.
    Put(Pending),
    /// What the front end said about the action it was put.
    Decided(Decision),
    /// A command that needs no conversation was answered.
    Apart {
        /// The command.
        asked: Command,
        /// What the application answered.
        outcome: Outcome,
    },
}

impl Client {
    /// Notes what `request` was answered with.
    pub(crate) fn answered(
        &self,
        request: &Request,
        conversation: &Conversation,
        outcome: Outcome,
    ) {
        self.note(Noted::Answered {
            asked: request.command().clone(),
            outcome,
            standing: snapshot(conversation, None),
        });
    }

    /// Notes a pending action the terminal was put.
    pub(crate) fn put(&self, pending: &Pending) {
        self.note(Noted::Put(pending.clone()));
    }

    /// Notes what the terminal said about a pending action.
    pub(crate) fn decided(&self, decision: &Decision) {
        self.note(Noted::Decided(decision.clone()));
    }

    /// Notes what `request`, which needs no conversation, was answered with.
    pub(crate) fn apart(&self, request: &Request, outcome: Outcome) {
        self.note(Noted::Apart {
            asked: request.command().clone(),
            outcome,
        });
    }

    /// Everything noted so far, in the order it happened.
    pub(crate) fn noted(&self) -> Vec<Noted> {
        self.noted
            .lock()
            .map(|noted| noted.clone())
            .unwrap_or_default()
    }

    fn note(&self, noted: Noted) {
        if let Ok(mut held) = self.noted.lock() {
            held.push(noted);
        }
    }
}

/// The name the counting tool is offered and called by.
const WRITE: &str = "write";

/// What a tool does when it runs, beside being counted.
type Pressing = Box<dyn Fn() + Send + Sync>;

/// A tool that counts how often it was let run, and does one more thing of a
/// test's choosing while it does — which is how a turn is interrupted from the
/// middle of itself, at the same moment on every run.
struct Counting {
    sensitivity: Sensitivity,
    ran: Arc<AtomicUsize>,
    pressing: Pressing,
}

impl std::fmt::Debug for Counting {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Counting")
            .field("ran", &self.ran)
            .finish_non_exhaustive()
    }
}

impl DescribeTool for Counting {
    fn name(&self) -> &str {
        WRITE
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Counting {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        self.sensitivity.clone()
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        Summary::new(args.as_str())
    }

    fn run(
        &self,
        _approved: Approved,
        _context: &ToolContext<'_>,
    ) -> Result<ToolOutput, ToolError> {
        self.ran.fetch_add(1, Ordering::Relaxed);
        (self.pressing)();
        Ok(ToolOutput::ok("done"))
    }
}

/// The round in which the model reaches for the counting tool.
fn calling() -> Vec<Delta> {
    vec![
        Delta::ToolStarted {
            id: ToolId::new("a"),
            name: WRITE.into(),
        },
        Delta::ToolArgs("{}".into()),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

fn saying(text: &str) -> Vec<Delta> {
    vec![
        Delta::Text(text.into()),
        Delta::Stopped(StopReason::Yielded),
    ]
}

/// A conversation that asks before anything is changed, answering from
/// `rounds` with the counting tool on offer.
fn asking(session: Session, rounds: Vec<Vec<Delta>>, tool: Counting) -> Conversation {
    let mut offered = Tools::new();
    offered.add_builtin(tool).expect("the tool registers");

    paired(Arc::new(session), |session| {
        scripted(Script::new(rounds), offered, session)
            .permitting(Permission::with(Mode::Ask, Rules::new()))
    })
}

/// The whole loop over `typed`, and everything it asked of the application.
fn at_the_terminal(terms: &Terms, conversation: Conversation, typed: &str) -> Vec<Noted> {
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(typed.as_bytes().to_vec());

    converse(conversation, &mut renderer, terms, &opening(), &mut input)
        .expect("the loop to finish");

    terms.client.noted()
}

/// Everything a client with no terminal asked and was answered, in order.
#[derive(Debug, Clone, Default)]
struct Journal(Arc<Mutex<Vec<Noted>>>);

impl Journal {
    fn note(&self, noted: Noted) {
        self.0.lock().expect("the journal").push(noted);
    }

    fn noted(&self) -> Vec<Noted> {
        self.0.lock().expect("the journal").clone()
    }
}

/// Requests numbered as they are made, each read back off the bytes it
/// travels as.
#[derive(Debug, Clone, Default)]
struct Wire(Arc<AtomicUsize>);

impl Wire {
    fn sent(&self, command: Command) -> Request {
        let number = self.0.fetch_add(1, Ordering::Relaxed) + 1;
        let request = Request::new(
            Capabilities::every(),
            Correlation::new(u64::try_from(number).expect("a small count")),
            command,
        );
        let bytes = request.encode().expect("a request fits its frame");

        Request::decode(&bytes).expect("what was written reads back")
    }
}

/// An outcome as its reader has it: written in a response, and read back.
fn received(request: &Request, outcome: Outcome) -> Outcome {
    let response = Response {
        correlation: Some(request.correlation()),
        outcome,
    };
    let bytes = response.encode().expect("a response fits its frame");

    Response::decode(&bytes)
        .expect("what was written reads back")
        .outcome
}

/// Where `conversation` stands, as a client reads it.
fn standing(conversation: &Conversation) -> Snapshot {
    let bytes = snapshot(conversation, None)
        .encode()
        .expect("a snapshot fits its frame");

    Snapshot::decode(&bytes).expect("what was written reads back")
}

/// A client with no terminal: it rules on what it is put from a list written
/// beforehand, and its rulings arrive as the bytes of a `decide` request.
struct Headless {
    wire: Wire,
    journal: Journal,
    rulings: std::vec::IntoIter<Ruling>,
}

impl Front for Headless {
    fn put(&mut self, pending: &Pending, _shown: Shown<'_>) -> Option<Decision> {
        self.journal.note(Noted::Put(pending.clone()));

        let sent = self.wire.sent(Command::Decide(Decision::Ruled {
            id: pending.id(),
            ruling: self.rulings.next()?,
            lasting: Lasting::Once,
        }));
        let Command::Decide(decision) = sent.command() else {
            return None;
        };
        self.journal.note(Noted::Decided(decision.clone()));

        Some(decision.clone())
    }

    fn refused(&mut self, _: Refusal) {}
}

/// What a client with no terminal holds for the length of a script.
struct Driving<'a> {
    terms: &'a Terms,
    wire: Wire,
    journal: Journal,
    cancel: Cancel,
    minting: Minting,
}

impl<'a> Driving<'a> {
    /// `terms` is the host's side of it — where sessions are kept and which
    /// providers there are — and nothing of the terminal's is asked of it.
    fn new(terms: &'a Terms) -> Self {
        Self {
            terms,
            wire: Wire::default(),
            journal: Journal::default(),
            cancel: Cancel::new(),
            minting: Minting::new(),
        }
    }

    /// What a tool calls to ask, as bytes, for the turn it runs in to stop.
    fn interrupting(&self) -> Pressing {
        let (wire, journal, cancel) =
            (self.wire.clone(), self.journal.clone(), self.cancel.clone());

        Box::new(move || {
            let request = wire.sent(Command::Cancel);
            let heard = client::interrupt(&request, &cancel);
            journal.note(Noted::Apart {
                asked: request.command().clone(),
                outcome: received(&request, heard),
            });
        })
    }

    /// Asks for `command` and notes the answer; `rulings` are what is said of
    /// whatever a turn stops on, in the order it stops.
    fn asks(&self, conversation: &mut Conversation, command: Command, rulings: Vec<Ruling>) {
        let request = self.wire.sent(command);
        let outcome = match request.command() {
            Command::Prompt(_) | Command::Compact => {
                let (events, reported) = mpsc::channel::<EventEnvelope>();
                let (steer, aside) = (Steer::new(), Aside::new());
                let mut front = Headless {
                    wire: self.wire.clone(),
                    journal: self.journal.clone(),
                    rulings: rulings.into_iter(),
                };
                self.cancel.reset();
                let run = conversation
                    .runner()
                    .starting(&events, &self.cancel, &steer, &aside);
                let ended = client::turn(
                    conversation,
                    &request,
                    Box::default(),
                    (&mut front, &self.minting),
                    &run,
                );
                drop(reported);

                ended.outcome()
            }
            _ => {
                let providers = self.terms.providers.snapshot();

                client::perform(conversation, &request, &self.terms.desk(&providers)).outcome()
            }
        };

        self.journal.note(Noted::Answered {
            asked: request.command().clone(),
            outcome: received(&request, outcome),
            standing: standing(conversation),
        });
    }
}

/// The prompt a line typed down a pipe is: the line and the ending it was read
/// with, which the loop has always sent as it arrived.
fn prompt(words: &str) -> Command {
    let words = &format!("{words}\n");

    Command::Prompt(Prompt::new(words).expect("a prompt the contract carries"))
}

/// `noted` with no session named, for holding two runs beside each other that
/// each recorded into a log of their own: not in where the conversation
/// stands, and `stood_in` wherever one was asked for by name.
fn unnamed(noted: Vec<Noted>, stood_in: &SessionId) -> Vec<Noted> {
    noted
        .into_iter()
        .map(|noted| match noted {
            Noted::Answered {
                asked,
                outcome,
                standing,
            } => Noted::Answered {
                asked: match asked {
                    Command::Resume(_) => Command::Resume(stood_in.clone()),
                    other => other,
                },
                outcome,
                standing: Snapshot {
                    session: None,
                    ..standing
                },
            },
            other => other,
        })
        .collect()
}

/// The session the last answer left the conversation in.
fn session(noted: &[Noted]) -> Option<SessionId> {
    noted.iter().rev().find_map(|noted| match noted {
        Noted::Answered { standing, .. } => standing.session.clone(),
        _ => None,
    })
}

/// The outcomes answered, in order, for saying what a pair agreed on.
fn outcomes(noted: &[Noted]) -> Vec<Outcome> {
    noted
        .iter()
        .filter_map(|noted| match noted {
            Noted::Answered { outcome, .. } | Noted::Apart { outcome, .. } => Some(outcome.clone()),
            _ => None,
        })
        .collect()
}

/// One permission question answered `typed` at the terminal and `ruling` on
/// the wire: what each side noted, and how often each let the tool run.
fn ruled(typed: &str, ruling: Ruling) -> ((Vec<Noted>, usize), (Vec<Noted>, usize)) {
    let rounds = || vec![calling(), saying("changed it")];
    let counting = |ran: &Arc<AtomicUsize>| Counting {
        sensitivity: changing(),
        ran: Arc::clone(ran),
        pressing: Box::new(|| ()),
    };

    let terms = plain();
    let ran = Arc::new(AtomicUsize::new(0));
    let conversation = asking(Session::nowhere(), rounds(), counting(&ran));
    let terminal = at_the_terminal(&terms, conversation, typed);
    let terminal = (terminal, ran.load(Ordering::Relaxed));

    let host = plain();
    let driving = Driving::new(&host);
    let ran = Arc::new(AtomicUsize::new(0));
    let mut conversation = asking(Session::nowhere(), rounds(), counting(&ran));
    driving.asks(&mut conversation, prompt("go"), vec![ruling]);

    (
        terminal,
        (driving.journal.noted(), ran.load(Ordering::Relaxed)),
    )
}

#[test]
fn a_yes_typed_and_a_yes_sent_run_the_tool_once_and_are_answered_alike() {
    let (terminal, headless) = ruled("go\ny\n", Ruling::Allow);

    assert_eq!(terminal, headless);
    assert_eq!(headless.1, 1, "the tool ran once");
    assert!(
        matches!(
            headless.0.as_slice(),
            [
                Noted::Put(Pending::Permission { .. }),
                Noted::Decided(Decision::Ruled {
                    ruling: Ruling::Allow,
                    ..
                }),
                Noted::Answered {
                    outcome: Outcome::Turn(TurnOutcome::Ran {
                        stop: Stop::Yielded
                    }),
                    ..
                },
            ]
        ),
        "{headless:?}"
    );
}

#[test]
fn a_no_typed_and_a_no_sent_never_run_the_tool_and_are_answered_alike() {
    let (terminal, headless) = ruled("go\nn\n", Ruling::Deny);

    assert_eq!(terminal, headless);
    assert_eq!(headless.1, 0, "the tool never ran");
    assert!(
        matches!(
            headless.0.as_slice(),
            [
                Noted::Put(Pending::Permission { .. }),
                Noted::Decided(Decision::Ruled {
                    ruling: Ruling::Deny,
                    ..
                }),
                Noted::Answered {
                    outcome: Outcome::Turn(TurnOutcome::Failed(problem)),
                    ..
                },
            ] if problem.code == ErrorCode::Failed
        ),
        "{headless:?}"
    );
}

#[test]
fn a_turn_interrupted_from_either_side_stops_at_the_same_place() {
    // The tool is what interrupts, so the request to stop lands at the same
    // moment of the same turn on both runs: after the tool ran, before the
    // model is asked again. The terminal's is the call its interrupt key makes.
    let rounds = || vec![calling(), saying("never said")];
    let reading = |ran: &Arc<AtomicUsize>, pressing: Pressing| Counting {
        sensitivity: Sensitivity::ReadOnly {
            target: crucible_core::Target::unresolved(),
        },
        ran: Arc::clone(ran),
        pressing,
    };

    let terms = plain();
    let (keys, cancel) = (terms.client.clone(), terms.cancel.clone());
    let ran = Arc::new(AtomicUsize::new(0));
    let pressing: Pressing = Box::new(move || keys.interrupt(&cancel));
    let conversation = asking(Session::nowhere(), rounds(), reading(&ran, pressing));
    let terminal = at_the_terminal(&terms, conversation, "go\n");
    let terminal = (terminal, ran.load(Ordering::Relaxed));

    let host = plain();
    let driving = Driving::new(&host);
    let ran = Arc::new(AtomicUsize::new(0));
    let mut conversation = asking(
        Session::nowhere(),
        rounds(),
        reading(&ran, driving.interrupting()),
    );
    driving.asks(&mut conversation, prompt("go"), Vec::new());
    let headless = (driving.journal.noted(), ran.load(Ordering::Relaxed));

    assert_eq!(terminal, headless);
    assert_eq!(
        outcomes(&headless.0).first(),
        Some(&Outcome::Cancelling),
        "{headless:?}"
    );
    assert_eq!(
        outcomes(&headless.0).last(),
        Some(&Outcome::Turn(TurnOutcome::Ran {
            stop: Stop::Cancelled
        })),
        "the model was asked again after the turn was told to stop: {headless:?}"
    );
}

/// A session recorded under `sample` and closed again, holding one exchange.
fn recorded(sample: &Sample) -> SessionId {
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    session.append(&Message::said("what was asked before"));
    session.append(&Message::Agent {
        continuation: None,
        text: "an answer".into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
    });

    session.id().expect("a recorded session has a name").clone()
}

#[test]
fn a_session_picked_up_from_either_side_is_carried_on_from_the_same_place() {
    // Two trees recorded alike, because a session carried on is a session
    // changed: each side picks up its own and the names are held apart.
    let rounds = || vec![saying("carried on")];
    let quiet = || Counting {
        sensitivity: changing(),
        ran: Arc::default(),
        pressing: Box::new(|| ()),
    };
    let under = |sample: &Sample| Terms {
        sessions: sample.logs(),
        workspace: sample.workspace(),
        ..plain()
    };

    let typed_under = Sample::new("client-resume-typed");
    let earlier = recorded(&typed_under);
    let terms = under(&typed_under);
    let conversation = asking(Session::nowhere(), rounds(), quiet());
    let terminal = at_the_terminal(
        &terms,
        conversation,
        &format!("/resume {}\nand then\n", earlier.as_str()),
    );
    assert_eq!(session(&terminal), Some(earlier));

    let sent_under = Sample::new("client-resume-sent");
    let earlier = recorded(&sent_under);
    let host = under(&sent_under);
    let driving = Driving::new(&host);
    let mut conversation = asking(Session::nowhere(), rounds(), quiet());
    driving.asks(
        &mut conversation,
        Command::Resume(earlier.clone()),
        Vec::new(),
    );
    driving.asks(&mut conversation, prompt("and then"), Vec::new());
    let headless = driving.journal.noted();
    assert_eq!(session(&headless), Some(earlier));

    let either = SessionId::new();
    assert_eq!(
        unnamed(terminal, &either),
        unnamed(headless.clone(), &either)
    );
    assert_eq!(
        outcomes(&headless),
        [
            Outcome::Resumed(ResumeOutcome::Picked { unclosed: None }),
            Outcome::Turn(TurnOutcome::Ran {
                stop: Stop::Yielded
            }),
        ]
    );
    assert!(
        matches!(headless.last(), Some(Noted::Answered { standing, .. }) if standing.messages == 10),
        "two messages picked up, and a prompt, what it was told and an answer since: {headless:?}"
    );
}

#[test]
fn every_prompt_the_box_holds_is_one_the_contract_carries() {
    // Two ceilings with two owners. A line the box took and the contract then
    // refused would be a prompt lost after it was typed.
    const { assert!(crucible_client_api::bounds::PROMPT_BYTES >= Editor::MAX_BYTES) };
}
