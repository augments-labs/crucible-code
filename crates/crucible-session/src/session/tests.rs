//! What a session log keeps, and what it hands back.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::{Arc, Mutex};

use crucible_core::{
    Calibration, Carried, Message, SessionId, Spend, StopReason, ToolArgs, ToolCall, ToolId,
    ToolOutput, ToolResult,
};
use serde_json::Value;

use super::claim::{Claimed, claim};
use super::{Session, SessionError, wire};
use crate::sample::Sample;

fn said(text: &str) -> Message {
    Message::said(text)
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

fn answering(text: &str) -> Message {
    Message::Agent {
        text: text.into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
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
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
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
fn a_write_that_left_nothing_leaves_no_scar() {
    // The disk refused the line before any of it landed, so the file is
    // exactly as it was. The next line starts clean: a newline written to end
    // a line that never began would put an empty line in the log for no
    // damage at all.
    let written = Written::default();
    let session = Session::writing(
        PathBuf::from("refused.jsonl"),
        Filling {
            written: Arc::clone(&written),
            writes: 0,
            // The first write of the first line, before a byte of it landed.
            fails_at: 1,
        },
    );

    session.append(&said("the line the disk refused"));
    session.append(&said("the one after it"));
    assert!(session.finish().is_some(), "the failure is still reported");

    let log = String::from_utf8(written.lock().expect("the writer is gone").clone())
        .expect("a log of text");
    let lines: Vec<&str> = log.lines().collect();

    assert_eq!(lines.len(), 1, "{log}");
    assert!(
        lines
            .first()
            .is_some_and(|line| wire::message(line).is_some()),
        "{log}"
    );
}

#[test]
fn a_fragment_nothing_can_finish_ends_the_log() {
    // The disk took part of a line and then filled. No newline can mend that:
    // ending the fragment makes a line that is not a message in the middle of
    // the log, and replay refuses everything from there on. So nothing more is
    // written, the file ends at the fragment, and replay reads it as what it
    // is — a log torn at the tail, whole up to its last line.
    let written = Written::default();
    let session = Session::writing(
        PathBuf::from("fragmented.jsonl"),
        Fragmenting {
            written: Arc::clone(&written),
            writes: 0,
            takes: 5,
        },
    );

    session.append(&said("the line the disk tore"));
    session.append(&said("the one after it"));
    assert!(session.finish().is_some(), "the failure is still reported");

    let log = written.lock().expect("the writer is gone").clone();

    assert_eq!(log.len(), 5, "{}", String::from_utf8_lossy(&log));
}

/// A log that takes part of one write, fails the next, and then works again —
/// a disk that filled in the middle of a line and was freed while the session
/// went on.
struct Fragmenting {
    written: Written,
    writes: usize,
    /// How many bytes of the first write are taken before the disk fills.
    takes: usize,
}

impl std::io::Write for Fragmenting {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.writes += 1;

        match self.writes {
            1 => {
                let taken = self.takes.min(bytes.len());
                self.written
                    .lock()
                    .expect("the test is holding it")
                    .extend_from_slice(bytes.get(..taken).unwrap_or(bytes));
                Ok(taken)
            }
            2 => Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "no space left on device",
            )),
            _ => {
                self.written
                    .lock()
                    .expect("the test is holding it")
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }
        }
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
fn a_log_from_the_format_before_this_one_is_still_a_session_to_continue() {
    // The refusal exists so a log is never half-understood. This format only
    // added — a line kind the older build never wrote — so a log from it means
    // here exactly what it meant there, and refusing it would cost somebody
    // their history to protect them from nothing.
    let sample = Sample::new("session-older-format");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let path = session.path().to_owned();
    session.append(&said("what an older build recorded"));
    drop(session);

    // The same log, headed the way the build before this one headed it.
    let text = fs::read_to_string(&path).expect("the log");
    let older = text.replacen(
        &format!("\"format\":{}", wire::FORMAT),
        &format!("\"format\":{}", wire::FORMAT - 1),
        1,
    );
    fs::write(&path, older).expect("a writable log");

    let (_, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the older session");

    assert_eq!(
        transcript.messages(),
        &[said("what an older build recorded")]
    );
}

#[test]
fn a_compacted_session_replays_as_the_notes_and_what_they_did_not_replace() {
    // The split the whole thing turns on: the log holds every message, and the
    // transcript holds what the model is sent. Replay is where they part, and
    // it parts them by the count the line carries — not by taking the line to
    // mean everything standing above it, which is a fact about the file rather
    // than about the compaction.
    let sample = Sample::new("session-compacted");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");

    session.append(&said("one"));
    session.append(&said("two"));
    session.append(&said("three"));
    session.compacted(2, "notes on two and three");
    session.append(&said("four"));
    drop(session);

    let (_, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    let replayed: Vec<&str> = transcript
        .messages()
        .iter()
        .map(|message| match message {
            Message::User { text, .. } => text.as_ref(),
            _ => "",
        })
        .collect();

    // `one` survives. The line said it replaced two, and two is what it
    // replaced.
    assert_eq!(replayed, ["one", "notes on two and three", "four"]);
}

#[test]
fn a_pruned_result_is_cleared_again_when_the_session_is_continued() {
    // The record and what the model is sent, parting company: the log keeps the
    // result whole, and a continued session clears it again, so the model is
    // never re-sent what it stopped seeing. The clearing rides on the result's
    // own id, which it shares with the call it answered.
    let sample = Sample::new("session-pruned");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");

    session.append(&calling("a", "read", r#"{"path":"big.rs"}"#));
    session.append(&answered("a", ToolOutput::ok("x".repeat(80_000))));
    session.append(&said("what did it say"));
    session.pruned(80_000, &[ToolId::new("a")]);
    session.append(&said("gone"));
    drop(session);

    let (_, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    let result = transcript
        .messages()
        .iter()
        .find_map(|message| match message {
            Message::ToolResults(results) => results.first(),
            _ => None,
        })
        .expect("the result is still there, holding a placeholder");

    assert!(
        result.output.text().contains("cleared to make room"),
        "the continued session re-sent the cleared text: {}",
        result.output.text()
    );
}

#[test]
fn a_log_that_says_it_forgot_is_continued_from_where_it_says_so() {
    // A shape on somebody's disk rather than one this build can produce:
    // `/clear` forgot in place once, and leaves a session of its own now, so
    // nothing writes this line any more. That is exactly why it is spelled out
    // here rather than produced by an API. The format is unchanged, so a log
    // holding one is still a log this build continues — and a reader that
    // stopped understanding the marker would replay a stretch the session it
    // came from was told to drop.
    //
    // What is above the marker stays in the file. The log is the record of what
    // happened, and cutting a stretch out of it is not something an append-only
    // file can do safely while another line is on its way to the disk.
    /// The line an earlier crucible wrote, exactly as it wrote it.
    const FORGOTTEN: &str = r#"{"forgotten":true}"#;

    let sample = Sample::new("session-forgot");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let path = session.path().to_owned();

    session.append(&said("what was said first"));
    drop(session);

    let mut log = fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("the log this session wrote");
    writeln!(log, "{FORGOTTEN}").expect("a writable log");
    drop(log);

    let (session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    assert_eq!(transcript.messages(), &[], "the marker started it again");

    session.append(&said("what was said after"));
    drop(session);

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    assert_eq!(transcript.messages(), &[said("what was said after")]);

    let written = fs::read_to_string(&path).expect("the log");
    assert!(written.contains("what was said first"), "{written}");
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
fn a_log_from_the_format_that_could_not_say_what_a_call_changed_still_replays_whole() {
    // Frozen bytes: a whole session as the build before this one left it on
    // somebody's disk, spelled out rather than produced, because the build that
    // wrote it is gone and only its output has to keep meaning what it meant.
    //
    // Format 9 added one key to a line of tool results and renamed nothing. A
    // result written without that key is a call that changed no file — which is
    // the only thing a build with nowhere to say otherwise could have meant —
    // so every line here reads here as it read there, files and all.
    let sample = Sample::new("session-format-eight");
    let id = "0000000000001-000001";

    sample.plant(
        id,
        &[
            sample.header(8, id),
            r#"{"user":"what is in this"}"#.to_owned(),
            r#"{"agent":"on it","calls":[{"args":"{}","id":"call-1","name":"read"}],"stop":"tools"}"#
                .to_owned(),
            concat!(
                r#"{"results":[{"attached":[{"hash":"HASH","#,
                r#""media_type":"image/png","modality":"image","path":"pictures/holiday.png"}],"#,
                r#""failed":false,"id":"call-1","text":"one match"}]}"#,
            )
            .replace("HASH", &"ab".repeat(32)),
        ],
    );

    let (_session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    let [asked, agent, results] = transcript.messages() else {
        panic!("three lines went in")
    };
    assert_eq!(asked, &said("what is in this"));
    assert_eq!(agent, &calling("call-1", "read", "{}"));

    let Message::ToolResults(answers) = results else {
        panic!("the third line is a line of results")
    };
    let [only] = answers.as_slice() else {
        panic!("one result went in")
    };
    assert_eq!(only.output.text(), "one match");
    assert_eq!(
        only.output.changed(),
        None,
        "a build with nowhere to say what it changed said it changed something"
    );
    let [file] = only.output.attachments() else {
        panic!("the file the result showed did not survive the older log")
    };
    assert_eq!(file.path.as_ref(), "pictures/holiday.png");
    assert_eq!(file.hash, [0xab; 32]);
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
fn the_branch_a_session_starts_on_reaches_the_listing() {
    let sample = Sample::new("session-branch");
    let session = Session::start(&sample.logs(), &sample.workspace(), Some("feature/picker"))
        .expect("a new session");
    session.append(&said("work on the branch"));
    drop(session);

    let offered = super::recent(&sample.logs(), &sample.workspace(), 4);

    assert_eq!(
        offered.first().and_then(super::Recorded::branch),
        Some("feature/picker")
    );
}

#[test]
fn the_message_count_follows_appends_and_survives_a_resume() {
    let sample = Sample::new("session-counted");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    session.append(&said("one"));
    session.append(&answering("two"));
    session.append(&said("three"));
    drop(session);

    let offered = super::recent(&sample.logs(), &sample.workspace(), 4);
    assert_eq!(offered.first().map(super::Recorded::messages), Some(3));

    let (continued, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    assert_eq!(transcript.len(), 3);
    continued.append(&answering("four"));
    drop(continued);

    let offered = super::recent(&sample.logs(), &sample.workspace(), 4);
    assert_eq!(offered.first().map(super::Recorded::messages), Some(4));
}

#[test]
fn a_session_from_another_workspace_is_not_offered() {
    // Sessions share one directory, so the workspace in the header is the only
    // thing keeping one project's session out of another's.
    let sample = Sample::new("session-elsewhere");
    Session::start(&sample.logs(), &sample.elsewhere(), None).expect("a new session");

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

/// A reading a session might have been told about itself.
fn reading(tokens: u64, spent: u64) -> Calibration {
    Calibration {
        carried: Carried::new(tokens),
        spent: Spend::new(spent),
        sent: 4_000,
        overhead: 900,
    }
}

#[test]
fn a_session_picked_up_is_told_again_what_its_last_request_carried() {
    let sample = Sample::new("session-carried");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let told = reading(12_345, 678);

    session.append(&said("what came before"));
    session.append(&answering("all done"));
    session.measured(&told);
    drop(session);

    let (session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages().len(), 2, "the messages come back too");
    assert_eq!(session.calibrated(), Some(told));
}

#[test]
fn a_reading_with_a_turn_written_after_it_is_not_about_this_transcript() {
    // The reading covered what stood when it was written. A message recorded
    // after it is a message it never saw, so what it says is about a shorter
    // transcript than the one being handed back — and a load told that number
    // would under-state itself, which is the direction that costs a turn.
    let sample = Sample::new("session-carried-stale");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");

    session.append(&answering("all done"));
    session.measured(&reading(12_345, 678));
    session.append(&said("one more thing"));
    drop(session);

    let (session, _transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(session.calibrated(), None);
}

#[test]
fn a_reading_covering_an_answer_nothing_answered_goes_with_it() {
    // A process that died between asking for a tool and recording its result
    // has its last message cut off, so a reading written over that message
    // describes more transcript than comes back.
    let sample = Sample::new("session-carried-cut");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");

    session.append(&said("fix the parser"));
    session.append(&calling("call-1", "read", r#"{"path":"src/main.rs"}"#));
    session.measured(&reading(12_345, 678));
    drop(session);

    let (session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages().len(), 1, "the call was cut off");
    assert_eq!(session.calibrated(), None);
}

#[test]
fn a_session_started_here_was_never_told_anything() {
    let sample = Sample::new("session-carried-fresh");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");

    assert_eq!(session.calibrated(), None);
}

#[test]
fn a_log_that_never_recorded_a_reading_is_still_a_session_to_continue() {
    // Every log written before this format has one. The session comes back
    // whole and measures itself again on its next answer, which is what it did
    // on every continue before there was a line to record.
    let sample = Sample::new("session-carried-absent");
    let messages = [said("what came before"), answering("all done")];

    record(&sample, &messages);

    let (session, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(transcript.messages(), messages.as_slice());
    assert_eq!(session.calibrated(), None);
}
