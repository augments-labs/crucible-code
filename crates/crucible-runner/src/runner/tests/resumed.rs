//! What a runner put on a different session carries into the next turn.
//!
//! The transcript is the observable part — it is what every request holds — and
//! the session handed back is the other half: a log is closed by consuming it,
//! so a swap that dropped the old one would lose the last thing it had to say.

use std::str::FromStr as _;

use crucible_core::SessionId;

use super::*;

/// A session recorded and closed, holding one turn, and the name it has.
fn earlier(sample: &Sample) -> SessionId {
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let id = session
        .path()
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| SessionId::from_str(stem).ok())
        .expect("the log is named by its session");

    session.append(&Message::User("what came before".into()));
    session.append(&Message::Agent {
        text: "an answer from before".into(),
        calls: Vec::new(),
    });

    // Dropping is what waits for the queue, so the log is complete after it.
    drop(session);
    id
}

/// The one this run is on, and what it was recording to before.
fn picking(scripted: &mut Scripted, sample: &Sample, id: &SessionId) -> Session {
    let (session, transcript) =
        Session::reopen(&sample.logs(), &sample.workspace(), id).expect("the session named");

    scripted.runner.pick_up(session, transcript)
}

#[test]
fn the_next_turn_is_asked_with_the_transcript_of_the_session_picked_up() {
    let sample = Sample::new("runner-picked-up");
    let id = earlier(&sample);

    let script = Script::new(vec![saying("here"), saying("and now")]);
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);

    scripted.turn("what is in main.rs?").unwrap();
    drop(picking(&mut scripted, &sample, &id));
    scripted.turn("and after that?").unwrap();

    assert_eq!(
        scripted.asked(),
        [1, 3],
        "the second request carried the earlier session's turn and the new prompt"
    );
    assert!(
        scripted
            .runner
            .transcript()
            .messages()
            .first()
            .is_some_and(|message| *message == Message::User("what came before".into())),
        "the transcript is the one picked up, not the one left behind"
    );
}

#[test]
fn the_turn_count_carries_on_from_the_session_picked_up() {
    // What the user is told each turn. A session continued at turn one says
    // this is a new one, which is the opposite of what was asked for.
    let sample = Sample::new("runner-picked-count");
    let id = earlier(&sample);

    let script = Script::new(vec![saying("here")]);
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);

    drop(picking(&mut scripted, &sample, &id));
    scripted.turn("and after that?").unwrap();

    assert_eq!(scripted.started(), [2]);
}

#[test]
fn the_session_left_behind_is_handed_back_rather_than_dropped() {
    // Closing one means consuming it, and the first write that failed is worth
    // saying while there is still a session on screen it belongs to.
    let sample = Sample::new("runner-picked-handed-back");
    let id = earlier(&sample);

    let script = Script::new(vec![saying("here")]);
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let was = session.path().to_owned();
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);

    let left = picking(&mut scripted, &sample, &id);

    assert_eq!(left.path(), was);
    assert_ne!(scripted.runner.session().path(), was);
    assert!(left.finish().is_none(), "nothing failed to be written");
}

#[test]
fn what_the_last_session_allowed_is_asked_about_again() {
    // "For the rest of this session" was answered about the session being left
    // behind. Carrying it across would run a tool in a session nobody was
    // asked about.
    let sample = Sample::new("runner-picked-permission");
    let id = earlier(&sample);

    let script = Script::new(vec![
        calling("a", "write", "{}"),
        saying("done"),
        calling("b", "write", "{}"),
        saying("done"),
        calling("c", "write", "{}"),
        saying("done"),
    ]);
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let mut scripted = Scripted::recording(
        script,
        tools([Fixed::new("write").risking(changing())]),
        Verdict::Allow,
        session,
    );
    scripted.says = Says::for_the_session();

    scripted.turn("write it").unwrap();
    scripted.turn("write it again").unwrap();
    assert_eq!(scripted.says.asked, 1, "the session allow held");

    drop(picking(&mut scripted, &sample, &id));
    scripted.turn("and once more").unwrap();

    assert_eq!(
        scripted.says.asked, 2,
        "the user answers the same way; what moved is the session it was for"
    );
}
