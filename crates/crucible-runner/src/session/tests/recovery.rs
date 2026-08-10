//! What a log that stopped badly gives back, and what it looks like after.
//!
//! Every case here is a shape a running session cannot produce on purpose: a
//! process killed mid-write, a line from a build that spelled things
//! differently, bytes that are not text. The second half of each test is the
//! part worth having — continuing the recovered session and reading it back
//! again, because a log is only recovered if the *next* run can read it too.

use super::*;

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
            sample.header(1, "damaged"),
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

    sample.plant(
        "0000000000003-000003",
        &[
            sample.header(1, "holed"),
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
    let path = sample.plant(
        "0000000000005-000005",
        &[sample.header(1, "torn"), r#"{"user":"kept"}"#.to_owned()],
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
            sample.header(1, "holed"),
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
