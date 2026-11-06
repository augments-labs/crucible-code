//! Questions asked without waiting: delivered once at a pass boundary,
//! reported while they wait, restated after compaction and picked up again on
//! resume.
//!
//! The tools here stand in for the one that asks and for the person who
//! answers, because neither belongs to this crate: the runner is handed a
//! registry and holds whatever lands in it to the rules below.

use crucible_context::{ContextSection, QuestionsSection};
use crucible_tools::Deferred;
use crucible_types::{
    Answer, Answered, Batch, ContextSnapshot, Fragment, Question, QuestionId, Reply, ToolArgs,
};

use super::*;

/// Words every delivered answer opens with.
const ANSWERED: &str = "The person answered question batch";

/// A tool that asks one question without waiting, as `ask_user` in its
/// deferred mode does, and answers with the id the batch is held as.
struct Asking {
    held: Deferred,
}

impl DescribeTool for Asking {
    fn name(&self) -> &'static str {
        "ask"
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Asking {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::pathless(),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        Summary::new(args.as_str())
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let id = self.held.register(vec![storage()]).expect("one batch fits");
            Ok(ToolOutput::ok(format!("held as {id}")))
        })
    }
}

/// A tool that stands in for the person: while it runs, the oldest waiting
/// batch is settled, and a line may be typed beside it.
struct Answering {
    held: Deferred,
    reply: fn() -> Reply,
    typed: Option<(Steer, &'static str)>,
}

impl DescribeTool for Answering {
    fn name(&self) -> &'static str {
        "answer"
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Answering {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::pathless(),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        Summary::new(args.as_str())
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            settle_oldest(&self.held, (self.reply)());
            if let Some((steer, line)) = &self.typed {
                steer.say((*line).to_owned());
            }
            Ok(ToolOutput::ok("looked"))
        })
    }
}

/// Where a turn's events go, settling the oldest waiting batch the first
/// time the model's prose arrives: an answer accepted while the last pass of
/// a turn is still streaming.
struct Settling {
    watching: Watching,
    held: Deferred,
}

impl Post for Settling {
    fn post(&self, reported: EventEnvelope) {
        let event = reported.into_event();
        if matches!(event, Event::Delta { .. }) && !self.held.waiting().is_empty() {
            settle_oldest(&self.held, chose());
        }
        drop(self.watching.0.send(event));
    }
}

fn storage() -> Question {
    Question::new(
        "Storage",
        "Which database should the cache use?",
        [Answer::new("SQLite"), Answer::new("Postgres")],
    )
}

fn chose() -> Reply {
    Reply::Answered(Answered::new(["SQLite"]))
}

fn declined() -> Reply {
    Reply::Declined
}

fn settle_oldest(held: &Deferred, reply: Reply) {
    let oldest = held
        .waiting()
        .first()
        .map(Batch::id)
        .expect("a batch waits");
    held.settle(oldest, vec![reply]).expect("the reply fits");
}

/// The tools a deferring session offers: the one that asks, and the
/// stand-in for the person.
fn offered(held: &Deferred, reply: fn() -> Reply, typed: Option<(Steer, &'static str)>) -> Tools {
    let mut offered = Tools::new();
    offered
        .add_builtin(Asking { held: held.clone() })
        .expect("a tool");
    offered
        .add_builtin(Answering {
            held: held.clone(),
            reply,
            typed,
        })
        .expect("a tool");
    offered
}

/// A session over `script` that holds its deferred questions in `held`.
fn deferring(script: Vec<Vec<Delta>>, held: &Deferred, store: Arc<Recording>) -> Scripted {
    let scripted = Scripted::recording(
        Script::new(script),
        offered(held, chose, None),
        Verdict::Allow,
        store,
    );
    Scripted {
        runner: scripted.runner.deferring(held.clone()),
        ..scripted
    }
}

/// Every delivered answer in `transcript`, with where it sits.
fn answers(transcript: &Transcript) -> Vec<(usize, String)> {
    transcript
        .messages()
        .iter()
        .enumerate()
        .filter_map(|(at, message)| match message {
            Message::User { text, .. } if text.starts_with(ANSWERED) => {
                Some((at, text.to_string()))
            }
            _ => None,
        })
        .collect()
}

/// Where the results answering the call `id` sit in `transcript`.
fn results_of(transcript: &Transcript, id: &str) -> usize {
    transcript
        .messages()
        .iter()
        .position(|message| match message {
            Message::ToolResults(results) => results.iter().any(|result| result.id.as_str() == id),
            _ => false,
        })
        .expect("the call was answered")
}

/// How many batches each `Waiting` posted so far reported.
fn reported(seen: &Receiver<Event>) -> Vec<usize> {
    seen.try_iter()
        .filter_map(|event| match event {
            Event::Waiting { batches } => Some(batches.len()),
            _ => None,
        })
        .collect()
}

/// Every questions fragment `transcript` holds.
fn told(transcript: &Transcript) -> Vec<Fragment> {
    transcript
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::Context(fragment) if fragment.section() == QuestionsSection::ID => {
                Some(fragment.clone())
            }
            _ => None,
        })
        .collect()
}

