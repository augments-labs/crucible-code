//! Where a result stands in the log, and reading it back from there.
//!
//! What the screen keeps of a result it had to cut is a place, not the words:
//! the call it answered and where in the log the record holding it begins. The
//! words come back from the log when somebody opens the row, one result at a
//! time, and only from a record that holds the call the place names.

use super::*;
use crate::session::Place;

/// Every place the display replay of `session` reports, in the order the
/// records were read.
fn replayed_places(session: &Session) -> Vec<Place> {
    let mut history = session
        .display_history()
        .expect("the log opens")
        .expect("a session with a log");
    let mut places = Vec::new();
    while let Some(item) = history.next() {
        let item = item.expect("the log reads");
        if let crate::DisplayItem::Message {
            message: Message::ToolResults(results),
            ..
        } = item
        {
            let position = history
                .placed()
                .expect("a message read from the log has a place");
            places.extend(
                results
                    .iter()
                    .map(|result| Place::new(result.id.clone(), position)),
            );
        }
    }
    places
}

/// A session of two calls and their results, recorded and picked up again.
fn two_results(sample: &Sample) -> Session {
    record(
        sample,
        &[
            said("look at both"),
            calling("call-1", "read", r#"{"path":"a.rs"}"#),
            answered("call-1", RecordedToolOutput::ok("what a.rs held")),
            calling("call-2", "read", r#"{"path":"b.rs"}"#),
            answered("call-2", RecordedToolOutput::ok("what b.rs held")),
            answering("both read"),
        ],
    );
    Session::resume(&sample.logs(), &sample.workspace())
        .expect("the session")
        .0
}

#[test]
fn a_result_is_read_back_from_the_place_its_record_begins() {
    let sample = Sample::new("placed-read-back");
    let session = two_results(&sample);

    let places = replayed_places(&session);
    let calls: Vec<&str> = places.iter().map(|place| place.call().as_str()).collect();
    assert_eq!(calls, ["call-1", "call-2"]);

    for (place, said) in places.iter().zip(["what a.rs held", "what b.rs held"]) {
        let output = session.read_back(place).expect("the record is there");
        assert_eq!(output, RecordedToolOutput::ok(said), "{place:?}");
    }
}

#[test]
fn a_place_whose_record_holds_another_call_reads_nothing() {
    // A place names its call as well as its position, so a record that is not
    // the one it was written for is refused rather than shown under the wrong
    // row.
    let sample = Sample::new("placed-wrong-call");
    let session = two_results(&sample);
    let places = replayed_places(&session);
    let first = places.first().expect("a place");

    let crossed = Place::new(ToolId::new("call-2"), first.position());
    assert!(session.read_back(&crossed).is_err());

    let nowhere = Place::new(ToolId::new("call-1"), 0);
    assert!(
        session.read_back(&nowhere).is_err(),
        "the header holds no result"
    );
}

#[test]
fn a_place_whose_line_was_cut_short_reads_nothing() {
    let sample = Sample::new("placed-cut-short");
    let path = record(
        &sample,
        &[
            said("look"),
            calling("call-1", "read", r#"{"path":"a.rs"}"#),
            answered("call-1", RecordedToolOutput::ok("what a.rs held")),
            answering("read"),
        ],
    );
    let session = Session::resume(&sample.logs(), &sample.workspace())
        .expect("the session")
        .0;
    let place = replayed_places(&session).remove(0);

    // Cut in the middle of the result's line, as a crash would leave it.
    let cut = place.position() + 20;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("the log")
        .set_len(cut)
        .expect("cut short");

    assert!(session.read_back(&place).is_err());

    // And a log that is gone reads nothing either, rather than failing
    // anything else.
    std::fs::remove_file(&path).expect("removed");
    assert!(session.read_back(&place).is_err());
}

#[test]
fn reading_a_result_back_changes_nothing_in_the_log() {
    let sample = Sample::new("placed-reads-only");
    let session = two_results(&sample);
    let places = replayed_places(&session);
    let before = std::fs::read(session.path()).expect("the log");

    for _ in 0..3 {
        for place in &places {
            session.read_back(place).expect("read");
        }
    }

    assert_eq!(std::fs::read(session.path()).expect("the log"), before);
}

#[test]
fn a_result_appended_now_says_where_it_went_and_reads_back_from_there() {
    // The live half: a result the running session writes has no replay to
    // report its place, so the thread that writes it does.
    let sample = Sample::new("placed-live");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a session");

    session.append(&said("look"));
    session.append(&calling("call-1", "read", r#"{"path":"a.rs"}"#));
    session.append(&answered(
        "call-1",
        RecordedToolOutput::ok("what a.rs held"),
    ));
    session.append(&calling("call-2", "read", r#"{"path":"b.rs"}"#));
    session.append(&answered(
        "call-2",
        RecordedToolOutput::ok("what b.rs held"),
    ));

    let places = session.take_placed();
    let calls: Vec<&str> = places.iter().map(|place| place.call().as_str()).collect();
    assert_eq!(calls, ["call-1", "call-2"]);
    for (place, said) in places.iter().zip(["what a.rs held", "what b.rs held"]) {
        assert_eq!(
            session.read_back(place).expect("read"),
            RecordedToolOutput::ok(said)
        );
    }

    // Taken once: the next take holds only what was written since.
    assert!(session.take_placed().is_empty());
}

#[test]
fn a_session_that_records_nothing_has_no_places_and_reads_nothing() {
    let session = Session::nowhere();
    session.append(&answered("call-1", RecordedToolOutput::ok("said")));

    assert!(session.take_placed().is_empty());
    assert!(
        session
            .read_back(&Place::new(ToolId::new("call-1"), 0))
            .is_err()
    );
}

#[test]
fn a_line_still_being_written_reads_nothing_until_it_is_whole() {
    // Another writer mid-line — a turn appending to the same log while a row is
    // opened — leaves a record with no end yet. What is read there is the whole
    // record or nothing, never the part that has landed.
    let sample = Sample::new("placed-half-written");
    let session = two_results(&sample);
    let line = format!(
        "{}\n",
        wire::line(&answered(
            "call-3",
            RecordedToolOutput::ok("what c.rs held")
        ))
    );
    let position = std::fs::metadata(session.path()).expect("the log").len();
    let place = Place::new(ToolId::new("call-3"), position);
    let (landed, rest) = line.split_at(line.len() / 2);

    let mut log = std::fs::OpenOptions::new()
        .append(true)
        .open(session.path())
        .expect("the log");
    log.write_all(landed.as_bytes()).expect("half the line");
    assert!(session.read_back(&place).is_err(), "half a line was read");

    log.write_all(rest.as_bytes()).expect("the rest of it");
    assert_eq!(
        session.read_back(&place).expect("the whole line"),
        RecordedToolOutput::ok("what c.rs held")
    );
}

#[test]
fn a_result_cut_to_the_ceiling_reads_back_with_the_note_it_was_cut_with() {
    // What the log keeps of a result too long for it is the cut result and the
    // note saying what went, and that is what is read back.
    let sample = Sample::new("placed-cut-to-ceiling");
    let mut output = RecordedToolOutput::ok(format!("HEAD{}TAIL", "x".repeat(40_000)));
    let retained = output.limit_encoded(crucible_types::TOOL_RESULT_BYTES);
    assert!(retained.omitted() > 0, "the result was not cut");
    record(
        &sample,
        &[
            said("print it all"),
            calling("call-1", "bash", r#"{"command":"cat big"}"#),
            answered("call-1", output.clone()),
        ],
    );
    let session = Session::resume(&sample.logs(), &sample.workspace())
        .expect("the session")
        .0;

    let place = replayed_places(&session).remove(0);
    assert_eq!(session.read_back(&place).expect("read"), output);
}

/// The results of `calls`, each saying which call it answered.
fn results_of(calls: &[&str]) -> Message {
    Message::ToolResults(
        calls
            .iter()
            .map(|call| crucible_types::ToolResult {
                id: ToolId::new(*call),
                output: RecordedToolOutput::ok(format!("what {call} said")),
            })
            .collect(),
    )
}

#[test]
fn a_result_appended_after_a_pick_up_is_placed_past_everything_already_there() {
    // The writer of a session picked up counts from the end of the log as it
    // was found, header and every turn, so what it places reads back.
    let sample = Sample::new("placed-after-resume");
    record(
        &sample,
        &[
            said("look"),
            calling("call-1", "read", r#"{"path":"a.rs"}"#),
            answered("call-1", RecordedToolOutput::ok("what a.rs held")),
            answering("read"),
        ],
    );
    let session = Session::resume(&sample.logs(), &sample.workspace())
        .expect("the session")
        .0;

    session.append(&said("and again"));
    session.append(&calling("call-2", "read", r#"{"path":"b.rs"}"#));
    session.append(&results_of(&["call-2"]));

    let places = session.take_placed();
    let calls: Vec<&str> = places.iter().map(|place| place.call().as_str()).collect();
    assert_eq!(calls, ["call-2"]);
    for place in &places {
        assert_eq!(
            session.read_back(place).expect("read"),
            RecordedToolOutput::ok("what call-2 said")
        );
    }
}

#[test]
fn a_result_placed_from_the_start_of_a_new_log_counts_its_header() {
    // A fresh log opens with its header, and the writer's count starts after
    // it: a result appended first reads back from where it was placed.
    let sample = Sample::new("placed-first");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a session");

    session.append(&results_of(&["call-1", "call-2"]));

    let places = session.take_placed();
    assert_eq!(places.len(), 2);
    assert!(
        places.iter().all(|place| place.position() > 0),
        "{places:?}"
    );
    for place in &places {
        assert!(session.read_back(place).is_ok(), "{place:?}");
    }
}

/// Where each place `written` holds begins, as the line of results there says.
fn landed_at(written: &Written, places: &[Place]) -> Vec<String> {
    let log = written.lock().expect("the writer is gone").clone();
    places
        .iter()
        .map(|place| {
            let from = usize::try_from(place.position()).expect("a short log");
            let line = log
                .get(from..)
                .and_then(|rest| rest.split(|byte| *byte == b'\n').next())
                .expect("a line at the place");
            let text = std::str::from_utf8(line).expect("text");
            match wire::message(text) {
                Some(Message::ToolResults(results)) => results
                    .iter()
                    .map(|result| result.id.as_str().to_owned())
                    .collect::<Vec<_>>()
                    .join(","),
                other => panic!("no results at {}: {other:?} {text:?}", place.position()),
            }
        })
        .collect()
}

/// A file with nothing in it, for a writer whose sink is not that file to
/// take its length from.
fn empty(sample: &Sample, name: &str) -> PathBuf {
    let path = sample.logs().join(name);
    std::fs::create_dir_all(sample.logs()).expect("the directory");
    std::fs::write(&path, b"").expect("the file");
    path
}

#[test]
fn a_line_whose_newline_did_not_land_is_not_placed_and_the_next_one_is() {
    // The disk took the line and refused its newline: the record is ended
    // before the next one starts, so the place of the next one counts that
    // newline too. The line cut short is placed nowhere.
    let sample = Sample::new("placed-torn");
    let written = Written::default();
    let session = Session::writing(
        empty(&sample, "torn.jsonl"),
        Filling {
            written: Arc::clone(&written),
            writes: 0,
            // The newline of the first line.
            fails_at: 2,
        },
    );

    session.append(&results_of(&["call-1"]));
    session.append(&results_of(&["call-2"]));
    session.append(&results_of(&["call-3"]));
    let places = session.take_placed();
    drop(session);

    assert_eq!(landed_at(&written, &places), ["call-2", "call-3"]);
}

#[test]
fn nothing_is_placed_after_a_fragment_ends_the_log() {
    let sample = Sample::new("placed-fragment");
    let written = Written::default();
    let session = Session::writing(
        empty(&sample, "fragment.jsonl"),
        Fragmenting {
            written: Arc::clone(&written),
            writes: 0,
            takes: 5,
        },
    );

    session.append(&results_of(&["call-1"]));
    session.append(&results_of(&["call-2"]));

    assert!(session.take_placed().is_empty());
}

#[test]
fn a_writer_that_could_not_find_where_the_file_ends_places_nothing() {
    // A count started anywhere but the file's end names places the log does
    // not bear out, so where its length could not be read nothing is placed.
    let written = Written::default();
    let session = Session::writing(
        PathBuf::from("no-such-directory/unmeasured.jsonl"),
        Filling {
            written: Arc::clone(&written),
            writes: 0,
            fails_at: usize::MAX,
        },
    );

    session.append(&results_of(&["call-1"]));

    assert!(session.take_placed().is_empty());
    assert!(
        !written.lock().expect("the writer").is_empty(),
        "nothing was written"
    );
}

#[test]
fn the_writer_keeps_the_newest_places_when_nobody_takes_them() {
    let sample = Sample::new("placed-bounded");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a session");

    let calls: Vec<String> = (0..1100).map(|call| format!("call-{call}")).collect();
    for call in &calls {
        session.append(&results_of(&[call.as_str()]));
    }

    let places = session.take_placed();
    assert_eq!(places.len(), super::super::log::PLACED);
    assert_eq!(
        places.first().map(|place| place.call().as_str()),
        Some("call-76")
    );
    assert_eq!(
        places.last().map(|place| place.call().as_str()),
        Some("call-1099")
    );
}

#[test]
fn a_place_is_handed_over_once_by_whichever_take_reaches_it() {
    // Taking without waiting hands over what has landed; taking after waiting
    // hands over the rest. Neither hands a place over twice.
    let sample = Sample::new("placed-landed");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a session");

    session.append(&results_of(&["call-1"]));
    session.append(&results_of(&["call-2"]));
    let mut taken = session.take_landed();
    taken.extend(session.take_placed());
    taken.extend(session.take_landed());

    let calls: Vec<&str> = taken.iter().map(|place| place.call().as_str()).collect();
    assert_eq!(calls, ["call-1", "call-2"]);
}

#[test]
fn every_place_a_replay_reports_reads_back_across_the_records_it_does_not_hand_over() {
    // A log holds more than messages: the record of each call as it ran, a
    // pruning's notice and a compaction's. The replay steps over them or holds
    // them back, and the place of every result after them is still its own.
    let sample = Sample::new("placed-across-records");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a session");
    session.append(&said("look at all three"));
    for (at, id) in ["call-1", "call-2", "call-3"].into_iter().enumerate() {
        session.append(&calling(id, "bash", "{}"));
        ran(
            &session,
            &call(id),
            RecordedToolOutput::ok(format!("what {id} said")),
        );
        session.append(&results_of(&[id]));
        match at {
            0 => session.pruned(10, &[ToolId::new(id)]),
            1 => session.compacted(4, "what happened so far"),
            _ => {}
        }
    }
    drop(session);
    let session = Session::resume(&sample.logs(), &sample.workspace())
        .expect("the session")
        .0;

    let places = replayed_places(&session);
    let calls: Vec<&str> = places.iter().map(|place| place.call().as_str()).collect();
    assert_eq!(calls, ["call-1", "call-2", "call-3"]);
    for place in &places {
        assert_eq!(
            session.read_back(place).expect("read"),
            RecordedToolOutput::ok(format!("what {} said", place.call().as_str())),
            "{place:?}"
        );
    }
}

#[test]
fn a_session_says_whether_what_it_appends_is_still_placed() {
    let sample = Sample::new("placed-still");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a session");
    assert!(session.places(), "a session writing its log places");
    assert!(!Session::nowhere().places(), "a session with no log places");

    let unmeasured = Session::writing(
        PathBuf::from("no-such-directory/unmeasured.jsonl"),
        Filling {
            written: Written::default(),
            writes: 0,
            fails_at: usize::MAX,
        },
    );
    assert!(
        !unmeasured.places(),
        "a writer with no length to count from places"
    );

    let torn = Session::writing(
        empty(&sample, "torn-for-good.jsonl"),
        Fragmenting {
            written: Written::default(),
            writes: 0,
            takes: 5,
        },
    );
    torn.append(&results_of(&["call-1"]));
    assert!(!torn.places(), "a log a fragment ended places");
}
