//! What the turn loop does, over a provider that answers from a script and
//! tools that answer from a field.

use std::sync::mpsc::{Receiver, channel};

use crucible_core::{ProviderError, Sensitivity, ToolId, Verdict};

use super::*;
use crate::fake::{Fixed, Says, Script, Sent};

/// A runner with somewhere for its events to go.
struct Session {
    runner: Runner,
    sent: Sent,
    says: Says,
    cancel: Cancel,
    events: Sender<Event>,
    seen: Receiver<Event>,
}

impl Session {
    fn new(script: Script, tools: Tools, verdict: Verdict) -> Self {
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
    let mut session = Session::new(script, Tools::new(), Verdict::Deny);

    assert_eq!(session.turn("hi").unwrap(), StopReason::Yielded);

    assert_eq!(session.said(), "Hello, world");
    assert_eq!(
        session.runner.transcript().messages(),
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
    let mut session = Session::new(
        script,
        tools([Fixed::new("read").answering("fn main() {}")]),
        Verdict::AllowOnce,
    );

    assert_eq!(session.turn("read x").unwrap(), StopReason::Yielded);

    let messages = session.runner.transcript().messages();
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
    let mut session = Session::new(script, tools([Fixed::new("read")]), Verdict::AllowOnce);

    session.turn("go").unwrap();

    assert_eq!(
        session.asked(),
        [1, 3],
        "first the prompt; then the prompt, the call, and its result"
    );
}

#[test]
fn a_tool_the_user_refused_ends_the_turn_and_is_still_answered() {
    let script = Script::new(vec![calling("a", "write", "{}")]);
    let mut session = Session::new(
        script,
        tools([Fixed::new("write").risking(Sensitivity::MutatesFile)]),
        Verdict::Deny,
    );

    let problem = session.turn("write it").unwrap_err();

    assert_eq!(problem.to_string(), "write was not allowed");
    assert!(
        matches!(
            session.runner.transcript().messages().last(),
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
    let mut session = Session::new(script, tools([Fixed::new("read")]), Verdict::AllowOnce);

    assert_eq!(session.turn("go").unwrap(), StopReason::Cancelled);

    assert_eq!(
        session.runner.transcript().messages(),
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
    let mut session = Session::new(script, Tools::new(), Verdict::AllowOnce);

    assert_eq!(session.turn("go").unwrap(), StopReason::Yielded);

    assert_eq!(session.asked().len(), 1, "the model was asked again");
}

#[test]
fn a_provider_that_fails_ends_the_turn() {
    let mut session = Session::new(Script::failing(), Tools::new(), Verdict::AllowOnce);

    let problem = session.turn("go").unwrap_err();

    assert!(matches!(
        problem,
        TurnError::Provider(ProviderError::Refused { .. })
    ));
}

#[test]
fn turns_are_numbered_from_one() {
    let script = Script::new(vec![saying("first"), saying("second")]);
    let mut session = Session::new(script, Tools::new(), Verdict::AllowOnce);

    session.turn("one").unwrap();
    session.turn("two").unwrap();

    let started: Vec<u32> = session
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
fn the_tools_a_session_offers_are_advertised_on_every_request() {
    let script = Script::new(vec![saying("done")]);
    let mut session = Session::new(script, tools([Fixed::new("read")]), Verdict::AllowOnce);

    session.turn("go").unwrap();

    assert_eq!(session.advertised(), ["read"]);
}

#[test]
fn a_call_is_announced_before_it_runs() {
    // The renderer draws the line for a running tool from this.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut session = Session::new(script, tools([Fixed::new("read")]), Verdict::AllowOnce);

    session.turn("go").unwrap();

    let requested: Vec<ToolCall> = session
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
    let mut session = Session::new(script, Tools::new(), Verdict::AllowOnce);
    session.cancel.request();

    assert_eq!(session.turn("go").unwrap(), StopReason::Yielded);
}
