//! What a sessions list says, and what it refuses to say.

use std::str::FromStr as _;

use serde_json::{Value, json};

use super::*;
use crate::bounds::{ITEMS, TEXT_BYTES};

fn id(nth: u64) -> SessionId {
    SessionId::from_str(&format!(
        "{:013}-0000{nth:02x}",
        1_700_000_000_000_u64 + nth
    ))
    .unwrap()
}

fn session(nth: u64, words: Option<&Text>) -> Session {
    Session {
        id: id(nth),
        branch: words.cloned(),
        messages: nth * 2,
        title: words.cloned(),
    }
}

/// One report for every status, and between them a session with and without
/// its branch and title.
pub(crate) fn reports(marked: &Text) -> Vec<Report> {
    vec![
        Report::Listed(Listing {
            sessions: vec![session(2, Some(marked)), session(1, None)],
            omitted: 0,
            unreadable: 0,
            index_full: false,
            unindexed: false,
        }),
        Report::Listed(Listing {
            sessions: vec![session(3, None)],
            omitted: 4,
            unreadable: 1,
            index_full: true,
            unindexed: false,
        }),
        Report::Failed(marked.clone()),
    ]
}

fn plain() -> Text {
    Text::cut("words")
}

/// The specimen at `at`, as a document to edit.
fn document(at: usize) -> Value {
    value(reports(&plain()).get(at).unwrap())
}

fn value(report: &Report) -> Value {
    serde_json::from_slice(&report.encode().unwrap()).unwrap()
}

fn at<'a>(document: &'a Value, pointer: &str) -> &'a Value {
    document.pointer(pointer).unwrap()
}

/// `document` with the field at `pointer` set to `to`, adding it where absent.
fn with(mut document: Value, pointer: &str, to: Value) -> Value {
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    document
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .unwrap()
        .insert(key.to_owned(), to);
    document
}

fn refused(document: &Value) -> ErrorCode {
    Report::decode(&serde_json::to_vec(document).unwrap())
        .unwrap_err()
        .code()
}

fn listing(sessions: Vec<Session>) -> Report {
    Report::Listed(Listing {
        sessions,
        omitted: 0,
        unreadable: 0,
        index_full: false,
        unindexed: false,
    })
}

#[test]
fn every_report_is_one_line_naming_its_format_kind_and_status() {
    let statuses: Vec<&str> = reports(&plain()).iter().map(Report::status).collect();
    assert_eq!(statuses, ["complete", "incomplete", "failed"]);
    let exits: Vec<u8> = reports(&plain()).iter().map(Report::exit).collect();
    assert_eq!(exits, [0, 0, 1]);

    for report in reports(&plain()) {
        let written = report.encode().unwrap();
        let line = written.strip_suffix(b"\n").unwrap();
        assert!(!line.contains(&b'\n'));
        let document = value(&report);
        assert_eq!(at(&document, "/format_version"), &json!(1));
        assert_eq!(at(&document, "/kind"), &json!("sessions"));
        assert_eq!(at(&document, "/status"), &json!(report.status()));
        assert_eq!(at(&document, "/truncated"), &json!(false));
        assert_eq!(Report::decode(&written).unwrap(), report);
    }
}

#[test]
fn each_session_crosses_with_its_id_start_count_branch_and_title() {
    let document = document(0);
    let first = id(2);
    let started = first
        .started()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    assert_eq!(at(&document, "/sessions/0/id"), &json!(first.as_str()));
    assert_eq!(at(&document, "/sessions/0/started"), &json!(started));
    assert_eq!(at(&document, "/sessions/0/messages"), &json!(4));
    assert_eq!(at(&document, "/sessions/0/branch/text"), &json!("words"));
    assert_eq!(at(&document, "/sessions/0/title/text"), &json!("words"));
    assert_eq!(document.pointer("/sessions/1/branch"), None);
    assert_eq!(document.pointer("/sessions/1/title"), None);
    assert_eq!(at(&document, "/omitted"), &json!(0));

    let incomplete = self::document(1);
    assert_eq!(at(&incomplete, "/omitted"), &json!(4));
    assert_eq!(at(&incomplete, "/unreadable"), &json!(1));
    assert_eq!(at(&incomplete, "/index_full"), &json!(true));
    assert_eq!(at(&incomplete, "/unindexed"), &json!(false));

    let failed = self::document(2);
    assert_eq!(at(&failed, "/problem/text"), &json!("words"));
    assert_eq!(failed.pointer("/sessions"), None);
}

