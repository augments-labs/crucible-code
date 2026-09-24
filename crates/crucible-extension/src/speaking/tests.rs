//! What driving a conversation over a pair of pipes has to guarantee.

use std::cell::RefCell;
use std::error::Error as _;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncWrite;

use super::{Asking, Over, Speaking, Turn};
use crucible_transport::FRAME_BYTES;

use crate::calls::{Call, Generation};
use crate::{CallError, CallId, Outcome, Spoken};

/// Call `number` of generation 0, which every conversation here is unless it
/// says otherwise.
const fn first(number: u64) -> Call {
    Call::new(Generation::numbered(0), CallId::new(number))
}

/// Drives one wait of the conversation to its answer.
fn on<F: Future>(work: F) -> F::Output {
    crate::testing::runtime().block_on(work)
}

/// An extension that says these things and then closes its output.
fn says(frames: &[&str]) -> Vec<u8> {
    let mut said = String::new();
    for frame in frames {
        said.push_str(frame);
        said.push('\n');
    }
    said.into_bytes()
}

/// A writer that keeps what it was sent where the test can still read it.
///
/// The conversation owns its writer and hands no way back to it, because on a
/// real extension that handle is a pipe anything holding it could write an
/// unframed byte to. A test that has to assert on what went out shares the
/// buffer instead, which is a fact about this double and not about the
/// conversation.
#[derive(Clone, Debug, Default)]
struct Kept(Rc<RefCell<Vec<u8>>>);

impl AsyncWrite for Kept {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Speaks to an extension that says `frames`, keeping what crucible sends.
fn talking_keeping(frames: &[&str]) -> (Speaking<io::Cursor<Vec<u8>>, Kept, &'static str>, Kept) {
    let kept = Kept::default();
    let talk = Speaking::new(
        io::Cursor::new(says(frames)),
        kept.clone(),
        Generation::numbered(0),
    );
    (talk, kept)
}

/// Speaks to an extension that says `frames`, for a test that asserts only on
/// what came back.
fn talking(frames: &[&str]) -> Speaking<io::Cursor<Vec<u8>>, Kept, &'static str> {
    talking_keeping(frames).0
}

/// A writer that refuses everything, the way a pipe does once the far end is
/// gone.
struct Gone;

impl AsyncWrite for Gone {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
    }
}

/// A writer that takes nothing, the way a pipe does once the far end has
/// stopped reading it.
struct Stuck;

impl AsyncWrite for Stuck {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Pending
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Pending
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Pending
    }
}

