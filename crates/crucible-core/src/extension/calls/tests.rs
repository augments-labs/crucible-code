//! What keeping calls straight has to guarantee.

use super::{Asked, CallError, EXTENSION_CALLS, Serving};
use crate::CallId;

/// An answer settles the call it names and no other, so what was remembered
/// against one call can never be handed back for a different one.
#[test]
fn an_answer_gives_back_what_was_remembered_against_that_call() {
    let mut asked = Asked::new();
    let first = asked.ask("summarise").expect("a first call");
    let second = asked.ask("translate").expect("a second call");

    assert_ne!(first, second, "two live calls must not share an identifier");
    assert_eq!(asked.answered(second), Ok(Some("translate")));
    assert_eq!(asked.answered(first), Ok(Some("summarise")));
}

/// Identifiers are handed out once for the life of the table. Reusing a number
/// after its call is answered would let an answer that arrives late settle a
/// call that only borrowed the same number.
#[test]
fn an_identifier_is_never_handed_out_twice() {
    let mut asked = Asked::new();
    let mut seen = Vec::new();
    for _ in 0..8 {
        let id = asked.ask(()).expect("a call");
        assert!(!seen.contains(&id), "identifier {id} was handed out twice");
        seen.push(id);
        asked.answered(id).expect("the answer to it");
    }
}

/// An answer to a call crucible never made is refused rather than ignored,
/// because it is the far end saying something about a conversation that does
/// not exist.
#[test]
fn an_answer_to_a_call_nobody_made_is_refused() {
    let mut asked: Asked<()> = Asked::new();
    let invented = CallId::new(7);

    assert_eq!(
        asked.answered(invented),
        Err(CallError::Unknown { id: invented })
    );
}

/// The second answer to one call is refused. The first already gave back what
/// was waiting; a second would have nothing to give and would let the far end
/// tell crucible the same thing twice.
#[test]
fn one_call_can_only_be_answered_once() {
    let mut asked = Asked::new();
    let id = asked.ask("search").expect("a call");

    assert_eq!(asked.answered(id), Ok(Some("search")));
    assert_eq!(asked.answered(id), Err(CallError::Unknown { id }));
}

/// Crucible bounds its own asking. Reaching the ceiling means calls are being
/// started and never collected, and a table that grows for the life of a run
/// is worse than a refusal that says so.
#[test]
fn crucible_will_not_wait_on_more_calls_than_it_allows() {
    let mut asked = Asked::new();
    for _ in 0..EXTENSION_CALLS {
        asked.ask(()).expect("a call below the ceiling");
    }

    assert_eq!(asked.waiting(), EXTENSION_CALLS);
    assert_eq!(
        asked.ask(()),
        Err(CallError::TooMany {
            maximum: EXTENSION_CALLS
        })
    );
}

/// The ceiling counts what is waiting, not what has ever been asked, so a table
/// that keeps up with its answers keeps working.
#[test]
fn answering_makes_room_for_another_call() {
    let mut asked = Asked::new();
    let mut open = Vec::new();
    for _ in 0..EXTENSION_CALLS {
        open.push(asked.ask(()).expect("a call below the ceiling"));
    }
    let first = *open.first().expect("the ceiling is not zero");
    asked.answered(first).expect("its answer");

    assert_eq!(asked.waiting(), EXTENSION_CALLS - 1);
    asked.ask(()).expect("the room that answer made");
}

/// When the far end goes away, everything still waiting comes back at once and
/// in call order, so the host can fail each one instead of leaving whatever
/// asked to wait for the length of the run.
#[test]
fn losing_the_far_end_hands_back_everything_still_waiting() {
    let mut asked = Asked::new();
    let first = asked.ask("one").expect("a first call");
    let second = asked.ask("two").expect("a second call");
    let third = asked.ask("three").expect("a third call");
    asked.answered(second).expect("the one answer that arrived");

    assert_eq!(asked.abandoned(), vec![(first, "one"), (third, "three")]);
    assert_eq!(asked.waiting(), 0, "nothing may be left waiting");
    assert_eq!(
        asked.answered(first),
        Err(CallError::Unknown { id: first }),
        "an answer arriving after the fact settles nothing"
    );
}

/// Abandoning does not rewind the numbering. An answer written before the far
/// end went away must not be able to settle a call made after it.
#[test]
fn identifiers_keep_counting_past_an_abandoned_call() {
    let mut asked = Asked::new();
    let before = asked.ask(()).expect("a call");
    asked.abandoned();
    let after = asked.ask(()).expect("a later call");

    assert_ne!(before, after);
}

/// Running out of identifiers is refused rather than wrapped, because wrapping
/// is the one thing that breaks the promise every other guard rests on.
#[test]
fn running_out_of_identifiers_is_refused_rather_than_wrapped() {
    let mut asked: Asked<()> = Asked::counting_from(u64::MAX);
    let last = asked.ask(()).expect("the last identifier there is");

    assert_eq!(last, CallId::new(u64::MAX));
    assert_eq!(asked.ask(()), Err(CallError::Exhausted));
}

/// The other direction: an extension asking twice under one identifier is
/// refused, because two live calls under one number are two calls that one
/// answer would settle, and the far end chose the number.
#[test]
fn an_extension_may_not_have_two_calls_under_one_identifier() {
    let mut serving = Serving::new();
    let id = CallId::new(3);
    serving.take(id).expect("the first call");

    assert_eq!(serving.take(id), Err(CallError::Repeated { id }));
    assert_eq!(serving.open(), 1);
}

