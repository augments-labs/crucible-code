//! Event reporting and recording contracts for the turn loop.

use super::*;

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
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
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
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let log = session.path().to_owned();

    let mut offered = Tools::new();
    offered.add_builtin(Logged { log }).unwrap();

    let script = Script::new(vec![calling("a", WATCH, "{}"), saying("done")]);
    let mut scripted = Scripted::recording(script, offered, Verdict::Allow, session);

    scripted.turn("go").expect("the turn");

    let messages = conversation(scripted.runner.transcript());
    let seen = match messages.get(2) {
        Some(Message::ToolResults(results)) => results
            .first()
            .map(|result| result.output.text().to_owned()),
        Some(Message::Context(_) | Message::User { .. } | Message::Agent { .. }) | None => None,
    }
    .expect("the tool ran and its result was recorded");

    assert!(
        seen.contains(WATCH),
        "the pass was still unrecorded while its tool ran: {seen}"
    );
}

/// A tool that hands back the session log as it stood while the tool ran.
struct Logged {
    log: PathBuf,
}

impl DescribeTool for Logged {
    fn name(&self) -> &str {
        WATCH
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Logged {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("")
    }

    fn run(
        &self,
        _approved: Approved,
        _context: &ToolContext<'_>,
    ) -> Result<ToolOutput, ToolError> {
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
    earlier
        .push(Message::said("one"))
        .expect("valid fixture transcript");
    earlier
        .push(Message::Agent {
            continuation: None,
            text: "first".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        })
        .expect("valid fixture transcript");
    earlier
        .push(Message::said("two"))
        .expect("valid fixture transcript");
    earlier
        .push(Message::Agent {
            continuation: None,
            text: "second".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        })
        .expect("valid fixture transcript");
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
            Event::ToolRequested { call, summary, .. } => Some((call, summary)),
            Event::TurnStarted { .. }
            | Event::PromptCache { .. }
            | Event::Sandbox { .. }
            | Event::Delta { .. }
            | Event::ToolFinished { .. }
            | Event::Wrote { .. }
            | Event::Carried { .. }
            | Event::Compacting { .. }
            | Event::Compacted { .. }
            | Event::Retrying
            | Event::Aged { .. }
            | Event::Unread { .. }
            | Event::Steered { .. }
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
fn a_call_is_announced_with_its_execution_capabilities() {
    let script = Script::new(vec![calling("a", "detachable", "{}"), saying("done")]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("detachable").detachable()]),
        Verdict::Allow,
    );

    scripted.turn("go").unwrap();

    let backgroundable = scripted.seen.try_iter().find_map(|event| match event {
        Event::ToolRequested { backgroundable, .. } => Some(backgroundable),
        Event::TurnStarted { .. }
        | Event::PromptCache { .. }
        | Event::Sandbox { .. }
        | Event::Delta { .. }
        | Event::ToolFinished { .. }
        | Event::Wrote { .. }
        | Event::Carried { .. }
        | Event::Compacting { .. }
        | Event::Compacted { .. }
        | Event::Retrying
        | Event::Aged { .. }
        | Event::Unread { .. }
        | Event::Steered { .. }
        | Event::TurnFinished { .. }
        | Event::Spent { .. }
        | Event::Failed { .. } => None,
    });

    assert_eq!(backgroundable, Some(true));
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
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);

    scripted.turn("write it all out").unwrap();

    // Dropping the runner drops the session, which is what waits for the queue.
    drop(scripted);
    let (_session, replayed) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(
        conversation(&replayed),
        [
            Message::said("write it all out"),
            Message::Agent {
                continuation: None,
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
        conversation(scripted.runner.transcript()),
        [
            Message::said("go"),
            Message::Agent {
                continuation: None,
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
            | Event::PromptCache { .. }
            | Event::Sandbox { .. }
            | Event::Delta { .. }
            | Event::ToolRequested { .. }
            | Event::Wrote { .. }
            | Event::Carried { .. }
            | Event::Compacting { .. }
            | Event::Compacted { .. }
            | Event::Retrying
            | Event::Aged { .. }
            | Event::Unread { .. }
            | Event::Steered { .. }
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
            Message::Context(_) | Message::User { .. } | Message::Agent { .. } => None,
        })
        .flatten()
        .map(|result| result.output.diff())
        .collect();

    assert_eq!(kept, [None]);
}

#[test]
fn a_command_that_ended_while_the_turn_ran_reaches_it_at_the_next_pass() {
    // The case the whole type is for. A command left running exits mid-turn;
    // the agent was told not to poll for it, so the fact has to be pushed at
    // the turn, and the pass after it is pushed has to carry it. Without this
    // the model is told at the top of the *next* turn — which is no use to an
    // agent that is waiting inside this one.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut steering = Steering::new(script, tools([Fixed::new("read")]));
    steering
        .aside
        .say("#1 `sleep 5` finished after printing 0 lines.".into());

    steering.turn("start the build").expect("a turn");

    let asked = steering.asked();
    let [first, second] = asked.as_slice() else {
        panic!("two passes: {asked:?}");
    };
    assert!(
        second > first,
        "the pass after the note carries it: {asked:?}"
    );
    assert!(
        !steering.aside.any(),
        "a note the turn took is a note nothing still owes"
    );
}

#[test]
fn a_note_handed_to_a_turn_is_not_recorded_as_something_the_reader_typed() {
    // An aside is the harness speaking, not the reader. It joins the transcript
    // because that is the only channel a running turn has, but it must not be
    // drawn back as the reader's own words — `Steered` is what the panel and
    // the transcript read to decide somebody typed something.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut steering = Steering::new(script, tools([Fixed::new("read")]));
    steering
        .aside
        .say("#1 `sleep 5` finished after printing 0 lines.".into());

    steering.turn("start the build").expect("a turn");

    let steered: Vec<String> = steering
        .seen
        .try_iter()
        .filter_map(|event| match event {
            Event::Steered { line } => Some(line.clone()),
            _ => None,
        })
        .collect();
    assert!(
        steered.is_empty(),
        "a machine note was drawn as the reader's typing: {steered:?}"
    );
}

#[test]
fn a_note_and_a_typed_line_both_land_on_the_pass_that_follows_them() {
    // The two queues are drained at the same boundary and neither swallows the
    // other: a command that ended while the reader was typing is one turn that
    // knows both.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut steering = Steering::new(script, tools([Fixed::new("read")]));
    steering.steer.say("check the log too".into());
    steering
        .aside
        .say("#1 `sleep 5` finished after printing 0 lines.".into());

    steering.turn("start the build").expect("a turn");

    assert!(!steering.steer.any());
    assert!(!steering.aside.any());

    // Both are in the transcript, and as two messages rather than one run
    // together: what the reader asked for and what the machine reported are
    // different kinds of fact, and a model reading them spliced would read the
    // note as part of the request.
    let said: Vec<&str> = steering
        .runner
        .transcript()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::User { text, .. } => Some(text.as_ref()),
            _ => None,
        })
        .collect();
    assert!(
        said.contains(&"check the log too"),
        "the typed line is missing: {said:?}"
    );
    assert!(
        said.contains(&"#1 `sleep 5` finished after printing 0 lines."),
        "the note is missing: {said:?}"
    );
}
