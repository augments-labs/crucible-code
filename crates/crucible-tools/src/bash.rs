//! Running a shell command.
//!
//! The command runs in the workspace root through `sh -c`, so the model gets
//! pipes and redirection without this file growing a shell of its own. Nothing
//! here confines what runs: a shell can reach anything the user can, which is
//! why every call comes through the permission engine and why the question the
//! user is asked names the command.
//!
//! A rule can be written about a command, so the line has to be read far enough
//! to say what it will run — that is [`command`], which recognises the shapes a
//! rule can honestly cover and refuses the rest. Refusing means being asked.

mod command;
mod output;
mod wrapper;

use std::process::Stdio;
use std::time::Duration;

use crucible_core::{
    Approved, Cancel, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, Workspace,
};

use crate::args::Args;

/// The name the model calls.
const NAME: &str = "bash";

/// How long a command may take when the call does not say, in seconds.
const SECONDS: usize = 120;

/// The longest a call may ask for, in seconds. A command that needs longer
/// than this wants a different tool, not a bigger number.
const CEILING: usize = 600;

/// How often the command is checked on while it runs. Short enough that a
/// cancelled turn stops promptly, long enough to cost nothing.
const TICK: Duration = Duration::from_millis(20);

/// The root `description` is the tool's own; everything below it describes the
/// arguments.
const SCHEMA: &str = r#"{
  "description": "Runs a shell command in the workspace root and returns its output and exit status.",
  "type": "object",
  "properties": {
    "command": {
      "type": "string",
      "description": "The command line to run, as a shell would read it."
    },
    "timeout": {
      "type": "integer",
      "minimum": 1,
      "description": "How many seconds to allow before stopping it. Defaults to 120, and cannot exceed 600."
    }
  },
  "required": ["command"]
}"#;

/// Runs shell commands in the workspace root.
#[derive(Debug)]
pub struct Bash {
    workspace: Workspace,
    cancel: Cancel,
    /// Laid over what crucible itself was started with — see [`Bash::exporting`].
    env: Vec<(Box<str>, Box<str>)>,
}

impl Bash {
    /// Runs in `workspace`, and stops when `cancel` says to.
    #[must_use]
    pub fn new(workspace: Workspace, cancel: Cancel) -> Self {
        Self {
            workspace,
            cancel,
            env: Vec::new(),
        }
    }

    /// Variables every command this tool runs is started with.
    ///
    /// Handed to each child rather than set in this process, which is not a
    /// workaround: writing to the environment is `unsafe` in edition 2024 and
    /// this workspace forbids it, and the narrower thing is the right thing
    /// anyway. The model's commands get these; nothing else does, and a thread
    /// reading the environment while another one writes it cannot happen.
    ///
    /// A name given here wins over the one crucible inherited, because somebody
    /// who wrote `PATH` into a configuration file meant it for the commands
    /// crucible runs.
    #[must_use]
    pub fn exporting<'a>(mut self, vars: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        self.env = vars
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        self
    }
}

impl Tool for Bash {
    fn name(&self) -> &'static str {
        NAME
    }

    fn schema(&self) -> &'static str {
        SCHEMA
    }

    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
        let command = match Args::parse(NAME, args)
            .and_then(|args| args.text("command").map(str::to_owned))
        {
            Ok(line) => command::read(&line),
            // A call this malformed will be refused by `run`, but it still has
            // to be given a sensitivity first — and the safe answer to "what is
            // about to run" when nobody can read it is everything that was
            // sent, reported as unreadable.
            Err(_) => crucible_core::Command::Opaque(args.as_str().into()),
        };

        Sensitivity::SpawnsProcess { command }
    }

    fn run(&self, approved: Approved) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, approved.args())?;
        let command = args.text("command")?;
        let seconds = args.count("timeout", SECONDS)?;

        if seconds > CEILING {
            return Ok(ToolOutput::failed(format!(
                "timeout must be {CEILING} seconds or less"
            )));
        }

        if self.cancel.requested() {
            return Err(ToolError::Cancelled(NAME));
        }

        let child = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(self.workspace.root())
            .envs(
                self.env
                    .iter()
                    .map(|(name, value)| (name.as_ref(), value.as_ref())),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| io("could not start a shell", source))?;

        // `count` already refused anything but a positive number, and the
        // ceiling above bounds it, so the fallback is unreachable arithmetic
        // rather than a decision.
        let allowed = Duration::from_secs(u64::try_from(seconds).unwrap_or(60));

        output::collect(child, allowed, &self.cancel)
    }
}

/// An operating-system failure, named for the model.
fn io(problem: &'static str, source: std::io::Error) -> ToolError {
    ToolError::Io {
        tool: NAME,
        problem: problem.into(),
        source,
    }
}

#[cfg(test)]
mod tests;
