//! What a command comes back as, and what stops one.

use std::ffi::OsString;
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::Command;

use super::output::OUTPUT;
use super::{Bash, Cancel, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, environment};
use crate::sample::{Sample, allowed};

fn bash(sample: &Sample, args: &str) -> Result<ToolOutput, ToolError> {
    let tool = Bash::new(sample.workspace(), Cancel::new());
    tool.run(allowed(&tool, args))
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
    let tool = Bash::new(sample.workspace(), cancel);
    let problem = tool
        .run(allowed(&tool, r#"{"command":"sleep 30"}"#))
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

    let tool = Bash::new(sample.workspace(), cancel);
    let problem = tool
        .run(allowed(&tool, r#"{"command":"touch should-not-exist"}"#))
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
fn a_command_that_floods_its_pipe_reports_everything_that_went() {
    // End to end, because the count comes from two places — what the reader
    // let go while the command ran, and what the cut took out at the end — and
    // the model is owed their sum rather than whichever half one of them saw.
    const FLOOD: usize = 400_000;
    let sample = Sample::new("bash-flood");

    let output = ran(
        &sample,
        &format!(r#"{{"command":"yes 0123456789abcdef | head -c {FLOOD}","timeout":30}}"#),
    );

    assert!(
        output.text().contains(&format!(
            "[{} bytes of output cut from the middle]",
            FLOOD - OUTPUT
        )),
        "{}",
        output.text()
    );
}

#[test]
fn the_sensitivity_carries_what_the_call_will_run() {
    let sample = Sample::new("bash-sensitivity");
    let tool = Bash::new(sample.workspace(), Cancel::new());

    let sensitivity = tool.sensitivity(&ToolArgs::new(r#"{"command":"/usr/bin/git status"}"#));

    assert_eq!(
        sensitivity,
        Sensitivity::SpawnsProcess {
            command: Command::Understood {
                parts: Box::from([Box::from("/usr/bin/git status")]),
            }
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
            command: Command::Opaque("not json at all".into())
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

    let tool =
        Bash::new(sample.workspace(), Cancel::new()).exporting([("CRUCIBLE_TEST_PAGER", "cat")]);
    let args = r#"{"command":"echo $CRUCIBLE_TEST_PAGER"}"#;
    let output = tool.run(allowed(&tool, args)).expect("the command ran");

    assert_eq!(output.text(), "cat");
}

#[test]
fn a_variable_the_tool_was_given_never_reaches_a_debug_line() {
    // This is the one tool that holds configured *values* rather than names,
    // because it is the one that has to hand them to a child — and a
    // `GITHUB_TOKEN` or an `ANTHROPIC_API_KEY` is an ordinary thing to put
    // there. Which name is set is what a reader needs; the value never is.
    let sample = Sample::new("bash-debug");

    let tool = Bash::new(sample.workspace(), Cancel::new()).exporting([("SECRET", "hunter2")]);
    let shown = format!("{tool:?}");

    assert!(shown.contains("SECRET"), "{shown}");
    assert!(!shown.contains("hunter2"), "{shown}");
}

#[test]
fn a_variable_the_tool_was_given_wins_over_the_one_crucible_was_started_with() {
    // The file is the nearer answer: somebody who wrote `PATH` or `RUST_LOG`
    // into their configuration meant it for the commands crucible runs, and
    // inheriting the shell's copy instead would leave it doing nothing.
    //
    // `HOME` is inherited, so the two are a real collision rather than a name
    // only one side ever sets.
    let sample = Sample::new("bash-env-over");

    let tool = Bash::new(sample.workspace(), Cancel::new())
        .exporting([("HOME", "/nowhere-in-particular")]);
    let args = r#"{"command":"echo $HOME"}"#;
    let output = tool.run(allowed(&tool, args)).expect("the command ran");

    assert_eq!(output.text(), "/nowhere-in-particular");
}

#[test]
fn a_variable_crucible_was_started_with_reaches_no_command_unless_the_list_names_it() {
    // Against the environment crucible really has rather than a stand-in for
    // one, because what a child ends up with is settled at the spawn and only a
    // real environment shows that it was cleared there. `cargo` sets several of
    // its own when it runs a test binary, and none of them are things a program
    // needs in order to run.
    let sample = Sample::new("bash-env-cleared");
    let (name, _) = std::env::vars()
        .find(|(name, value)| name.starts_with("CARGO_") && !value.is_empty())
        .expect("cargo sets variables of its own when it runs a test binary");

    let output = ran(&sample, &format!(r#"{{"command":"echo \"[${name}]\""}}"#));

    assert_eq!(output.text(), "[]", "{name} reached the command");
}

#[test]
fn a_key_under_a_name_nothing_could_have_guessed_never_reaches_a_command() {
    // Why the list says what to keep rather than what to drop. `apiKeyEnv`
    // takes a name, so a credential is called whatever the person holding it
    // called it — this one is on no list of the names keys usually have, and
    // what keeps it from the command is that it was never asked for. The
    // command is the one a model would run to find it.
    let sample = Sample::new("bash-env-key");
    let crucibles_own = |name: &str| match name {
        "WORK_KEY" => Some(OsString::from("s3cr3t")),
        _ => std::env::var_os(name),
    };

    let tool = Bash::inheriting(sample.workspace(), Cancel::new(), crucibles_own);
    let args = r#"{"command":"echo \"[$WORK_KEY]\"; env"}"#;
    let output = tool.run(allowed(&tool, args)).expect("the command ran");

    assert!(output.text().starts_with("[]"), "{}", output.text());
    assert!(!output.text().contains("s3cr3t"), "{}", output.text());
}

#[cfg(not(windows))]
#[test]
fn the_path_crucible_was_started_with_reaches_the_command() {
    // What everything else here stands on. The clear takes `PATH` with the
    // rest, and a command with no `PATH` has nothing to find a program with —
    // the shell that reads the line included.
    //
    // Unix only, because this compares the text. The shell on Windows is an
    // MSYS one, which rewrites a Windows `PATH` into POSIX form before a
    // command reads it, so the same list comes back spelled differently. What
    // stands in for this there is the rest of the file: `cat`, `sleep`, `yes`
    // and `head` are programs rather than builtins, and none of them is found
    // without a `PATH`.
    let sample = Sample::new("bash-env-path");
    let inherited = std::env::var("PATH").expect("crucible was started with a PATH");

    let output = ran(&sample, r#"{"command":"echo \"$PATH\""}"#);

    assert_eq!(output.text(), inherited);
}

#[test]
fn a_name_crucible_has_no_value_for_is_left_unset_rather_than_given_one() {
    // `PATH` included. A shell started without one falls back to the path POSIX
    // makes it carry; a default written into this crate instead would be this
    // crate deciding where a command's programs come from.
    assert!(environment::inherited(|_| None).is_empty());
}

#[cfg(unix)]
#[test]
fn the_shell_is_not_something_the_workspace_can_supply() {
    // An empty element on the PATH means the current directory to whatever
    // resolves a bare name, and the current directory of every command this
    // tool runs is the workspace. So a file the model wrote called `sh` would
    // be the shell that reads every command line after it — including the ones
    // a user was asked about and allowed.
    use std::os::unix::fs::PermissionsExt as _;

    let sample = Sample::new("bash-shell");
    sample.write("sh", "#!/bin/sh\necho owned\n");
    std::fs::set_permissions(
        sample.root().join("sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("a writable temporary directory");

    let inherited = std::env::var("PATH").expect("crucible was started with a PATH");
    let empty_element_first = move |name: &str| match name {
        "PATH" => Some(OsString::from(format!(":{inherited}"))),
        other => std::env::var_os(other),
    };

    let tool = Bash::inheriting(sample.workspace(), Cancel::new(), empty_element_first);
    let output = tool
        .run(allowed(&tool, r#"{"command":"echo hello"}"#))
        .expect("the command ran");

    assert_eq!(output.text(), "hello");
}
