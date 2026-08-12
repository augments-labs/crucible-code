//! What a session keeps after it has forgotten what was said.
//!
//! The transcript is the one thing here that grows, and it is what every
//! request carries. So what forgetting has to be is the next request going out
//! smaller — not a flag somewhere saying it should.

use super::*;

#[test]
fn the_turn_after_a_session_forgot_carries_nothing_that_came_before_it() {
    // The whole of what `/clear` is for. A transcript that was cleared and then
    // sent anyway is a session that costs the same tokens and answers the same
    // questions, with a line on screen saying otherwise.
    let script = Script::new(vec![saying("first"), saying("second")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    scripted.turn("what is in main.rs?").unwrap();
    assert_eq!(scripted.runner.forget(), 2, "the prompt and the answer");

    scripted.turn("and in lib.rs?").unwrap();

    assert_eq!(
        scripted.asked(),
        [1, 1],
        "one prompt each: the second request carried nothing of the first turn"
    );
}

#[test]
fn the_turn_after_a_session_forgot_is_the_first_one_again() {
    // The count is what the user is told each turn. Numbering the next one
    // after a transcript that is no longer anywhere would name a turn nothing
    // on screen or in the log has any record of.
    let script = Script::new(vec![saying("first"), saying("second")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    scripted.turn("one").unwrap();
    scripted.runner.forget();
    scripted.turn("two").unwrap();

    assert_eq!(scripted.started(), [1, 1]);
}

#[test]
fn forgetting_a_session_that_has_said_nothing_forgets_nothing() {
    let script = Script::new(vec![saying("first")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    assert_eq!(scripted.runner.forget(), 0);
    assert!(scripted.runner.transcript().is_empty());
}

#[test]
fn a_session_that_forgot_is_continued_from_where_it_started_again() {
    // The log is what a continued session is built from, so forgetting has to
    // reach it: a transcript cleared only in memory comes back whole on the
    // next `--continue`, and the session the user cleared is the session they
    // get.
    let sample = Sample::new("runner-forgot");
    let script = Script::new(vec![saying("before"), saying("after")]);
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);

    scripted.turn("what came before").unwrap();
    scripted.runner.forget();
    scripted.turn("what came after").unwrap();

    // Dropping the runner drops the session, which is what waits for the queue.
    drop(scripted);
    let (_session, replayed) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");

    assert_eq!(
        replayed.messages(),
        [
            Message::User("what came after".into()),
            Message::Agent {
                text: "after".into(),
                calls: Vec::new(),
            },
        ]
    );
}
