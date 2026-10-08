//! Which plan a resumed session stands over the prompt: the last one a
//! `todo_write` put down, not the last one the model asked for.

use crucible_types::{RecordedToolOutput, ToolArgs, ToolId, ToolResult};

use super::*;

/// A `todo_write` call naming one task per line of `tasks`, each open.
fn planning(id: &str, tasks: &[&str]) -> ToolCall {
    let tasks = tasks
        .iter()
        .map(|task| format!(r#"{{"task":"{task}","state":"open"}}"#))
        .collect::<Vec<_>>()
        .join(",");

    ToolCall {
        id: ToolId::new(id),
        name: PLANNING.into(),
        args: ToolArgs::new(format!(r#"{{"tasks":[{tasks}]}}"#)),
    }
}

fn asked(calls: Vec<ToolCall>) -> Message {
    Message::Agent {
        text: "".into(),
        calls,
        stop: None,
        continuation: None,
    }
}

fn answered(results: Vec<(&str, RecordedToolOutput)>) -> Message {
    Message::ToolResults(
        results
            .into_iter()
            .map(|(id, output)| ToolResult {
                id: ToolId::new(id),
                output,
            })
            .collect(),
    )
}

fn transcript(messages: Vec<Message>) -> Transcript {
    let mut transcript = Transcript::new();
    for message in messages {
        transcript.push(message).expect("no continuation to hold");
    }
    transcript
}

fn restored(transcript: &Transcript) -> Vec<String> {
    let plan = Plan::new();
    planned(&plan, transcript);
    plan.tasks()
        .iter()
        .map(|task| task.said().to_owned())
        .collect()
}

/// A command the user declined ends the pass, and the `todo_write` asked for
/// after it is answered as never run: the plan it carried was never written,
/// so the one a resume stands up is the plan before it.
#[test]
fn restores_the_plan_before_one_answered_as_not_run() {
    let shell = ToolCall {
        id: ToolId::new("shell"),
        name: "bash".into(),
        args: ToolArgs::new(r#"{"command":"make"}"#),
    };
    let session = transcript(vec![
        Message::said("plan it"),
        asked(vec![planning("first", &["plan A"])]),
        answered(vec![("first", RecordedToolOutput::ok("1 task"))]),
        Message::said("go on"),
        asked(vec![shell, planning("second", &["plan B"])]),
        answered(vec![
            (
                "shell",
                RecordedToolOutput::failed("the user did not allow this"),
            ),
            (
                "second",
                RecordedToolOutput::failed("not run: the turn ended first"),
            ),
        ]),
    ]);

    assert_eq!(restored(&session), ["plan A"]);
}

/// A `todo_write` the tool refused for its bounds put nothing down, so the
/// plan before it is still the one in force.
#[test]
fn restores_the_plan_before_one_refused_for_its_bounds() {
    let many = (0..65).map(|at| format!("task {at}")).collect::<Vec<_>>();
    let many = many.iter().map(String::as_str).collect::<Vec<_>>();
    let session = transcript(vec![
        Message::said("plan it"),
        asked(vec![planning("first", &["plan A"])]),
        answered(vec![("first", RecordedToolOutput::ok("1 task"))]),
        asked(vec![planning("second", &many)]),
        answered(vec![(
            "second",
            RecordedToolOutput::failed("todo_write: at most 64 tasks"),
        )]),
    ]);

    assert_eq!(restored(&session), ["plan A"]);
}

/// A call with no result paired to it, as a log cut off mid-pass leaves one,
/// never took effect either.
#[test]
fn restores_the_plan_before_one_with_no_result() {
    let session = transcript(vec![
        asked(vec![planning("first", &["plan A"])]),
        answered(vec![("first", RecordedToolOutput::ok("1 task"))]),
        asked(vec![planning("second", &["plan B"])]),
    ]);

    assert_eq!(restored(&session), ["plan A"]);
}

/// The last plan that took effect is still the one stood up when it is also
/// the last asked for, and a session with none that took effect has no plan.
#[test]
fn restores_the_last_written_plan_and_nothing_where_none_was_written() {
    let written = transcript(vec![
        asked(vec![planning("first", &["plan A"])]),
        answered(vec![("first", RecordedToolOutput::ok("1 task"))]),
        asked(vec![planning("second", &["plan B"])]),
        answered(vec![("second", RecordedToolOutput::ok("1 task"))]),
    ]);
    let refused = transcript(vec![
        asked(vec![planning("first", &["plan A"])]),
        answered(vec![(
            "first",
            RecordedToolOutput::failed("the user did not allow this"),
        )]),
    ]);

    assert_eq!(restored(&written), ["plan B"]);
    assert!(restored(&refused).is_empty());
}
