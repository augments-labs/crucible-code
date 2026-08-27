//! What the end of a log shows a picker, and what it keeps from one.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::str::FromStr as _;

use crucible_core::SessionId;

use super::*;
use crate::sample::Sample;

/// A log for this sample's own workspace, holding `lines` after its header.
fn planted(sample: &Sample, id: &str, lines: &[String]) {
    let mut all = vec![sample.header(wire::FORMAT, id)];
    all.extend(lines.iter().cloned());
    sample.plant(id, &all);
}

/// A user line and an agent line, spelled by the same code that writes logs.
fn spoken(user: &str, agent: &str) -> Vec<String> {
    vec![
        wire::line(&crucible_core::Message::said(user)),
        wire::line(&crucible_core::Message::Agent {
            text: agent.into(),
            calls: Vec::new(),
            stop: None,
        }),
    ]
}

/// The glimpse of `id` in this sample, which the test expects to get one.
fn glimpsed(sample: &Sample, id: &str) -> Glimpse {
    let id = SessionId::from_str(id).expect("a well-formed session id");
    glimpse(&sample.logs(), &sample.workspace(), &id).expect("a session to glimpse")
}

#[test]
fn a_small_log_comes_back_whole_oldest_first() {
    let sample = Sample::new("glimpse-whole");
    let mut lines = spoken("does the caret drift", "it does, one cell per wrap");
    lines.extend(spoken("since when", "since the resize handler moved"));
    planted(&sample, "0000000000001-000001", &lines);

    let glimpse = glimpsed(&sample, "0000000000001-000001");

    let said: Vec<(bool, &str)> = glimpse
        .said()
        .iter()
        .map(|said| (said.user(), said.text()))
        .collect();
    assert_eq!(
        said,
        [
            (true, "does the caret drift"),
            (false, "it does, one cell per wrap"),
            (true, "since when"),
            (false, "since the resize handler moved"),
        ]
    );
    assert!(!glimpse.cut());
    assert!(!glimpse.busy());
}

#[test]
fn a_log_wider_than_the_window_says_it_was_cut() {
    // The read is bounded from the end, so an early conversation this long
    // cannot be in it — and a glimpse that read as everything would be a lie
    // about the session it previews.
    let sample = Sample::new("glimpse-cut");
    let mut lines = Vec::new();
    for nth in 0..64 {
        lines.extend(spoken(
            &format!("question {nth}"),
            &"a long answer ".repeat(256),
        ));
    }
    planted(&sample, "0000000000001-000001", &lines);

    let glimpse = glimpsed(&sample, "0000000000001-000001");

    assert!(glimpse.cut());
    let last = glimpse.said().last().expect("the newest message");
    assert!(!glimpse.said().is_empty());
    assert!(!last.user());
}

#[test]
fn a_line_a_crash_tore_in_half_is_left_out_and_says_so() {
    let sample = Sample::new("glimpse-torn");
    planted(
        &sample,
        "0000000000001-000001",
        &spoken("the whole question", "the whole answer"),
    );

    let path = sample.logs().join("0000000000001-000001.jsonl");
    let mut log = OpenOptions::new()
        .append(true)
        .open(path)
        .expect("the log just planted");
    write!(log, "{{\"user\":\"a line the process never fini").expect("an append");
    drop(log);

    let glimpse = glimpsed(&sample, "0000000000001-000001");

    assert!(glimpse.cut());
    let texts: Vec<&str> = glimpse.said().iter().map(Said::text).collect();
    assert_eq!(texts, ["the whole question", "the whole answer"]);
}

#[test]
fn a_session_another_crucible_holds_open_says_it_is_busy() {
    let sample = Sample::new("glimpse-busy");
    let session = crate::Session::start(&sample.logs(), &sample.workspace(), None)
        .expect("a session to start");
    session.append(&crucible_core::Message::said("still being written"));
    let id = session.id().expect("a recorded session has a name").clone();

    let held = glimpse(&sample.logs(), &sample.workspace(), &id).expect("a glimpse while open");
    assert!(held.busy());

    drop(session.finish());

    let released = glimpse(&sample.logs(), &sample.workspace(), &id).expect("a glimpse after");
    assert!(!released.busy());
}

#[test]
fn a_session_of_a_different_workspace_is_not_glimpsed() {
    // The same refusal `/resume` gives the id: naming a session is a shorter
    // way to reach one, not a way past whose it is.
    let sample = Sample::new("glimpse-elsewhere");
    planted(
        &sample,
        "0000000000001-000001",
        &spoken("their question", "their answer"),
    );

    let id = SessionId::from_str("0000000000001-000001").expect("a well-formed session id");
    let refused = glimpse(&sample.logs(), &sample.elsewhere(), &id);

    assert!(matches!(refused, Err(crate::SessionError::Unknown { .. })));
}

#[test]
fn what_a_terminal_would_act_on_does_not_survive_the_read() {
    // The text goes to a screen, and a file on disk can claim anything.
    let sample = Sample::new("glimpse-control");
    planted(
        &sample,
        "0000000000001-000001",
        &spoken("two\nlines\u{1b}[31m", "an answer\u{7}"),
    );

    let glimpse = glimpsed(&sample, "0000000000001-000001");

    let texts: Vec<&str> = glimpse.said().iter().map(Said::text).collect();
    assert_eq!(texts, ["two\nlines[31m", "an answer"]);
}
