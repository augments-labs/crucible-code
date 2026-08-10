//! When a round reaches the log, measured against when its tools run.
//!
//! Recording is queued rather than written, so what a test can see from inside
//! a running tool is the log as the disk has it — which is the only place the
//! ordering shows. A tool that reads the log while the round is still going is
//! how that becomes an observation rather than a claim about the source.

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{Approved, Sensitivity, Target, Tool, ToolArgs, ToolError, ToolOutput};

use super::*;

/// The tool whose call the log is watched for.
///
/// A word that appears nowhere else in the turn, so finding it in the log can
/// only mean the call was recorded.
const WATCH: &str = "watch";

/// How long the tool waits for what was queued to reach the disk.
///
/// The write happens on the session's own thread, so a log that has not caught
/// up yet is slow rather than wrong. Long enough that a loaded machine does not
/// report the delay as a record that was never made, and bounded so that a
/// record which is never made fails instead of hanging.
const SETTLE: Duration = Duration::from_secs(5);

#[test]
fn the_calls_of_a_round_are_recorded_before_the_tools_run() {
    // Running a tool is what changes the tree. A turn that ends part way
    // through a round — killed, or out of power — leaves a log whose last word
    // is the prompt, and the next `--continue` hands the model a transcript in
    // which files it has already edited have never been touched. Recording the
    // calls first costs a line the replay knows how to drop; recording them
    // last costs the work.
    let sample = Sample::new("runner-recorded");
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let log = session.path().to_owned();

    let mut offered = Tools::new();
    offered.add(Box::new(Watching { log }));

    let script = Script::new(vec![calling("a", WATCH, "{}"), saying("done")]);
    let mut scripted = Scripted::recording(script, offered, Verdict::Allow, session);

    scripted.turn("go").expect("the turn");

    let messages = scripted.runner.transcript().messages();
    let seen = match messages.get(2) {
        Some(Message::ToolResults(results)) => results
            .first()
            .map(|result| result.output.text().to_owned()),
        _ => None,
    }
    .expect("the tool ran and its result was recorded");

    assert!(
        seen.contains(WATCH),
        "the round was still unrecorded while its tool ran: {seen}"
    );
}

/// A tool that hands back the session log as it stood while the tool ran.
struct Watching {
    log: PathBuf,
}

impl Tool for Watching {
    fn name(&self) -> &'static str {
        WATCH
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn run(&self, _approved: Approved) -> Result<ToolOutput, ToolError> {
        let deadline = Instant::now() + SETTLE;

        loop {
            let held = std::fs::read_to_string(&self.log).unwrap_or_default();

            if held.contains(WATCH) || Instant::now() >= deadline {
                return Ok(ToolOutput::ok(held));
            }

            thread::sleep(Duration::from_millis(1));
        }
    }
}
