//! Frames a server could send, including the ones it should not.

use serde_json::json;

use super::{Call, Garbled, Heard, NO_SUCH_METHOD, RPC, Reply, SAID_BYTES, Sent};

#[test]
fn an_answer_settles_the_call_it_names() {
    let heard =
        Heard::read(r#"{"jsonrpc":"2.0","id":7,"result":{"tools":[]}}"#).expect("an answer");

    assert_eq!(
        heard,
        Heard::Answer {
            call: Call::new(7),
            reply: Reply::Worked(json!({ "tools": [] })),
        }
    );
}

#[test]
fn a_failure_keeps_the_code_and_the_words_the_server_chose() {
    let heard =
        Heard::read(r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32602,"message":"no such tool"}}"#)
            .expect("a failure");

    assert_eq!(
        heard,
        Heard::Answer {
            call: Call::new(2),
            reply: Reply::Failed {
                code: -32602,
                said: "no such tool".into(),
            },
        }
    );
}

#[test]
fn a_notification_is_told_apart_from_a_question_by_having_no_identifier() {
    let told =
        Heard::read(r#"{"jsonrpc":"2.0","method":"notifications/message","params":{"a":1}}"#)
            .expect("a notification");
    assert_eq!(
        told,
        Heard::Told {
            method: "notifications/message".into(),
            params: json!({ "a": 1 }),
        }
    );

    let asked = Heard::read(r#"{"jsonrpc":"2.0","id":4,"method":"sampling/createMessage"}"#)
        .expect("a question");
    assert_eq!(
        asked,
        Heard::Asked {
            call: Call::new(4),
            method: "sampling/createMessage".into(),
        }
    );
}

#[test]
fn a_frame_that_does_not_say_which_protocol_it_is_speaking_is_refused() {
    // The one member both ends agree on before anything else means anything.
    // Assumed, it would let a frame from something that is not an MCP server
    // at all be read as one.
    let refused = Heard::read(r#"{"id":1,"result":{}}"#).expect_err("no version");
    assert_eq!(refused, Garbled::Unversioned);

    let refused = Heard::read(r#"{"jsonrpc":"1.0","id":1,"result":{}}"#).expect_err("wrong");
    assert_eq!(refused, Garbled::Unversioned);
}

#[test]
fn an_answer_to_a_call_crucible_could_not_have_made_is_refused() {
    // Crucible numbers its calls, so a string is somebody else's identifier and
    // a negative number is nobody's. Matching either loosely would settle a
    // call crucible is still waiting on with an answer to a different one.
    for id in ["\"abc\"", "-1", "1.5"] {
        let frame = format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{}}}}"#);
        let refused = Heard::read(&frame).expect_err(id);
        assert_eq!(refused, Garbled::NotACall { found: id.into() }, "{id}");
    }

    // `null` is what JSON-RPC puts there when it could not read the identifier
    // at all, so it names no call either.
    let refused = Heard::read(r#"{"jsonrpc":"2.0","id":null,"result":{}}"#).expect_err("null");
    assert_eq!(
        refused,
        Garbled::NotACall {
            found: "null".into()
        }
    );
}

#[test]
fn an_identifier_too_long_to_repeat_is_refused_rather_than_quoted_back() {
    // The spelling of an unreadable identifier goes into a sentence somebody
    // reads, and it is text off a pipe: a frame is a megabyte, so without a
    // ceiling the sentence could be one.
    let id = format!("\"{}\"", "x".repeat(SAID_BYTES));
    let frame = format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{}}}}"#);

    let refused = Heard::read(&frame).expect_err("too long to keep");

    assert!(
        matches!(refused, Garbled::TooLong { field: "id", .. }),
        "{refused:?}"
    );
}

