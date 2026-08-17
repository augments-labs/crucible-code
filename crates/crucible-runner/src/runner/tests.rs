//! What the turn loop does, over a provider that answers from a script and
//! tools that answer from a field.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    Approved, Change, Diff, Line, ProviderError, Sensitivity, SessionId, Spend, Summary, Target,
    Tool, ToolArgs, ToolError, ToolId, ToolOutput, Verdict,
};

use super::*;
use crate::fake::{Fixed, Says, Script, Sent, changing};
use crate::sample::Sample;

mod pick_up;

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
                    effort: None,
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
                Event::TurnStarted { .. }
                | Event::ToolRequested { .. }
                | Event::ToolFinished { .. }
                | Event::Retrying
                | Event::TurnFinished { .. }
                | Event::Spent { .. }
                | Event::Failed { .. } => None,
            })
            .collect()
    }

    /// Which turn each start announced, in order.
    fn started(&self) -> Vec<u32> {
        self.seen
            .try_iter()
            .filter_map(|event| match event {
                Event::TurnStarted { turn } => Some(turn.get()),
                Event::Delta { .. }
                | Event::ToolRequested { .. }
                | Event::ToolFinished { .. }
                | Event::Retrying
                | Event::TurnFinished { .. }
                | Event::Spent { .. }
                | Event::Failed { .. } => None,
            })
            .collect()
    }

    /// Why each turn ended, in order.
    fn finished(&self) -> Vec<StopReason> {
        self.seen
            .try_iter()
            .filter_map(|event| match event {
                Event::TurnFinished { stop, .. } => Some(stop),
                Event::TurnStarted { .. }
                | Event::Delta { .. }
                | Event::ToolRequested { .. }
                | Event::ToolFinished { .. }
                | Event::Retrying
                | Event::Spent { .. }
                | Event::Failed { .. } => None,
            })
            .collect()
    }

    /// What the turn had spent at each reading, in order.
    fn spent(&self) -> Vec<u64> {
        self.seen
            .try_iter()
            .filter_map(|event| match event {
                Event::Spent { spend } => Some(spend.tokens()),
                Event::TurnStarted { .. }
                | Event::Delta { .. }
                | Event::ToolRequested { .. }
                | Event::ToolFinished { .. }
                | Event::Retrying
                | Event::TurnFinished { .. }
                | Event::Failed { .. } => None,
            })
            .collect()
    }

    /// How many responses were asked for again.
    fn retried(&self) -> usize {
        self.seen
            .try_iter()
            .filter(|event| matches!(event, Event::Retrying))
            .count()
    }

    /// How much transcript each request carried, in order.
    fn asked(&self) -> Vec<usize> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.transcript_len)
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

fn many_calls(first: usize, count: usize) -> Vec<Delta> {
    let mut deltas = Vec::with_capacity(count.saturating_mul(2).saturating_add(1));
    for number in first..first.saturating_add(count) {
        deltas.push(Delta::ToolStarted {
            id: ToolId::new(number.to_string()),
            name: "missing".into(),
        });
        deltas.push(Delta::ToolArgs("{}".into()));
    }
    deltas.push(Delta::Stopped(StopReason::WantsTools));
    deltas
}

fn saying(text: &str) -> Vec<Delta> {
    vec![
        Delta::Text(text.into()),
        Delta::Stopped(StopReason::Yielded),
    ]
}

#[test]
fn what_a_turn_has_spent_is_every_response_of_it_added_up() {
    // Two responses, because that is where the two readings have to be told
    // apart: within one response the number is that response's total so far and
    // replaces the one before it, and across responses they add. Reading either
    // as the other gives a count that stalls or one that doubles, and on a row
    // watched while it moves both look like the truth.
    let script = Script::new(vec![
        vec![
            Delta::Spent(Spend::new(40)),
            Delta::ToolStarted {
                id: ToolId::new("a"),
                name: "read".into(),
            },
            Delta::ToolArgs("{}".into()),
            Delta::Spent(Spend::new(90)),
            Delta::Stopped(StopReason::WantsTools),
        ],
        vec![
            Delta::Text("found it".into()),
            Delta::Spent(Spend::new(30)),
            Delta::Stopped(StopReason::Yielded),
        ],
    ]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);

    scripted.turn("go").expect("the turn to finish");

    assert_eq!(scripted.spent(), [40, 90, 120]);
}