/// The batches a registry restored from `store`'s recorded state holds.
fn restored(store: &Recording) -> Vec<Batch> {
    let held = Deferred::default();
    if let Some(saved) = store
        .context_snapshot()
        .and_then(|snapshot| snapshot.get(QuestionsSection::ID).cloned())
    {
        held.restore(&saved).expect("the saved state restores");
    }
    held.waiting()
}

/// The same session picked up from `store`, holding its questions in `held`.
fn resumed(store: &Recording, script: Vec<Vec<Delta>>, held: Option<&Deferred>) -> Scripted {
    let (picked, transcript) = store.reopened();
    resumed_from(picked, transcript, script, held)
}

/// A session picked up as `transcript` in `picked`, holding its questions in
/// `held`.
fn resumed_from(
    picked: Arc<Recording>,
    transcript: Transcript,
    script: Vec<Vec<Delta>>,
    held: Option<&Deferred>,
) -> Scripted {
    let fresh = Deferred::default();
    let started = Scripted::recording(
        Script::new(script),
        offered(held.unwrap_or(&fresh), chose, None),
        Verdict::Allow,
        picked,
    );
    let runner = started.runner.resuming(transcript);
    Scripted {
        runner: match held {
            Some(held) => runner.deferring(held.clone()),
            None => runner,
        },
        ..started
    }
}

#[test]
fn a_deferred_registration_leads_to_a_second_pass_before_any_answer() {
    let held = Deferred::default();
    let mut scripted = deferring(
        vec![calling("c1", "ask", "{}"), saying("going on meanwhile")],
        &held,
        Recording::nowhere(),
    );

    assert_eq!(
        scripted.turn("plan it").expect("a turn"),
        StopReason::Yielded
    );

    let sent = scripted.sent.lock().unwrap();
    assert_eq!(sent.len(), 2, "the model went on before anybody answered");
    assert!(
        sent.get(1)
            .expect("the second")
            .carried_result("held as q0")
    );
    assert!(answers(scripted.runner.transcript()).is_empty());
    assert_eq!(held.waiting().len(), 1, "the batch still waits");
}

#[test]
fn a_batch_settled_in_a_tool_pass_is_delivered_once_right_after_that_pass_results() {
    let held = Deferred::default();
    let mut scripted = deferring(
        vec![
            calling("c1", "ask", "{}"),
            calling("c2", "answer", "{}"),
            saying("thanks"),
            saying("a later turn"),
        ],
        &held,
        Recording::nowhere(),
    );

    scripted.turn("plan it").expect("a turn");
    scripted.turn("and then").expect("a later turn");

    let transcript = scripted.runner.transcript();
    let [(at, text)] = answers(transcript)
        .try_into()
        .expect("delivered exactly once");
    assert!(text.contains("- Storage: SQLite"), "{text}");
    assert!(
        at > results_of(transcript, "c2"),
        "an answer came before the results"
    );
    assert!(
        transcript
            .messages()
            .iter()
            .take(at)
            .skip(results_of(transcript, "c2") + 1)
            .all(|message| matches!(message, Message::Context(_))),
        "something other than context came between the results and the answer"
    );
    let sent = scripted.sent.lock().unwrap();
    let carrying: Vec<bool> = sent.iter().map(|one| one.transcript_len > at).collect();
    assert_eq!(
        carrying,
        [false, false, true, true],
        "the next request is the first to carry it"
    );
    assert!(!held.any_settled(), "the delivered batch is still held");
}

