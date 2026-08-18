//! What a command comes back as, and what stops one.

use std::ffi::OsString;
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    Ask, Command, Mode, Permission, Remember, Rules, ToolCall, ToolId, Unwatched, Verdict,
};

use super::{Bash, Cancel, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, environment};
use crate::bound::OUTPUT;
use crate::sample::{Sample, allowed};

fn bash(sample: &Sample, args: &str) -> Result<ToolOutput, ToolError> {
    let tool = Bash::new(sample.workspace(), Cancel::new());
    tool.run(allowed(&tool, args), &Unwatched)
}

fn ran(sample: &Sample, args: &str) -> ToolOutput {
    bash(sample, args).expect("the command ran")
}

/// A watcher that keeps what it was told, in the order it was told.
#[derive(Default)]
struct Watched(std::sync::Mutex<String>);

impl crucible_core::Watch for Watched {
    fn wrote(&self, text: crucible_core::Wrote) {
        if let Ok(mut held) = self.0.lock() {
            held.push_str(text.as_str());
        }
    }
}

impl Watched {
    fn said(&self) -> String {
        self.0.lock().map(|held| held.clone()).unwrap_or_default()
    }
}

#[test]
fn what_a_command_prints_is_handed_over_while_it_is_still_running() {
    // The command outlives its own output on purpose: what is handed over is
    // handed over *during* the wait, and a command that prints and exits inside
    // one tick has nothing to watch. That is not a gap — a result nobody had to
    // wait for is a result, and this is the surface for the other kind.
    let sample = Sample::new("bash-watched");
    let tool = Bash::new(sample.workspace(), Cancel::new());
    let watched = Watched::default();

    let args = r#"{"command":"printf 'Compiling one\nCompiling two\n'; sleep 1"}"#;
    let output = tool
        .run(allowed(&tool, args), &watched)
        .expect("the command ran");

    assert_eq!(
        watched.said(),
        "Compiling one\nCompiling two\n",
        "what the command printed never reached the watcher"
    );
    // And the result is untouched by having been watched.
    assert_eq!(output.text(), "Compiling one\nCompiling two");
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
fn what_a_stopped_command_left_running_is_stopped_with_it() {
    // A backgrounded process is disowned by the shell and reparented, so it
    // used to outlive the command and go on holding the pipe — which is why the
    // report has a note for output that is still arriving. It is in the group
    // the command was killed with, so what the model gets here is the timeout
    // and nothing else, and the machine is left with nothing running.
    //
    // What the note is still for is a process that left that group of its own
    // accord, and `output::tests` pins how that reads.
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
    assert!(
        !output.text().contains("still holds the output open"),
        "{}",
        output.text()
    );
}

#[test]
fn a_command_that_leaves_something_running_still_comes_back() {
    // A background process inherits the pipe. The process scope stops it when
    // the shell exits, so neither waiting for EOF nor cleaning it up can hang.
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
fn a_contained_background_command_leaves_no_missing_output() {
    // Once the shell exits, its scope ends the background holder and the
    // pollable readers reach EOF. A note claiming more output is arriving would
    // now be false, and would hide a reader that failed to join.
    let sample = Sample::new("bash-arriving");

    let output = ran(&sample, r#"{"command":"(sleep 30 &) ; echo started"}"#);

    assert!(output.text().contains("started"), "{}", output.text());
    assert!(
        !output.text().contains("still arriving"),
        "{}",
        output.text()
    );
}

#[test]
fn repeated_silent_background_commands_leave_no_readers_or_processes() {
    let sample = Sample::new("bash-background-repeated");
    let started = Instant::now();

    for _ in 0..20 {
        let output = ran(&sample, r#"{"command":"(sleep 30 &) ; true"}"#);
        assert_eq!(output.text(), "(no output)");
    }

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "blocked output readers accumulated across calls"
    );
}

#[test]
fn a_background_descendant_cannot_act_after_its_shell_returns() {
    let sample = Sample::new("bash-background-marker");

    let output = ran(
        &sample,
        r#"{"command":"(sleep 2; touch outlived) & echo done"}"#,
    );

    assert_eq!(output.text(), "done");
    thread::sleep(Duration::from_secs(3));
    assert!(
        !sample.root().join("outlived").exists(),
        "a background descendant escaped the command scope"
    );
}

#[test]
fn a_pipeline_the_command_started_is_stopped_with_it() {
    // `kill` reaches the shell alone. Every other member of a pipeline is a
    // child of it, so they reparent and go on running after the tool has
    // returned — `yes > /dev/null | cat` burns a core until the session ends.
    // What is signalled is the process group, and the marker is what proves it:
    // written by a member of the pipeline after the command was stopped, it can
    // only exist if that member outlived the kill.
    let sample = Sample::new("bash-pipeline");

    let output = ran(
        &sample,
        r#"{"command":"(sleep 2; touch outlived) | cat","timeout":1}"#,
    );

    assert!(output.is_failed(), "{}", output.text());
    thread::sleep(Duration::from_secs(3));
    assert!(
        !sample.root().join("outlived").exists(),
        "a member of the pipeline outlived the command"
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
        .run(allowed(&tool, r#"{"command":"sleep 30"}"#), &Unwatched)
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
        .run(
            allowed(&tool, r#"{"command":"touch should-not-exist"}"#),
            &Unwatched,
        )
        .expect_err("the turn was stopped");

    assert!(matches!(problem, ToolError::Cancelled("bash")));
    assert!(!sample.root().join("should-not-exist").exists());
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
        .run(allowed(&tool, r#"{"command":"echo hello"}"#), &Unwatched)
        .expect("the command ran");

    assert_eq!(output.text(), "hello");
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
                sent: "/usr/bin/git status".into(),
                parts: Box::from([Box::from("/usr/bin/git status")]),
            }
        }
    );
}

/// Whether a mode puts this command line to the user, decided the way a turn
/// decides one.
///
/// Over the sensitivity this tool worked out rather than one written down here,
/// so what the tests below pin is the whole route a call takes: the words of
/// the line, the sensitivity handed over, and the arm the mode answers with.
fn asks(sample: &Sample, mode: Mode, line: &str) -> bool {
    struct Watching(bool);

    impl Ask for Watching {
        fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
            self.0 = true;
            (Verdict::Allow, Remember::Never)
        }
    }

    let tool = Bash::new(sample.workspace(), Cancel::new());
    let call = ToolCall {
        id: ToolId::new("one"),
        name: tool.name().into(),
        args: ToolArgs::new(format!(r#"{{"command":"{line}"}}"#)),
    };

    let mut watching = Watching(false);
    Permission::with(mode, Rules::new()).decide(
        &call,
        &tool.sensitivity(&call.args),
        &mut watching,
    );

    watching.0
}

/// The lines a mode is asked about here: ones naming nothing but paths that
/// exist inside the workspace, and ones nobody would call an edit.
///
/// The first group is the point. Each of them changes what `write` may change
/// and names it in words a reader could check, and each is still put to the
/// user — because `sh` looks those words up again when the command runs, and
/// what it finds then is not what a check on the text found.
const LINES: [&str; 9] = [
    "mkdir demo",
    "touch src/b.rs",
    "cp src/a.rs src/b.rs",
    "mv src/a.rs src/b.rs",
    "rm src/a.rs",
    "ls src",
    "cargo test",
    "rm -rf build",
    "mkdir demo; touch src/b.rs",
];

#[test]
fn allow_edits_asks_before_every_command_line() {
    // What the mode's name says, with nothing behind it about what a line was
    // read to be. A shell is what `allowEdits` stops at, whatever it was given
    // to run.
    let sample = Sample::new("bash-allow-edits");
    sample.write("src/a.rs", "");

    for line in LINES {
        assert!(asks(&sample, Mode::AllowEdits, line), "{line}");
    }
}

#[test]
fn ask_asks_before_the_same_ones() {
    // `allowEdits` and `ask` answer a command identically, which is the whole
    // of the difference between them being about files.
    let sample = Sample::new("bash-ask");
    sample.write("src/a.rs", "");

    for line in LINES {
        assert!(asks(&sample, Mode::Ask, line), "{line}");
    }
}

#[test]
fn full_access_asks_before_none_of_them() {
    // The same lines under the one mode that runs a command unasked, so the
    // two tests above are pinning the mode rather than something the tool or
    // the engine would have done to every call anyway.
    let sample = Sample::new("bash-full-access");
    sample.write("src/a.rs", "");

    for line in LINES {
        assert!(!asks(&sample, Mode::FullAccess, line), "{line}");
    }
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
    let output = tool
        .run(allowed(&tool, args), &Unwatched)
        .expect("the command ran");

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
    let output = tool
        .run(allowed(&tool, args), &Unwatched)
        .expect("the command ran");

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
    let output = tool
        .run(allowed(&tool, args), &Unwatched)
        .expect("the command ran");

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
