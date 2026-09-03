//! Conversations a server could hold, including the ones it should not.

use std::io::Cursor;

use serde_json::{Value, json};

use super::{ASIDES, Talking, Trouble};
use crate::wire::NO_SUCH_METHOD;

/// Holds a conversation against a fixed script and hands back what crucible
/// said.
///
/// The script is what the server sends, one frame per line, and it is written
/// out in full before crucible reads a byte — which is exactly the shape a
/// server that answers before crucible has finished asking would have, and one
/// this crate has to be right about either way.
fn against(script: &str, calls: &[(&str, Value)]) -> (Vec<Result<Value, Trouble>>, Vec<String>) {
    let mut said = Vec::new();
    let answers = {
        let mut talking = Talking::new(Cursor::new(script.to_owned()), &mut said);
        calls
            .iter()
            .map(|(method, params)| talking.ask(method, params))
            .collect()
    };
    let said = String::from_utf8(said).expect("crucible writes text");
    (
        answers,
        said.lines().map(ToOwned::to_owned).collect::<Vec<_>>(),
    )
}

#[test]
fn a_call_comes_back_with_what_the_server_answered() {
    let (answers, said) = against(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[]}}\n",
        &[("tools/list", json!({}))],
    );

    assert_eq!(
        answers.first().map(|answer| answer.as_ref().ok()),
        Some(Some(&json!({ "tools": [] })))
    );

    let sent: Value = serde_json::from_str(said.first().expect("one frame")).expect("json");
    assert_eq!(
        sent.get("method").and_then(Value::as_str),
        Some("tools/list")
    );
    assert_eq!(sent.get("id").and_then(Value::as_u64), Some(1));
}

#[test]
fn each_call_is_numbered_afresh_so_two_answers_cannot_be_swapped() {
    let (answers, said) = against(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"first\"}\n\
         {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":\"second\"}\n",
        &[("one", json!({})), ("two", json!({}))],
    );

    let read = |answer: &Result<Value, Trouble>| answer.as_ref().expect("answered").clone();
    assert_eq!(
        answers.iter().map(read).collect::<Vec<_>>(),
        ["first", "second"]
    );

    let numbers = said
        .iter()
        .map(|frame| {
            serde_json::from_str::<Value>(frame)
                .expect("json")
                .get("id")
                .and_then(Value::as_u64)
        })
        .collect::<Vec<_>>();
    assert_eq!(numbers, [Some(1), Some(2)]);
}

#[test]
fn an_answer_to_a_call_crucible_is_not_waiting_on_stops_the_conversation() {
    // Reading on would mean matching every later answer to the wrong question,
    // and the caller would never be told the two ends had drifted apart.
    let (answers, _) = against(
        "{\"jsonrpc\":\"2.0\",\"id\":9,\"result\":\"stale\"}\n",
        &[("tools/list", json!({}))],
    );

    assert!(
        matches!(
            answers.first(),
            Some(Err(Trouble::Astray { call, found }))
                if call.number() == 1 && found.number() == 9
        ),
        "{:?}",
        answers.first()
    );
}

#[test]
fn a_question_the_server_asks_is_refused_before_the_answer_is_waited_for() {
    // The server is waiting on it, and what it is waiting to get on with is
    // the call crucible made. Going quiet would deadlock both ends.
    let (answers, said) = against(
        "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"sampling/createMessage\"}\n\
         {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"done\"}\n",
        &[("tools/list", json!({}))],
    );

    assert_eq!(
        answers.first().and_then(|answer| answer.as_ref().ok()),
        Some(&json!("done"))
    );

    let refusal: Value = serde_json::from_str(said.get(1).expect("a refusal")).expect("json");
    assert_eq!(refusal.get("id").and_then(Value::as_u64), Some(4));
    assert_eq!(
        refusal.pointer("/error/code").and_then(Value::as_i64),
        Some(NO_SUCH_METHOD)
    );
}

#[test]
fn a_notification_while_waiting_is_dropped_and_the_answer_still_arrives() {
    let (answers, said) = against(
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n\
         {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"done\"}\n",
        &[("tools/list", json!({}))],
    );

    assert_eq!(
        answers.first().and_then(|answer| answer.as_ref().ok()),
        Some(&json!("done"))
    );
    assert_eq!(said.len(), 1, "nothing is owed a notification: {said:?}");
}

#[test]
fn a_server_that_says_anything_but_the_answer_is_stopped_rather_than_waited_on() {
    let chatter = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n"
        .repeat(ASIDES + 1);
    let (answers, _) = against(
        &format!("{chatter}{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"late\"}}\n"),
        &[("tools/list", json!({}))],
    );

    assert!(
        matches!(answers.first(), Some(Err(Trouble::Talkative { most, .. })) if *most == ASIDES),
        "{:?}",
        answers.first()
    );
}

#[test]
fn exactly_as_much_chatter_as_the_bound_allows_still_gets_its_answer() {
    // The awkward legal case. A bound that refused the last frame it says it
    // allows would be a bound nobody could write a server against.
    let chatter =
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n".repeat(ASIDES);
    let (answers, _) = against(
        &format!("{chatter}{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"in time\"}}\n"),
        &[("tools/list", json!({}))],
    );

    assert_eq!(
        answers.first().and_then(|answer| answer.as_ref().ok()),
        Some(&json!("in time"))
    );
}

#[test]
fn a_server_that_stops_before_answering_says_which_call_it_left_open() {
    let (answers, _) = against("", &[("tools/list", json!({}))]);

    assert!(
        matches!(answers.first(), Some(Err(Trouble::Stopped { call })) if call.number() == 1),
        "{:?}",
        answers.first()
    );
}

#[test]
fn a_failure_comes_back_as_the_code_and_the_words_the_server_gave() {
    let (answers, _) = against(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32000,\"message\":\"no catalogue\"}}\n",
        &[("tools/list", json!({}))],
    );

    assert!(
        matches!(
            answers.first(),
            Some(Err(Trouble::Refused { code, said, .. }))
                if *code == -32000 && &**said == "no catalogue"
        ),
        "{:?}",
        answers.first()
    );
}

#[test]
fn a_frame_that_is_not_a_message_stops_the_call_rather_than_being_skipped() {
    // Skipping it would mean crucible carrying on against a server it has
    // already failed to understand once.
    let (answers, _) = against(
        "{\"jsonrpc\":\"2.0\"}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"done\"}\n",
        &[("tools/list", json!({}))],
    );

    assert!(
        matches!(answers.first(), Some(Err(Trouble::Garbled(_)))),
        "{:?}",
        answers.first()
    );
}
