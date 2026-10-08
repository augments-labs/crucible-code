//! A call recorded as started and never answered, read back after a crash.
//!
//! The runner records a call as started immediately before it runs it, so a
//! started record with nothing after it is a call that may already have done
//! what it was asked. Whether it did is not something the log can say, and a
//! replay that dropped the pass would leave the model free to run it again.

use super::*;

/// What a started call with no recorded result is answered with, word for
/// word.
const MAY_HAVE_RUN: &str = "interrupted: this call started and no result was recorded; it may have taken effect. Check before running it again.";

/// What a call of a recovered pass that recorded nothing at all is answered
/// with.
const NOTHING_RECORDED: &str =
    "tool execution was interrupted before a durable result was recorded";

/// Records `call` as the runner does before it runs it: prepared, and then,
/// where `start` says so, started.
fn invoked(session: &Session, call: &ToolCall, start: bool) {
    let mut record = InvocationRecord::new(
        call.clone(),
        Ancestry::new(),
        ToolEffect::NonIdempotent,
        None,
    );
    session.append_journal(&RunItem::Invocation {
        record: record.clone(),
        preview: None,
    });
    if start {
        record.start().expect("a prepared record starts");
        session.append_journal(&RunItem::Invocation {
            record,
            preview: None,
        });
    }
}

fn asking(ids: &[&str]) -> Message {
    Message::Agent {
        continuation: None,
        text: "on it".into(),
        calls: ids.iter().map(|id| call(id)).collect(),
        stop: Some(StopReason::WantsTools),
    }
}

#[test]
fn a_started_call_with_no_result_comes_back_answered_as_possibly_run() {
    let sample = Sample::new("started-alone");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let path = session.path().to_owned();
    session.append(&said("delete the branch"));
    session.append(&asking(&["call-1"]));
    invoked(&session, &call("call-1"), true);
    drop(session);
    let before = fs::read(&path).expect("the log");

    let (continued, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    drop(continued);

    assert_eq!(
        transcript.messages(),
        &[
            said("delete the branch"),
            asking(&["call-1"]),
            answered("call-1", RecordedToolOutput::failed(MAY_HAVE_RUN)),
        ]
    );
    let after = fs::read(&path).expect("the log");
    assert!(
        after.starts_with(&before),
        "the log was cut back past the started call"
    );

    // The answer is on the log now, as an ordinary result line, so the next
    // resume reads it back and has nothing left to recover.
    let (_again, again) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    assert_eq!(again.messages(), transcript.messages());
    assert_eq!(
        fs::read(&path).expect("the log"),
        after,
        "a second resume recovered the pass again"
    );
}

#[test]
fn started_calls_of_a_pass_none_finished_are_kept_beside_the_one_never_started() {
    let sample = Sample::new("started-two-of-three");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let path = session.path().to_owned();
    session.append(&said("clean up"));
    session.append(&asking(&["call-1", "call-2", "call-3"]));
    invoked(&session, &call("call-1"), true);
    invoked(&session, &call("call-2"), true);
    drop(session);
    let before = fs::read(&path).expect("the log");

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    let answers = Message::ToolResults(vec![
        ToolResult {
            id: ToolId::new("call-1"),
            output: RecordedToolOutput::failed(MAY_HAVE_RUN),
        },
        ToolResult {
            id: ToolId::new("call-2"),
            output: RecordedToolOutput::failed(MAY_HAVE_RUN),
        },
        ToolResult {
            id: ToolId::new("call-3"),
            output: RecordedToolOutput::failed(NOTHING_RECORDED),
        },
    ]);
    assert_eq!(
        transcript.messages(),
        &[
            said("clean up"),
            asking(&["call-1", "call-2", "call-3"]),
            answers
        ]
    );
    assert!(
        fs::read(&path).expect("the log").starts_with(&before),
        "the log was cut back past the started calls"
    );
}

#[test]
fn a_pass_with_no_started_record_is_still_dropped() {
    // Prepared is the record written before a call is let run, so a pass that
    // got no further than that did nothing, and is cut as it always was.
    let sample = Sample::new("started-never");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let path = session.path().to_owned();
    session.append(&said("clean up"));
    session.append(&asking(&["call-1"]));
    invoked(&session, &call("call-1"), false);
    drop(session);
    let before = fs::read(&path).expect("the log");

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages(), &[said("clean up")]);
    assert!(
        fs::read(&path).expect("the log").len() < before.len(),
        "the unanswered pass was left on the log"
    );
}

#[test]
fn a_started_call_that_then_finished_is_answered_with_what_it_finished_with() {
    // Every call that ran to its end was started first, so a started record
    // must never stand in front of the finished one after it.
    let sample = Sample::new("started-then-finished");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    session.append(&said("check the tests"));
    session.append(&asking(&["call-1", "call-2"]));
    invoked(&session, &call("call-1"), true);
    ran(
        &session,
        &call("call-1"),
        RecordedToolOutput::ok("2 passed"),
    );
    invoked(&session, &call("call-2"), true);
    drop(session);

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(
        transcript.messages().get(2),
        Some(&Message::ToolResults(vec![
            ToolResult {
                id: ToolId::new("call-1"),
                output: RecordedToolOutput::ok("2 passed"),
            },
            ToolResult {
                id: ToolId::new("call-2"),
                output: RecordedToolOutput::failed(MAY_HAVE_RUN),
            },
        ]))
    );
}
