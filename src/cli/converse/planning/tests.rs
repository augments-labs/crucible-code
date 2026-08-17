use crucible_core::ToolArgs;

use super::*;

/// A `todo_write` call as the model writes one.
///
/// Built as text rather than from a type, because text is what the tool parses
/// and this side of the session is not allowed to know any more about the call
/// than the tool does.
fn wrote(tasks: &[(&str, &str)]) -> ToolArgs {
    let said = tasks
        .iter()
        .map(|(task, state)| format!(r#"{{"task":"{task}","state":"{state}"}}"#))
        .collect::<Vec<_>>()
        .join(",");

    ToolArgs::new(format!(r#"{{"tasks":[{said}]}}"#))
}

/// A plan of `count` open tasks, each named after where it is in the list.
fn several(count: usize) -> ToolArgs {
    let said = (0..count)
        .map(|at| format!(r#"{{"task":"Task {at}","state":"open"}}"#))
        .collect::<Vec<_>>()
        .join(",");

    ToolArgs::new(format!(r#"{{"tasks":[{said}]}}"#))
}

/// Whether any row says it.
fn says(rows: &[Row], said: &str) -> bool {
    rows.iter().any(|row| row.text().contains(said))
}

#[test]
fn a_plan_the_tool_wrote_is_read_on_the_next_look_and_not_again() {
    let plan = crucible_tools::Plan::new();
    let mut planning = Planning::new(plan.clone());

    assert!(!planning.moved());

    plan.replay(&wrote(&[("Build the gate script", "doing")]));
    assert!(planning.moved());

    // The second look is the frame that must not happen: the row above the box
    // redraws four times a second, and a plan that answered *moved* every time
    // would lay the box out again on every one of them.
    assert!(!planning.moved());
}

#[test]
fn a_session_picked_up_opens_with_the_plan_it_stopped_at() {
    let plan = crucible_tools::Plan::new();
    plan.replay(&wrote(&[("Build the gate script", "doing")]));

    // Made after the write, the way the wiring makes it: the transcript is
    // replayed into the plan before the loop that reads it exists.
    let planning = Planning::new(plan);

    assert!(says(
        &planning.rows(40, 12, Glyphs::Unicode),
        "Build the gate script"
    ));
}

#[test]
fn a_plan_nobody_has_written_draws_nothing_at_all() {
    let planning = Planning::new(crucible_tools::Plan::new());

    // Not a rule, a blank and a heading over an empty list: an agent that has
    // not started is drawn as a prompt with nothing above it.
    assert!(planning.rows(40, 12, Glyphs::Unicode).is_empty());
}

#[test]
fn emptying_the_plan_is_a_move_like_any_other() {
    let plan = crucible_tools::Plan::new();
    plan.replay(&wrote(&[("Build the gate script", "doing")]));

    let mut planning = Planning::new(plan.clone());

    // What `/clear` runs. The panel has to come down on the frame after it, or
    // the session that starts empty opens under the last one's plan.
    plan.forget();

    assert!(planning.moved());
    assert!(planning.rows(40, 12, Glyphs::Unicode).is_empty());
}

#[test]
fn the_key_opens_the_whole_plan_and_bounds_it_again() {
    let plan = crucible_tools::Plan::new();
    plan.replay(&several(9));

    let mut planning = Planning::new(plan);
    let bounded = planning.rows(40, 40, Glyphs::Unicode);

    assert!(planning.expand());
    let whole = planning.rows(40, 40, Glyphs::Unicode);

    assert!(!says(&bounded, "Task 8"));
    assert!(says(&whole, "Task 8"));

    assert!(planning.expand());
    assert_eq!(planning.rows(40, 40, Glyphs::Unicode).len(), bounded.len());
}

#[test]
fn the_key_pressed_against_a_session_with_no_plan_costs_no_frame() {
    let mut planning = Planning::new(crucible_tools::Plan::new());

    // What the answer is for. The loop above redraws on it, and a key that
    // toggled a setting nobody can see would lay the box out again for a screen
    // nothing on it had changed — the same waste an arrow held down against the
    // end of a line would be.
    assert!(!planning.expand());
    assert!(planning.rows(40, 12, Glyphs::Unicode).is_empty());
}

#[test]
fn every_state_the_tool_writes_has_a_word_the_panel_draws() {
    let plan = crucible_tools::Plan::new();
    plan.replay(&wrote(&[
        ("Design the architecture", "open"),
        ("Run the validation spikes", "doing"),
        ("Build the gate script", "done"),
    ]));

    let planning = Planning::new(plan);
    let rows = planning.rows(60, 20, Glyphs::Unicode);

    // The counts line, which is drawn from the three states together: a state
    // paired with the wrong one on the way across would show up here as a
    // figure against the wrong word.
    assert!(says(&rows, "1 done"));
    assert!(says(&rows, "1 doing"));
    assert!(says(&rows, "1 open"));
}
