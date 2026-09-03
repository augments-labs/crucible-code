//! Results a server could hand back for one tool call, including the ones it
//! should not.

use std::{fmt::Write as _, io::Cursor};

use serde_json::{Value, json};

use super::{Answered, BLOCKS, CUT, RESULT_BYTES, Unanswered, call};
use crate::catalogue::{hello, tools};
use crate::talking::Talking;

/// A server's side of the conversation, one frame per line.
fn script(frames: &[Value]) -> String {
    let mut written = String::new();
    for frame in frames {
        writeln!(written, "{frame}").expect("a string accepts what is written to it");
    }
    written
}

/// One member of a message, or a panic saying which one was not there.
fn at<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key} in {value}"))
}

/// The handshake and the catalogue every one of these scripts opens with.
fn opening() -> [Value; 2] {
    [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "docs", "version": "1" },
            },
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [{ "name": "search", "inputSchema": { "type": "object" } }],
            },
        }),
    ]
}

/// A `tools/call` answer carrying `content`, failed or not.
fn produced(content: &Value, failed: bool) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 3,
        "result": { "content": content, "isError": failed },
    })
}

/// One text block.
fn text(said: &str) -> Value {
    json!({ "type": "text", "text": said })
}

/// Greets a server, reads its catalogue, and calls the tool it offered.
///
/// The whole sequence, because a call is gated on an [`Offered`] and there is
/// no way to reach one except by having read a catalogue that carried it.
fn called(answer: Value, arguments: &Value) -> (Result<Answered, Unanswered>, Vec<Value>) {
    let mut frames = opening().to_vec();
    frames.push(answer);
    let mut said = Vec::new();
    let answered = {
        let mut talking = Talking::new(Cursor::new(script(&frames)), &mut said);
        let greeting = hello(&mut talking).expect("these scripts open agreeably");
        let offered = tools(&mut talking, &greeting).expect("these scripts offer one tool");
        let tool = offered
            .first()
            .expect("the catalogue carried a tool")
            .clone();
        call(&mut talking, &tool, arguments)
    };
    (answered, spoken(&said))
}

/// What crucible wrote, read back as messages.
fn spoken(said: &[u8]) -> Vec<Value> {
    String::from_utf8(said.to_vec())
        .expect("crucible writes text")
        .lines()
        .map(|line| serde_json::from_str(line).expect("crucible writes messages"))
        .collect()
}

/// The `tools/call` crucible sent, or a panic saying it sent none.
fn calling(said: &[Value]) -> &Value {
    said.iter()
        .find(|message| message.get("method") == Some(&json!("tools/call")))
        .unwrap_or_else(|| panic!("crucible sent no tools/call in {said:?}"))
}

#[test]
fn a_call_names_the_tool_the_catalogue_offered_and_carries_its_arguments() {
    let (answered, said) = called(
        produced(&json!([text("nothing matched")]), false),
        &json!({ "query": "sandbox" }),
    );

    let answered = answered.expect("the server answered");
    assert_eq!(answered.text(), "nothing matched");
    assert!(!answered.failed());
    assert_eq!(answered.omitted(), 0);

    let sent = calling(&said);
    assert_eq!(at(at(sent, "params"), "name"), "search");
    assert_eq!(
        at(at(sent, "params"), "arguments"),
        &json!({ "query": "sandbox" }),
        "the arguments go across as they were given, because the server's own \
         schema is what judges them"
    );
}

#[test]
fn several_text_blocks_arrive_as_one_result() {
    let (answered, _said) = called(
        produced(
            &json!([text("first"), text("second"), text("third")]),
            false,
        ),
        &json!({}),
    );

    let answered = answered.expect("the server answered");
    assert_eq!(
        answered.text(),
        "first\nsecond\nthird",
        "blocks are a server's own paragraphing, and joining them is what makes \
         one result out of them"
    );
}