#[test]
fn an_answer_accepted_during_the_last_pass_is_delivered_before_the_turn_ends() {
    let held = Deferred::default();
    let scripted = deferring(
        vec![
            calling("c1", "ask", "{}"),
            saying("nothing else to do"),
            saying("with the answer in hand"),
        ],
        &held,
        Recording::nowhere(),
    );
    let mut runner = scripted.runner;
    let settling = Settling {
        watching: scripted.events,
        held: held.clone(),
    };
    let run = runner.starting(
        &settling,
        &scripted.cancel,
        &scripted.steer,
        &scripted.aside,
    );
    let mut says = scripted.says;

    let turned = runner
        .turn("plan it", Box::new([]), &mut says, &run)
        .awaited()
        .map(ran);

    assert_eq!(turned.expect("a turn"), StopReason::Yielded);
    assert_eq!(
        scripted.sent.lock().unwrap().len(),
        3,
        "the turn ended on an undelivered answer"
    );
    let transcript = runner.transcript();
    let [(at, _)] = answers(transcript)
        .try_into()
        .expect("delivered exactly once");
    let last = transcript.messages().last().expect("the last answer");
    assert!(
        matches!(last, Message::Agent { text, .. } if &**text == "with the answer in hand"),
        "{last:?}"
    );
    assert!(at < transcript.len() - 1);
    assert!(!held.any_settled());
}

#[test]
fn a_steered_line_and_an_answer_arriving_together_are_both_recorded_and_told_apart() {
    let held = Deferred::default();
    let steer = Steer::new();
    let scripted = Scripted::recording(
        Script::new(vec![
            calling("c1", "ask", "{}"),
            calling("c2", "answer", "{}"),
            saying("both read"),
        ]),
        offered(&held, chose, Some((steer.clone(), "use the fast one"))),
        Verdict::Allow,
        Recording::nowhere(),
    );
    let mut scripted = Scripted {
        runner: scripted.runner.deferring(held.clone()),
        steer,
        ..scripted
    };

    scripted.turn("plan it").expect("a turn");

    let transcript = scripted.runner.transcript();
    let steered = transcript
        .messages()
        .iter()
        .position(|message| matches!(message, Message::User { text, .. } if &**text == "use the fast one"))
        .expect("the steered line was recorded");
    let [(answer, _)] = answers(transcript)
        .try_into()
        .expect("the answer was recorded");
    assert!(
        steered < answer,
        "the answer came before the line typed beside it"
    );
    let lines: Vec<String> = scripted
        .seen
        .try_iter()
        .filter_map(|event| match event {
            Event::Steered { line } => Some(line),
            _ => None,
        })
        .collect();
    assert_eq!(
        lines,
        ["use the fast one"],
        "an answer was reported as typed"
    );
}

#[test]
fn a_declined_batch_reaches_the_model_as_unresolved() {
    let held = Deferred::default();
    let scripted = Scripted::recording(
        Script::new(vec![
            calling("c1", "ask", "{}"),
            calling("c2", "answer", "{}"),
            saying("noted"),
        ]),
        offered(&held, declined, None),
        Verdict::Allow,
        Recording::nowhere(),
    );
    let mut scripted = Scripted {
        runner: scripted.runner.deferring(held.clone()),
        ..scripted
    };

    scripted.turn("plan it").expect("a turn");

    let [(_, text)] = answers(scripted.runner.transcript())
        .try_into()
        .expect("the declined batch was delivered");
    assert!(text.contains("Storage: unresolved"), "{text}");
    assert!(!text.contains("SQLite"), "{text}");
}

#[test]
fn waiting_is_reported_when_a_batch_is_asked_and_delivered_and_not_when_nothing_changed() {
    let held = Deferred::default();
    let mut scripted = deferring(
        vec![
            calling("c1", "ask", "{}"),
            calling("c2", "answer", "{}"),
            saying("thanks"),
            saying("nothing new"),
        ],
        &held,
        Recording::nowhere(),
    );

    scripted.turn("plan it").expect("a turn");
    // Asked, then answered in the next tool pass, then delivered at the top
    // of the pass after it.
    assert_eq!(reported(&scripted.seen), [1, 0, 0]);

    scripted.turn("and then").expect("a later turn");
    assert_eq!(
        reported(&scripted.seen),
        Vec::<usize>::new(),
        "nothing changed"
    );

    let mut plain = Scripted::new(
        Script::new(vec![saying("no registry")]),
        Tools::new(),
        Verdict::Allow,
    );
    plain.turn("hello").expect("a turn");
    assert_eq!(
        reported(&plain.seen),
        Vec::<usize>::new(),
        "a runner holding none reports none"
    );
}

