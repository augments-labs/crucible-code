//! What the person asked for, and every later instruction, reaching the model
//! after compaction took the messages that said them.

use crucible_context::Room;
use crucible_tools::Deferred;
use crucible_types::{Answer, Answered, Question, Reply};

use super::*;

/// The persisted id of the section that keeps the person's words.
const TASK: &str = "task";

/// A session that compacts down to the least it can keep.
fn compacting(script: Vec<Vec<Delta>>, tools: Tools) -> Scripted {
    let mut scripted = Scripted::new(Script::new(script), tools, Verdict::Allow);
    scripted.runner.policy.compaction = Compaction {
        keep_tokens: 1,
        ..Compaction::default()
    };
    scripted
}

/// Compacts, and fails the test if no room was made.
fn compacted(scripted: &mut Scripted) {
    let room = scripted.compacting().expect("a structured compaction");
    assert!(matches!(room, Room::Made(_)), "nothing compacted: {room:?}");
}

/// What the last request sent restated of the person's words, if anything.
fn restated(sent: &Sent) -> Option<String> {
    sent.lock()
        .unwrap()
        .last()
        .expect("a request was sent")
        .context
        .iter()
        .find(|fragment| fragment.section() == TASK)
        .map(|fragment| fragment.text().to_owned())
}

/// Where each of `said` sits in `text`, failing the test if one is missing or
/// they are out of order.
fn in_order(text: &str, said: &[&str]) {
    let mut from = 0;
    for one in said {
        let Some(at) = text.get(from..).and_then(|rest| rest.find(one)) else {
            panic!("{one:?} is missing, or out of order, in {text:?}");
        };
        from += at + one.len();
    }
}

/// How many times `text` appears in what the transcript holds: the person's
/// messages and the context fragments, which is what a request is built from.
fn occurrences(transcript: &Transcript, text: &str) -> usize {
    transcript
        .messages()
        .iter()
        .map(|message| match message {
            Message::User { text: said, .. } => said.matches(text).count(),
            Message::Context(fragment) => fragment.text().matches(text).count(),
            Message::Agent { .. } | Message::ToolResults(_) => 0,
        })
        .sum()
}

#[test]
fn before_any_compaction_no_request_carries_the_first_prompt_twice() {
    let mut scripted = compacting(vec![saying("first"), saying("second")], Tools::new());
    scripted.turn("make the parser streaming").expect("a turn");
    scripted.steer.say("keep the old entry point".into());
    scripted.turn("and add a benchmark").expect("a second turn");

    let transcript = scripted.runner.transcript();
    // The transcript only grows before a compaction, so what it holds now is
    // every request sent so far with the words each added.
    assert_eq!(occurrences(transcript, "make the parser streaming"), 1);
    assert_eq!(occurrences(transcript, "keep the old entry point"), 1);
    assert_eq!(occurrences(transcript, "and add a benchmark"), 1);
    assert!(
        restated(&scripted.sent).is_none_or(|told| !told.contains("make the parser")),
        "the section copied the person's words while their messages were still there"
    );
}

#[test]
fn after_a_compaction_the_next_request_restates_the_request_and_each_steer_in_order() {
    let mut scripted = compacting(
        vec![
            saying("first"),
            saying("second"),
            recap("kept context"),
            saying("third"),
        ],
        Tools::new(),
    );
    scripted.turn("make the parser streaming").expect("a turn");
    scripted.steer.say("keep the old entry point".into());
    scripted.turn("and add a benchmark").expect("a second turn");
    compacted(&mut scripted);

    scripted.turn("go on").expect("the turn after compaction");

    let told = restated(&scripted.sent).expect("the person's words were restated");
    in_order(
        &told,
        &[
            "make the parser streaming",
            "and add a benchmark",
            "keep the old entry point",
            "go on",
        ],
    );
}

#[test]
fn a_steer_after_a_compaction_is_carried_and_restated_after_a_second() {
    let mut scripted = compacting(
        vec![
            saying("first"),
            saying("second"),
            recap("kept context"),
            saying("third"),
            saying("fourth"),
            recap("kept again"),
            saying("fifth"),
        ],
        Tools::new(),
    );
    scripted.turn("make the parser streaming").expect("a turn");
    scripted.turn("and add a benchmark").expect("a second turn");
    compacted(&mut scripted);
    scripted.steer.say("use no new dependency".into());
    scripted.turn("go on").expect("the turn after compaction");
    assert!(
        scripted.runner.transcript().messages().iter().any(
            |message| matches!(message, Message::User { text, .. } if &**text == "use no new dependency")
        ),
        "the steer was not carried in the next request"
    );
    scripted
        .turn("keep going")
        .expect("a second turn after compaction");
    compacted(&mut scripted);

    scripted
        .turn("finish")
        .expect("the turn after the second compaction");

    let told = restated(&scripted.sent).expect("the person's words were restated");
    in_order(
        &told,
        &[
            "make the parser streaming",
            "and add a benchmark",
            "go on",
            "use no new dependency",
            "keep going",
            "finish",
        ],
    );
}

