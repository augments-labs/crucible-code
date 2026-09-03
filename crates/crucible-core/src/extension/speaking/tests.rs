//! What driving a conversation over a pair of pipes has to guarantee.

use std::error::Error as _;
use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

use super::{Asking, Over, Speaking, Turn};
use crate::{CallError, CallId, EXTENSION_FRAME_BYTES, Outcome, Spoken};

/// An extension that says these things and then closes its output.
fn says(frames: &[&str]) -> Vec<u8> {
    let mut said = String::new();
    for frame in frames {
        said.push_str(frame);
        said.push('\n');
    }
    said.into_bytes()
}

/// Speaks to an extension that says `frames`, collecting what crucible sends.
fn talking(frames: &[&str]) -> Speaking<io::Cursor<Vec<u8>>, Vec<u8>, &'static str> {
    Speaking::new(io::Cursor::new(says(frames)), Vec::new())
}

/// A writer that refuses everything, the way a pipe does once the far end is
/// gone.
struct Gone;

impl Write for Gone {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

/// The frames crucible sent, one per line.
fn sent<R: BufRead>(talk: &Speaking<R, Vec<u8>, &'static str>) -> Vec<String> {
    String::from_utf8(talk.written().clone())
        .expect("crucible writes text")
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

/// An extension asking crucible for something comes back as a turn the host
/// has to answer.
#[test]
fn what_the_extension_asks_for_comes_back_as_a_turn() {
    let mut talk = talking(&[r#"{"id":1,"method":"read","params":{"path":"notes"}}"#]);

    assert_eq!(
        talk.turn().expect("a turn"),
        Turn::Asked {
            id: CallId::new(1),
            method: "read".into(),
            params: json!({ "path": "notes" }),
        }
    );
}

/// Something that expects nothing back is passed through without any call
/// being taken on.
#[test]
fn what_expects_nothing_back_comes_back_as_a_turn() {
    let mut talk = talking(&[r#"{"method":"progress","params":{"done":3}}"#]);

    assert_eq!(
        talk.turn().expect("a turn"),
        Turn::Told {
            method: "progress".into(),
            params: json!({ "done": 3 }),
        }
    );
}

/// An extension that closes its output has ended the conversation, and that is
/// the one ending that is nobody's fault.
#[test]
fn an_extension_that_stops_speaking_ends_the_conversation() {
    let mut talk = talking(&[]);

    assert!(matches!(talk.turn(), Err(Over::Silent)));
}

/// A frame that is not readable has no identifier in it, so there is no call to
/// name in a refusal and nothing to say back. The conversation ends.
#[test]
fn a_frame_that_cannot_be_understood_ends_the_conversation() {
    let mut talk = talking(&["{not json at all"]);

    let over = talk.turn().expect_err("an unreadable frame is the end");
    assert!(
        matches!(over, Over::Misspoken { .. }),
        "an unreadable frame is the extension misspeaking: {over:?}"
    );
}

/// The same holds one level down: a frame crucible could not even read off the
/// wire leaves the reader without a boundary it trusts.
#[test]
fn a_frame_that_cannot_be_read_ends_the_conversation() {
    let enormous = "x".repeat(EXTENSION_FRAME_BYTES.saturating_add(1));
    let mut talk: Speaking<io::Cursor<Vec<u8>>, Vec<u8>, &str> =
        Speaking::new(io::Cursor::new(says(&[&enormous])), Vec::new());

    let over = talk.turn().expect_err("an unreadable frame is the end");
    assert!(
        matches!(over, Over::Unreadable { .. }),
        "a frame past the ceiling is unreadable: {over:?}"
    );
}

/// Once the conversation is over it stays over. A reader that has lost its
/// place does not get a second chance at the bytes after it.
#[test]
fn a_conversation_that_is_over_stays_over() {
    let mut talk = talking(&["{not json at all", r#"{"id":1,"method":"read"}"#]);
    assert!(talk.turn().is_err());

    assert!(
        matches!(talk.turn(), Err(Over::Finished)),
        "a frame after the end must not be acted on"
    );
}

/// An answer crucible cannot place is the two ends disagreeing about which
/// calls exist, which ends the conversation rather than being refused.
#[test]
fn an_answer_to_a_call_crucible_never_made_ends_the_conversation() {
    let mut talk = talking(&[r#"{"id":7,"result":null}"#]);

    let over = talk.turn().expect_err("an unplaceable answer is the end");
    assert!(
        matches!(over, Over::Broke { .. }),
        "an unplaceable answer breaks the conversation: {over:?}"
    );
}

/// A refusal is crucible's own word about a call it declined, so it goes on the
/// wire from here rather than being handed to the host to send.
#[test]
fn a_refused_call_is_answered_without_troubling_the_host() {
    let ceiling = u64::try_from(crate::EXTENSION_CALLS).expect("the ceiling fits");
    let mut asked: Vec<String> = (0..=ceiling)
        .map(|number| format!(r#"{{"id":{number},"method":"read"}}"#))
        .collect();
    asked.push(r#"{"method":"done"}"#.to_owned());
    let borrowed: Vec<&str> = asked.iter().map(String::as_str).collect();
    let mut talk = talking(&borrowed);

    for _ in 0..ceiling {
        assert!(matches!(talk.turn(), Ok(Turn::Asked { .. })));
    }
    assert_eq!(
        talk.turn().expect("the turn after the refused one"),
        Turn::Told {
            method: "done".into(),
            params: Value::Null,
        },
        "the refused call must not surface as a turn"
    );

    let frames = sent(&talk);
    assert_eq!(frames.len(), 1, "exactly one refusal was sent: {frames:?}");
    let Ok(Spoken::Answer { id, outcome }) = Spoken::read(frames.first().expect("a frame")) else {
        panic!("the refusal must be an answer: {frames:?}");
    };
    assert_eq!(id, CallId::new(ceiling));
    assert!(matches!(outcome, Outcome::Failed(_)));
}

/// Answering a call the extension made puts one frame on the wire and closes
/// the call.
#[test]
fn answering_a_call_sends_one_frame() {
    let mut talk = talking(&[r#"{"id":2,"method":"read"}"#]);
    assert!(matches!(talk.turn(), Ok(Turn::Asked { .. })));

    talk.answer(CallId::new(2), Outcome::Worked(json!("notes")))
        .expect("the answer goes out");

    assert_eq!(sent(&talk), vec![r#"{"id":2,"result":"notes"}"#.to_owned()]);
    assert!(
        matches!(
            talk.answer(CallId::new(2), Outcome::Worked(Value::Null)),
            Err(Asking::Refused(CallError::Unknown { .. }))
        ),
        "a call may only be answered once"
    );
}

/// A call of crucible's own goes out as a request, and its answer comes back
/// carrying what was remembered against it.
#[test]
fn a_call_crucible_makes_comes_back_with_what_was_waiting() {
    let mut talk: Speaking<io::Cursor<Vec<u8>>, Vec<u8>, &str> = Speaking::new(
        io::Cursor::new(says(&[r#"{"id":0,"result":["a kettle"]}"#])),
        Vec::new(),
    );
    let id = talk
        .ask("search", json!({ "for": "kettle" }), "the search")
        .expect("the call goes out");

    assert_eq!(
        sent(&talk),
        vec![r#"{"id":0,"method":"search","params":{"for":"kettle"}}"#.to_owned()]
    );
    assert_eq!(
        talk.turn().expect("its answer"),
        Turn::Answer {
            waiting: "the search",
            outcome: Outcome::Worked(json!(["a kettle"])),
        }
    );
    assert_eq!(id, CallId::new(0));
}

/// A pipe that has gone ends the conversation rather than reporting a call that
/// was started but never said.
#[test]
fn a_call_that_cannot_be_sent_ends_the_conversation() {
    let mut talk: Speaking<io::Cursor<Vec<u8>>, Gone, &str> =
        Speaking::new(io::Cursor::new(says(&[])), Gone);

    let amiss = talk
        .ask("search", Value::Null, "the search")
        .expect_err("a gone pipe cannot carry a call");
    assert!(
        matches!(amiss, Asking::Over(Over::Unanswerable { .. })),
        "a gone pipe ends the conversation: {amiss:?}"
    );
}

/// Whatever crucible was waiting on when the conversation ended has to come
/// back, because nothing is ever going to answer it now.
#[test]
fn what_was_waiting_when_it_ended_comes_back() {
    let mut talk: Speaking<io::Cursor<Vec<u8>>, Vec<u8>, &str> =
        Speaking::new(io::Cursor::new(says(&[])), Vec::new());
    talk.ask("search", Value::Null, "the search")
        .expect("the call goes out");
    assert!(matches!(talk.turn(), Err(Over::Silent)));

    let waiting: Vec<&str> = talk.ended().into_iter().map(|(_, about)| about).collect();
    assert_eq!(waiting, vec!["the search"]);
}

/// Every ending says which one it was, because the reader is usually somebody
/// working out why their extension stopped.
#[test]
fn every_ending_says_what_it_was() {
    assert_eq!(Over::Silent.to_string(), "the extension stopped speaking");

    let mut talk = talking(&["{not json at all"]);
    let over = talk.turn().expect_err("an unreadable frame is the end");
    let underneath = over
        .source()
        .expect("a wrapped ending has a reason")
        .to_string();
    assert!(
        over.to_string().contains(&underneath),
        "an ending must carry the reason underneath it: {over}"
    );
}

/// Giving up on a call does not end the conversation, and the answer that
/// crosses it is read past rather than handed to the host. The host was given
/// what it was waiting on when it gave up; being told again would be a second
/// final answer for one call, and ending the conversation over it would kill an
/// extension for a race crucible started.
#[test]
fn an_answer_to_a_call_crucible_gave_up_on_is_read_past() {
    let mut talk = talking(&[
        r#"{"id":0,"result":["a kettle"]}"#,
        r#"{"method":"ready","params":null}"#,
    ]);
    let id = talk
        .ask("search", json!({ "for": "kettle" }), "the search")
        .expect("a call");

    assert_eq!(talk.give_up(id).expect("giving up on it"), "the search");
    assert_eq!(
        talk.turn().expect("the conversation goes on"),
        Turn::Told {
            method: "ready".into(),
            params: Value::Null,
        }
    );
}

/// Once the conversation is over, everything outstanding has already been
/// handed back by `ended`, so giving up again would produce a second final
/// answer for a call that already has one.
#[test]
fn a_call_cannot_be_given_up_on_once_the_conversation_is_over() {
    let mut talk = talking(&[]);
    let id = talk
        .ask("search", Value::Null, "the search")
        .expect("a call");
    talk.turn().expect_err("the extension said nothing");

    let refused = talk.give_up(id).expect_err("the conversation is over");

    assert!(
        matches!(refused, Asking::Over(Over::Finished)),
        "{refused:?}"
    );
}

/// Giving up is not a way to make a call vanish: an identifier crucible is not
/// waiting on is refused here the same as anywhere else.
#[test]
fn a_call_crucible_is_not_waiting_on_cannot_be_given_up_on() {
    let mut talk = talking(&[]);
    let invented = CallId::new(7);

    let refused = talk.give_up(invented).expect_err("nothing waits on it");

    assert!(
        matches!(refused, Asking::Refused(CallError::Unknown { id }) if id == invented),
        "{refused:?}"
    );
}
