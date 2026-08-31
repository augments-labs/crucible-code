//! Behaviour the execution-kernel extraction is not allowed to change.
//!
//! Deliberately about the parts the inherited suite left to inference: where a
//! steered line lands in the transcript, whether the tool-output boundary is
//! one budget for a whole turn or one per pass, and that a pass writes an
//! answer for every call it recorded however it ended. Each names one
//! divergence the loop could take and asserts the observable that divergence
//! would change, which is why they are here rather than left to the tests that
//! happen to cross the same lines.

use super::*;

/// The shape of a transcript, one word per message, in order.
fn shape(transcript: &Transcript) -> Vec<&'static str> {
    transcript
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::Context(_) => None,
            Message::User { .. } => Some("user"),
            Message::Agent { .. } => Some("agent"),
            Message::ToolResults(_) => Some("results"),
        })
        .collect()
}

#[test]
fn a_steered_line_is_recorded_at_the_top_of_the_pass_it_joins() {
    // Not merely "the next request is longer". The line is the reader's own
    // words about what the agent should do next, so it has to precede the
    // answer it is meant to change — a turn that recorded it after the pass
    // would be showing the model advice about work it had already done.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut steering = Steering::new(script, tools([Fixed::new("read")]));
    steering.steer.say("actually do this".into());

    steering.turn("first").expect("a turn");

    assert_eq!(
        shape(steering.runner.transcript()),
        ["user", "user", "agent", "results", "agent"],
    );
    assert!(!steering.steer.any(), "the queue was not drained");
}

#[test]
fn the_tool_output_boundary_is_one_budget_for_the_whole_turn() {
    // Two passes, each comfortably inside the ceiling on its own, together
    // over it. A boundary counted per pass rather than per turn would let a
    // model that produces a little output for ever go on producing it.
    let script = Script::new(vec![
        calling("a", "read", "{}"),
        calling("b", "read", "{}"),
        saying("done"),
    ]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("read").answering("fivee")]),
        Verdict::Allow,
    );

    let run = RunContext::new(
        holding(8),
        &scripted.events,
        &scripted.cancel,
        &scripted.steer,
        &scripted.aside,
    );
    let problem = scripted
        .runner
        .exchange(&mut scripted.says, &run)
        .unwrap_err();

    assert!(
        matches!(problem, TurnError::ToolOutputBytes { maximum: 8 }),
        "the second pass was not held against the first: {problem:?}"
    );
}

#[test]
fn a_line_typed_while_a_call_is_out_lands_after_the_answer_it_waited_for() {
    // The queue is drained at the top of a pass, never in the middle of one.
    // A line that landed between the call and its results would put the
    // reader's words inside an exchange the provider reads as one unit, and a
    // replay would carry a prompt where an answer belongs.
    let script = Script::new(vec![calling("a", "type", "{}"), saying("done")]);
    let steer = Steer::new();
    let mut offered = Tools::new();
    offered
        .add_builtin(Typing::new("type", steer.clone(), "actually do this"))
        .unwrap();
    let mut steering = Steering::steered(steer, script, offered);

    steering.turn("first").expect("a turn");

    assert_eq!(
        shape(steering.runner.transcript()),
        ["user", "agent", "results", "user", "agent"],
    );
    assert!(!steering.steer.any(), "the queue was not drained");
}

#[test]
fn a_line_typed_while_the_answer_arrives_still_waits_for_the_call_it_interrupted() {
    // The same invariant against the other moment a line can appear inside a
    // pass. Above, the tool types, so the queue is empty until after the call
    // was recorded; here the line is already waiting at that point, which is
    // what a reader typing while the model answers actually produces. A drain
    // added between the recorded call and the pass that answers it changes
    // nothing in the test above and puts the reader's words inside an exchange
    // here.
    let steer = Steer::new();
    let script = Script::typing(
        steer.clone(),
        "actually do this",
        vec![calling("a", "read", "{}"), saying("done")],
    );
    let mut steering = Steering::steered(steer, script, tools([Fixed::new("read")]));

    steering.turn("first").expect("a turn");

    assert_eq!(
        shape(steering.runner.transcript()),
        ["user", "agent", "results", "user", "agent"],
    );
    assert!(!steering.steer.any(), "the queue was not drained");
}

#[test]
fn a_turn_stopped_in_a_tool_pass_still_records_what_its_calls_answered() {
    // A provider refuses a transcript holding a request with no answer, so the
    // results are written whatever ended the pass. A turn that returned the
    // stop before recording them would leave a session that cannot be resumed
    // and a log a replay has to repair.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("never asked")]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("read").cancelling()]),
        Verdict::Allow,
    );

    let stop = scripted
        .turn("go")
        .expect("a turn that ended rather than failed");

    assert_eq!(stop, StopReason::Cancelled);
    assert_eq!(
        shape(scripted.runner.transcript()),
        ["user", "agent", "results"],
    );
}