#[test]
fn waiting_questions_are_restated_after_compaction() {
    let held = Deferred::default();
    let mut scripted = deferring(
        vec![
            calling("c1", "ask", "{}"),
            saying("first"),
            saying("second"),
            recap("kept context"),
            saying("after"),
        ],
        &held,
        Recording::nowhere(),
    );
    scripted.runner.policy.compaction = Compaction {
        keep_tokens: 1,
        ..Compaction::default()
    };
    scripted.turn("first").expect("a first turn");
    scripted.turn("second").expect("a second turn");
    let [before] = told(scripted.runner.transcript())
        .try_into()
        .expect("told once");
    assert!(before.text().contains("q0"), "{before:?}");

    let room = scripted.compacting().expect("a compaction");
    assert!(matches!(room, Room::Made(_)), "{room:?}");
    assert!(told(scripted.runner.transcript()).is_empty());

    scripted.turn("third").expect("the turn after compaction");
    let [after] = told(scripted.runner.transcript())
        .try_into()
        .expect("told again");
    assert!(after.text().contains("q0"), "{after:?}");
    assert!(
        after
            .text()
            .contains("Which database should the cache use?")
    );
}

#[test]
fn a_resumed_session_restores_its_waiting_batches_and_an_answer_never_delivered() {
    let store = Recording::started("a session asking later");
    let held = Deferred::default();
    let mut first = deferring(
        vec![
            calling("c1", "ask", "{}"),
            calling("c2", "ask", "{}"),
            saying("waiting"),
        ],
        &held,
        Arc::clone(&store),
    );
    first.turn("plan it").expect("a turn");
    // Answered after the turn ended, and never delivered before the restart.
    held.settle(QuestionId::numbered(1), vec![chose()])
        .expect("the second batch settles");
    drop(first);

    let later = Deferred::default();
    let mut resumed = resumed(
        &store,
        vec![calling("c3", "ask", "{}"), saying("picked up")],
        Some(&later),
    );
    resumed.turn("continue").expect("the resumed turn");

    let ids: Vec<String> = later
        .waiting()
        .iter()
        .map(|batch| batch.id().to_string())
        .collect();
    assert_eq!(
        ids,
        ["q0", "q1", "q2"],
        "a batch was lost, or an id handed out again"
    );
    assert_eq!(
        reported(&resumed.seen).first(),
        Some(&2),
        "the restored batches were not reported"
    );
}

#[test]
fn a_runner_with_no_registry_resuming_waiting_questions_is_told_once_they_lapsed() {
    let store = Recording::started("a session asking later");
    let held = Deferred::default();
    let mut first = deferring(
        vec![calling("c1", "ask", "{}"), saying("waiting")],
        &held,
        Arc::clone(&store),
    );
    first.turn("plan it").expect("a turn");
    drop(first);
    let (_, before) = store.reopened();
    let already = told(&before).len();

    let mut resumed = resumed(&store, vec![saying("one"), saying("two")], None);
    resumed.turn("continue").expect("a resumed turn");
    resumed.turn("again").expect("another");

    let told = told(resumed.runner.transcript());
    let lapsed: Vec<&Fragment> = told.iter().skip(already).collect();
    let [lapsed] = lapsed.as_slice() else {
        panic!("told {lapsed:?}");
    };
    assert!(
        lapsed.text().contains("can no longer be answered"),
        "{lapsed:?}"
    );
    assert!(!lapsed.text().contains("Which database"), "{lapsed:?}");
}

