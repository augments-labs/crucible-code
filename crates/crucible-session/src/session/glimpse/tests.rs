//! What the end of a log shows a picker, and what it keeps from one.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::str::FromStr as _;

use crucible_core::{Message, SessionId, ToolCall, ToolId, ToolOutput, ToolResult};

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
        wire::line(&Message::said(user)),
        wire::line(&Message::Agent {
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

/// What each message of a glimpse says, in the order it says it.
fn said(glimpse: &Glimpse) -> Vec<(bool, String)> {
    glimpse
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::User { text, .. } => Some((true, text.to_string())),
            Message::Agent { text, .. } => Some((false, text.to_string())),
            Message::Context(_) | Message::ToolResults(_) => None,
        })
        .collect()
}

#[test]
fn a_small_log_comes_back_whole_oldest_first() {
    let sample = Sample::new("glimpse-whole");
    let mut lines = spoken("does the caret drift", "it does, one cell per wrap");
    lines.extend(spoken("since when", "since the resize handler moved"));
    planted(&sample, "0000000000001-000001", &lines);

    let glimpse = glimpsed(&sample, "0000000000001-000001");

    assert_eq!(
        said(&glimpse),
        [
            (true, "does the caret drift".to_owned()),
            (false, "it does, one cell per wrap".to_owned()),
            (true, "since when".to_owned()),
            (false, "since the resize handler moved".to_owned()),
        ]
    );
    assert!(!glimpse.cut());
    assert!(!glimpse.busy());
}

#[test]
fn the_calls_a_turn_made_and_what_came_back_are_kept_with_it() {
    // What a session looks like is its tool work as much as its prose, and a
    // preview that dropped both would show a conversation nobody had: an
    // answer that mentions a file, with nothing saying the file was read.
    let sample = Sample::new("glimpse-calls");
    let lines = vec![
        wire::line(&Message::said("read the config")),
        wire::line(&Message::Agent {
            text: "I will look at it.".into(),
            calls: vec![ToolCall {
                id: ToolId::new("c-1"),
                name: "read".into(),
                args: crucible_core::ToolArgs::new(r#"{"path":"crucible.json"}"#),
            }],
            stop: None,
        }),
        wire::line(&Message::ToolResults(vec![ToolResult {
            id: ToolId::new("c-1"),
            output: ToolOutput::ok("theme = midnight"),
        }])),
    ];
    planted(&sample, "0000000000001-000001", &lines);

    let glimpse = glimpsed(&sample, "0000000000001-000001");

    let called = glimpse.messages().iter().find_map(|message| match message {
        Message::Agent { calls, .. } => calls.first(),
        _ => None,
    });
    assert_eq!(
        called.map(|call| &*call.name),
        Some("read"),
        "the call the turn made is missing"
    );

    let back = glimpse.messages().iter().find_map(|message| match message {
        Message::ToolResults(results) => results.first(),
        _ => None,
    });
    assert_eq!(
        back.map(|result| result.output.text()),
        Some("theme = midnight"),
        "what came back is missing"
    );
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
    let last = said(&glimpse).pop().expect("the newest message");
    assert!(!last.0);
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
    let texts: Vec<String> = said(&glimpse).into_iter().map(|(_, text)| text).collect();
    assert_eq!(texts, ["the whole question", "the whole answer"]);
}

#[test]
fn a_session_another_crucible_holds_open_says_it_is_busy() {
    let sample = Sample::new("glimpse-busy");
    let session = crate::Session::start(&sample.logs(), &sample.workspace(), None)
        .expect("a session to start");
    session.append(&Message::said("still being written"));
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

    let texts: Vec<String> = said(&glimpse).into_iter().map(|(_, text)| text).collect();
    assert_eq!(texts, ["two\nlines[31m", "an answer"]);
}

#[test]
fn what_a_terminal_would_act_on_does_not_survive_a_tool_result_either() {
    // A result is text a tool produced from a file, a process, or the network,
    // and it reaches the preview pane through the same door prose does.
    let sample = Sample::new("glimpse-control-result");
    let lines = vec![wire::line(&Message::ToolResults(vec![ToolResult {
        id: ToolId::new("c-1"),
        output: ToolOutput::ok("two\nlines\u{1b}[31m\u{7}"),
    }]))];
    planted(&sample, "0000000000001-000001", &lines);

    let glimpse = glimpsed(&sample, "0000000000001-000001");

    let back = glimpse.messages().iter().find_map(|message| match message {
        Message::ToolResults(results) => results.first(),
        _ => None,
    });
    assert_eq!(
        back.map(|result| result.output.text()),
        Some("two\nlines[31m")
    );
}
