//! What a session log keeps, and what it hands back.

use std::fs;
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::{Arc, Mutex};

use crucible_core::{Message, SessionId, ToolArgs, ToolCall, ToolId, ToolOutput, ToolResult};
use serde_json::Value;

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

// Picking up a session by name rather than by being the newest.
//
// Everything [`Session::resume`] is asked of, asked of a log somebody chose:
// it is the same directory, open to whatever is in it, and the identifier
// naming a file is not a reason to skip the questions about what that file
// turns out to be.

/// The identifier a planted log is named by.
fn named(id: &str) -> SessionId {
    SessionId::from_str(id).expect("a session id")
}

#[test]
fn a_session_named_outright_is_the_one_picked_up() {
    // Not the newest, which is what `--continue` would have found. Naming one
    // is the whole point: the session worth going back to is rarely the last
    // one started.
    let sample = Sample::new("session-named");
    sample.plant(
        "0000000000001-000001",
        &[
            sample.header(wire::FORMAT, "older"),
            r#"{"user":"the one asked for"}"#.to_owned(),
        ],
    );
    record(&sample, &[said("the newest")]);

    let (_session, transcript) = Session::reopen(
        &sample.logs(),
        &sample.workspace(),
        &named("0000000000001-000001"),
    )
    .expect("the session named");

    assert_eq!(transcript.messages(), &[said("the one asked for")]);
}

#[test]
fn a_session_this_directory_has_no_log_of_is_refused_by_name() {
    let sample = Sample::new("session-named-missing");
    record(&sample, &[said("the only one")]);

    let problem = Session::reopen(
        &sample.logs(),
        &sample.workspace(),
        &named("0000000000007-000007"),
    )
    .expect_err("nothing of that name");

    assert!(matches!(problem, SessionError::Unknown { .. }), "{problem}");
    assert!(problem.to_string().contains("0000000000007-000007"));
}

#[test]
fn a_session_belonging_to_another_workspace_is_not_reachable_by_naming_it() {
    // The list somebody picked from holds this directory's sessions, so an
    // identifier that names another one did not come off it. What it would
    // continue is somebody else's transcript, in this directory's terminal.
    let sample = Sample::new("session-named-elsewhere");
    let session = Session::start(&sample.logs(), &sample.elsewhere()).expect("a new session");
    let id = session
        .path()
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(named)
        .expect("the log is named by its session");
    drop(session);

    let problem = Session::reopen(&sample.logs(), &sample.workspace(), &id)
        .expect_err("not this workspace's");

    assert!(matches!(problem, SessionError::Unknown { .. }), "{problem}");
}

#[test]
fn a_session_named_from_a_build_that_spelled_things_differently_says_which() {
    // Refused rather than reported missing. The file is there and it is this
    // directory's; what it is not is readable by this build, and that is the
    // answer worth having.
    let sample = Sample::new("session-named-foreign");
    sample.plant(
        "0000000000002-000002",
        &[
            sample.header(99, "future"),
            r#"{"utterance":"something this build has never heard of"}"#.to_owned(),
        ],
    );

    let problem = Session::reopen(
        &sample.logs(),
        &sample.workspace(),
        &named("0000000000002-000002"),
    )
    .expect_err("not ours");

    assert!(matches!(problem, SessionError::Foreign { .. }), "{problem}");
}

#[test]
fn a_session_another_crucible_has_open_is_refused_by_name_too() {
    let sample = Sample::new("session-named-busy");
    let open = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let id = open
        .path()
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(named)
        .expect("the log is named by its session");

    let problem = Session::reopen(&sample.logs(), &sample.workspace(), &id);

    // A filesystem with no locks cannot say a session is open, and there this
    // is the ordinary continuation it is everywhere else — see `claim`.
    match problem {
        Err(SessionError::Busy { .. }) | Ok(_) => {}
        Err(other) => panic!("{other}"),
    }
}

#[test]
fn a_session_picked_up_by_name_is_appended_to_rather_than_started_over() {
    let sample = Sample::new("session-named-append");
    let path = record(&sample, &[said("first")]);
    let id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(named)
        .expect("the log is named by its session");

    let (session, _) =
        Session::reopen(&sample.logs(), &sample.workspace(), &id).expect("the session named");
    assert_eq!(session.path(), path);
    assert_eq!(session.id(), Some(&id), "the one that was asked for");
    session.append(&said("second"));
    drop(session);

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages(), &[said("first"), said("second")]);
}

// What a log that stopped badly gives back, and what it looks like after.
//
// Every case here is a shape a running session cannot produce on purpose: a
// process killed mid-write, a line from a build that spelled things
// differently, bytes that are not text. The second half of each test is the
// part worth having — continuing the recovered session and reading it back
// again, because a log is only recovered if the *next* run can read it too.

