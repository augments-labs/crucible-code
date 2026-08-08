//! What the turn loop does, over a provider that answers from a script and
//! tools that answer from a field.

use std::sync::mpsc::{Receiver, channel};

use crucible_core::{ProviderError, Sensitivity, ToolId, Verdict};

use super::*;
use crate::fake::{Fixed, Says, Script, Sent};
use crate::sample::Sample;

/// A runner over a scripted provider, with somewhere for its events to go.
struct Scripted {
    runner: Runner,
    sent: Sent,
    says: Says,
    cancel: Cancel,
    events: Sender<Event>,
    seen: Receiver<Event>,
}

impl Scripted {
    fn new(script: Script, tools: Tools, verdict: Verdict) -> Self {
        Self::recording(script, tools, verdict, Session::nowhere())
    }

    fn recording(script: Script, tools: Tools, verdict: Verdict, session: Session) -> Self {
        let (events, seen) = channel();
        let sent = script.sent();

        Self {
            runner: Runner::new(
                Box::new(script),
                tools,
                Model {
                    name: "claude-test".into(),
                    max_tokens: 1024,
                    system: None,
                },
                session,
            ),
            sent,
            says: Says::new(verdict),
            cancel: Cancel::new(),
            events,
            seen,
        }
    }

    fn turn(&mut self, prompt: &str) -> Result<StopReason, TurnError> {
        self.runner
            .turn(prompt, &mut self.says, &self.events, &self.cancel)
    }

    fn said(&self) -> String {
        self.seen
            .try_iter()
            .filter_map(|event| match event {
                Event::Delta { text } => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }

    /// How much transcript each request carried, in order.
    fn asked(&self) -> Vec<usize> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.transcript.len())
            .collect()
    }

    /// The tools the last request advertised.
    fn advertised(&self) -> Vec<&'static str> {
        self.sent
            .lock()
            .unwrap()
            .last()
            .map(|request| request.tools.iter().map(|tool| tool.name).collect())
            .unwrap_or_default()
    }
}

fn tools(tools: impl IntoIterator<Item = Fixed>) -> Tools {
    let mut offered = Tools::new();
    for tool in tools {
        offered.add(Box::new(tool));
    }
    offered
}

fn calling(id: &str, name: &str, args: &str) -> Vec<Delta> {
    vec![
        Delta::ToolStarted {
            id: ToolId::new(id),
            name: name.into(),
        },
        Delta::ToolArgs(args.into()),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

fn saying(text: &str) -> Vec<Delta> {
    vec![
        Delta::Text(text.into()),
        Delta::Stopped(StopReason::Yielded),
    ]
}

#[test]
fn a_turn_that_yields_records_what_the_model_said() {
    let script = Script::new(vec![vec![
        Delta::Text("Hello".into()),
        Delta::Text(", world".into()),
        Delta::Stopped(StopReason::Yielded),
    ]]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);

    assert_eq!(scripted.turn("hi").unwrap(), StopReason::Yielded);

    assert_eq!(scripted.said(), "Hello, world");
    assert_eq!(
        scripted.runner.transcript().messages(),
        [
            Message::User("hi".into()),
            Message::Agent {
                text: "Hello, world".into(),
                calls: Vec::new(),
            },
        ]
    );
}

#[test]
fn a_tool_call_runs_and_what_it_produced_goes_back_to_the_model() {
    let script = Script::new(vec![
        calling("a", "read", r#"{"path":"x"}"#),
        saying("it says hello"),
    ]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("read").answering("fn main() {}")]),
        Verdict::AllowOnce,
    );

    assert_eq!(scripted.turn("read x").unwrap(), StopReason::Yielded);

    let messages = scripted.runner.transcript().messages();
    assert_eq!(messages.len(), 4, "prompt, call, result, answer");
    assert!(matches!(
        messages.get(2),
        Some(Message::ToolResults(results))
            if results.first().is_some_and(|r| r.output.text() == "fn main() {}")
    ));
}

#[test]
fn the_second_request_carries_the_first_round_in_full() {
    // Without it the model answers the same question again, having no
    // record of the tool it just called.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::AllowOnce);

    scripted.turn("go").unwrap();

    assert_eq!(
        scripted.asked(),
        [1, 3],
        "first the prompt; then the prompt, the call, and its result"
    );
}

#[test]
fn a_tool_the_user_refused_ends_the_turn_and_is_still_answered() {
    let script = Script::new(vec![calling("a", "write", "{}")]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("write").risking(Sensitivity::MutatesFile)]),
        Verdict::Deny,
    );

    let problem = scripted.turn("write it").unwrap_err();

    assert_eq!(problem.to_string(), "write was not allowed");
    assert!(
        matches!(
            scripted.runner.transcript().messages().last(),
            Some(Message::ToolResults(results)) if results.len() == 1
        ),
        "a call with no result is a transcript the provider refuses"
    );
}