/// The frames crucible sent, one per line.
fn sent(kept: &Kept) -> Vec<String> {
    String::from_utf8(kept.0.borrow().clone())
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
        on(talk.turn()).expect("a turn"),
        Turn::Asked {
            id: first(1),
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
        on(talk.turn()).expect("a turn"),
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

    assert!(matches!(on(talk.turn()), Err(Over::Silent)));
}

/// A frame that is not readable has no identifier in it, so there is no call to
/// name in a refusal and nothing to say back. The conversation ends.
#[test]
fn a_frame_that_cannot_be_understood_ends_the_conversation() {
    let mut talk = talking(&["{not json at all"]);

    let over = on(talk.turn()).expect_err("an unreadable frame is the end");
    assert!(
        matches!(over, Over::Misspoken { .. }),
        "an unreadable frame is the extension misspeaking: {over:?}"
    );
}

/// The same holds one level down: a frame crucible could not even read off the
/// wire leaves the reader without a boundary it trusts.
#[test]
fn a_frame_that_cannot_be_read_ends_the_conversation() {
    let enormous = "x".repeat(FRAME_BYTES.saturating_add(1));
    let mut talk: Speaking<io::Cursor<Vec<u8>>, Vec<u8>, &str> = Speaking::new(
        io::Cursor::new(says(&[&enormous])),
        Vec::new(),
        Generation::numbered(0),
    );

    let over = on(talk.turn()).expect_err("an unreadable frame is the end");
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
    assert!(on(talk.turn()).is_err());

    assert!(
        matches!(on(talk.turn()), Err(Over::Finished)),
        "a frame after the end must not be acted on"
    );
}

/// An answer crucible cannot place is the two ends disagreeing about which
/// calls exist, which ends the conversation rather than being refused.
#[test]
fn an_answer_to_a_call_crucible_never_made_ends_the_conversation() {
    let mut talk = talking(&[r#"{"id":7,"result":null}"#]);

    let over = on(talk.turn()).expect_err("an unplaceable answer is the end");
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
    let (mut talk, kept) = talking_keeping(&borrowed);

    for _ in 0..ceiling {
        assert!(matches!(on(talk.turn()), Ok(Turn::Asked { .. })));
    }
    assert_eq!(
        on(talk.turn()).expect("the turn after the refused one"),
        Turn::Told {
            method: "done".into(),
            params: Value::Null,
        },
        "the refused call must not surface as a turn"
    );

    let frames = sent(&kept);
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
    let (mut talk, kept) = talking_keeping(&[r#"{"id":2,"method":"read"}"#]);
    assert!(matches!(on(talk.turn()), Ok(Turn::Asked { .. })));

    on(talk.answer(first(2), Outcome::Worked(json!("notes")))).expect("the answer goes out");

    assert_eq!(sent(&kept), vec![r#"{"id":2,"result":"notes"}"#.to_owned()]);
    assert!(
        matches!(
            on(talk.answer(first(2), Outcome::Worked(Value::Null))),
            Err(Asking::Refused(CallError::Unknown { .. }))
        ),
        "a call may only be answered once"
    );
}

/// A call of crucible's own goes out as a request, and its answer comes back
/// carrying what was remembered against it.
#[test]
fn a_call_crucible_makes_comes_back_with_what_was_waiting() {
    let (mut talk, kept) = talking_keeping(&[r#"{"id":0,"result":["a kettle"]}"#]);
    let id = on(talk.ask("search", json!({ "for": "kettle" }), "the search"))
        .expect("the call goes out");

    assert_eq!(
        sent(&kept),
        vec![r#"{"id":0,"method":"search","params":{"for":"kettle"}}"#.to_owned()]
    );
    assert_eq!(
        on(talk.turn()).expect("its answer"),
        Turn::Answer {
            waiting: "the search",
            outcome: Outcome::Worked(json!(["a kettle"])),
        }
    );
    assert_eq!(id, first(0));
}

/// A pipe that has gone ends the conversation rather than reporting a call that
/// was started but never said.
#[test]
fn a_call_that_cannot_be_sent_ends_the_conversation() {
    let mut talk: Speaking<io::Cursor<Vec<u8>>, Gone, &str> =
        Speaking::new(io::Cursor::new(says(&[])), Gone, Generation::numbered(0));

    let amiss = on(talk.ask("search", Value::Null, "the search"))
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
    let mut talk: Speaking<io::Cursor<Vec<u8>>, Vec<u8>, &str> = Speaking::new(
        io::Cursor::new(says(&[])),
        Vec::new(),
        Generation::numbered(0),
    );
    on(talk.ask("search", Value::Null, "the search")).expect("the call goes out");
    assert!(matches!(on(talk.turn()), Err(Over::Silent)));

    let waiting: Vec<&str> = talk.ended().into_iter().map(|(_, about)| about).collect();
    assert_eq!(waiting, vec!["the search"]);
}

/// Every ending says which one it was, because the reader is usually somebody
/// working out why their extension stopped.
#[test]
fn every_ending_says_what_it_was() {
    assert_eq!(Over::Silent.to_string(), "the extension stopped speaking");

    let mut talk = talking(&["{not json at all"]);
    let over = on(talk.turn()).expect_err("an unreadable frame is the end");
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
    let id = on(talk.ask("search", json!({ "for": "kettle" }), "the search")).expect("a call");

    assert_eq!(talk.give_up(id).expect("giving up on it"), "the search");
    assert_eq!(
        on(talk.turn()).expect("the conversation goes on"),
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
    let id = on(talk.ask("search", Value::Null, "the search")).expect("a call");
    on(talk.turn()).expect_err("the extension said nothing");

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
    let invented = first(7);

    let refused = talk.give_up(invented).expect_err("nothing waits on it");

    assert!(
        matches!(refused, Asking::Refused(CallError::Unknown { id }) if id == invented.id()),
        "{refused:?}"
    );
}

/// A send given up on partway may have left part of a frame on the wire, and
/// whatever went after it would be read as the rest of that frame. So the
/// conversation is over from there, and the call it was sending comes back
/// with everything else that was outstanding.
#[test]
fn a_send_given_up_on_leaves_the_conversation_over() {
    let mut talk: Speaking<io::Cursor<Vec<u8>>, Stuck, &str> = Speaking::new(
        io::Cursor::new(says(&[r#"{"method":"ready"}"#])),
        Stuck,
        Generation::numbered(0),
    );

    let given_up = on(async {
        tokio::time::timeout(
            Duration::from_millis(20),
            talk.ask("search", Value::Null, "the search"),
        )
        .await
    });
    assert!(given_up.is_err(), "the send never finished: {given_up:?}");

    assert!(
        matches!(on(talk.turn()), Err(Over::Finished)),
        "nothing the extension says is acted on after a half-sent frame"
    );
    assert!(matches!(
        on(talk.answer(first(0), Outcome::Worked(Value::Null))),
        Err(Asking::Over(Over::Finished))
    ));
    let waiting: Vec<&str> = talk.ended().into_iter().map(|(_, about)| about).collect();
    assert_eq!(waiting, vec!["the search"]);
}
