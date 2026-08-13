//! What a new session does with a name something else already holds.
//!
//! A name is a millisecond and twenty-four bits, so this is rare rather than
//! impossible — and rare is the reason to write these down: no ordinary run ever
//! reaches the path, and what it costs when it is reached is somebody else's
//! session. The name is handed in rather than minted, because a minted one
//! cannot be made to collide on purpose.

use super::*;

/// The name a running session is recording under.
fn recording(session: &Session) -> SessionId {
    session
        .id()
        .expect("the log is named by its session")
        .clone()
}

/// A minter that hands out `names` in order, one per call.
fn minting(names: &[SessionId]) -> impl FnMut() -> SessionId {
    let mut names: Vec<SessionId> = names.iter().rev().cloned().collect();
    move || names.pop().expect("a name for every attempt")
}

#[test]
fn a_name_another_session_is_recording_under_is_refused_rather_than_written_into() {
    // Opened for append, the second session writes its own header onto the end
    // of the first one's log and the two then interleave a conversation each
    // into one file. Neither notices, and what the next `--continue` reads back
    // is one session's prompts with another's answers under them.
    let sample = Sample::new("session-collision");
    let running = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let taken = recording(&running);
    let path = running.path().to_owned();
    running.append(&said("what the first session said"));

    let problem = Session::naming(&sample.logs(), &sample.workspace(), || taken.clone())
        .expect_err("every name it tried was taken");

    assert!(matches!(problem, SessionError::Taken { .. }), "{problem}");

    // Dropping is what waits for the queue, so the file is complete after it.
    drop(running);
    let written = fs::read_to_string(&path).expect("the log");
    let lines: Vec<&str> = written.lines().collect();

    assert_eq!(lines.len(), 2, "a header and one message: {written}");
    assert!(
        lines
            .get(1)
            .is_some_and(|line| line.contains("what the first session said")),
        "{written}"
    );
}

#[test]
fn a_name_that_is_taken_is_passed_over_for_the_next_one_minted() {
    // What a collision costs: a name. The session starts under the next one and
    // the user is told nothing, because nothing about their session went wrong.
    let sample = Sample::new("session-collision-retry");
    let running = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let taken = recording(&running);
    let free = named("0000000000001-000001");

    let started = Session::naming(
        &sample.logs(),
        &sample.workspace(),
        minting(&[taken, free.clone()]),
    )
    .expect("the session under the second name");

    assert_eq!(started.id(), Some(&free));
    assert_eq!(
        started.path(),
        sample.logs().join("0000000000001-000001.jsonl")
    );
}

#[test]
fn a_mark_another_crucible_holds_is_a_name_in_use_even_with_no_log_beside_it() {
    // A busy claim is one answer at `--continue` and another here. There it
    // names the session that was asked for and stops; a session starting asked
    // for nothing, so it is a name to step over. Read as an absence — which is
    // what a filesystem with no locks reports — this starts an unguarded
    // session on a name another crucible believes is its own.
    let sample = Sample::new("session-collision-marked");
    let held = sample.logs().join("0000000000001-000001.jsonl");

    // Held for the whole test, the way the crucible on the other end of it
    // would be holding it.
    let mark = claim(&held).expect("the mark beside a log there is none of");
    assert!(
        matches!(mark, Claimed::Taken(_)),
        "the filesystem under the tests has no locks to take"
    );

    let started = Session::naming(
        &sample.logs(),
        &sample.workspace(),
        minting(&[named("0000000000001-000001"), named("0000000000002-000002")]),
    )
    .expect("the session under the second name");

    assert_eq!(started.id(), Some(&named("0000000000002-000002")));
    assert!(
        !held.exists(),
        "a log was started under a name another crucible is holding"
    );
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