#[test]
fn a_tool_that_ran_and_failed_is_a_result_rather_than_a_broken_conversation() {
    let (answered, _said) = called(
        produced(&json!([text("no such document")]), true),
        &json!({}),
    );

    let answered = answered.expect("a failed tool still answered");
    assert!(
        answered.failed(),
        "the model asked for something that did not work, which is a fact to \
         react to rather than a reason to stop talking to the server"
    );
    assert_eq!(answered.text(), "no such document");
}

#[test]
fn a_block_crucible_cannot_show_is_named_rather_than_carried() {
    let (answered, _said) = called(
        produced(
            &json!([
                text("here it is"),
                { "type": "image", "data": "aGVsbG8=", "mimeType": "image/png" },
            ]),
            false,
        ),
        &json!({}),
    );

    let answered = answered.expect("the server answered");
    assert!(
        answered.text().contains("here it is"),
        "the text a server did send is still the answer: {}",
        answered.text()
    );
    assert!(
        answered.text().contains("image"),
        "a block whose kind crucible has no way to show is named, so the model \
         knows the server sent something rather than nothing: {}",
        answered.text()
    );
    assert!(
        !answered.text().contains("aGVsbG8="),
        "and its bytes are not smuggled into the context as text: {}",
        answered.text()
    );
}

#[test]
fn an_answer_with_no_content_is_refused_rather_than_read_as_an_empty_one() {
    let answer = json!({ "jsonrpc": "2.0", "id": 3, "result": { "isError": false } });
    let (answered, _said) = called(answer, &json!({}));

    let refused = answered.expect_err("a result with no content is not a shape crucible reads");
    assert!(matches!(refused, Unanswered::Missing { field: "content" }));
}

#[test]
fn a_result_past_its_ceiling_is_cut_and_says_how_much_went() {
    let long = "x".repeat(RESULT_BYTES * 2);
    let (answered, _said) = called(produced(&json!([text(&long)]), false), &json!({}));

    let answered = answered.expect("the server answered");
    assert!(
        answered.text().len() <= RESULT_BYTES,
        "a result is bounded where its bytes are first retained, and this one is \
         {} bytes",
        answered.text().len()
    );
    assert!(
        answered.omitted() > 0,
        "a cut result that says it is whole is a result the model will trust as \
         one"
    );
    assert!(
        answered.text().contains(CUT),
        "nothing below this is obliged to render the count, so the text says on \
         its own that it is not contiguous"
    );
    assert!(
        answered.text().starts_with('x') && answered.text().ends_with('x'),
        "the ending of a result is usually where a tool says what happened, so \
         the cut comes out of the middle"
    );
}

#[test]
fn a_result_of_more_blocks_than_crucible_reads_is_cut_rather_than_walked_forever() {
    let many: Vec<Value> = (0..BLOCKS * 2).map(|_| text("block")).collect();
    let (answered, _said) = called(produced(&Value::Array(many), false), &json!({}));

    let answered = answered.expect("the server answered");
    assert!(
        answered.omitted() > 0,
        "the blocks past the bound are not in the result, so the result says so"
    );
}

#[test]
fn a_result_that_fits_is_not_cut() {
    let exact = "y".repeat(RESULT_BYTES);
    let (answered, _said) = called(produced(&json!([text(&exact)]), false), &json!({}));

    let answered = answered.expect("the server answered");
    assert_eq!(
        answered.omitted(),
        0,
        "a result exactly at its ceiling is a whole result, and cutting one is a \
         false claim in the other direction"
    );
    assert_eq!(answered.text().len(), RESULT_BYTES);
}

#[test]
fn a_result_that_says_nothing_about_failing_did_not_fail() {
    let answer = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "result": { "content": [text("here you go")] },
    });
    let (answered, _said) = called(answer, &json!({}));

    let answered = answered.expect("the server answered");
    assert!(
        !answered.failed(),
        "the member says a tool failed, so a server that left it off has not \
         said anything went wrong — reading its absence as failure would make \
         every ordinary result an error"
    );
}
