//! What a conversation with an extension has to guarantee.

use serde_json::{Value, json};

use super::CROWDED;
use super::{Broken, Conversation, Next};
use crate::{CallError, CallId, EXTENSION_CALLS, EXTENSION_SAID_BYTES, Outcome, Spoken};

/// Fills the extension's side to the ceiling, and hands back the identifiers.
fn crowded(talk: &mut Conversation<&'static str>) -> Vec<CallId> {
    let ceiling = u64::try_from(EXTENSION_CALLS).expect("the ceiling fits");
    (0..ceiling)
        .map(|number| {
            let id = CallId::new(number);
            match talk.heard(asks(id, "read")) {
                Next::Asked { id: taken, .. } => assert_eq!(taken, id),
                other => panic!("a call below the ceiling was not taken on: {other:?}"),
            }
            id
        })
        .collect()
}

/// One request from the extension.
fn asks(id: CallId, method: &str) -> Spoken {
    Spoken::Request {
        id,
        method: method.into(),
        params: Value::Null,
    }
}

/// The answer to a call crucible made carries back what was remembered against
/// it, so the host does not have to keep a second table of its own.
#[test]
fn an_answer_carries_back_what_was_waiting_on_it() {
    let mut talk = Conversation::new();
    let sent = talk
        .ask("search", json!({ "for": "kettle" }), "the search")
        .expect("a call");
    let Spoken::Request { id, method, params } = sent else {
        panic!("asking must produce a request");
    };

    assert_eq!(&*method, "search");
    assert_eq!(params, json!({ "for": "kettle" }));
    assert_eq!(
        talk.heard(Spoken::Answer {
            id,
            outcome: Outcome::Worked(json!(["a kettle"])),
        }),
        Next::Answer {
            waiting: "the search",
            outcome: Outcome::Worked(json!(["a kettle"])),
        }
    );
}

/// An answer crucible cannot place ends the conversation. There is nothing to
/// reply to it with that would not be a guess about which call the far end
/// meant.
#[test]
fn an_answer_to_a_call_crucible_never_made_ends_the_conversation() {
    let mut talk: Conversation<&str> = Conversation::new();
    let invented = CallId::new(9);

    assert_eq!(
        talk.heard(Spoken::Answer {
            id: invented,
            outcome: Outcome::Worked(Value::Null),
        }),
        Next::Stop(Broken::Unmatched { id: invented })
    );
}

/// The second answer to one call is the same disagreement as the first
/// unplaceable one: the far end thinks a call is open that crucible has closed.
#[test]
fn answering_one_call_twice_ends_the_conversation() {
    let mut talk = Conversation::new();
    let sent = talk
        .ask("search", Value::Null, "the search")
        .expect("a call");
    let Spoken::Request { id, .. } = sent else {
        panic!("asking must produce a request");
    };
    let answer = || Spoken::Answer {
        id,
        outcome: Outcome::Worked(Value::Null),
    };

    assert!(matches!(talk.heard(answer()), Next::Answer { .. }));
    assert_eq!(talk.heard(answer()), Next::Stop(Broken::Unmatched { id }));
}

/// A request from the extension is taken on and handed to the host with the
/// identifier it owes an answer under.
#[test]
fn a_request_from_the_extension_is_taken_on() {
    let mut talk: Conversation<&str> = Conversation::new();
    let id = CallId::new(4);

    assert_eq!(
        talk.heard(Spoken::Request {
            id,
            method: "read".into(),
            params: json!({ "path": "notes" }),
        }),
        Next::Asked {
            id,
            method: "read".into(),
            params: json!({ "path": "notes" }),
        }
    );
}

/// A second call under a live identifier cannot be refused politely, because a
/// refusal would have to name that identifier and would settle the call the
/// extension already has open under it.
#[test]
fn a_second_call_under_a_live_identifier_ends_the_conversation() {
    let mut talk: Conversation<&str> = Conversation::new();
    let id = CallId::new(4);
    assert!(matches!(talk.heard(asks(id, "read")), Next::Asked { .. }));

    assert_eq!(
        talk.heard(asks(id, "write")),
        Next::Stop(Broken::Doubled { id })
    );
}

/// Asking faster than crucible answers is refused rather than fatal: the call
/// is named unambiguously, so crucible can say no and both ends still agree
/// about what is in flight.
#[test]
fn asking_past_the_ceiling_is_refused_and_the_conversation_goes_on() {
    let mut talk = Conversation::new();
    let open = crowded(&mut talk);
    let over = CallId::new(u64::try_from(EXTENSION_CALLS).expect("the ceiling fits"));

    let refusal = talk.heard(asks(over, "read"));
    let Next::Refuse(Spoken::Answer { id, outcome }) = refusal else {
        panic!("the ceiling must be refused with an answer, not: {refusal:?}");
    };
    assert_eq!(id, over, "the refusal must name the call it refuses");
    let Outcome::Failed(trouble) = outcome else {
        panic!("a refusal is a failure");
    };
    assert!(
        trouble.said().contains(&EXTENSION_CALLS.to_string()),
        "the refusal should say what the ceiling is: {}",
        trouble.said()
    );

    let first = *open.first().expect("the ceiling is not zero");
    talk.answer(first, Outcome::Worked(Value::Null))
        .expect("an answer to a call that is open");
    assert!(
        matches!(talk.heard(asks(over, "read")), Next::Asked { .. }),
        "answering one call must make room for another"
    );
}