#[test]
fn a_frame_that_is_two_shapes_at_once_is_not_guessed_at() {
    let refused =
        Heard::read(r#"{"jsonrpc":"2.0","id":1,"result":{},"error":{"code":1,"message":"x"}}"#)
            .expect_err("both");
    assert_eq!(refused, Garbled::Shapeless);

    let refused = Heard::read(r#"{"jsonrpc":"2.0","id":1}"#).expect_err("neither");
    assert_eq!(refused, Garbled::Shapeless);
}

#[test]
fn an_answer_with_no_identifier_settles_nothing() {
    let refused = Heard::read(r#"{"jsonrpc":"2.0","result":{}}"#).expect_err("no id");
    assert_eq!(refused, Garbled::Shapeless);
}

#[test]
fn a_failure_that_is_not_the_shape_the_standard_gives_is_refused() {
    let refused =
        Heard::read(r#"{"jsonrpc":"2.0","id":1,"error":"broken"}"#).expect_err("a string");
    assert_eq!(
        refused,
        Garbled::WrongKind {
            field: "error",
            found: "a string",
            wanted: "an object",
        }
    );

    let refused =
        Heard::read(r#"{"jsonrpc":"2.0","id":1,"error":{"message":"x"}}"#).expect_err("no code");
    assert_eq!(
        refused,
        Garbled::WrongKind {
            field: "error.code",
            found: "nothing",
            wanted: "an integer",
        }
    );
}

#[test]
fn text_a_server_chose_the_length_of_is_held_to_a_ceiling() {
    // A method name goes into what this crate matches on and a failure's words
    // go on a screen. Neither is somewhere the far end picks the size.
    let long = "x".repeat(SAID_BYTES + 1);

    let frame = json!({ "jsonrpc": RPC, "method": long }).to_string();
    let refused = Heard::read(&frame).expect_err("a long method");
    assert!(
        matches!(&refused, Garbled::TooLong { field, maximum, .. }
            if *field == "method" && *maximum == SAID_BYTES),
        "{refused}"
    );

    let frame =
        json!({ "jsonrpc": RPC, "id": 1, "error": { "code": 1, "message": long } }).to_string();
    let refused = Heard::read(&frame).expect_err("a long message");
    assert!(
        matches!(&refused, Garbled::TooLong { field, .. } if *field == "error.message"),
        "{refused}"
    );
}

#[test]
fn a_frame_that_is_not_a_json_object_is_refused_with_what_it_was() {
    let refused = Heard::read("[1, 2]").expect_err("a list");
    assert_eq!(refused, Garbled::NotAMessage { found: "a list" });

    let refused = Heard::read("not json").expect_err("not json");
    assert!(matches!(refused, Garbled::Unparsed { .. }), "{refused}");
}

#[test]
fn everything_crucible_sends_states_the_protocol_and_carries_no_newline() {
    // The framing is a line, so a frame holding one would let whatever composed
    // it decide where crucible's messages end. serde_json escapes a newline
    // inside a string, and this is the test that says so rather than assuming.
    let sent = [
        Sent::asking(Call::new(1), "tools/list", &json!({ "cursor": "a\nb" })),
        Sent::telling("notifications/initialized", &json!({})),
        Sent::refusing(Call::new(3), "sampling/createMessage"),
    ];

    for frame in sent.iter().map(Sent::frame) {
        assert!(!frame.contains('\n'), "{frame}");
        let read = serde_json::from_str::<serde_json::Value>(&frame).expect("json");
        assert_eq!(
            read.get("jsonrpc").and_then(|held| held.as_str()),
            Some(RPC)
        );
    }
}

#[test]
fn a_question_crucible_will_not_answer_is_refused_rather_than_left_waiting() {
    // A server that asked and heard nothing waits, and what it is waiting on is
    // the work crucible asked it for.
    let frame = Sent::refusing(Call::new(9), "roots/list").frame();
    let read: serde_json::Value = serde_json::from_str(&frame).expect("json");

    assert_eq!(read.get("id").and_then(serde_json::Value::as_u64), Some(9));
    assert_eq!(
        read.pointer("/error/code")
            .and_then(serde_json::Value::as_i64),
        Some(NO_SUCH_METHOD)
    );
}
