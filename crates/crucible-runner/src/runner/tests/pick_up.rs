//! What a runner put on a different session carries into the next turn.
//!
//! The transcript is the observable part — it is what every request holds — and
//! the store is the other half: what a swap wrote down is what the next pick-up
//! reads, so a swap that moved the transcript and wrote nothing would come
//! apart the moment the session was opened again. Neither is read off the
//! runner's own fields, which is what keeps a swap that set every one of them
//! correctly from passing while the record it left behind said something else.

use crucible_runtime::Bridge;
use crucible_types::ResultProvenance;

use super::unanswered::{Withheld, Withholding};
use super::*;

/// Whose sessions these are. One reader throughout, since nothing here is
/// about telling two of them apart.
const OWNER: &str = "a reader's own sessions";

/// A session holding one turn, closed, ready to be picked up.
fn earlier() -> Arc<Recording> {
    let store = Recording::started(OWNER);
    crucible_runtime::answered!(store.append_message(&Message::said("what came before")));
    crucible_runtime::answered!(store.append_message(&Message::Agent {
        continuation: None,
        text: "an answer from before".into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
    }));
    store
}

/// Puts this run on the session `store` recorded, and answers the store the
/// next turn is recorded to.
///
/// The transcript comes back from the store's own replay rather than from
/// anything the test held, which is the whole of what a pick-up depends on.
fn picking(scripted: &mut Scripted, store: &Recording) -> Arc<Recording> {
    let (picked, transcript) = store.reopened();
    let onto: Arc<dyn JournalStore> = picked.clone();
    scripted.runner.pick_up(onto, transcript);
    picked
}

/// What each clearing this store was told about freed, in order.
pub(super) fn restrictions(store: &Recording) -> Vec<usize> {
    store
        .kept()
        .iter()
        .filter_map(|one| match one {
            Kept::Restricted { freed, .. } => Some(*freed),
            _ => None,
        })
        .collect()
}

/// A run that recorded one restricted search result, and the store it used.
fn restricted_search() -> (Scripted, Arc<Recording>) {
    let store = Recording::started(OWNER);
    let restricting = Script::new(vec![
        calling("call_search", "web_search", r#"{"query":"rust"}"#),
        saying("an answer from the vendor that restricts its results"),
    ])
    .with_name("google")
    .restricting(RESTRICTED);

    let scripted =
        Scripted::recording(restricting, searching(), Verdict::Allow, Arc::clone(&store));
    (scripted, store)
}

/// The search both runs advertise, so a reading one took covers the fixed
/// content of the other's request.
fn searching() -> Tools {
    tools([Fixed::new("web_search")
        .answering("grounded search results canary")
        .answered_by(
            ResultProvenance::answered("google", Some(RESTRICTED)).expect("a bounded term"),
        )])
}

#[test]
fn the_next_turn_is_asked_with_the_transcript_of_the_session_picked_up() {
    let before = earlier();

    let script = Script::new(vec![saying("here"), saying("and now")]);
    let mut scripted = Scripted::recording(
        script,
        Tools::new(),
        Verdict::Allow,
        Recording::started(OWNER),
    );

    scripted.turn("what is in main.rs?").unwrap();
    picking(&mut scripted, &before);
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
    let before = earlier();

    let script = Script::new(vec![saying("here")]);
    let mut scripted = Scripted::recording(
        script,
        Tools::new(),
        Verdict::Allow,
        Recording::started(OWNER),
    );

    picking(&mut scripted, &before);
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
    let script = Script::new(vec![saying("here"), saying("and now")]);
    let mut scripted = Scripted::recording(
        script,
        Tools::new(),
        Verdict::Allow,
        Recording::started(OWNER),
    );

    scripted.turn("what is in main.rs?").unwrap();

    scripted
        .runner
        .pick_up(Recording::started(OWNER), Transcript::new());
    scripted.turn("and now?").unwrap();

    assert_eq!(
        scripted.asked(),
        [7, 7],
        "one prompt and six fresh sections each; no conversation crossed sessions"
    );
    assert_eq!(scripted.started(), [1, 1]);
}

#[test]
fn the_session_left_behind_stops_being_written_to_and_keeps_what_it_held() {
    // The runner hands nothing back, because the caller that opened the store
    // still holds it: closing it and saying what its last write came to are
    // that caller's. What this watches is the other half of the same promise —
    // that after the swap the turns go to the store handed in, and the one left
    // behind is exactly as complete as it was.
    let left = Recording::started(OWNER);
    let script = Script::new(vec![saying("here"), saying("and now")]);
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, Arc::clone(&left));

    scripted.turn("what is in main.rs?").unwrap();
    let held = left.said();
    assert!(!held.is_empty(), "the first session recorded nothing");

    let onto = picking(&mut scripted, &earlier());
    scripted.turn("and after that?").unwrap();

    assert_eq!(left.said(), held, "the session left behind was written to");
    assert!(
        onto.said()
            .iter()
            .any(|said| *said == Message::said("and after that?")),
        "the turn after the swap was not recorded to the session picked up"
    );
}

