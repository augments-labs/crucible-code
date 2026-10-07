//! What a doctor's report says, and what it refuses to say.

use serde_json::{Value, json};

use super::*;
use crate::bounds::TEXT_BYTES;

fn check(id: &str, status: Status, words: &Text) -> Check {
    Check {
        id: Name::new(id).unwrap(),
        status,
        reason: words.clone(),
        remedy: (status != Status::Ok).then(|| words.clone()),
    }
}

/// One report for every overall status, and between them every arm of a
/// check: an unavailable one with and without a remedy.
pub(crate) fn reports(marked: &Text) -> Vec<Report> {
    let mut unexplained = check("later", Status::Unavailable, marked);
    unexplained.remedy = None;
    vec![
        Report {
            checks: vec![check("first", Status::Ok, marked), unexplained.clone()],
        },
        Report {
            checks: vec![
                check("first", Status::Ok, marked),
                check("second", Status::Warning, marked),
            ],
        },
        Report {
            checks: vec![
                check("first", Status::Failed, marked),
                check("second", Status::Warning, marked),
                check("third", Status::Unavailable, marked),
            ],
        },
    ]
}

fn plain() -> Text {
    Text::cut("words")
}

/// The specimen at `at`, as a document to edit.
fn document(at: usize) -> Value {
    value(reports(&plain()).get(at).unwrap())
}

/// The document `report` is.
fn value(report: &Report) -> Value {
    serde_json::from_slice(&report.encode().unwrap()).unwrap()
}

/// The field at `pointer`.
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

/// `document` without the field at `pointer`.
fn without(mut document: Value, pointer: &str) -> Value {
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    document
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .unwrap()
        .remove(key);
    document
}

/// What decoding `document` is refused with.
fn refused(document: &Value) -> ErrorCode {
    Report::decode(&serde_json::to_vec(document).unwrap())
        .unwrap_err()
        .code()
}

#[test]
fn every_report_is_one_line_naming_its_format_kind_and_status() {
    let statuses: Vec<&str> = reports(&plain()).iter().map(Report::status).collect();
    assert_eq!(statuses, ["healthy", "warnings", "failed"]);

    for report in reports(&plain()) {
        let written = report.encode().unwrap();
        let line = written.strip_suffix(b"\n").unwrap();
        assert!(!line.contains(&b'\n'));
        let document = value(&report);
        assert_eq!(at(&document, "/format_version"), &json!(1));
        assert_eq!(at(&document, "/kind"), &json!("doctor"));
        assert_eq!(at(&document, "/status"), &json!(report.status()));
        assert_eq!(at(&document, "/truncated"), &json!(false));
        assert_eq!(Report::decode(&written).unwrap(), report);
    }
}

#[test]
fn the_exit_code_is_two_for_a_failure_one_for_a_warning_and_zero_otherwise() {
    let codes: Vec<u8> = reports(&plain()).iter().map(Report::exit).collect();
    assert_eq!(codes, [0, 1, 2]);

    // A check that could not run is not a problem of its own: whatever kept it
    // from running is the check that says so.
    let alone = Report {
        checks: vec![check("later", Status::Unavailable, &plain())],
    };
    assert_eq!((alone.status(), alone.exit()), ("healthy", 0));
}

#[test]
fn each_check_crosses_with_its_id_status_reason_and_remedy() {
    let document = document(2);
    assert_eq!(at(&document, "/checks/0/id"), &json!("first"));
    assert_eq!(at(&document, "/checks/0/status"), &json!("failed"));
    assert_eq!(at(&document, "/checks/0/reason/text"), &json!("words"));
    assert_eq!(at(&document, "/checks/0/remedy/text"), &json!("words"));
    assert_eq!(at(&document, "/checks/1/status"), &json!("warning"));
    assert_eq!(at(&document, "/checks/2/status"), &json!("unavailable"));
    assert_eq!(at(&self::document(0), "/checks/0/status"), &json!("ok"));
}

#[test]
fn another_format_version_is_refused_by_name() {
    for version in [0, 2, u64::MAX] {
        let other = with(document(0), "/format_version", json!(version));
        assert_eq!(refused(&other), ErrorCode::UnsupportedVersion);
    }
}

#[test]
fn a_problem_without_a_remedy_or_a_success_with_one_is_refused() {
    for (specimen, check) in [(1, 1), (2, 0)] {
        let bare = without(document(specimen), &format!("/checks/{check}/remedy"));
        assert_eq!(refused(&bare), ErrorCode::Malformed, "{specimen}/{check}");
    }
    let mut bare = reports(&plain()).swap_remove(1);
    bare.checks.get_mut(1).unwrap().remedy = None;
    assert_eq!(bare.encode().unwrap_err().code(), ErrorCode::Malformed);

    let told = with(
        document(0),
        "/checks/0/remedy",
        at(&document(1), "/checks/1/remedy").clone(),
    );
    assert_eq!(refused(&told), ErrorCode::Malformed);
}

#[test]
fn a_check_named_twice_is_refused() {
    let mut twice = reports(&plain()).swap_remove(1);
    twice.checks.get_mut(1).unwrap().id = Name::new("first").unwrap();
    assert_eq!(twice.encode().unwrap_err().code(), ErrorCode::Malformed);

    let twice = with(document(1), "/checks/1/id", json!("first"));
    assert_eq!(refused(&twice), ErrorCode::Malformed);
}

#[test]
fn a_status_that_disagrees_with_the_checks_is_refused() {
    for (specimen, status) in [(0, "warnings"), (1, "healthy"), (2, "warnings"), (0, "ill")] {
        let other = with(document(specimen), "/status", json!(status));
        assert_eq!(
            refused(&other),
            ErrorCode::Malformed,
            "{specimen} as {status}"
        );
    }
    let other = with(document(0), "/checks/0/status", json!("fine"));
    assert_eq!(refused(&other), ErrorCode::Malformed);
}

#[test]
fn what_was_cut_is_said_at_the_top() {
    let long = Text::cut(&"é".repeat(TEXT_BYTES));
    assert!(long.truncated());
    for report in reports(&long) {
        let document = value(&report);
        assert_eq!(at(&document, "/truncated"), &json!(true));
        assert_eq!(Report::decode(&report.encode().unwrap()).unwrap(), report);

        let hidden = with(document, "/truncated", json!(false));
        assert_eq!(refused(&hidden), ErrorCode::Malformed);
    }
    let claimed = with(document(0), "/truncated", json!(true));
    assert_eq!(refused(&claimed), ErrorCode::Malformed);
}

#[test]
fn a_list_over_its_ceiling_is_refused_rather_than_cut() {
    let many = Report {
        checks: (0..=ITEMS)
            .map(|at| check(&format!("check-{at}"), Status::Ok, &plain()))
            .collect(),
    };
    assert_eq!(many.encode().unwrap_err().code(), ErrorCode::TooLarge);

    let good = document(0);
    let one = at(&good, "/checks/0").clone();
    let over = with(good, "/checks", Value::Array(vec![one; ITEMS + 1]));
    assert_eq!(refused(&over), ErrorCode::TooLarge);
}

#[test]
fn a_field_nobody_asked_for_refuses_the_document() {
    for specimen in 0..3 {
        let other = with(document(specimen), "/extra", json!(1));
        assert_eq!(refused(&other), ErrorCode::Malformed, "{specimen}");
        let other = with(document(specimen), "/checks/0/extra", json!(1));
        assert_eq!(refused(&other), ErrorCode::Malformed, "{specimen}");
    }
}
