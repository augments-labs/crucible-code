//! What a runner put on a different session carries into the next turn.
//!
//! The transcript is the observable part — it is what every request holds — and
//! the session handed back is the other half: a log is closed by consuming it,
//! so a swap that dropped the old one would lose the last thing it had to say.
//! Neither is read off the runner's own fields, which is what keeps a swap that
//! set every one of them correctly from passing while the log it left behind
//! went unclosed.

use std::str::FromStr as _;

use super::*;

/// A session recorded and closed, holding one turn, and the name it has.
fn earlier(sample: &Sample) -> SessionId {
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let id = named(&session);

    session.append(&Message::said("what came before"));
    session.append(&Message::Agent {
        continuation: None,
        text: "an answer from before".into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
    });

    // Dropping is what waits for the queue, so the log is complete after it.
    drop(session);
    id
}

/// Which session a log belongs to, read back from what it is called.
fn named(session: &Session) -> SessionId {
    session
        .path()
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| SessionId::from_str(stem).ok())
        .expect("the log is named by its session")
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
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);

    scripted.turn("what is in main.rs?").unwrap();
    drop(picking(&mut scripted, &sample, &id));
    scripted.turn("and after that?").unwrap();

    assert_eq!(
        scripted.asked(),
        [7, 9],
        "each session renders context once; the second also carries the earlier turn"
    );
    assert!(
        scripted
            .runner
            .transcript()
            .messages()
            .first()
            .is_some_and(|message| *message == Message::said("what came before")),
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
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);

    drop(picking(&mut scripted, &sample, &id));
    scripted.turn("and after that?").unwrap();

    assert_eq!(scripted.started(), [2]);
}

#[test]
fn a_session_picked_up_with_nothing_in_it_is_asked_at_turn_one_and_carries_nothing() {
    // The shape `/clear` uses: a session that has just been started, and a
    // transcript with nothing in it. What is being watched is the pair. A
    // request still carrying the last session's turn is the same session under
    // a new name, and a turn counted after the one before it names a turn
    // nothing on screen or in either log has any record of.
    let sample = Sample::new("runner-picked-empty");

    let script = Script::new(vec![saying("here"), saying("and now")]);
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);

    scripted.turn("what is in main.rs?").unwrap();

    let fresh =
        Session::start(&sample.logs(), &sample.workspace(), None).expect("a second session");
    drop(scripted.runner.pick_up(fresh, Transcript::new()));
    scripted.turn("and now?").unwrap();

    assert_eq!(
        scripted.asked(),
        [7, 7],
        "one prompt and six fresh sections each; no conversation crossed sessions"
    );
    assert_eq!(scripted.started(), [1, 1]);
}