#[test]
fn a_child_runs_passes_take_nothing_from_the_root_registry() {
    let held = Deferred::default();
    let id = held.register(vec![storage()]).expect("a batch");
    held.settle(id, vec![chose()]).expect("settled");
    let mut scripted = deferring(
        vec![saying("a child's answer")],
        &held,
        Recording::nowhere(),
    );
    let told = held.revision();

    let mut root = scripted.runner.starting(
        &scripted.events,
        &scripted.cancel,
        &scripted.steer,
        &scripted.aside,
    );
    let child = root.child(RunPolicy::default());
    let turned = scripted
        .runner
        .turn("a child's task", Box::new([]), &mut scripted.says, &child)
        .awaited()
        .map(ran);

    assert_eq!(turned.expect("a turn"), StopReason::Yielded);
    assert!(held.any_settled(), "a child run took the root's answer");
    assert_eq!(held.revision(), told);
    assert!(answers(scripted.runner.transcript()).is_empty());
    assert_eq!(reported(&scripted.seen), Vec::<usize>::new());
}

#[test]
fn a_snapshot_with_a_section_this_build_does_not_know_still_replays() {
    let store = Recording::started("a session from a later build");
    let held = Deferred::default();
    let mut first = deferring(
        vec![calling("c1", "ask", "{}"), saying("waiting")],
        &held,
        Arc::clone(&store),
    );
    first.turn("plan it").expect("a turn");
    drop(first);
    // A section named by nothing this build assembles, holding any object.
    let mut later_build = ContextSnapshot::new();
    later_build
        .record("unheard", Deferred::default().state())
        .expect("an object");
    let unheard = later_build
        .patch_from(&ContextSnapshot::new())
        .expect("a patch");
    store.contextual(&unheard).awaited().expect("recorded");

    let later = Deferred::default();
    let mut resumed = resumed(&store, vec![saying("still here")], Some(&later));

    assert_eq!(
        resumed.turn("continue").expect("a resumed turn"),
        StopReason::Yielded
    );
    assert_eq!(later.waiting().len(), 1, "the questions did not come back");
}

#[test]
fn a_crash_after_an_id_reached_the_log_never_hands_that_id_out_again() {
    let store = Recording::started("a session cut short");
    let held = Deferred::default();
    let mut first = deferring(
        vec![calling("c1", "ask", "{}"), saying("waiting")],
        &held,
        Arc::clone(&store),
    );
    first.turn("plan it").expect("a turn");
    drop(first);
    let receipt = store
        .kept()
        .iter()
        .position(|one| matches!(one, Kept::Said(Message::ToolResults(_))))
        .expect("the receipt was recorded");

    // Lost right after the receipt naming the id was written, before the
    // pass that would have told the model what waits.
    let cut = store.cut_short(receipt + 1);
    let later = Deferred::default();
    let mut resumed = resumed(
        &cut,
        vec![calling("c2", "ask", "{}"), saying("again")],
        Some(&later),
    );
    resumed.turn("continue").expect("the resumed turn");

    let sent = resumed.sent.lock().unwrap();
    let after = sent.last().expect("the resumed turn's last request");
    assert!(
        after.carried_result("held as q1"),
        "q0 was handed out again"
    );
    let ids: Vec<String> = later
        .waiting()
        .iter()
        .map(|batch| batch.id().to_string())
        .collect();
    assert_eq!(
        ids,
        ["q0", "q1"],
        "the batch the receipt names did not come back"
    );
}

#[test]
fn an_answer_is_never_lost_to_a_crash_between_its_settling_and_its_delivery() {
    let store = Recording::started("a session cut short");
    let held = Deferred::default();
    let mut first = deferring(
        vec![
            calling("c1", "ask", "{}"),
            calling("c2", "answer", "{}"),
            saying("thanks"),
        ],
        &held,
        Arc::clone(&store),
    );
    first.turn("plan it").expect("a turn");
    drop(first);
    let kept = store.kept();
    let receipt = kept
        .iter()
        .position(|one| matches!(one, Kept::Said(Message::ToolResults(_))))
        .expect("the receipt was recorded");

    // Every place the log can end once the batch was asked: the answer is in
    // the transcript a pick-up reads, or the batch is restored as waiting.
    for at in receipt + 1..=kept.len() {
        let cut = store.cut_short(at);
        let (_, transcript) = cut.reopened();
        let delivered = !answers(&transcript).is_empty();
        let waiting = restored(&cut.reopened().0)
            .iter()
            .any(|batch| batch.id().to_string() == "q0");
        assert!(
            delivered || waiting,
            "a crash before record {at} lost the answer"
        );
    }
}

