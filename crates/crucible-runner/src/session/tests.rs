//! What a session log keeps, and what it hands back.

use std::fs;
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::{Arc, Mutex};

use crucible_core::{
    Message, SessionId, StopReason, ToolArgs, ToolCall, ToolId, ToolOutput, ToolResult,
};
use serde_json::Value;

use super::claim::{Claimed, claim};
use super::{Session, SessionError, wire};
use crate::sample::Sample;

fn said(text: &str) -> Message {
    Message::User(text.into())
}

fn calling(id: &str, name: &str, args: &str) -> Message {
    Message::Agent {
        text: "on it".into(),
        calls: vec![ToolCall {
            id: ToolId::new(id),
            name: name.into(),
            args: ToolArgs::new(args),
        }],
        stop: Some(StopReason::WantsTools),
    }
}

fn answered(id: &str, output: ToolOutput) -> Message {
    Message::ToolResults(vec![ToolResult {
        id: ToolId::new(id),
        output,
    }])
}

/// Records `messages` in a fresh session and returns where it was written.
fn record(sample: &Sample, messages: &[Message]) -> PathBuf {
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let path = session.path().to_owned();

    for message in messages {
        session.append(message);
    }

    // Dropping is what waits for the queue, so the file is complete after it.
    drop(session);
    path
}

#[test]
fn finishing_reports_a_failure_the_loop_could_not_have_seen() {
    // `trouble` answers for what the writer thread has already reached. When a
    // session ends the last turn is still queued, so the write worth reporting
    // most is the one nothing has had a chance to look at yet. Holding the sink
    // is what makes that ordering a fact here rather than a race.
    let (release, held) = std::sync::mpsc::channel();
    let session = Session::writing(PathBuf::from("held.jsonl"), Blocked { held });

    session.append(&said("the last thing anyone said"));
    assert!(
        session.trouble().is_none(),
        "the writer is still blocked, so nothing can have been recorded yet"
    );

    release.send(()).expect("the writer is waiting");
    assert!(
        session.finish().is_some(),
        "the failure happened while the queue was draining"
    );
}

/// A log that fails, once the test lets it get that far.
struct Blocked {
    held: std::sync::mpsc::Receiver<()>,
}