#[test]
fn a_turn_that_is_never_told_what_it_spent_says_nothing_about_it() {
    // Every provider reports this differently and one of them may not report it
    // at all. Nothing here invents a number for that case: no reading, no
    // event, and the row above the box has one segment fewer.
    let script = Script::new(vec![saying("done")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny);

    scripted.turn("go").expect("the turn to finish");

    assert!(scripted.spent().is_empty());
}

#[test]
fn how_hard_the_session_was_told_to_think_is_on_every_request() {
    // Every turn, not the first one. The loop asks again after each tool call,
    // and a rung that reached only the opening request would leave the thinking
    // the user paid for on the turn that did the least work.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);
    scripted.runner.model.effort = Some(Effort::Max);

    scripted.turn("go").expect("the turn to finish");

    let sent = scripted.sent.lock().unwrap();
    assert_eq!(sent.len(), 2, "one request, then one after the tool ran");
    assert!(
        sent.iter()
            .all(|request| request.effort == Some(Effort::Max)),
        "a request went out without it: {sent:?}"
    );
}

#[test]
fn a_provider_handed_over_mid_session_is_the_one_the_next_turn_is_sent_to() {
    // The half a key given to `/login` needs: a run with no credential resolves
    // the provider that answers nothing, and until it can be replaced that run
    // refuses every turn no matter what it is handed afterwards.
    let first = Script::new(vec![saying("from the first")]);
    let mut scripted = Scripted::new(first, tools([]), Verdict::Allow);

    scripted.turn("go").expect("the turn to finish");

    let second = Script::new(vec![saying("from the second")]);
    let after = second.sent();
    scripted.runner.serve(Box::new(second));
    scripted.turn("again").expect("the turn to finish");

    assert_eq!(
        scripted.sent.lock().unwrap().len(),
        1,
        "the one it started on"
    );
    assert_eq!(after.lock().unwrap().len(), 1, "the one it was handed");

    // And what was said before the swap goes with it. A vendor is who a
    // transcript is sent to, not something a transcript belongs to.
    let sent = after.lock().unwrap();
    let carried = sent.first().expect("the request it was just handed");
    assert!(
        carried.carried("from the first"),
        "the first provider's answer was not carried"
    );
}

#[test]
fn the_vendor_a_session_names_is_the_one_it_would_write_to_now() {
    // What a status row is drawn from. `/login` hands over a provider mid
    // session, so a name remembered beside the provider rather than read off
    // it would go on naming the vendor the session opened with — and the row
    // saying that is the row somebody checks before sending anything.
    let script = Script::new(vec![saying("answered")]);
    let mut scripted = Scripted::new(script, tools([]), Verdict::Allow);

    assert_eq!(scripted.runner.serving(), "script");

    scripted.runner.serve(Box::new(Elsewhere));

    assert_eq!(scripted.runner.serving(), ELSEWHERE);
}

/// A provider that answers nothing, under a name of its own.
///
/// Every other provider here is called the same thing, and one assertion needs
/// two that can be told apart.
struct Elsewhere;

/// What it calls itself.
const ELSEWHERE: &str = "elsewhere";

impl Provider for Elsewhere {
    fn name(&self) -> &'static str {
        ELSEWHERE
    }

    fn stream(
        &self,
        _request: Request<'_>,
        _cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        Err(ProviderError::Transport {
            provider: ELSEWHERE,
            problem: "nothing is there".into(),
        })
    }
}