#[test]
fn a_call_the_model_never_finished_asking_for_is_not_recorded() {
    // Cancelled mid-sentence: the arguments are half a JSON object, and
    // there will never be a result to pair with the call.
    let script = Script::new(vec![vec![
        Delta::Text("looking".into()),
        Delta::ToolStarted {
            id: ToolId::new("a"),
            name: "read".into(),
        },
        Delta::ToolArgs("{\"path\":".into()),
        Delta::Stopped(StopReason::Cancelled),
    ]]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::AllowOnce);

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Cancelled);

    assert_eq!(
        scripted.runner.transcript().messages(),
        [
            Message::User("go".into()),
            Message::Agent {
                text: "looking".into(),
                calls: Vec::new(),
            },
        ]
    );
}

#[test]
fn a_model_that_wants_tools_but_names_none_yields_instead_of_asking_again() {
    // An unchanged transcript sent again produces the same answer again.
    let script = Script::new(vec![vec![Delta::Stopped(StopReason::WantsTools)]]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::AllowOnce);

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Yielded);

    assert_eq!(scripted.asked().len(), 1, "the model was asked again");
}

#[test]
fn a_provider_that_fails_ends_the_turn() {
    let mut scripted = Scripted::new(Script::failing(), Tools::new(), Verdict::AllowOnce);

    let problem = scripted.turn("go").unwrap_err();

    assert!(matches!(
        problem,
        TurnError::Provider(ProviderError::Refused { .. })
    ));
}

#[test]
fn turns_are_numbered_from_one() {
    let script = Script::new(vec![saying("first"), saying("second")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::AllowOnce);

    scripted.turn("one").unwrap();
    scripted.turn("two").unwrap();

    let started: Vec<u32> = scripted
        .seen
        .try_iter()
        .filter_map(|event| match event {
            Event::TurnStarted { turn } => Some(turn.get()),
            _ => None,
        })
        .collect();

    assert_eq!(started, [1, 2]);
}

#[test]
fn the_tools_a_runner_offers_are_advertised_on_every_request() {
    let script = Script::new(vec![saying("done")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::AllowOnce);

    scripted.turn("go").unwrap();

    assert_eq!(scripted.advertised(), ["read"]);
}

#[test]
fn a_call_is_announced_before_it_runs() {
    // The renderer draws the line for a running tool from this.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::AllowOnce);

    scripted.turn("go").unwrap();

    let requested: Vec<ToolCall> = scripted
        .seen
        .try_iter()
        .filter_map(|event| match event {
            Event::ToolRequested { call } => Some(call),
            _ => None,
        })
        .collect();

    assert_eq!(requested.len(), 1);
    assert_eq!(requested.first().map(|call| &*call.name), Some("read"));
}

#[test]
fn a_turn_starts_with_the_stop_the_last_one_left_behind_cleared() {
    // Otherwise the next turn is cancelled before it sends anything.
    let script = Script::new(vec![saying("done")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::AllowOnce);
    scripted.cancel.request();

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Yielded);
}

#[test]
fn everything_a_turn_adds_to_the_transcript_is_also_recorded() {
    // The two are written in one place so they cannot drift apart. A turn that
    // pushed a message without recording it would leave a session that
    // continues from somewhere other than where it stopped.
    let sample = Sample::new("runner-recording");
    let script = Script::new(vec![
        calling("a", "read", r#"{"path":"x"}"#),
        saying("done"),
    ]);
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let mut scripted = Scripted::recording(
        script,
        tools([Fixed::new("read").answering("fn main() {}")]),
        Verdict::AllowOnce,
        session,
    );

    scripted.turn("read x").unwrap();
    let held = scripted.runner.transcript().messages().to_vec();

    // Dropping the runner drops the session, which is what waits for the queue.
    drop(scripted);
    let (_session, replayed) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(replayed.messages(), held.as_slice());
}

#[test]
fn a_continued_conversation_goes_on_counting_where_it_stopped() {
    // Numbering the first continued turn 1 would tell the user this is a new
    // conversation, which is exactly what they asked it not to be.
    let script = Script::new(vec![saying("still here")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::AllowOnce);

    let mut earlier = Transcript::new();
    earlier.push(Message::User("one".into()));
    earlier.push(Message::Agent {
        text: "first".into(),
        calls: Vec::new(),
    });
    earlier.push(Message::User("two".into()));
    earlier.push(Message::Agent {
        text: "second".into(),
        calls: Vec::new(),
    });
    scripted.runner = scripted.runner.resuming(earlier);

    scripted.turn("three").unwrap();

    let started: Vec<u32> = scripted
        .seen
        .try_iter()
        .filter_map(|event| match event {
            Event::TurnStarted { turn } => Some(turn.get()),
            _ => None,
        })
        .collect();

    assert_eq!(started, [3]);
}