impl std::io::Write for Blocked {
    fn write(&mut self, _line: &[u8]) -> std::io::Result<usize> {
        // Ends immediately once the sender is gone, which is how a session that
        // never releases it still finishes.
        let _ = self.held.recv();
        Err(std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            "no space left on device",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_line_a_failed_write_cut_short_is_ended_before_the_next_one_starts() {
    // A line reaches the file as its bytes and then the newline that ends it,
    // so a disk that fills between the two leaves a line with nothing after it.
    // Written straight onto, the next message makes one line that is neither —
    // damage in the middle of the log, which costs every turn recorded after it
    // rather than the one the disk cost.
    let written = Written::default();
    let session = Session::writing(
        PathBuf::from("filled.jsonl"),
        Filling {
            written: Arc::clone(&written),
            writes: 0,
            // The newline of the first line, and nothing else.
            fails_at: 2,
        },
    );

    session.append(&said("the line the disk cut short"));
    session.append(&said("the one after it"));
    assert!(session.finish().is_some(), "the failure is still reported");

    let log = String::from_utf8(written.lock().expect("the writer is gone").clone())
        .expect("a log of text");
    let lines: Vec<&str> = log.lines().collect();

    assert_eq!(lines.len(), 2, "{log}");
    assert!(
        lines.iter().all(|line| wire::message(line).is_some()),
        "{log}"
    );
}

/// What a log kept, shared with the test that reads it back.
type Written = Arc<Mutex<Vec<u8>>>;

/// A log that stops taking bytes part way through a line and then works again
/// — a disk that filled up and was freed while the session went on.
struct Filling {
    written: Written,
    writes: usize,
    /// Which write fails. Counted from one, and only one of them does.
    fails_at: usize,
}

impl std::io::Write for Filling {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.writes += 1;

        if self.writes == self.fails_at {
            return Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "no space left on device",
            ));
        }

        self.written
            .lock()
            .expect("the test is holding it")
            .extend_from_slice(bytes);

        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn every_message_reaches_the_file_in_the_order_it_happened() {
    let sample = Sample::new("session-order");

    let path = record(&sample, &[said("first"), said("second"), said("third")]);

    let written = fs::read_to_string(path).expect("the log");
    let lines: Vec<&str> = written.lines().collect();

    assert_eq!(lines.len(), 4, "a header and three messages: {written}");
    assert!(lines.get(1).is_some_and(|line| line.contains("first")));
    assert!(lines.get(3).is_some_and(|line| line.contains("third")));
}

#[test]
fn a_log_says_what_it_is_and_which_workspace_it_belongs_to() {
    let sample = Sample::new("session-header");

    let path = record(&sample, &[]);
    let written = fs::read_to_string(path).expect("the log");
    let header = written.lines().next().expect("a header");

    // Read back as what it is rather than searched as text. A path is written
    // into JSON escaped, so on a platform whose separator is the escape
    // character a substring check compares the two spellings of the same path
    // and reports the difference as a missing workspace.
    let header: Value = serde_json::from_str(header).expect("a header line of JSON");
    let root = sample.workspace().root().display().to_string();

    assert_eq!(
        header.get("format").and_then(Value::as_u64),
        Some(u64::from(wire::FORMAT)),
        "{header}"
    );
    assert_eq!(
        header.get("workspace").and_then(Value::as_str),
        Some(root.as_str()),
        "{header}"
    );
}

#[test]
fn a_session_comes_back_exactly_as_it_was_recorded() {
    // Arguments stay the text the model wrote and a failed result stays failed:
    // a transcript that comes back subtly different is a transcript the model
    // will answer differently.
    let sample = Sample::new("session-round-trip");
    let messages = vec![
        said("fix the parser"),
        calling("call-1", "read", r#"{"path":"src/main.rs","limit":40}"#),
        answered("call-1", ToolOutput::failed("src/main.rs does not exist")),
        said("try again"),
    ];

    record(&sample, &messages);

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages(), messages.as_slice());
}

#[test]
fn a_session_that_forgot_is_continued_from_where_it_started_again() {
    // What `/clear` leaves in the log. The marker is a line like any other, so
    // what was said before it is still on the disk — the log is the record of
    // what happened, and forgetting happened at a point in it — and none of it
    // comes back, because the model was never going to be told it again.
    let sample = Sample::new("session-forgot");
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");

    session.append(&said("what was said first"));
    session.forgot();
    session.append(&said("what was said after"));
    drop(session);

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages(), &[said("what was said after")]);
}

#[test]
fn what_a_session_forgot_is_still_in_the_file() {
    // Written down rather than cut out. A log that quietly lost a stretch of
    // what happened would be a worse record than one saying where the session
    // started again — and cutting it is not something an append-only log can do
    // safely while another line is on its way to the disk.
    let sample = Sample::new("session-forgot-record");
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let path = session.path().to_owned();

    session.append(&said("what was said first"));
    session.forgot();
    drop(session);

    let written = fs::read_to_string(path).expect("the log");
    let lines: Vec<&str> = written.lines().collect();

    assert_eq!(
        lines.len(),
        3,
        "a header, a message and the marker: {written}"
    );
    assert!(
        lines.get(1).is_some_and(|line| line.contains("first")),
        "{written}"
    );
    assert!(
        lines.get(2).is_some_and(|line| wire::forgets(line)),
        "{written}"
    );
}

#[test]
fn continuing_a_session_appends_to_the_same_log() {
    // A continued session is the same session. Starting a second file would
    // split one transcript across two, and the next `--continue` would find
    // only the half.
    let sample = Sample::new("session-append");
    let path = record(&sample, &[said("first")]);

    let (session, _) = Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    assert_eq!(session.path(), path);
    session.append(&said("second"));
    drop(session);

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    assert_eq!(transcript.messages().len(), 2);
}

#[test]
fn an_agent_line_with_a_stop_word_this_build_lacks_is_not_read_as_a_finish() {
    // A word from a build that spelled things differently cannot reach here —
    // the format in the header refuses that log outright. What can is a line
    // this build wrote before a reason was renamed under it. Read as a finish,
    // the session continues with a truncated turn in it that nothing marks, so
    // the safe reading is the one that says nobody knows.
    let sample = Sample::new("session-strange-stop");

    sample.plant(
        "0000000000001-000001",
        &[
            sample.header(wire::FORMAT, "0000000000001-000001"),
            r#"{"user":"go"}"#.to_owned(),
            r#"{"agent":"as I was say","calls":[],"stop":"something-new"}"#.to_owned(),
        ],
    );

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(
        transcript.messages().last(),
        Some(&Message::Agent {
            text: "as I was say".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Unknown),
        })
    );
}