#[test]
fn anything_left_out_makes_a_list_incomplete() {
    let whole = || Listing {
        sessions: vec![session(1, None)],
        omitted: 0,
        unreadable: 0,
        index_full: false,
        unindexed: false,
    };
    assert_eq!(Report::Listed(whole()).status(), "complete");

    let mut omitted = whole();
    omitted.omitted = 1;
    let mut unreadable = whole();
    unreadable.unreadable = 1;
    let mut full = whole();
    full.index_full = true;
    let mut unindexed = whole();
    unindexed.unindexed = true;
    let mut cut = whole();
    cut.sessions = vec![session(1, Some(&Text::cut(&"a".repeat(TEXT_BYTES + 1))))];
    for (what, listing) in [
        ("omitted", omitted),
        ("unreadable", unreadable),
        ("full", full),
        ("unindexed", unindexed),
        ("cut", cut),
    ] {
        let report = Report::Listed(listing);
        assert_eq!(report.status(), "incomplete", "{what}");
        assert_eq!(report.exit(), 0, "{what}");
        assert_eq!(
            Report::decode(&report.encode().unwrap()).unwrap(),
            report,
            "{what}"
        );
    }
}

#[test]
fn another_format_version_is_refused_by_name() {
    for version in [0, 2, u64::MAX] {
        let other = with(document(0), "/format_version", json!(version));
        assert_eq!(refused(&other), ErrorCode::UnsupportedVersion);
    }
}

#[test]
fn a_status_or_truncation_flag_that_disagrees_with_the_list_is_refused() {
    for (specimen, status) in [(0, "incomplete"), (1, "complete"), (0, "failed")] {
        let lying = with(document(specimen), "/status", json!(status));
        assert_eq!(refused(&lying), ErrorCode::Malformed, "{specimen} {status}");
    }
    let lying = with(document(0), "/truncated", json!(true));
    assert_eq!(refused(&lying), ErrorCode::Malformed);
}

#[test]
fn a_start_that_is_not_the_one_its_id_names_is_refused() {
    let lying = with(document(0), "/sessions/0/started", json!(1));
    assert_eq!(refused(&lying), ErrorCode::Malformed);
    let unnamed = with(document(0), "/sessions/0/id", json!("not an id"));
    assert_eq!(refused(&unnamed), ErrorCode::Malformed);
}

#[test]
fn a_field_nobody_wrote_is_refused() {
    let added = with(document(0), "/sessions/0/prompt", json!("what was asked"));
    assert_eq!(refused(&added), ErrorCode::Malformed);
    let added = with(document(2), "/sessions", json!([]));
    assert_eq!(refused(&added), ErrorCode::Malformed);
}

#[test]
fn a_session_said_twice_is_refused() {
    let once = listing(vec![session(1, None), session(2, None)]);
    let document = value(&once);
    let twice = listing(vec![session(1, None), session(1, None)]);
    assert_eq!(twice.encode().unwrap_err().code(), ErrorCode::Malformed);

    let repeated = with(
        document.clone(),
        "/sessions",
        json!([at(&document, "/sessions/0"), at(&document, "/sessions/0")]),
    );
    assert_eq!(refused(&repeated), ErrorCode::Malformed);
}

#[test]
fn more_sessions_than_a_list_holds_are_refused_rather_than_cut() {
    let many = listing(
        (0..=u64::try_from(ITEMS).unwrap())
            .map(|nth| session(nth, None))
            .collect(),
    );
    assert_eq!(many.encode().unwrap_err().code(), ErrorCode::TooLarge);
    let most = listing(
        (0..u64::try_from(ITEMS).unwrap())
            .map(|nth| session(nth, None))
            .collect(),
    );
    assert_eq!(Report::decode(&most.encode().unwrap()).unwrap(), most);
}
