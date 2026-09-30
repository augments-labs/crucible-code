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