#[test]
fn an_agent_line_with_no_stop_at_all_reads_as_an_answer_that_never_ended() {
    // What the runner writes for a response that broke off part way, and the
    // one reading that must not become a finish.
    let sample = Sample::new("session-no-stop");

    sample.plant(
        "0000000000001-000001",
        &[
            sample.header(wire::FORMAT, "0000000000001-000001"),
            r#"{"user":"go"}"#.to_owned(),
            r#"{"agent":"as I was say","calls":[],"stop":null}"#.to_owned(),
        ],
    );

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(
        transcript.messages().last(),
        Some(&Message::Agent {
            text: "as I was say".into(),
            calls: Vec::new(),
            stop: None,
        })
    );
}

#[test]
fn the_newest_session_for_this_workspace_is_the_one_continued() {
    let sample = Sample::new("session-newest");

    sample.plant(
        "0000000000001-000001",
        &[
            sample.header(wire::FORMAT, "old"),
            r#"{"user":"long ago"}"#.to_owned(),
        ],
    );
    record(&sample, &[said("just now")]);

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages(), &[said("just now")]);
}

#[test]
fn a_session_from_another_workspace_is_not_offered() {
    // Sessions share one directory, so the workspace in the header is the only
    // thing keeping one project's session out of another's.
    let sample = Sample::new("session-elsewhere");
    Session::start(&sample.logs(), &sample.elsewhere()).expect("a new session");

    let problem = Session::resume(&sample.logs(), &sample.workspace()).expect_err("nothing here");

    assert!(matches!(problem, SessionError::Nothing { .. }));
}

#[test]
fn nothing_to_continue_says_so_rather_than_starting_over() {
    let sample = Sample::new("session-empty");

    let problem = Session::resume(&sample.logs(), &sample.workspace()).expect_err("nothing here");

    assert_eq!(
        problem.to_string(),
        format!(
            "no earlier session for {}",
            sample.workspace().root().display()
        )
    );
}

#[test]
fn calls_nothing_ever_answered_do_not_come_back() {
    // The process died between asking for a tool and recording its result. A
    // provider is entitled to reject a transcript whose last word is a question
    // the transcript itself never answers.
    let sample = Sample::new("session-outstanding");
    record(
        &sample,
        &[said("check the tests"), calling("call-1", "bash", "{}")],
    );

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages(), &[said("check the tests")]);
}

#[test]
fn a_session_that_records_nothing_is_still_a_session() {
    let session = Session::nowhere();

    session.append(&said("into the void"));

    assert!(session.trouble().is_none());
    assert_eq!(session.path(), std::path::Path::new(""));
}

/// What a name something else already holds costs. Its own file because no
/// ordinary run reaches any of it, and what it guards is somebody else's log.
mod colliding;