#[test]
fn a_rung_asked_for_mid_session_is_on_the_next_request_and_not_the_last_one() {
    // The half `/effort` needs: a session opens on whatever the command line
    // and the files settled, and what is chosen afterwards has to reach the
    // wire without ending the session to do it.
    let script = Script::new(vec![saying("first"), saying("second")]);
    let mut scripted = Scripted::new(script, tools([]), Verdict::Allow);

    assert_eq!(scripted.runner.effort(), None, "nothing has said yet");
    scripted.turn("go").expect("the turn to finish");

    scripted.runner.think(Effort::Low);
    assert_eq!(scripted.runner.effort(), Some(Effort::Low));
    scripted.turn("again").expect("the turn to finish");

    let sent = scripted.sent.lock().unwrap();
    let asked: Vec<Option<Effort>> = sent.iter().map(|request| request.effort).collect();
    assert_eq!(asked, [None, Some(Effort::Low)]);
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
                stop: Some(StopReason::Yielded),
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
        Verdict::Allow,
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
fn the_second_request_carries_the_first_pass_in_full() {
    // Without it the model answers the same question again, having no
    // record of the tool it just called.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);

    scripted.turn("go").unwrap();

    assert_eq!(
        scripted.asked(),
        [1, 3],
        "first the prompt; then the prompt, the call, and its result"
    );
}

#[test]
fn a_turn_cannot_keep_asking_the_provider_forever() {
    let rounds = (0..MAX_PROVIDER_RESPONSES_PER_TURN)
        .map(|number| calling(&number.to_string(), "missing", "{}"))
        .collect();
    let mut scripted = Scripted::new(Script::new(rounds), Tools::new(), Verdict::Allow);

    let problem = scripted.turn("go").unwrap_err();

    assert!(matches!(
        problem,
        TurnError::Provider(ProviderError::Limit {
            limit: ProviderLimit::ProviderResponses,
            maximum: MAX_PROVIDER_RESPONSES_PER_TURN,
            ..
        })
    ));
    assert_eq!(scripted.asked().len(), MAX_PROVIDER_RESPONSES_PER_TURN);
}

#[test]
fn tool_calls_are_bounded_across_every_provider_response_in_a_turn() {
    let script = Script::new(vec![many_calls(0, 64), many_calls(64, 65)]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    let problem = scripted.turn("go").unwrap_err();

    assert!(matches!(
        problem,
        TurnError::Provider(ProviderError::Limit {
            limit: ProviderLimit::TurnToolCalls,
            maximum: MAX_TOOL_CALLS_PER_TURN,
            ..
        })
    ));
}

#[test]
fn tool_results_share_one_retained_boundary_across_a_turn() {
    let script = Script::new(vec![calling("a", "read", "{}")]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("read").answering("ninebytes")]),
        Verdict::Allow,
    );

    let problem = scripted
        .runner
        .exchange_with_tool_output_limit(&mut scripted.says, &scripted.events, &scripted.cancel, 8)
        .unwrap_err();

    assert!(matches!(problem, TurnError::ToolOutputBytes { maximum: 8 }));
    assert!(matches!(
        scripted.runner.transcript().messages().last(),
        Some(Message::ToolResults(results)) if results.len() == 1
    ));
}

#[test]
fn a_tool_the_user_refused_ends_the_turn_and_is_still_answered() {
    let script = Script::new(vec![calling("a", "write", "{}")]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("write").risking(changing())]),
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
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Cancelled);

    assert_eq!(
        scripted.runner.transcript().messages(),
        [
            Message::User("go".into()),
            Message::Agent {
                text: "looking".into(),
                calls: Vec::new(),
                stop: Some(StopReason::Cancelled),
            },
        ]
    );
}

#[test]
fn a_model_that_wants_tools_but_names_none_yields_instead_of_asking_again() {
    // An unchanged transcript sent again produces the same answer again.
    let script = Script::new(vec![vec![Delta::Stopped(StopReason::WantsTools)]]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Yielded);

    assert_eq!(scripted.asked().len(), 1, "the model was asked again");
}

#[test]
fn a_provider_that_fails_ends_the_turn() {
    let mut scripted = Scripted::new(Script::failing(), Tools::new(), Verdict::Allow);

    let problem = scripted.turn("go").unwrap_err();

    assert!(matches!(
        problem,
        TurnError::Provider(ProviderError::Refused { .. })
    ));

    // A key without access says the same thing however many times it is asked,
    // so asking again spends the user's time to reach the same message.
    assert_eq!(scripted.asked().len(), 1, "the request went out again");
    assert_eq!(scripted.retried(), 0);
}