/// Once answered the number is free again, because an extension counting from
/// zero each session, or reusing numbers it has finished with, is honest.
#[test]
fn an_identifier_is_free_again_once_it_is_answered() {
    let mut serving = Serving::new();
    let id = CallId::new(3);
    serving.take(id).expect("the first call");
    serving.answered(id).expect("its answer");

    assert_eq!(serving.open(), 0);
    serving
        .take(id)
        .expect("a later call under the same number");
}

/// How much work a program somebody else wrote can make this process hold is
/// crucible's decision, not the extension's.
#[test]
fn an_extension_cannot_open_more_calls_than_crucible_allows() {
    let mut serving = Serving::new();
    let ceiling = u64::try_from(EXTENSION_CALLS).expect("the ceiling fits");
    for number in 0..ceiling {
        serving
            .take(CallId::new(number))
            .expect("a call below the ceiling");
    }
    let over = CallId::new(ceiling);

    assert_eq!(
        serving.take(over),
        Err(CallError::TooMany {
            maximum: EXTENSION_CALLS
        })
    );
    assert_eq!(serving.open(), EXTENSION_CALLS);
}

/// Answering something crucible never took on is refused, so a bug that answers
/// twice cannot quietly free a slot that belongs to a live call.
#[test]
fn answering_a_call_that_was_never_taken_on_is_refused() {
    let mut serving = Serving::new();
    let id = CallId::new(11);

    assert_eq!(serving.answered(id), Err(CallError::Unknown { id }));
    serving.take(id).expect("the call");
    serving.answered(id).expect("its answer");
    assert_eq!(serving.answered(id), Err(CallError::Unknown { id }));
}

/// Each refusal says which call and which ceiling, because the reader is
/// usually somebody working out why their extension stopped being answered.
#[test]
fn every_refusal_names_what_went_wrong() {
    let id = CallId::new(42);

    assert_eq!(
        CallError::TooMany { maximum: 64 }.to_string(),
        "more than 64 calls would be in flight at once"
    );
    assert_eq!(
        CallError::Unknown { id }.to_string(),
        "call 42 is not one that is in flight"
    );
    assert_eq!(
        CallError::Repeated { id }.to_string(),
        "call 42 is already in flight"
    );
    assert_eq!(
        CallError::Exhausted.to_string(),
        "there are no call identifiers left"
    );
}

/// Giving up hands back what was remembered right then, because the host owes
/// its own caller an answer at that moment rather than whenever the extension
/// gets there.
#[test]
fn giving_up_on_a_call_hands_back_what_was_remembered_against_it() {
    let mut asked = Asked::new();
    let id = asked.ask("search").expect("a call");

    assert_eq!(asked.given_up(id), Ok("search"));
}

/// The extension was never told, so it may still answer. That answer is placed
/// rather than refused: refusing it would end the conversation over crucible's
/// own act.
#[test]
fn a_call_given_up_on_is_still_recognised_when_it_is_answered() {
    let mut asked = Asked::new();
    let id = asked.ask("search").expect("a call");
    asked.given_up(id).expect("giving up on it");

    assert_eq!(asked.answered(id), Ok(None));
}

/// Once its answer has arrived the identifier is finished with, and a second
/// one is the far end saying something about a call that no longer exists.
#[test]
fn a_call_given_up_on_is_finished_by_the_answer_that_arrives_for_it() {
    let mut asked = Asked::new();
    let id = asked.ask("search").expect("a call");
    asked.given_up(id).expect("giving up on it");
    asked.answered(id).expect("the answer that crossed it");

    assert_eq!(asked.answered(id), Err(CallError::Unknown { id }));
}

/// Giving up is what produces the host's final answer for a call, so a second
/// one would produce a second final answer for one call.
#[test]
fn one_call_can_only_be_given_up_on_once() {
    let mut asked = Asked::new();
    let id = asked.ask("search").expect("a call");
    asked.given_up(id).expect("giving up on it");

    assert_eq!(asked.given_up(id), Err(CallError::Unknown { id }));
}

/// A call crucible never made cannot be given up on either.
#[test]
fn a_call_nobody_made_cannot_be_given_up_on() {
    let mut asked: Asked<()> = Asked::new();
    let invented = CallId::new(7);

    assert_eq!(
        asked.given_up(invented),
        Err(CallError::Unknown { id: invented })
    );
}

/// Giving up frees nothing until the extension answers. This is what bounds it:
/// a host that gives up in a loop against an extension that never replies runs
/// out of room rather than out of memory.
#[test]
fn a_call_given_up_on_holds_its_place_until_the_extension_answers() {
    let mut asked = Asked::new();
    for _ in 0..EXTENSION_CALLS {
        let id = asked.ask(()).expect("a call below the ceiling");
        asked.given_up(id).expect("giving up on it");
    }

    assert_eq!(asked.waiting(), EXTENSION_CALLS);
    assert_eq!(
        asked.ask(()),
        Err(CallError::TooMany {
            maximum: EXTENSION_CALLS
        })
    );
}

/// The far end going away must not hand a call back twice. Whatever was
/// waiting on a given-up call left with the giving up.
#[test]
fn a_call_given_up_on_is_not_handed_back_again_when_the_extension_goes_away() {
    let mut asked = Asked::new();
    let abandoned = asked.ask("still wanted").expect("a call");
    let given_up = asked.ask("no longer wanted").expect("another call");
    asked.given_up(given_up).expect("giving up on the second");

    assert_eq!(asked.abandoned(), vec![(abandoned, "still wanted")]);
}
