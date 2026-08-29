//! Behaviour the execution-kernel extraction is not allowed to change.
//!
//! Written before the loop was moved, and deliberately about the parts the
//! inherited suite left to inference: where a steered line lands in the
//! transcript, and that the tool-output boundary is one budget for a whole
//! turn rather than one per pass. Both survived a divergence the rest of the
//! suite did not notice, which is why they are here.

use super::*;

/// The shape of a transcript, one word per message, in order.
fn shape(transcript: &Transcript) -> Vec<&'static str> {
    transcript
        .messages()
        .iter()
        .map(|message| match message {
            Message::User { .. } => "user",
            Message::Agent { .. } => "agent",
            Message::ToolResults(_) => "results",
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

    let problem = scripted
        .runner
        .exchange(
            &mut scripted.says,
            &scripted.events,
            &scripted.cancel,
            &scripted.steer,
            &scripted.aside,
            8,
        )
        .unwrap_err();

    assert!(
        matches!(problem, TurnError::ToolOutputBytes { maximum: 8 }),
        "the second pass was not held against the first: {problem:?}"
    );
}