#[test]
fn a_delivered_answer_is_restated_after_compaction() {
    let held = Deferred::default();
    let id = held
        .register(vec![Question::new(
            "Storage",
            "Which database should the cache use?",
            [Answer::new("SQLite"), Answer::new("Postgres")],
        )])
        .expect("a batch fits");
    held.settle(id, vec![Reply::Answered(Answered::new(["SQLite"]))])
        .expect("the reply fits");
    let scripted = compacting(
        vec![saying("first"), recap("kept context"), saying("second")],
        Tools::new(),
    );
    let mut scripted = Scripted {
        runner: scripted.runner.deferring(held),
        ..scripted
    };
    scripted
        .turn("add a cache")
        .expect("a turn that delivers the answer");
    let answer = scripted
        .runner
        .transcript()
        .messages()
        .iter()
        .find_map(|message| match message {
            Message::User { text, .. } if text.contains("SQLite") => Some(text.to_string()),
            _ => None,
        })
        .expect("the answer was delivered");
    compacted(&mut scripted);

    scripted.turn("go on").expect("the turn after compaction");

    let told = restated(&scripted.sent).expect("the person's words were restated");
    // Restated a line at a time, each quoted as the person's.
    let mut said = vec!["add a cache"];
    said.extend(answer.lines());
    said.push("go on");
    in_order(&told, &said);
}

#[test]
fn a_todo_write_that_replaces_the_plan_leaves_the_restated_task_unchanged() {
    let restating = |planning: bool| {
        let mut script = Vec::new();
        if planning {
            script.push(calling(
                "p",
                "todo_write",
                r#"{"tasks":[{"text":"rewrite everything in another language","state":"doing"}]}"#,
            ));
        }
        script.extend([
            saying("first"),
            saying("second"),
            recap("kept context"),
            saying("third"),
        ]);
        let mut scripted = compacting(script, tools([Fixed::new("todo_write")]));
        scripted.turn("make the parser streaming").expect("a turn");
        scripted.turn("and add a benchmark").expect("a second turn");
        compacted(&mut scripted);
        scripted.turn("go on").expect("the turn after compaction");
        restated(&scripted.sent).expect("the person's words were restated")
    };

    assert_eq!(restating(true), restating(false));
}

#[test]
fn a_resumed_session_restates_the_task_its_log_recorded() {
    let store = Recording::started("a session resumed after a compaction");
    let mut first = Scripted::recording(
        Script::new(vec![
            saying("first"),
            saying("second"),
            recap("kept context"),
        ]),
        Tools::new(),
        Verdict::Allow,
        Arc::clone(&store),
    );
    first.runner.policy.compaction = Compaction {
        keep_tokens: 1,
        ..Compaction::default()
    };
    first.turn("make the parser streaming").expect("a turn");
    first.steer.say("keep the old entry point".into());
    first.turn("and add a benchmark").expect("a second turn");
    compacted(&mut first);
    drop(first);

    // Picked up in a process that never heard the person: what it restates
    // can only come from what the log recorded.
    let (picked, replayed) = store.reopened();
    let mut later = Scripted::new(
        Script::new(vec![saying("continued")]),
        Tools::new(),
        Verdict::Allow,
    );
    later.runner.pick_up(picked, replayed);
    later.turn("go on").expect("the turn after the pick-up");

    let told = restated(&later.sent).expect("the person's words were restated");
    in_order(
        &told,
        &[
            "make the parser streaming",
            "and add a benchmark",
            "keep the old entry point",
            "go on",
        ],
    );
}

#[test]
fn a_session_picked_up_never_restates_the_words_of_the_session_left() {
    let mut scripted = compacting(
        vec![
            saying("first"),
            saying("second"),
            saying("third"),
            recap("kept context"),
            saying("fourth"),
        ],
        Tools::new(),
    );
    scripted
        .turn("rewrite the session left behind")
        .expect("a turn in the session left");

    scripted
        .runner
        .pick_up(Recording::started("a new session"), Transcript::new());
    scripted.turn("make the parser streaming").expect("a turn");
    scripted.turn("and add a benchmark").expect("a second turn");
    compacted(&mut scripted);
    scripted.turn("go on").expect("the turn after compaction");

    let told = restated(&scripted.sent).expect("the person's words were restated");
    in_order(
        &told,
        &["make the parser streaming", "and add a benchmark", "go on"],
    );
    assert!(
        !told.contains("rewrite the session left behind"),
        "the session left's words were restated in the one picked up: {told:?}"
    );
}