#[test]
fn what_the_last_session_allowed_is_asked_about_again() {
    // "For the rest of this session" was answered about the session being left
    // behind. Carrying it across would run a tool in a session nobody was
    // asked about.
    let before = earlier();

    let script = Script::new(vec![
        calling("a", "write", "{}"),
        saying("done"),
        calling("b", "write", "{}"),
        saying("done"),
        calling("c", "write", "{}"),
        saying("done"),
    ]);
    let mut scripted = Scripted::recording(
        script,
        tools([Fixed::new("write").risking(changing())]),
        Verdict::Allow,
        Recording::started(OWNER),
    );
    scripted.says = Says::for_the_session();

    scripted.turn("write it").unwrap();
    scripted.turn("write it again").unwrap();
    assert_eq!(scripted.says.asked, 1, "the session allow held");

    picking(&mut scripted, &before);
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
    // read off a store arrives with none of it — so a session picked up used to
    // come back estimating, with the row saying nothing until the next answer
    // reported. The record says what the last request carried for exactly this.
    let store = Recording::started(OWNER);
    let mut scripted =
        Scripted::recording(measured(), Tools::new(), Verdict::Allow, Arc::clone(&store));
    scripted.runner.state.window = Some(200_000);

    scripted.turn("go").expect("a measured turn");
    assert_eq!(
        scripted.runner.left(),
        Some(72),
        "the exact output correction visibly freed uncompacted context"
    );

    // Started, so it brings nothing back with it.
    scripted
        .runner
        .pick_up(Recording::started(OWNER), Transcript::new());
    assert_eq!(
        scripted.runner.left(),
        Some(100),
        "the fresh empty transcript was not estimated"
    );
    assert_eq!(scripted.runner.state.load.calibrated(), None);

    picking(&mut scripted, &store);

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
    let store = Recording::started(OWNER);
    let mut scripted =
        Scripted::recording(measured(), Tools::new(), Verdict::Allow, Arc::clone(&store));
    scripted.runner.state.window = Some(200_000);

    scripted.turn("go").expect("a measured turn");

    scripted
        .runner
        .pick_up(Recording::started(OWNER), Transcript::new());
    scripted
        .runner
        .redefine(|agent| agent.telling("answer only in French"));

    picking(&mut scripted, &store);

    assert_eq!(scripted.runner.left(), Some(99));
    assert_eq!(
        scripted.runner.state.load.calibrated(),
        None,
        "the reading taken against other instructions was reused"
    );
    assert!(
        scripted.runner.state.load.tokens() < 1_000,
        "nothing of the reading was taken: what came back is a few bytes of          transcript, counted at the rate a session with no report of its own uses"
    );
}

