//! What the turn loop does, over a provider that answers from a script and
//! tools that answer from a field.

use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    Approved, ProviderError, Sensitivity, SessionId, Target, Tool, ToolArgs, ToolError, ToolId,
    ToolOutput, Verdict,
};

use super::*;
use crate::fake::{Fixed, Says, Script, Sent, changing};
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
                | Event::TurnFinished { .. }
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
                | Event::TurnFinished { .. }
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
                | Event::Failed { .. } => None,
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
fn the_second_request_carries_the_first_round_in_full() {
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
    // it: the user would have pressed Ctrl-C and watched the turn carry on.
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

// What a session keeps after it has forgotten what was said.
//
// The transcript is the one thing here that grows, and it is what every
// request carries. So what forgetting has to be is the next request going out
// smaller — not a flag somewhere saying it should.

#[test]
fn the_turn_after_a_session_forgot_carries_nothing_that_came_before_it() {
    // The whole of what `/clear` is for. A transcript that was cleared and then
    // sent anyway is a session that costs the same tokens and answers the same
    // questions, with a line on screen saying otherwise.
    let script = Script::new(vec![saying("first"), saying("second")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    scripted.turn("what is in main.rs?").unwrap();
    assert_eq!(scripted.runner.forget(), 2, "the prompt and the answer");

    scripted.turn("and in lib.rs?").unwrap();

    assert_eq!(
        scripted.asked(),
        [1, 1],
        "one prompt each: the second request carried nothing of the first turn"
    );
}

#[test]
fn the_turn_after_a_session_forgot_is_the_first_one_again() {
    // The count is what the user is told each turn. Numbering the next one
    // after a transcript that is no longer anywhere would name a turn nothing
    // on screen or in the log has any record of.
    let script = Script::new(vec![saying("first"), saying("second")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    scripted.turn("one").unwrap();
    scripted.runner.forget();
    scripted.turn("two").unwrap();

    assert_eq!(scripted.started(), [1, 1]);
}

#[test]
fn forgetting_a_session_that_has_said_nothing_forgets_nothing() {
    let script = Script::new(vec![saying("first")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    assert_eq!(scripted.runner.forget(), 0);
    assert!(scripted.runner.transcript().is_empty());
}

#[test]
fn a_session_that_forgot_is_continued_from_where_it_started_again() {
    // The log is what a continued session is built from, so forgetting has to
    // reach it: a transcript cleared only in memory comes back whole on the
    // next `--continue`, and the session the user cleared is the session they
    // get.
    let sample = Sample::new("runner-forgot");
    let script = Script::new(vec![saying("before"), saying("after")]);
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);

    scripted.turn("what came before").unwrap();
    scripted.runner.forget();
    scripted.turn("what came after").unwrap();

    // Dropping the runner drops the session, which is what waits for the queue.
    drop(scripted);
    let (_session, replayed) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(
        replayed.messages(),
        [
            Message::User("what came after".into()),
            Message::Agent {
                text: "after".into(),
                calls: Vec::new(),
                stop: Some(StopReason::Yielded),
            },
        ]
    );
}

// When a round reaches the log, measured against when its tools run.
//
// Recording is queued rather than written, so what a test can see from inside
// a running tool is the log as the disk has it — which is the only place the
// ordering shows. A tool that reads the log while the round is still going is
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
fn the_calls_of_a_round_are_recorded_before_the_tools_run() {
    // Running a tool is what changes the tree. A turn that ends part way
    // through a round — killed, or out of power — leaves a log whose last word
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
        "the round was still unrecorded while its tool ran: {seen}"
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
fn a_call_is_announced_before_it_runs() {
    // The renderer draws the line for a running tool from this.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);

    scripted.turn("go").unwrap();

    let requested: Vec<ToolCall> = scripted
        .seen
        .try_iter()
        .filter_map(|event| match event {
            Event::ToolRequested { call } => Some(call),
            Event::TurnStarted { .. }
            | Event::Delta { .. }
            | Event::ToolFinished { .. }
            | Event::TurnFinished { .. }
            | Event::Failed { .. } => None,
        })
        .collect();

    assert_eq!(requested.len(), 1);
    assert_eq!(requested.first().map(|call| &*call.name), Some("read"));
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

// What a runner put on a different session carries into the next turn.
//
// The transcript is the observable part — it is what every request holds — and
// the session handed back is the other half: a log is closed by consuming it,
// so a swap that dropped the old one would lose the last thing it had to say.

/// A session recorded and closed, holding one turn, and the name it has.
fn earlier(sample: &Sample) -> SessionId {
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let id = session
        .path()
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| SessionId::from_str(stem).ok())
        .expect("the log is named by its session");

    session.append(&Message::User("what came before".into()));
    session.append(&Message::Agent {
        text: "an answer from before".into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
    });

    // Dropping is what waits for the queue, so the log is complete after it.
    drop(session);
    id
}

/// The one this run is on, and what it was recording to before.
fn picking(scripted: &mut Scripted, sample: &Sample, id: &SessionId) -> Session {
    let (session, transcript) =
        Session::reopen(&sample.logs(), &sample.workspace(), id).expect("the session named");

    scripted.runner.pick_up(session, transcript)
}

#[test]
fn the_next_turn_is_asked_with_the_transcript_of_the_session_picked_up() {
    let sample = Sample::new("runner-picked-up");
    let id = earlier(&sample);

    let script = Script::new(vec![saying("here"), saying("and now")]);
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);

    scripted.turn("what is in main.rs?").unwrap();
    drop(picking(&mut scripted, &sample, &id));
    scripted.turn("and after that?").unwrap();

    assert_eq!(
        scripted.asked(),
        [1, 3],
        "the second request carried the earlier session's turn and the new prompt"
    );
    assert!(
        scripted
            .runner
            .transcript()
            .messages()
            .first()
            .is_some_and(|message| *message == Message::User("what came before".into())),
        "the transcript is the one picked up, not the one left behind"
    );
}

#[test]
fn the_turn_count_carries_on_from_the_session_picked_up() {
    // What the user is told each turn. A session continued at turn one says
    // this is a new one, which is the opposite of what was asked for.
    let sample = Sample::new("runner-picked-count");
    let id = earlier(&sample);

    let script = Script::new(vec![saying("here")]);
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);

    drop(picking(&mut scripted, &sample, &id));
    scripted.turn("and after that?").unwrap();

    assert_eq!(scripted.started(), [2]);
}

#[test]
fn the_session_left_behind_is_handed_back_rather_than_dropped() {
    // Closing one means consuming it, and the first write that failed is worth
    // saying while there is still a session on screen it belongs to.
    let sample = Sample::new("runner-picked-handed-back");
    let id = earlier(&sample);

    let script = Script::new(vec![saying("here")]);
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let was = session.path().to_owned();
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);

    let left = picking(&mut scripted, &sample, &id);

    assert_eq!(left.path(), was);
    assert_ne!(scripted.runner.session().path(), was);
    assert!(left.finish().is_none(), "nothing failed to be written");
}

#[test]
fn what_the_last_session_allowed_is_asked_about_again() {
    // "For the rest of this session" was answered about the session being left
    // behind. Carrying it across would run a tool in a session nobody was
    // asked about.
    let sample = Sample::new("runner-picked-permission");
    let id = earlier(&sample);

    let script = Script::new(vec![
        calling("a", "write", "{}"),
        saying("done"),
        calling("b", "write", "{}"),
        saying("done"),
        calling("c", "write", "{}"),
        saying("done"),
    ]);
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let mut scripted = Scripted::recording(
        script,
        tools([Fixed::new("write").risking(changing())]),
        Verdict::Allow,
        session,
    );
    scripted.says = Says::for_the_session();

    scripted.turn("write it").unwrap();
    scripted.turn("write it again").unwrap();
    assert_eq!(scripted.says.asked, 1, "the session allow held");

    drop(picking(&mut scripted, &sample, &id));
    scripted.turn("and once more").unwrap();

    assert_eq!(
        scripted.says.asked, 2,
        "the user answers the same way; what moved is the session it was for"
    );
}