#[test]
fn an_answer_delivered_just_before_the_log_ends_is_restated_after_a_resume() {
    let held = Deferred::default();
    let id = held
        .register(vec![Question::new(
            "Storage",
            "Which database should the cache use?",
            [Answer::new("SQLite"), Answer::new("Postgres")],
        )])
        .expect("a batch fits");
    held.settle(id, vec![Reply::Answered(Answered::new(["SQLite"]))])
        .expect("the reply fits");
    let store = Recording::started("a session that ends after an answer");
    let first = Scripted::recording(
        Script::new(vec![saying("first")]),
        Tools::new(),
        Verdict::Allow,
        Arc::clone(&store),
    );
    let mut first = Scripted {
        runner: first.runner.deferring(held),
        ..first
    };
    // The answer is delivered at the turn's only assembly, so nothing after
    // it records the task again before the log ends.
    first
        .turn("add a cache")
        .expect("a turn that delivers the answer");
    let answer = first
        .runner
        .transcript()
        .messages()
        .iter()
        .find_map(|message| match message {
            Message::User { text, .. } if text.contains("SQLite") => Some(text.to_string()),
            _ => None,
        })
        .expect("the answer was delivered");
    drop(first);

    let (picked, replayed) = store.reopened();
    let mut later = compacting(
        vec![saying("continued"), recap("kept context"), saying("after")],
        Tools::new(),
    );
    later.runner.pick_up(picked, replayed);
    later.turn("go on").expect("the turn after the pick-up");
    compacted(&mut later);
    later.turn("finish").expect("the turn after compaction");

    let told = restated(&later.sent).expect("the person's words were restated");
    let mut said = vec!["add a cache"];
    said.extend(answer.lines());
    said.extend(["go on", "finish"]);
    in_order(&told, &said);
    assert!(
        told.contains("only the reply is theirs"),
        "the answer came back without saying whose words it quotes: {told:?}"
    );
}

#[test]
fn an_aside_is_not_an_amendment() {
    let mut scripted = compacting(
        vec![
            saying("first"),
            saying("second"),
            recap("kept context"),
            saying("third"),
        ],
        Tools::new(),
    );
    scripted.aside.say("a note the harness added".into());
    scripted.turn("make the parser streaming").expect("a turn");
    scripted.turn("and add a benchmark").expect("a second turn");
    compacted(&mut scripted);

    scripted.turn("go on").expect("the turn after compaction");

    let told = restated(&scripted.sent).expect("the person's words were restated");
    in_order(
        &told,
        &["make the parser streaming", "and add a benchmark", "go on"],
    );
    assert!(
        !told.contains("a note the harness added"),
        "an aside was restated as the person's: {told:?}"
    );
}

#[test]
fn a_child_run_neither_reads_nor_amends_the_roots_task() {
    let store = Recording::started("a session with a child");
    let mut scripted = Scripted::recording(
        Script::new(vec![
            saying("first"),
            saying("second"),
            recap("kept context"),
            saying("a child's answer"),
            saying("third"),
        ]),
        Tools::new(),
        Verdict::Allow,
        Arc::clone(&store),
    );
    scripted.runner.policy.compaction = Compaction {
        keep_tokens: 1,
        ..Compaction::default()
    };
    scripted
        .turn("make the parser streaming")
        .expect("a root turn");
    scripted
        .turn("and add a benchmark")
        .expect("a second root turn");
    compacted(&mut scripted);
    let recorded = || {
        store
            .context_snapshot()
            .and_then(|snapshot| snapshot.get(TASK).cloned())
    };
    let before = recorded();

    let mut root = scripted.runner.starting(
        &scripted.events,
        &scripted.cancel,
        &scripted.steer,
        &scripted.aside,
    );
    let child = root.child(RunPolicy::default());
    scripted
        .runner
        .turn("a child's task", Box::new([]), &mut scripted.says, &child)
        .awaited()
        .map(ran)
        .expect("a child turn");

    assert_eq!(
        restated(&scripted.sent),
        None,
        "a child run was told the task"
    );
    assert_eq!(recorded(), before, "a child run rewrote the root's task");

    scripted.turn("go on").expect("the root's next turn");
    let told = restated(&scripted.sent).expect("the person's words were restated");
    in_order(
        &told,
        &["make the parser streaming", "and add a benchmark", "go on"],
    );
    assert!(
        !told.contains("a child's task"),
        "a child's prompt amended the root's task: {told:?}"
    );
}