/// The ids of the batches `held` lists as waiting, in order.
fn waiting_ids(held: &Deferred) -> Vec<String> {
    held.waiting()
        .iter()
        .map(|batch| batch.id().to_string())
        .collect()
}

#[test]
fn a_batch_asked_after_earlier_ones_were_told_is_told_too() {
    let held = Deferred::default();
    let mut scripted = deferring(
        vec![
            calling("c1", "ask", "{}"),
            saying("first"),
            calling("c2", "ask", "{}"),
            saying("second"),
        ],
        &held,
        Recording::nowhere(),
    );
    scripted.turn("first").expect("a first turn");
    scripted.turn("second").expect("a second turn");

    let told = told(scripted.runner.transcript());
    let last = told.last().expect("the questions were told");
    assert!(last.text().contains("q0"), "{told:?}");
    assert!(
        last.text().contains("q1"),
        "the second batch was never told: {told:?}"
    );
}

#[test]
fn a_child_runs_passes_leave_the_recorded_questions_as_the_root_left_them() {
    let store = Recording::started("a session with a child");
    let held = Deferred::default();
    let mut scripted = deferring(
        vec![
            calling("c1", "ask", "{}"),
            saying("waiting"),
            saying("a child's answer"),
        ],
        &held,
        Arc::clone(&store),
    );
    scripted.turn("plan it").expect("a root turn");
    // Answered while the child runs, and not yet delivered to the root.
    settle_oldest(&held, chose());
    let recorded = || {
        store
            .context_snapshot()
            .and_then(|snapshot| snapshot.get(QuestionsSection::ID).cloned())
    };
    let before = recorded();

    let mut root = scripted.runner.starting(
        &scripted.events,
        &scripted.cancel,
        &scripted.steer,
        &scripted.aside,
    );
    let child = root.child(RunPolicy::default());
    let turned = scripted
        .runner
        .turn("a child's task", Box::new([]), &mut scripted.says, &child)
        .awaited()
        .map(ran);

    assert_eq!(turned.expect("a child turn"), StopReason::Yielded);
    assert_eq!(
        recorded(),
        before,
        "a child run rewrote the root's questions"
    );
    assert!(
        restored(&store)
            .iter()
            .any(|batch| batch.id().to_string() == "q0"),
        "a crash now would lose the answer the root has not delivered"
    );
}

#[test]
fn a_runner_with_no_registry_is_told_waiting_questions_lapsed_after_a_compaction_too() {
    let store = Recording::started("a session asking later");
    let held = Deferred::default();
    let mut first = deferring(
        vec![
            calling("c1", "ask", "{}"),
            saying("first"),
            saying("second"),
            recap("kept context"),
        ],
        &held,
        Arc::clone(&store),
    );
    first.runner.policy.compaction = Compaction {
        keep_tokens: 1,
        ..Compaction::default()
    };
    first.turn("plan it").expect("a turn");
    first.turn("go on").expect("another");
    let room = first.compacting().expect("a compaction");
    assert!(matches!(room, Room::Made(_)), "{room:?}");
    assert!(told(first.runner.transcript()).is_empty());
    drop(first);

    let mut resumed = resumed(&store, vec![saying("after")], None);
    resumed.turn("continue").expect("the turn after compaction");

    let told = told(resumed.runner.transcript());
    let [lapsed] = told.as_slice() else {
        panic!("told {told:?}");
    };
    assert!(
        lapsed.text().contains("can no longer be answered"),
        "{lapsed:?}"
    );
}

#[test]
fn a_run_with_no_registry_between_two_that_have_one_never_hands_an_id_out_again() {
    let store = Recording::started("a session asking later");
    let held = Deferred::default();
    let mut first = deferring(
        vec![calling("c1", "ask", "{}"), saying("waiting")],
        &held,
        Arc::clone(&store),
    );
    first.turn("plan it").expect("a turn");
    drop(first);

    let (between_store, transcript) = store.reopened();
    let mut between = resumed_from(
        Arc::clone(&between_store),
        transcript,
        vec![saying("nothing to hold")],
        None,
    );
    between
        .turn("look around")
        .expect("a turn holding no registry");
    drop(between);

    let later = Deferred::default();
    let mut last = resumed(
        &between_store,
        vec![calling("c2", "ask", "{}"), saying("asked")],
        Some(&later),
    );
    last.turn("ask again")
        .expect("a turn holding a registry again");

    assert_eq!(waiting_ids(&later), ["q1"], "q0 was handed out again");
}

