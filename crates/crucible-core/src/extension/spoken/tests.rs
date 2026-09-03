//! Tests for what one frame says.

use serde_json::{Value, json};

use super::{CallId, EXTENSION_SAID_BYTES, Malformed, Outcome, Spoken, SpokenError, Trouble, kind};

/// The message this frame says, or the panic that says it did not.
fn read(frame: &str) -> Spoken {
    Spoken::read(frame).unwrap_or_else(|err| panic!("{frame}: {err}"))
}

/// What went wrong with this frame, or the panic that says nothing did.
fn refused(frame: &str) -> SpokenError {
    Spoken::read(frame).expect_err(frame).clone()
}

#[test]
fn a_request_carries_its_call_its_method_and_whatever_rides_with_it() {
    let said = read(r#"{"id": 7, "method": "tools/list", "params": {"deep": [1, null]}}"#);
    assert_eq!(
        said,
        Spoken::Request {
            id: CallId::new(7),
            method: "tools/list".into(),
            params: json!({"deep": [1, null]}),
        }
    );
}

#[test]
fn a_telling_is_a_method_with_no_call_to_answer() {
    let said = read(r#"{"method": "session/started", "params": {"at": 1}}"#);
    assert_eq!(
        said,
        Spoken::Told {
            method: "session/started".into(),
            params: json!({"at": 1}),
        }
    );
}

#[test]
fn an_answer_says_which_call_it_settles_and_which_way_it_went() {
    let worked = read(r#"{"id": 7, "result": {"tools": []}}"#);
    assert_eq!(
        worked,
        Spoken::Answer {
            id: CallId::new(7),
            outcome: Outcome::Worked(json!({"tools": []})),
        }
    );

    let failed = read(r#"{"id": 7, "error": "the index would not open"}"#);
    let Spoken::Answer {
        id,
        outcome: Outcome::Failed(trouble),
    } = failed
    else {
        panic!("{failed:?}")
    };
    assert_eq!(id, CallId::new(7));
    assert_eq!(trouble.said(), "the index would not open");
}

#[test]
fn what_rides_inside_is_carried_whatever_shape_it_is() {
    // The names in there were chosen by whoever wrote the method. Crucible has
    // never read that documentation, so every one of these is as legitimate as
    // the object a reader would expect.
    for written in [
        r#""a string""#,
        "[1, 2, 3]",
        "3",
        "true",
        "null",
        r#"{"$ref": {"nested": {"deep": [{"and": "back"}]}}}"#,
    ] {
        let said = read(&format!(
            r#"{{"id": 1, "method": "m", "params": {written}}}"#
        ));
        let Spoken::Request { params, .. } = said else {
            panic!("{written}")
        };
        assert_eq!(
            params,
            serde_json::from_str::<Value>(written).expect("the sample parses")
        );
    }
}

#[test]
fn a_request_with_nothing_riding_along_says_so_rather_than_being_refused() {
    let said = read(r#"{"id": 1, "method": "tools/list"}"#);
    assert_eq!(
        said,
        Spoken::Request {
            id: CallId::new(1),
            method: "tools/list".into(),
            params: Value::Null,
        }
    );
}

#[test]
fn a_frame_that_is_not_json_is_refused_in_the_parsers_own_words() {
    let err = refused("{not json");
    assert!(matches!(err, SpokenError::NotJson { .. }), "{err:?}");
}

#[test]
fn a_frame_that_is_not_an_object_is_refused_saying_what_it_was() {
    for (written, called) in [
        ("[1, 2]", "a list"),
        (r#""a string""#, "a string"),
        ("3", "a number"),
        ("null", "nothing"),
        ("true", "a true or false"),
    ] {
        let err = refused(written);
        assert!(
            matches!(err, SpokenError::NotAnObject { found } if found == called),
            "{written}: {err:?}"
        );
    }
}

#[test]
fn an_object_that_says_nothing_usable_is_refused_saying_which_way() {
    for (written, problem) in [
        ("{}", Malformed::Silent),
        (r#"{"id": 1}"#, Malformed::Silent),
        (
            r#"{"id": 1, "method": "m", "result": {}}"#,
            Malformed::Muddled,
        ),
        (
            r#"{"id": 1, "result": {}, "error": "no"}"#,
            Malformed::Doubled,
        ),
        (r#"{"result": {}}"#, Malformed::Unasked),
        (r#"{"error": "no"}"#, Malformed::Unasked),
    ] {
        let err = refused(written);
        assert!(
            matches!(err, SpokenError::Malformed { problem: which } if which == problem),
            "{written}: {err:?}"
        );
        assert!(err.to_string().contains(problem.as_str()), "{err}");
    }
}

#[test]
fn a_call_identifier_that_is_not_a_whole_number_is_not_one() {
    for written in [
        r#"{"id": "7", "method": "m"}"#,
        r#"{"id": -1, "method": "m"}"#,
        r#"{"id": 1.5, "method": "m"}"#,
        r#"{"id": [7], "method": "m"}"#,
        r#"{"id": null, "result": {}}"#,
    ] {
        let err = refused(written);
        assert!(matches!(err, SpokenError::NotACall), "{written}: {err:?}");
    }
}

#[test]
fn a_method_name_is_held_to_its_ceiling_in_either_shape() {
    let over = "m".repeat(EXTENSION_SAID_BYTES + 1);
    for written in [
        format!(r#"{{"id": 1, "method": "{over}"}}"#),
        format!(r#"{{"method": "{over}"}}"#),
    ] {
        let err = refused(&written);
        assert!(
            matches!(
                err,
                SpokenError::TooLong { field, maximum, actual }
                    if field == "method" && maximum == EXTENSION_SAID_BYTES
                        && actual == EXTENSION_SAID_BYTES + 1
            ),
            "{err:?}"
        );
    }

    let err = refused(r#"{"id": 1, "method": 3}"#);
    assert!(
        matches!(err, SpokenError::WrongShape { field, .. } if field == "method"),
        "{err:?}"
    );

    let err = refused(r#"{"id": 1, "method": ""}"#);
    assert!(
        matches!(err, SpokenError::Empty { field } if field == "method"),
        "{err:?}"
    );
}

#[test]
fn the_words_of_a_failure_are_held_to_the_same_ceiling() {
    let over = "x".repeat(EXTENSION_SAID_BYTES + 1);
    let err = refused(&format!(r#"{{"id": 1, "error": "{over}"}}"#));
    assert!(
        matches!(
            err,
            SpokenError::TooLong { field, actual, .. }
                if field == "failure" && actual == EXTENSION_SAID_BYTES + 1
        ),
        "{err:?}"
    );

    let err = refused(r#"{"id": 1, "error": ""}"#);
    assert!(
        matches!(err, SpokenError::Empty { field } if field == "failure"),
        "{err:?}"
    );
}

#[test]
fn a_failure_that_is_not_words_is_refused_rather_than_described_by_crucible() {
    // An extension answering with a number where a sentence belongs has not
    // said what went wrong, and a sentence crucible wrote around the number
    // would be crucible explaining a failure it knows nothing about.
    for (written, was) in [
        (r#"{"id": 1, "error": {"code": 3}}"#, "an object"),
        (r#"{"id": 1, "error": 3}"#, "a number"),
        (r#"{"id": 1, "error": null}"#, "nothing"),
        (r#"{"id": 1, "error": ["no"]}"#, "a list"),
    ] {
        let err = refused(written);
        assert!(
            matches!(
                err,
                SpokenError::WrongShape { field, found, wanted }
                    if field == "failure" && found == was && wanted == "a string"
            ),
            "{written}: {err:?}"
        );
    }
}

#[test]
fn every_message_survives_being_written_and_read_back() {
    let sent = [
        Spoken::Request {
            id: CallId::new(1),
            method: "tools/call".into(),
            params: json!({"name": "grep", "args": {"pattern": "x"}}),
        },
        Spoken::Request {
            id: CallId::new(u64::MAX),
            method: "m".into(),
            params: Value::Null,
        },
        Spoken::Told {
            method: "session/ended".into(),
            params: json!([1, "two", null]),
        },
        Spoken::Answer {
            id: CallId::new(2),
            outcome: Outcome::Worked(json!({"ok": true})),
        },
        Spoken::Answer {
            id: CallId::new(3),
            outcome: Outcome::Failed(Trouble::new("the index would not open").unwrap()),
        },
    ];
    for one in sent {
        assert_eq!(read(&one.written()), one, "{one:?}");
    }
}

#[test]
fn nothing_written_can_end_a_frame_early() {
    // The boundary belongs to the framing above. A payload that could put a
    // newline here is a payload choosing where crucible's own frames end.
    let said = Spoken::Request {
        id: CallId::new(1),
        method: "m".into(),
        params: json!({"forged": "a\nb", "deep": ["\n", {"also\n": "\r\n"}]}),
    };
    let written = said.written();
    assert!(!written.contains('\n'), "{written}");
    assert!(!written.contains('\r'), "{written}");
    // And it still says what it said.
    assert_eq!(read(&written), said);
}

#[test]
fn no_two_ways_of_saying_nothing_read_the_same() {
    // Not that the list is complete — Rust cannot enumerate a variant, and the
    // module says as much. What this catches is the other half: two of them
    // reading the same, which turns a report into one that names nothing.
    let mut spellings: Vec<&str> = Malformed::EVERY.iter().map(|one| one.as_str()).collect();
    spellings.sort_unstable();
    let mut apart = spellings.clone();
    apart.dedup();
    assert_eq!(spellings, apart, "two of them read the same");
    assert!(spellings.iter().all(|one| !one.is_empty()));
}

#[test]
fn what_a_value_is_called_is_settled_for_every_shape_there_is() {
    for (value, called) in [
        (Value::Null, "nothing"),
        (json!(true), "a true or false"),
        (json!(1), "a number"),
        (json!("s"), "a string"),
        (json!([]), "a list"),
        (json!({}), "an object"),
    ] {
        assert_eq!(kind(&value), called, "{value}");
    }
}