#[test]
fn an_answer_the_connection_broke_off_is_still_in_the_transcript() {
    // Those deltas were posted as they arrived, so the user has read them.
    // Dropping them leaves a transcript the user and the model disagree about:
    // the next prompt follows the last one with nothing in between, and every
    // request for the rest of the session — and every continuation of it —
    // carries the two questions back to back.
    let script = Script::breaking(vec![vec![
        Delta::Text("let me look at ".into()),
        Delta::Text("src/main.rs".into()),
    ]]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    let problem = scripted.turn("what is in main.rs?").unwrap_err();

    assert!(
        matches!(
            problem,
            TurnError::Provider(ProviderError::Transport { .. })
        ),
        "{problem}"
    );
    assert_eq!(
        scripted.runner.transcript().messages(),
        [
            Message::User("what is in main.rs?".into()),
            Message::Agent {
                text: "let me look at src/main.rs".into(),
                calls: Vec::new(),
                stop: None,
            },
        ]
    );

    // And is never asked for again. The deltas are on screen; a second answer
    // would be written under the half of the first one the user already read.
    assert_eq!(scripted.asked().len(), 1, "the request went out again");
    assert_eq!(scripted.retried(), 0);
}

#[test]
fn a_response_that_went_away_before_it_said_anything_is_asked_for_again() {
    // The failure this exists for: a connection the provider closed while the
    // tools ran. The request is accepted, the stream produces nothing at all,
    // and the turn that would have ended there instead asks once more.
    let script = Script::dropping(1, vec![saying("done")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Yielded);

    assert_eq!(scripted.retried(), 1);
    assert_eq!(scripted.asked().len(), 2, "the request went out once");

    // Nothing of the attempt that went away is left behind: an empty agent
    // message here is one the next request carries, and every request after it.
    assert_eq!(
        scripted.runner.transcript().messages(),
        [
            Message::User("go".into()),
            Message::Agent {
                text: "done".into(),
                calls: Vec::new(),
                stop: Some(StopReason::Yielded),
            },
        ]
    );
}

#[test]
fn a_response_that_keeps_going_away_ends_the_turn() {
    let script = Script::dropping(usize::MAX, Vec::new());
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    let problem = scripted.turn("go").unwrap_err();

    assert!(
        matches!(
            problem,
            TurnError::Provider(ProviderError::Transport { .. })
        ),
        "{problem}"
    );

    // Bounded, and the bound is the constant rather than a number written out
    // here: what this pins is that the loop stops rather than how soon.
    assert_eq!(scripted.retried(), usize::from(RETRIES));
    assert_eq!(scripted.asked().len(), 1 + usize::from(RETRIES));
}

#[test]
fn a_service_that_says_it_is_busy_is_asked_again_and_a_key_without_access_is_not() {
    // Both are refusals and only the status tells them apart, which is the whole
    // of what `transient` decides: 503 is about the moment, 401 about the key.
    let mut busy = Scripted::new(Script::refusing(503), Tools::new(), Verdict::Allow);
    busy.turn("go").unwrap_err();
    assert_eq!(busy.asked().len(), 1 + usize::from(RETRIES));

    let mut refused = Scripted::new(Script::refusing(403), Tools::new(), Verdict::Allow);
    refused.turn("go").unwrap_err();
    assert_eq!(refused.asked().len(), 1);
}

#[test]
fn asking_to_stop_during_the_pause_stops_the_retry() {
    // The pause is the one place a turn waits with nothing arriving, so it is
    // the one place Esc could be swallowed. Measured from when the cancel was
    // raised, because when the thread that raises it gets to run is not this
    // loop's doing.
    let script = Script::dropping(usize::MAX, Vec::new());
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    let cancel = scripted.cancel.clone();
    let (report, when) = channel();
    let esc = thread::spawn(move || {
        thread::sleep(CANCEL_SLICE);
        let at = Instant::now();
        cancel.request();
        report.send(at).unwrap();
    });

    scripted.turn("go").unwrap_err();
    let returned = Instant::now();
    esc.join().unwrap();

    let raised_at = when.recv().unwrap();
    let waited = returned.saturating_duration_since(raised_at);
    assert!(
        waited <= CANCEL_SLICE * 5,
        "the pause held for {waited:?} after the cancel was raised"
    );
    assert!(
        scripted.asked().len() < 1 + usize::from(RETRIES),
        "every attempt went out anyway"
    );
}

#[test]
fn the_tools_a_runner_offers_are_advertised_on_every_request() {
    let script = Script::new(vec![saying("done")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);

    scripted.turn("go").unwrap();

    assert_eq!(scripted.advertised(), ["read"]);
}

#[test]
fn a_turn_that_finds_the_flag_raised_stops_without_sending_anything() {
    // The press arrived after the caller cleared the flag and before this
    // thread reached its first instruction. Clearing it here instead would wipe
    // it: the user would have pressed Esc and watched the turn carry on.
    let script = Script::new(vec![saying("done")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.cancel.request();

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Cancelled);

    assert!(
        scripted.asked().is_empty(),
        "a request went out for a turn the user had already stopped"
    );
    assert_eq!(
        scripted.finished(),
        [StopReason::Cancelled],
        "the turn ended without saying so"
    );
    assert!(
        scripted.runner.transcript().is_empty(),
        "a turn that never ran recorded a prompt the model was never told"
    );
}

#[test]
fn the_number_a_stopped_turn_announced_is_the_one_the_next_turn_takes() {
    // The count follows the transcript, and a turn stopped before it began adds
    // nothing to it. So the number it announced is still free, and the prompt
    // after it is that turn — taken for real this time. Numbering the next one
    // higher would leave a gap nothing in the log or on screen accounts for.
    let script = Script::new(vec![saying("first"), saying("second")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    scripted.turn("one").unwrap();

    scripted.cancel.request();
    scripted.turn("stopped on the way in").unwrap();

    // What the loop does before it hands the next turn over, and the reason the
    // runner does not do it for itself.
    scripted.cancel.reset();
    scripted.turn("two").unwrap();

    assert_eq!(scripted.started(), [1, 2, 2]);
}

#[test]
fn a_turn_leaves_the_flag_to_the_thread_that_raises_it() {
    // Clearing it is the caller's, on the thread reading the keyboard, before
    // this turn's thread exists. A turn that cleared it as well would be
    // clearing whatever arrived in between, which is the one press nothing else
    // can see.
    let script = Script::new(vec![saying("done")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.cancel.request();

    scripted.turn("go").unwrap();

    assert!(
        scripted.cancel.requested(),
        "the turn cleared a request that was not made of it"
    );
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
        Verdict::Allow,
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

// When a pass reaches the log, measured against when its tools run.
//
// Recording is queued rather than written, so what a test can see from inside
// a running tool is the log as the disk has it — which is the only place the
// ordering shows. A tool that reads the log while the pass is still going is
// how that becomes an observation rather than a claim about the source.

/// The tool whose call the log is watched for.
///
/// A word that appears nowhere else in the turn, so finding it in the log can
/// only mean the call was recorded.
const WATCH: &str = "watch";

/// How long the tool waits for what was queued to reach the disk.
///
/// The write happens on the session's own thread, so a log that has not caught
/// up yet is slow rather than wrong. Long enough that a loaded machine does not
/// report the delay as a record that was never made, and bounded so that a
/// record which is never made fails instead of hanging.
const SETTLE: Duration = Duration::from_secs(5);

#[test]
fn the_calls_of_a_pass_are_recorded_before_the_tools_run() {
    // Running a tool is what changes the tree. A turn that ends part way
    // through a pass — killed, or out of power — leaves a log whose last word
    // is the prompt, and the next `--continue` hands the model a transcript in
    // which files it has already edited have never been touched. Recording the
    // calls first costs a line the replay knows how to drop; recording them
    // last costs the work.
    let sample = Sample::new("runner-recorded");
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let log = session.path().to_owned();

    let mut offered = Tools::new();
    offered.add(Box::new(Watching { log }));

    let script = Script::new(vec![calling("a", WATCH, "{}"), saying("done")]);
    let mut scripted = Scripted::recording(script, offered, Verdict::Allow, session);

    scripted.turn("go").expect("the turn");

    let messages = scripted.runner.transcript().messages();
    let seen = match messages.get(2) {
        Some(Message::ToolResults(results)) => results
            .first()
            .map(|result| result.output.text().to_owned()),
        Some(Message::User(_) | Message::Agent { .. }) | None => None,
    }
    .expect("the tool ran and its result was recorded");

    assert!(
        seen.contains(WATCH),
        "the pass was still unrecorded while its tool ran: {seen}"
    );
}

/// A tool that hands back the session log as it stood while the tool ran.
struct Watching {
    log: PathBuf,
}

impl Tool for Watching {
    fn name(&self) -> &'static str {
        WATCH
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("")
    }

    fn run(&self, _approved: Approved) -> Result<ToolOutput, ToolError> {
        let deadline = Instant::now() + SETTLE;

        loop {
            let held = std::fs::read_to_string(&self.log).unwrap_or_default();

            if held.contains(WATCH) || Instant::now() >= deadline {
                return Ok(ToolOutput::ok(held));
            }

            thread::sleep(Duration::from_millis(1));
        }
    }
}

// What a turn tells the thread that draws.
//
// Separate from the transcript tests next door because the two can disagree:
// a turn that recorded everything correctly and reported none of it leaves a
// session that is right in the log and wrong on the screen.

#[test]
fn turns_are_numbered_from_one() {
    let script = Script::new(vec![saying("first"), saying("second")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    scripted.turn("one").unwrap();
    scripted.turn("two").unwrap();

    assert_eq!(scripted.started(), [1, 2]);
}

#[test]
fn a_continued_session_goes_on_counting_where_it_stopped() {
    // Numbering the first continued turn 1 would tell the user this is a new
    // session, which is exactly what they asked it not to be.
    let script = Script::new(vec![saying("still here")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    let mut earlier = Transcript::new();
    earlier.push(Message::User("one".into()));
    earlier.push(Message::Agent {
        text: "first".into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
    });
    earlier.push(Message::User("two".into()));
    earlier.push(Message::Agent {
        text: "second".into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
    });
    scripted.runner = scripted.runner.resuming(earlier);

    scripted.turn("three").unwrap();

    assert_eq!(scripted.started(), [3]);
}

#[test]
fn a_call_is_announced_before_it_runs_with_what_it_is_about() {
    // The renderer draws the line for a running tool from this, and the words
    // beside the name come from the tool rather than from the renderer: only
    // the tool knows which of its arguments the call is about. `Fixed` answers
    // with the whole of them, which is a value nothing else here produces.
    let asked = r#"{"path":"src/main.rs"}"#;
    let script = Script::new(vec![calling("a", "read", asked), saying("done")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);

    scripted.turn("go").unwrap();

    let requested: Vec<(ToolCall, Summary)> = scripted
        .seen
        .try_iter()
        .filter_map(|event| match event {
            Event::ToolRequested { call, summary } => Some((call, summary)),
            Event::TurnStarted { .. }
            | Event::Delta { .. }
            | Event::ToolFinished { .. }
            | Event::Retrying
            | Event::TurnFinished { .. }
            | Event::Spent { .. }
            | Event::Failed { .. } => None,
        })
        .collect();

    assert_eq!(requested.len(), 1);
    let (call, summary) = requested.first().expect("the call to have been announced");
    assert_eq!(&*call.name, "read");
    assert_eq!(summary.as_str(), asked);
}

#[test]
fn a_turn_reports_why_it_stopped() {
    // The reason is the only thing that separates a finished answer from one
    // that was cut off: both leave prose in the transcript and hand the prompt
    // back. Returning it to the caller is not enough on its own — the thread
    // that draws never sees a return value.
    let script = Script::new(vec![vec![
        Delta::Text("as I was say".into()),
        Delta::Stopped(StopReason::OutOfTokens),
    ]]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    assert_eq!(scripted.turn("go").unwrap(), StopReason::OutOfTokens);

    assert_eq!(scripted.finished(), [StopReason::OutOfTokens]);
}

#[test]
fn a_turn_that_was_cut_off_comes_back_from_a_replay_still_cut_off() {
    // The live notice covers the session; the log is what covers the restart.
    // Without the reason on the line, the user hits the ceiling mid-sentence,
    // quits, continues, and replay hands the half-sentence back as a finished
    // turn — so the model is shown its own truncation as an answer it chose to
    // end.
    let sample = Sample::new("runner-cut-off");
    let script = Script::new(vec![vec![
        Delta::Text("as I was say".into()),
        Delta::Stopped(StopReason::OutOfTokens),
    ]]);
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);

    scripted.turn("write it all out").unwrap();

    // Dropping the runner drops the session, which is what waits for the queue.
    drop(scripted);
    let (_session, replayed) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(
        replayed.messages(),
        [
            Message::User("write it all out".into()),
            Message::Agent {
                text: "as I was say".into(),
                calls: Vec::new(),
                stop: Some(StopReason::OutOfTokens),
            },
        ]
    );
}

#[test]
fn a_stream_that_never_said_why_it_stopped_fails_the_turn_rather_than_finishing_it() {
    // Silence is what a finished response and one that stopped arriving have in
    // common. Read as a finish, half an answer reaches the user looking whole —
    // and reaches the model that way on every turn afterwards. Both providers
    // here prevent it; this is what a third one that forgot would meet.
    let script = Script::new(vec![vec![Delta::Text("as I was say".into())]]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    let problem = scripted.turn("go").unwrap_err();

    assert!(
        matches!(problem, TurnError::Provider(ProviderError::Protocol { .. })),
        "{problem:?}"
    );

    // What the user already read is still recorded, and it is recorded as an
    // answer that never reached an ending.
    assert_eq!(
        scripted.runner.transcript().messages(),
        [
            Message::User("go".into()),
            Message::Agent {
                text: "as I was say".into(),
                calls: Vec::new(),
                stop: None,
            },
        ]
    );
    assert_eq!(scripted.finished(), [], "a failed turn has no ending");
}

#[test]
fn a_turn_a_tool_round_ended_reports_why_as_well() {
    // Two returns end a turn. A reason posted from only one of them leaves the
    // other looking finished, which is the whole failure this guards.
    let script = Script::new(vec![calling("a", "bash", "{}")]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("bash").cancelling()]),
        Verdict::Allow,
    );

    assert_eq!(scripted.turn("go").unwrap(), StopReason::Cancelled);

    assert_eq!(scripted.finished(), [StopReason::Cancelled]);
}

#[test]
fn a_turn_that_failed_reports_no_reason_because_it_reached_none() {
    // The failure is its own event. Posting a stop as well would put two
    // endings on the screen for one turn.
    let mut scripted = Scripted::new(Script::failing(), Tools::new(), Verdict::Allow);

    scripted.turn("go").unwrap_err();

    assert_eq!(scripted.finished(), []);
}

#[test]
fn a_diff_reaches_the_reader_and_stops_before_the_transcript() {
    // The one thing here that goes to one of the two and not the other. A diff
    // is drawn once; the transcript is replayed to the model every turn for the
    // rest of the session, so a copy kept there would be paid for again on
    // every turn after the edit it describes -- and paid for in the one value
    // that is allowed to grow, against a bound that counts what was said.
    let diff = Diff::new([Line::new(315, Change::Added, "budgets:")]);
    let script = Script::new(vec![calling("a", "edit", "{}"), saying("done")]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("edit").showing(diff)]),
        Verdict::Allow,
    );

    scripted.turn("go").unwrap();

    let shown: Vec<Option<usize>> = scripted
        .seen
        .try_iter()
        .filter_map(|event| match event {
            Event::ToolFinished { output, .. } => Some(output.diff().map(Diff::added)),
            Event::TurnStarted { .. }
            | Event::Delta { .. }
            | Event::ToolRequested { .. }
            | Event::Retrying
            | Event::TurnFinished { .. }
            | Event::Spent { .. }
            | Event::Failed { .. } => None,
        })
        .collect();

    assert_eq!(shown, [Some(1)]);

    let kept: Vec<Option<&Diff>> = scripted
        .runner
        .transcript()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults(results) => Some(results),
            Message::User(_) | Message::Agent { .. } => None,
        })
        .flatten()
        .map(|result| result.output.diff())
        .collect();

    assert_eq!(kept, [None]);
}
