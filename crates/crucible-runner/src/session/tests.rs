//! What a session log keeps, and what it hands back.

use std::fs;
use std::path::PathBuf;

use crucible_core::{Message, ToolArgs, ToolCall, ToolId, ToolOutput, ToolResult};

use super::{Session, SessionError};
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
fn a_log_is_readable_only_by_the_user_who_started_it() {
    use std::os::unix::fs::PermissionsExt as _;

    // A transcript carries what was typed, the contents of files that were
    // read and everything a command printed. On a shared machine the default
    // 0644 hands all of it to anyone with an account.
    let sample = Sample::new("session-mode");
    let path = record(&sample, &[said("hello")]);

    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "log is {:o}", mode & 0o777);

    let directory = fs::metadata(sample.logs()).unwrap().permissions().mode();
    assert_eq!(directory & 0o077, 0, "directory is {:o}", directory & 0o777);
}

#[test]
fn a_sessions_directory_left_open_is_narrowed_before_anything_is_written_to_it() {
    use std::os::unix::fs::PermissionsExt as _;

    // `DirBuilderExt::mode` applies only when the call creates the directory, so
    // one made by an earlier build — or by a hand running `mkdir -p` — keeps
    // whatever the umask gave it, and every log written into it afterwards sits
    // somewhere the whole machine can list.
    let sample = Sample::new("session-widened-directory");
    fs::set_permissions(sample.logs(), fs::Permissions::from_mode(0o755)).expect("a temporary dir");

    record(&sample, &[said("hello")]);

    let mode = fs::metadata(sample.logs()).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "directory is {:o}", mode & 0o777);
}

#[test]
fn a_log_left_open_is_narrowed_when_it_is_continued() {
    use std::os::unix::fs::PermissionsExt as _;

    // Same for the log: `OpenOptionsExt::mode` says nothing about a file that
    // already exists, and `--continue` is exactly the path that opens one.
    let sample = Sample::new("session-widened-log");
    let path = record(&sample, &[said("hello")]);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("a temporary file");

    let (session, _) = Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    drop(session);

    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "log is {:o}", mode & 0o777);
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

    assert!(header.contains(r#""format":1"#), "{header}");
    assert!(
        header.contains(&sample.workspace().root().display().to_string()),
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
fn the_newest_session_for_this_workspace_is_the_one_continued() {
    let sample = Sample::new("session-newest");
    let workspace = sample.workspace().root().display().to_string();

    sample.plant(
        "0000000000001-000001",
        &[
            format!(r#"{{"format":1,"session":"old","workspace":"{workspace}"}}"#),
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
fn a_log_from_a_different_version_is_refused_rather_than_half_read() {
    let sample = Sample::new("session-foreign");
    let workspace = sample.workspace().root().display().to_string();

    sample.plant(
        "0000000000002-000002",
        &[
            format!(r#"{{"format":99,"session":"future","workspace":"{workspace}"}}"#),
            r#"{"utterance":"something this build has never heard of"}"#.to_owned(),
        ],
    );

    let problem = Session::resume(&sample.logs(), &sample.workspace()).expect_err("not ours");

    assert!(matches!(problem, SessionError::Foreign { .. }));
}

#[test]
fn a_half_written_last_line_costs_only_that_line() {
    // What a process killed mid-write leaves behind. Everything before it is
    // still a transcript.
    let sample = Sample::new("session-torn");
    let path = record(&sample, &[said("kept"), said("also kept")]);

    let whole = fs::read_to_string(&path).expect("the log");
    let torn = format!("{whole}{{\"user\":\"cut off mid");
    fs::write(&path, torn).expect("a writable temporary directory");

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages(), &[said("kept"), said("also kept")]);
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

#[test]
fn a_line_that_cannot_be_read_at_all_is_a_failure_rather_than_a_shorter_session() {
    // Bytes that are not text stop the read wherever they sit. Taken for the
    // end of the log, one damaged line in the middle silently drops every turn
    // after it, and `--continue` hands back a transcript missing its middle
    // with nothing anywhere to say so.
    let sample = Sample::new("session-unreadable");
    let workspace = sample.workspace().root().display().to_string();
    let path = sample.plant(
        "0000000000004-000004",
        &[
            format!(r#"{{"format":1,"session":"damaged","workspace":"{workspace}"}}"#),
            r#"{"user":"before"}"#.to_owned(),
        ],
    );

    let mut damaged = fs::read(&path).expect("the log");
    damaged.extend_from_slice(b"{\"user\":\"\xff\xfe\"}\n");
    damaged.extend_from_slice(br#"{"user":"after"}"#);
    damaged.push(b'\n');
    fs::write(&path, damaged).expect("a writable temporary directory");

    let problem = Session::resume(&sample.logs(), &sample.workspace()).expect_err("unreadable");

    assert!(matches!(problem, SessionError::Log { .. }), "{problem}");
}

#[test]
fn a_line_that_is_not_a_message_stops_the_replay() {
    // Recognising some lines and skipping others would hand the model a
    // transcript with a hole in it, which reads as the user contradicting
    // themselves.
    let sample = Sample::new("session-hole");
    let workspace = sample.workspace().root().display().to_string();

    sample.plant(
        "0000000000003-000003",
        &[
            format!(r#"{{"format":1,"session":"holed","workspace":"{workspace}"}}"#),
            r#"{"user":"kept"}"#.to_owned(),
            r#"{"whatever":"not a message"}"#.to_owned(),
            r#"{"user":"never reached"}"#.to_owned(),
        ],
    );

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages(), &[said("kept")]);
}

#[test]
fn a_last_line_torn_mid_character_ends_the_log_rather_than_failing_it() {
    // A process that dies while writing stops wherever it stops, including
    // between the bytes of one character. That is the same half-written last
    // line the replay already forgives; refusing to continue over it costs the
    // user every turn in the log, and the file says nothing followed.
    let sample = Sample::new("session-torn-character");
    let workspace = sample.workspace().root().display().to_string();
    let path = sample.plant(
        "0000000000005-000005",
        &[
            format!(r#"{{"format":1,"session":"torn","workspace":"{workspace}"}}"#),
            r#"{"user":"kept"}"#.to_owned(),
        ],
    );

    // The first two bytes of a three-byte character, and then the power cut.
    let mut torn = fs::read(&path).expect("the log");
    torn.extend_from_slice(b"{\"user\":\"\xe2\x82");
    fs::write(&path, torn).expect("a writable temporary directory");

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages(), &[said("kept")]);
}