#[test]
fn the_session_left_behind_is_handed_back_rather_than_dropped() {
    // Closing one means consuming it, and the first write that failed is worth
    // saying while there is still a session on screen it belongs to.
    let sample = Sample::new("runner-picked-handed-back");
    let id = earlier(&sample);

    let script = Script::new(vec![saying("here")]);
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
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
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
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

/// One answer that reports both halves of what it cost.
fn measured() -> Script {
    Script::new(vec![vec![
        Delta::Carried(Carried::new(40_000)),
        Delta::Text("done".into()),
        Delta::Spent(Spend::new(10_000)),
        Delta::Stopped(StopReason::Yielded),
    ]])
}

#[test]
fn a_session_picked_up_estimates_window_left_before_it_answers_again() {
    // Everything the load measured belongs to this process, and a transcript
    // read off a disk arrives with none of it — so a session picked up used to
    // come back estimating, with the row saying nothing until the next answer
    // reported. The log records what the last request carried for exactly this.
    let sample = Sample::new("runner-picked-up-carrying");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let id = named(&session);
    let mut scripted = Scripted::recording(measured(), Tools::new(), Verdict::Allow, session);
    scripted.runner.spec.model.window = Some(200_000);

    scripted.turn("go").expect("a measured turn");
    assert_eq!(
        scripted.runner.left(),
        Some(72),
        "the exact output correction visibly freed uncompacted context"
    );

    // Started, so it brings nothing back with it, and closing the recorded one
    // is what finishes writing its log.
    let fresh = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    drop(scripted.runner.pick_up(fresh, Transcript::new()));
    assert_eq!(
        scripted.runner.left(),
        Some(100),
        "the fresh empty transcript was not estimated"
    );
    assert_eq!(scripted.runner.load.calibrated(), None);

    drop(picking(&mut scripted, &sample, &id));

    assert_eq!(
        scripted.runner.left(),
        Some(72),
        "the session came back using its last exact provider measurement"
    );
}

#[test]
fn a_reading_taken_against_other_instructions_is_reestimated_for_this_run() {
    // What a request carries includes its fixed content, and the reading covers
    // the two together. Sent under different instructions it describes neither,
    // so the estimate stands and the next answer measures this run for itself.
    let sample = Sample::new("runner-picked-up-elsewhere");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let id = named(&session);
    let mut scripted = Scripted::recording(measured(), Tools::new(), Verdict::Allow, session);
    scripted.runner.spec.model.window = Some(200_000);

    scripted.turn("go").expect("a measured turn");

    let fresh = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    drop(scripted.runner.pick_up(fresh, Transcript::new()));
    scripted.runner.spec.told("answer only in French");

    drop(picking(&mut scripted, &sample, &id));

    assert_eq!(scripted.runner.left(), Some(99));
    assert_eq!(
        scripted.runner.load.calibrated(),
        None,
        "the reading taken against other instructions was reused"
    );
    assert!(
        scripted.runner.load.tokens() < 1_000,
        "nothing of the reading was taken: what came back is a few bytes of          transcript, counted at the rate a session with no report of its own uses"
    );
}

#[test]
fn a_result_cleared_for_another_vendor_stays_cleared_when_the_session_comes_back() {
    // The half a live swap could not answer for. Clearing moved the transcript
    // and nothing else, so the log still held what the transcript no longer
    // did, and the next `--resume` read it back and sent it to the vendor it
    // had just been taken away from — the swap undone by the reopen, silently,
    // with no second swap to notice.
    let sample = Sample::new("runner-picked-restricted");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let id = named(&session);

    let restricting = Script::new(vec![
        calling("call_search", "web_search", r#"{"query":"rust"}"#),
        saying("an answer from the vendor that restricts its results"),
    ])
    .with_name("google")
    .restricting(RESTRICTED);

    let mut scripted = Scripted::recording(
        restricting,
        tools([Fixed::new("web_search").answering("grounded search results canary")]),
        Verdict::Allow,
        session,
    );

    scripted.turn("search for rust").expect("a search turn");
    scripted
        .runner
        .serve(Box::new(Script::new(vec![]).with_name("anthropic")));

    // The run ends: the session being recorded to is released, which is what
    // waits for its queue, and then it is opened again the way `--resume` does.
    drop(
        scripted
            .runner
            .pick_up(Session::nowhere(), Transcript::new()),
    );
    drop(picking(&mut scripted, &sample, &id));

    let result = only_result(&scripted);
    assert!(
        !result
            .output
            .text()
            .contains("grounded search results canary"),
        "a restricted result came back off the log and into another vendor's request: {}",
        result.output.text()
    );
    assert_eq!(
        result.output.text(),
        RESTRICTED,
        "the result came back without the sentence saying why it is empty"
    );
}

#[test]
fn a_session_moved_twice_says_once_that_its_results_were_taken_away() {
    // The write side of the same line. A second swap walks a transcript whose
    // results are already the sentence, and clearing a sentence with itself
    // reports the sentence's own length as freed — so a runner that did not
    // look first would write a second line claiming a clearing that had already
    // happened, and every later swap another.
    let sample = Sample::new("runner-picked-restricted-twice");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let path = session.path().to_owned();

    let restricting = Script::new(vec![
        calling("call_search", "web_search", r#"{"query":"rust"}"#),
        saying("an answer from the vendor that restricts its results"),
    ])
    .with_name("google")
    .restricting(RESTRICTED);

    let mut scripted = Scripted::recording(
        restricting,
        tools([Fixed::new("web_search").answering("grounded search results canary")]),
        Verdict::Allow,
        session,
    );

    // Away, back, and away again — the shape a user gets by trying the other
    // vendor and changing their mind. Only the first move has anything to take.
    scripted.turn("search for rust").expect("a search turn");
    scripted
        .runner
        .serve(Box::new(Script::new(vec![]).with_name("anthropic")));
    scripted.runner.serve(Box::new(
        Script::new(vec![])
            .with_name("google")
            .restricting(RESTRICTED),
    ));
    scripted
        .runner
        .serve(Box::new(Script::new(vec![]).with_name("openai")));

    drop(
        scripted
            .runner
            .pick_up(Session::nowhere(), Transcript::new()),
    );

    let written = std::fs::read_to_string(&path).expect("the log the run wrote");
    assert_eq!(
        written
            .lines()
            .filter(|line| line.contains("\"restricted\""))
            .count(),
        1,
        "the log says more than once that the same results were taken away:\n{written}"
    );
}