#[test]
fn a_runner_that_picks_up_a_session_restores_the_batches_it_left_waiting() {
    let store = Recording::started("a session asking later");
    let held = Deferred::default();
    let mut first = deferring(
        vec![calling("c1", "ask", "{}"), saying("waiting")],
        &held,
        Arc::clone(&store),
    );
    first.turn("plan it").expect("a turn");
    drop(first);

    let later = Deferred::default();
    let mut other = deferring(
        vec![saying("another session"), saying("picked up")],
        &later,
        Recording::nowhere(),
    );
    other
        .turn("something else")
        .expect("a turn before the pick-up");
    let (picked, transcript) = store.reopened();
    other.runner.pick_up(picked, transcript);
    other.turn("continue").expect("the picked-up turn");

    assert_eq!(waiting_ids(&later), ["q0"], "the waiting batch was lost");
}

#[test]
fn a_runner_still_holding_its_own_batch_picks_up_the_batches_the_session_left_waiting() {
    let store = Recording::started("a session asking twice");
    let held = Deferred::default();
    let mut first = deferring(
        vec![
            calling("c1", "ask", "{}"),
            saying("one waits"),
            calling("c2", "ask", "{}"),
            saying("two wait"),
        ],
        &held,
        Arc::clone(&store),
    );
    first.turn("plan it").expect("a first turn");
    first.turn("plan more").expect("a second turn");
    drop(first);

    let later = Deferred::default();
    let mut other = deferring(
        vec![
            calling("c1", "ask", "{}"),
            saying("another session asks"),
            saying("picked up"),
        ],
        &later,
        Recording::nowhere(),
    );
    other
        .turn("something else")
        .expect("a turn before the pick-up");
    assert_eq!(waiting_ids(&later), ["q0"], "the other session asked");
    let (picked, transcript) = store.reopened();
    other.runner.pick_up(picked.clone(), transcript);
    other.turn("continue").expect("the picked-up turn");

    assert_eq!(
        waiting_ids(&later),
        ["q0", "q1"],
        "the picked-up session's batches were not restored"
    );
    let recorded: Vec<String> = restored(&picked)
        .iter()
        .map(|batch| batch.id().to_string())
        .collect();
    assert_eq!(
        recorded,
        ["q0", "q1"],
        "the picked-up record was overwritten"
    );
}

#[test]
fn a_recorded_state_this_build_refuses_never_lets_an_id_the_log_names_out_again() {
    let store = Recording::started("a session asking three times");
    let held = Deferred::default();
    let mut first = deferring(
        vec![
            calling("c1", "ask", "{}"),
            calling("c2", "ask", "{}"),
            calling("c3", "ask", "{}"),
            saying("three wait"),
        ],
        &held,
        Arc::clone(&store),
    );
    first.turn("plan it").expect("a turn");
    drop(first);
    // Its next number kept, its waiting list one this build cannot read, as
    // another build or a damaged log could leave it.
    let before = store.context_snapshot().expect("a snapshot");
    let mut unreadable = before
        .get(QuestionsSection::ID)
        .cloned()
        .expect("the questions were recorded");
    assert_eq!(
        unreadable.get("next").map(ToString::to_string).as_deref(),
        Some("3")
    );
    unreadable
        .as_object_mut()
        .expect("an object")
        .insert("waiting".to_owned(), "unreadable".into());
    let mut damaged = before.clone();
    damaged
        .record(QuestionsSection::ID, unreadable)
        .expect("an object");
    let patch = damaged.patch_from(&before).expect("a patch");
    store.contextual(&patch).awaited().expect("recorded");

    let later = Deferred::default();
    let mut resumed = resumed(
        &store,
        vec![
            saying("told they lapsed"),
            calling("c4", "ask", "{}"),
            saying("asked"),
        ],
        Some(&later),
    );
    resumed.turn("continue").expect("a turn after the refusal");
    resumed.turn("ask again").expect("a turn that asks");

    assert_eq!(
        waiting_ids(&later),
        ["q3"],
        "an id the log names was handed out again"
    );
}