#[test]
fn a_log_from_a_different_version_is_refused_rather_than_half_read() {
    let sample = Sample::new("session-foreign");

    sample.plant(
        "0000000000002-000002",
        &[
            sample.header(99, "future"),
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
fn a_line_that_cannot_be_read_at_all_is_a_failure_rather_than_a_shorter_session() {
    // Bytes that are not text stop the read wherever they sit. Taken for the
    // end of the log, one damaged line in the middle silently drops every turn
    // after it, and `--continue` hands back a transcript missing its middle
    // with nothing anywhere to say so.
    let sample = Sample::new("session-unreadable");
    let path = sample.plant(
        "0000000000004-000004",
        &[
            sample.header(wire::FORMAT, "damaged"),
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
fn turns_recorded_after_a_line_that_is_not_a_message_are_refused_rather_than_cut_off() {
    // The shape a disk that filled and was then freed leaves: a line the write
    // stopped part way through, the next line welded onto it, and an hour of
    // turns recorded after that. Read as the end of the log it costs all of
    // them — from the transcript handed back, and from the file, which is cut
    // to what was read before anything is appended to it.
    let sample = Sample::new("session-hole");

    let path = sample.plant(
        "0000000000003-000003",
        &[
            sample.header(wire::FORMAT, "holed"),
            r#"{"user":"before the disk filled"}"#.to_owned(),
            r#"{"user":"the line the disk cut sh{"user":"welded onto it"}"#.to_owned(),
            r#"{"user":"after the disk was freed"}"#.to_owned(),
            r#"{"user":"and an hour more"}"#.to_owned(),
        ],
    );
    let whole = fs::read(&path).expect("the log");

    let problem = Session::resume(&sample.logs(), &sample.workspace()).expect_err("damaged");

    assert!(matches!(problem, SessionError::Log { .. }), "{problem}");
    assert_eq!(
        fs::read(&path).expect("the log"),
        whole,
        "the turns after the damage were cut off the file"
    );
}

#[test]
fn a_last_line_torn_mid_character_ends_the_log_rather_than_failing_it() {
    // A process that dies while writing stops wherever it stops, including
    // between the bytes of one character. That is the same half-written last
    // line the replay already forgives; refusing to continue over it costs the
    // user every turn in the log, and the file says nothing followed.
    let sample = Sample::new("session-torn-character");
    let path = sample.plant(
        "0000000000005-000005",
        &[
            sample.header(wire::FORMAT, "torn"),
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

#[test]
fn a_tail_torn_mid_character_is_cut_off_rather_than_written_onto() {
    // Forgiving the half-written line is only half the job. The log is opened
    // for append, so with nothing cut the next turn is welded onto the fragment
    // — and the line that makes is damage in the *middle*, the one thing the
    // replay refuses to continue over. Crashing would cost the last line;
    // crashing and then continuing would cost the session.
    let sample = Sample::new("session-torn-continued");
    let path = record(&sample, &[said("before the crash")]);

    let mut torn = fs::read(&path).expect("the log");
    torn.extend_from_slice(b"{\"user\":\"\xe2\x82");
    fs::write(&path, torn).expect("a writable temporary directory");

    let (session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    assert_eq!(transcript.messages(), &[said("before the crash")]);

    session.append(&said("after the crash"));
    session.append(&said("and one more"));
    drop(session);

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session again");

    assert_eq!(
        transcript.messages(),
        &[
            said("before the crash"),
            said("after the crash"),
            said("and one more"),
        ]
    );
}

#[test]
fn a_line_that_is_not_a_message_is_cut_off_rather_than_written_past() {
    // The replay stops at this line, so anything appended after it is written
    // somewhere no replay will ever reach. Every turn of the continued session
    // would be recorded and none of it would come back.
    let sample = Sample::new("session-hole-continued");

    sample.plant(
        "0000000000006-000006",
        &[
            sample.header(wire::FORMAT, "holed"),
            r#"{"user":"kept"}"#.to_owned(),
            r#"{"whatever":"not a message"}"#.to_owned(),
        ],
    );

    let (session, _) = Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    session.append(&said("and on we go"));
    drop(session);

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session again");

    assert_eq!(transcript.messages(), &[said("kept"), said("and on we go")]);
}

#[test]
fn an_empty_line_is_read_past_rather_than_taken_for_the_end_of_the_log() {
    // What the writer lays down when a write failed before it put any bytes on
    // the disk: the line that ends whatever the failure may have left. Nothing
    // was recorded there, so the turns after it are still turns — and the
    // offset the file is cut to has to count it, or continuing writes over the
    // last of them.
    let sample = Sample::new("session-empty-line");
    sample.plant(
        "0000000000009-000009",
        &[
            sample.header(wire::FORMAT, "gapped"),
            r#"{"user":"before"}"#.to_owned(),
            String::new(),
            r#"{"user":"after"}"#.to_owned(),
        ],
    );

    let (session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    assert_eq!(transcript.messages(), &[said("before"), said("after")]);

    session.append(&said("and on we go"));
    drop(session);

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session again");

    assert_eq!(
        transcript.messages(),
        &[said("before"), said("after"), said("and on we go")]
    );
}

#[test]
fn a_header_the_process_never_finished_is_passed_over_rather_than_written_onto() {
    // The header is written before a session can record anything, so a log
    // that stopped in the middle of one holds no turns to lose. Taken for a
    // header it would have the first turn of the continued session appended
    // onto the end of it, making one line that names no workspace — and a log
    // no later run can find, holding the whole session.
    let sample = Sample::new("session-torn-header");
    record(&sample, &[said("the session before it")]);

    // Named so it sorts newest, which is the only way it is ever looked at.
    let torn = sample.logs().join("9999999999999-999999.jsonl");
    fs::write(&torn, sample.header(wire::FORMAT, "unfinished"))
        .expect("a writable temporary directory");

    let (session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    assert_eq!(transcript.messages(), &[said("the session before it")]);

    session.append(&said("and on we go"));
    drop(session);

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session again");

    assert_eq!(
        transcript.messages(),
        &[said("the session before it"), said("and on we go")]
    );
}

#[test]
fn a_session_another_crucible_still_has_open_is_not_continued_over() {
    // Continuing a log that is still being written cuts it back to what the
    // replay read — deleting lines the running session has already written and
    // still believes are there — and then appends to it, leaving two of them
    // writing to one file. What comes back afterwards is one conversation's
    // prompts interleaved with another's, and neither process notices.
    let sample = Sample::new("session-open");
    let running = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    running.append(&said("still going"));

    let problem = Session::resume(&sample.logs(), &sample.workspace()).expect_err("still open");
    assert!(matches!(problem, SessionError::Busy { .. }), "{problem}");

    // Untouched by the attempt, and continued as usual once it has ended: the
    // claim is the operating system's, so it is given back however the process
    // goes away.
    running.append(&said("and going"));
    drop(running);

    let (_session, transcript) = Session::resume(&sample.logs(), &sample.workspace())
        .expect("the session, now it has ended");

    assert_eq!(
        transcript.messages(),
        &[said("still going"), said("and going")]
    );
}

#[test]
fn a_claim_that_could_not_be_attempted_stops_the_session_rather_than_being_read_as_none() {
    // The guard is gone either way; what differs is whether anything says so.
    // Read as the filesystem having none to take — the other answer with no
    // claim in it — this is `--continue` cutting the log back and appending to
    // it with no idea whether another crucible is doing the same, and neither
    // of them noticing until the transcript comes back interleaved.
    //
    // A directory where the mark goes is a mark that cannot be made, on every
    // platform, and the log beside it is untouched and perfectly readable.
    let sample = Sample::new("session-unclaimable");
    let log = record(&sample, &[said("the session before it")]);
    let mark = PathBuf::from(format!("{}.lock", log.display()));

    fs::remove_file(&mark).expect("the mark the session made");
    fs::create_dir(&mark).expect("something in its place");

    let problem = Session::resume(&sample.logs(), &sample.workspace())
        .expect_err("a claim that could not be attempted");

    assert!(matches!(problem, SessionError::Claim { .. }), "{problem}");

    // Nothing was read and nothing was cut: the order is claim first, and a log
    // that stopped a session is a log the next crucible can still continue.
    fs::remove_dir(&mark).expect("the directory in the way");

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session, now it can be");

    assert_eq!(transcript.messages(), &[said("the session before it")]);
}

#[test]
fn a_call_nothing_answered_is_cut_off_rather_than_left_for_the_next_replay() {
    // The unanswered call is dropped from the transcript handed back, but the
    // line is still on disk. Appending after it puts the unanswered question in
    // the *middle* of the next replay, where nothing drops it — and a provider
    // is entitled to reject that transcript, which makes the continued session
    // unusable rather than merely shorter.
    let sample = Sample::new("session-outstanding-continued");
    record(
        &sample,
        &[said("check the tests"), calling("call-1", "bash", "{}")],
    );

    let (session, _) = Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    session.append(&said("never mind"));
    drop(session);

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session again");

    assert_eq!(
        transcript.messages(),
        &[said("check the tests"), said("never mind")]
    );
}
