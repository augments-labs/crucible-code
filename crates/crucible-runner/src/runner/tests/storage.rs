//! What a store that is nothing but this crate's contract has to be able to do.
//!
//! Every test in this directory records into one because it is convenient;
//! this one is about the store. A session is run into something that is no
//! file, no directory and no session crate, and the same contract hands it back
//! afterwards — so what is watched is the round trip, the transcript a reopen
//! replays against the one the run held. A runner writing through methods
//! nothing reads back would pass every other test here and fail this one.

use super::*;

/// Whose sessions these are.
const OWNER: &str = "a reader's own sessions";

/// The messages a provider is sent, without the harness fragments that are
/// rendered fresh for each request.
fn conversation(transcript: &Transcript) -> Vec<Message> {
    transcript
        .messages()
        .iter()
        .filter(|message| !matches!(message, Message::Context(_)))
        .cloned()
        .collect()
}

#[test]
fn a_session_recorded_through_the_contract_alone_comes_back_the_way_it_was_left() {
    let script = Script::new(vec![
        calling("a", "read", r#"{"path":"x"}"#),
        saying("first"),
        saying("second"),
    ]);
    let store = Recording::started(OWNER);
    let mut scripted = Scripted::recording(
        script,
        tools([Fixed::new("read").answering("fn main() {}")]),
        Verdict::Allow,
        Arc::clone(&store),
    );

    scripted.turn("read x").expect("a turn with a tool in it");
    scripted.turn("and again").expect("a second turn");
    let held = conversation(scripted.runner.state.transcript());

    // The run is over and the store outlives it: closing it was never the
    // runner's, so the records are all still here to be read.
    drop(scripted);
    let (picked, replayed) = store.reopened();

    assert_eq!(
        conversation(&replayed),
        held,
        "the session did not come back the way it was left"
    );
    assert_eq!(
        picked.session_id(),
        store.id(),
        "the session picked up answers to a different name"
    );
    assert_eq!(
        picked.owner(),
        store.owner(),
        "the session picked up belongs to someone else"
    );
}

#[test]
fn a_run_that_picks_up_a_session_from_the_contract_alone_goes_on_from_where_it_stopped() {
    let before = Recording::started(OWNER);
    let recorded = Scripted::recording(
        Script::new(vec![saying("first")]),
        Tools::new(),
        Verdict::Allow,
        Arc::clone(&before),
    );
    let mut recorded = recorded;
    recorded.turn("one").expect("a first turn");
    let held = conversation(recorded.runner.state.transcript());
    drop(recorded);

    let (picked, replayed) = before.reopened();
    let onto: Arc<dyn JournalStore> = picked.clone();
    let mut later = Scripted::new(
        Script::new(vec![saying("second")]),
        Tools::new(),
        Verdict::Allow,
    );
    later.runner.pick_up(onto, replayed);
    later.turn("two").expect("the turn after the pick-up");

    assert_eq!(
        later.started(),
        [2],
        "the continued session was numbered as a new one"
    );
    assert_eq!(
        conversation(later.runner.state.transcript())
            .get(..held.len())
            .unwrap_or_default(),
        held.as_slice(),
        "what the session already held was not carried into the next turn"
    );
    assert!(
        picked
            .said()
            .iter()
            .any(|said| *said == Message::said("two")),
        "the turn after the pick-up was recorded somewhere else"
    );
}

#[test]
fn the_results_beside_a_record_are_settled_after_the_answer_that_carries_them() {
    // A receipt says a result is durable. Settling before the message that
    // carries the result is written down would say it about something the
    // record does not yet hold, so a crash between the two leaves an accepted
    // result nothing replays. The store keeps both in one sequence for exactly
    // this reading.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let store = Recording::started(OWNER);
    let mut scripted = Scripted::recording(
        script,
        tools([Fixed::new("read").answering("fn main() {}")]),
        Verdict::Allow,
        Arc::clone(&store),
    );

    scripted.turn("read x").expect("a turn with a tool in it");

    let kept = store.kept();
    let settled = kept
        .iter()
        .position(|one| matches!(one, Kept::Settled))
        .expect("the results were never settled");
    let results = kept
        .iter()
        .position(|one| matches!(one, Kept::Said(Message::ToolResults(_))))
        .expect("the results were never recorded");

    assert_eq!(store.settled(), 1, "one results message, settled once");
    assert!(
        results < settled,
        "the results were called durable before they were written down"
    );
}

#[test]
fn a_session_from_before_typed_context_is_of_unknown_vintage_again_when_it_is_picked_up() {
    // Unknown vintage is what a log is, not something one of its records
    // says. A reopen that answered a known empty snapshot would tell the run
    // picking the session up that the model was last told nothing, when what
    // is true is that nobody wrote down what it was told: the next pass would
    // then describe changes against a baseline that never existed instead of
    // superseding what it cannot see.
    let store = Recording::pre_context(OWNER);
    store.append_message(&Message::said("recorded before typed context"));

    let (picked, replayed) = store.reopened();

    assert_eq!(
        picked.context_snapshot(),
        None,
        "picking the session up invented a baseline the log never held"
    );
    assert_eq!(
        conversation(&replayed),
        [Message::said("recorded before typed context")],
        "the session did not come back the way it was left"
    );
}