/// A refused call was never taken on, so the extension may use that identifier
/// again without the conversation treating it as a second live call.
#[test]
fn a_refused_call_leaves_its_identifier_free() {
    let mut talk = Conversation::new();
    let open = crowded(&mut talk);
    let over = CallId::new(u64::try_from(EXTENSION_CALLS).expect("the ceiling fits"));
    assert!(matches!(talk.heard(asks(over, "read")), Next::Refuse(_)));

    let first = *open.first().expect("the ceiling is not zero");
    talk.answer(first, Outcome::Worked(Value::Null))
        .expect("an answer to a call that is open");

    assert!(
        matches!(talk.heard(asks(over, "read")), Next::Asked { .. }),
        "a refused identifier must not be held against the extension"
    );
}

/// Something said that expects nothing back needs no table and gets no answer.
#[test]
fn what_expects_nothing_back_is_passed_straight_through() {
    let mut talk: Conversation<&str> = Conversation::new();

    assert_eq!(
        talk.heard(Spoken::Told {
            method: "progress".into(),
            params: json!({ "done": 3 }),
        }),
        Next::Told {
            method: "progress".into(),
            params: json!({ "done": 3 }),
        }
    );
}

/// Answering a call the extension made produces the frame to send and closes
/// the call, so answering it a second time is refused rather than sent twice.
#[test]
fn a_call_the_extension_made_can_only_be_answered_once() {
    let mut talk: Conversation<&str> = Conversation::new();
    let id = CallId::new(2);
    assert!(matches!(talk.heard(asks(id, "read")), Next::Asked { .. }));

    assert_eq!(
        talk.answer(id, Outcome::Worked(json!("notes"))),
        Ok(Spoken::Answer {
            id,
            outcome: Outcome::Worked(json!("notes")),
        })
    );
    assert_eq!(
        talk.answer(id, Outcome::Worked(json!("notes"))),
        Err(CallError::Unknown { id })
    );
}

/// Answering a call the extension never made is refused, so a bug in the host
/// cannot put a frame on the wire that settles nothing.
#[test]
fn answering_a_call_the_extension_never_made_is_refused() {
    let mut talk: Conversation<&str> = Conversation::new();
    let invented = CallId::new(5);

    assert_eq!(
        talk.answer(invented, Outcome::Worked(Value::Null)),
        Err(CallError::Unknown { id: invented })
    );
}

/// When the conversation ends, everything crucible was waiting on comes back so
/// the host can fail each one. Nothing else is ever going to answer them.
#[test]
fn ending_hands_back_everything_crucible_was_waiting_on() {
    let mut talk = Conversation::new();
    let first = talk.ask("one", Value::Null, "the first").expect("a call");
    talk.ask("two", Value::Null, "the second").expect("a call");
    let Spoken::Request { id: first, .. } = first else {
        panic!("asking must produce a request");
    };
    assert!(matches!(
        talk.heard(Spoken::Answer {
            id: first,
            outcome: Outcome::Worked(Value::Null),
        }),
        Next::Answer { .. }
    ));

    let waiting: Vec<&str> = talk.ended().into_iter().map(|(_, about)| about).collect();
    assert_eq!(waiting, vec!["the second"]);
    assert!(
        talk.ended().is_empty(),
        "nothing may be handed back a second time"
    );
}

/// Each way a conversation can break says which call broke it, because the
/// reader is usually somebody working out why their extension was disconnected.
#[test]
fn every_break_names_the_call_that_caused_it() {
    let id = CallId::new(8);

    assert_eq!(
        Broken::Unmatched { id }.to_string(),
        "the extension answered call 8, which crucible is not waiting on"
    );
    assert_eq!(
        Broken::Doubled { id }.to_string(),
        "the extension started call 8 while it was already in flight"
    );
}

/// Crucible's refusal is a literal, so nothing checks it on the way out. These
/// are the two things `Trouble::new` would have checked had the words arrived
/// from somewhere: that it says something, and that it fits in the frame that
/// has to carry it.
#[test]
fn crucibles_own_refusal_is_something_a_frame_can_carry() {
    assert!(!CROWDED.is_empty());
    assert!(
        CROWDED.len() <= EXTENSION_SAID_BYTES,
        "the refusal is {} bytes; a frame carries {EXTENSION_SAID_BYTES}",
        CROWDED.len()
    );
}