#[test]
fn a_result_cleared_for_another_vendor_stays_cleared_when_the_session_comes_back() {
    // The half a live swap could not answer for. Clearing moved the transcript
    // and nothing else, so the record still held what the transcript no longer
    // did, and the next pick-up read it back and sent it to the vendor it had
    // just been taken away from — the swap undone by the reopen, silently, with
    // no second swap to notice.
    let (mut scripted, store) = restricted_search();

    scripted.turn("search for rust").expect("a search turn");
    scripted
        .runner
        .serve(Box::new(Script::new(vec![]).with_name("anthropic")));

    // The run ends: the session being recorded to is let go of, and then it is
    // opened again the way `--resume` does.
    scripted
        .runner
        .pick_up(Recording::nowhere(), Transcript::new());
    picking(&mut scripted, &store);

    let result = only_result(&scripted);
    assert!(
        !result
            .output
            .text()
            .contains("grounded search results canary"),
        "a restricted result came back off the record and into another vendor's request: {}",
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
    let (mut scripted, store) = restricted_search();

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

    scripted
        .runner
        .pick_up(Recording::nowhere(), Transcript::new());

    let cleared = restrictions(&store);
    assert_eq!(
        cleared.len(),
        1,
        "the record says more than once that the same results were taken away"
    );
    assert!(
        cleared.iter().all(|freed| *freed > 0),
        "a clearing that took real results away was recorded as freeing nothing"
    );
}

#[test]
fn a_session_picked_up_by_a_run_serving_another_vendor_leaves_out_what_its_vendor_restricted() {
    // No switch is ever observed here: the run that recorded the results served
    // the vendor that restricts them, and the run that picks the session up was
    // started on another. What the record holds is all that can say the results
    // may not go where this run sends its requests.
    let (mut recorded, store) = restricted_search();
    recorded.turn("search for rust").expect("a search turn");
    recorded
        .runner
        .pick_up(Recording::nowhere(), Transcript::new());

    let mut elsewhere = Scripted::new(
        Script::new(vec![saying("an answer from elsewhere")]).with_name("anthropic"),
        tools([]),
        Verdict::Allow,
    );
    let picked = picking(&mut elsewhere, &store);

    let result = only_result(&elsewhere);
    assert_eq!(
        result.output.text(),
        RESTRICTED,
        "a run serving another vendor picked up a result that vendor may not be sent: {}",
        result.output.text()
    );

    elsewhere
        .runner
        .pick_up(Recording::nowhere(), Transcript::new());
    assert_eq!(
        restrictions(&picked).len(),
        1,
        "the clearing was not written down, so the next pick-up depends on who does it"
    );
}

#[test]
fn a_run_started_on_another_vendor_resumes_a_session_without_what_its_vendor_restricted() {
    // The same question asked of `--resume`, which hands the transcript to a
    // runner being built rather than to one already running.
    let (mut recorded, store) = restricted_search();
    recorded.turn("search for rust").expect("a search turn");
    recorded
        .runner
        .pick_up(Recording::nowhere(), Transcript::new());

    let (picked, transcript) = store.reopened();
    let started = Scripted::recording(
        Script::new(vec![saying("an answer from elsewhere")]).with_name("anthropic"),
        tools([]),
        Verdict::Allow,
        picked,
    );
    let resumed = Scripted {
        runner: started.runner.resuming(transcript),
        ..started
    };

    assert_eq!(
        only_result(&resumed).output.text(),
        RESTRICTED,
        "a run started on another vendor resumed a result that vendor may not be sent"
    );
}

/// A session whose last answer measured a request that carried a search result
/// its vendor restricts, closed so its record is complete.
fn measured_restricted_search() -> Arc<Recording> {
    let store = Recording::started(OWNER);
    let restricting = Script::new(vec![
        calling("call_search", "web_search", r#"{"query":"rust"}"#),
        vec![
            Delta::Carried(Carried::new(40_000)),
            Delta::Text("an answer from the vendor that restricts its results".into()),
            Delta::Spent(Spend::new(10_000)),
            Delta::Stopped(StopReason::Yielded),
        ],
    ])
    .with_name("google")
    .restricting(RESTRICTED);
    let mut recorded =
        Scripted::recording(restricting, searching(), Verdict::Allow, Arc::clone(&store));
    recorded
        .turn("search for rust")
        .expect("a measured search turn");
    assert!(
        recorded.runner.state.load.calibrated().is_some(),
        "the answer left no reading to take back"
    );
    recorded
        .runner
        .pick_up(Recording::nowhere(), Transcript::new());
    store
}

#[test]
fn a_session_picked_up_without_what_its_vendor_restricted_does_not_trust_the_reading_taken_with_it()
{
    // The record's last reading measured a request that carried the results,
    // and this run sends one without them. The reading describes a request
    // nobody will send again, which is why replaying the clearing drops it too.
    let store = measured_restricted_search();

    let mut elsewhere = Scripted::new(
        Script::new(vec![saying("an answer from elsewhere")]).with_name("anthropic"),
        searching(),
        Verdict::Allow,
    );
    picking(&mut elsewhere, &store);

    assert_eq!(only_result(&elsewhere).output.text(), RESTRICTED);
    assert_eq!(
        elsewhere.runner.state.load.calibrated(),
        None,
        "the reading taken with the restricted results in the request was trusted after they were taken out"
    );
}

#[test]
fn a_session_resumed_without_what_its_vendor_restricted_does_not_trust_the_reading_taken_with_it() {
    // The same question asked of `--resume`.
    let store = measured_restricted_search();

    let (picked, transcript) = store.reopened();
    let started = Scripted::recording(
        Script::new(vec![saying("an answer from elsewhere")]).with_name("anthropic"),
        searching(),
        Verdict::Allow,
        picked,
    );
    let resumed = Scripted {
        runner: started.runner.resuming(transcript),
        ..started
    };

    assert_eq!(only_result(&resumed).output.text(), RESTRICTED);
    assert_eq!(
        resumed.runner.state.load.calibrated(),
        None,
        "the reading taken with the restricted results in the request was trusted after they were taken out"
    );
}

#[test]
fn a_session_picked_up_where_nothing_is_set_up_keeps_what_its_vendor_answered() {
    // A run with no usable credential serves a stand-in that sends nothing, so
    // nothing has to be kept from it — and clearing for its sake would take the
    // results away from the vendor the session goes back to once one is set up.
    let (mut recorded, store) = restricted_search();
    recorded.turn("search for rust").expect("a search turn");
    recorded
        .runner
        .pick_up(Recording::nowhere(), Transcript::new());

    let mut unset = Scripted::new(
        Script::new(vec![]).with_name("none").reaching_nothing(),
        tools([]),
        Verdict::Allow,
    );
    let picked = picking(&mut unset, &store);
    unset.runner.serve(Box::new(
        Script::new(vec![])
            .with_name("google")
            .restricting(RESTRICTED),
    ));

    assert_eq!(
        only_result(&unset).output.text(),
        "grounded search results canary",
        "a stand-in that sends nothing took the results away from the vendor that answered them"
    );
    unset
        .runner
        .pick_up(Recording::nowhere(), Transcript::new());
    assert!(
        restrictions(&picked).is_empty(),
        "a clearing was written for a provider nothing is sent to"
    );
}

#[test]
fn a_clearing_line_the_session_never_took_ends_the_next_turn_before_anything_is_sent() {
    // Changing vendor happens between turns, where no caller waits on an
    // error, so a clearing line the session would not take is held for the
    // turn that follows. Whether it was kept is not known, and a session that
    // cannot say what it recorded is not one to carry on writing to.
    let (mut scripted, store) = restricted_search();
    scripted.runner.store = Arc::new(Withholding {
        recording: store,
        withheld: Withheld::Restricted,
    });
    scripted.turn("search for rust").expect("a search turn");
    let anthropic = Script::new(vec![saying("never asked")]).with_name("anthropic");
    let sent = anthropic.sent();

    scripted.runner.serve(Box::new(anthropic));

    assert_eq!(
        only_result(&scripted).output.text(),
        RESTRICTED,
        "the clearing was not made because its line was not taken"
    );
    let problem = scripted.turn("and now?").unwrap_err();
    assert!(
        matches!(
            &problem,
            TurnError::Unready(unready) if unready.bridge() == Bridge::TurnSession
        ),
        "{problem:?}"
    );
    assert!(
        sent.lock().unwrap().is_empty(),
        "the turn asked the provider before reporting the line it held"
    );
}

#[test]
fn a_clearing_line_the_session_never_took_ends_the_next_compaction_before_anything_is_sent() {
    // `/compact` is admitted between turns the way a turn is, and the line it
    // would write past is as unknown to it. Two turns, so there is a middle to
    // recap: a compaction that carried on would record and ask the provider on
    // top of a record that cannot say what it holds.
    let store = Recording::started(OWNER);
    let restricting = Script::new(vec![
        calling("call_search", "web_search", r#"{"query":"rust"}"#),
        saying("an answer from the vendor that restricts its results"),
        saying("a second answer, so the first turn is a middle to recap"),
    ])
    .with_name("google")
    .restricting(RESTRICTED);
    let mut scripted =
        Scripted::recording(restricting, searching(), Verdict::Allow, Arc::clone(&store));
    scripted.runner.store = Arc::new(Withholding {
        recording: Arc::clone(&store),
        withheld: Withheld::Restricted,
    });
    scripted.runner.policy.compaction = Compaction {
        keep_tokens: 1,
        ..Compaction::default()
    };
    scripted.turn("search for rust").expect("a search turn");
    scripted.turn("and then?").expect("a turn to keep");
    let anthropic = Script::new(vec![recap("never asked")]).with_name("anthropic");
    let sent = anthropic.sent();

    scripted.runner.serve(Box::new(anthropic));
    let recorded = store.kept().len();
    let compacted = scripted.compacting();

    assert!(
        matches!(
            &compacted,
            Err(TurnError::Unready(unready)) if unready.bridge() == Bridge::TurnSession
        ),
        "{compacted:?}"
    );
    assert_eq!(
        store.kept().len(),
        recorded,
        "the compaction recorded past the line it held"
    );
    assert!(
        sent.lock().unwrap().is_empty(),
        "the compaction asked the provider before reporting the line it held"
    );
}
