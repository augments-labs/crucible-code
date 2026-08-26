//! Running a shell command.
//!
//! The command runs in the workspace root through `sh -c`, so the model gets
//! pipes and redirection without this file growing a shell of its own. Nothing
//! here confines what runs: a shell can reach anything the user can, which is
//! why every call comes through the permission engine and why the question the
//! user is asked names the command.
//!
//! What a command is *started with* is confined. crucible's own environment
//! holds the provider credential, so a child is given a chosen set of variables
//! rather than an inherited copy of that one — [`environment`] says which, and
//! why an allowlist is the only shape that can work.
//!
//! A rule can be written about a command, so the line has to be read far enough
//! to say what it will run — that is [`command`], which recognises the shapes a
//! rule can honestly cover and refuses the rest. Refusing means being asked.
//!
//! Reading it that far says what will run and not where it will land, and no
//! amount of reading could say the second. Whatever a word in the line was
//! found to point at, the shell looks it up again by name when the command
//! runs, so a symbolic link put there in between sends the write somewhere
//! else and nobody was asked. The file tools have no such gap — they keep hold
//! of the directory they proved — and `sh` cannot be made to work that way, so
//! the mode that runs a command without a question is `fullAccess` and there
//! is no other.

mod background;
mod command;
mod environment;
mod output;
mod platform;
mod shell;
mod wrapper;

use std::ffi::OsString;
use std::process::Stdio;
use std::time::Duration;

pub use background::{Background, Ended, MOST, Standing};
use crucible_core::{
    Approved, Cancel, Sensitivity, Summary, Tool, ToolArgs, ToolError, ToolOutput, Watch, Workspace,
};

use std::sync::LazyLock;

use crate::args::Args;
use crate::schema::{Field, Schema, Shape, Whole};
use crate::summary;

/// The name the model calls.
const NAME: &str = "bash";

/// The command line to run.
const COMMAND: &str = "command";

/// How long to allow it.
const TIMEOUT: &str = "timeout";

/// How long a command may take when the call does not say, in seconds.
const SECONDS: usize = 120;

/// The longest a call may ask for, in seconds. A command that needs longer
/// than this wants a different tool, not a bigger number.
const CEILING: usize = 600;

/// How long a command asked to be left running is watched for before the call
/// answers, unless [`Bash::watching`] was told otherwise.
///
/// Long enough for `npm: command not found` to be a failure the model is told
/// about now, and short enough that starting a dev server does not read as a
/// pause. A command still going after this is the case the argument was sent for.
const FIRST: Duration = Duration::from_millis(200);

/// How often the command is checked on while it runs. Short enough that a
/// cancelled turn stops promptly, long enough to cost nothing.
const TICK: Duration = Duration::from_millis(20);

/// What the model is told after a command is handed to the background registry.
///
/// The registry owns watching it from that point on. Without this sentence, a
/// model sees only that the call returned while work continues and may spend the
/// next step inventing a way to poll it — duplicating the watcher already here.
const LEFT_RUNNING: &str = "completion is reported automatically; do not poll or wait";

/// The root `description` is the tool's own; everything below it describes the
/// arguments.
///
/// Two of those arguments are not for this tool. The account fields declared
/// last are never read here — they are drawn on the panel where somebody
/// decides whether this call may run, and the reason they arrive with the call
/// rather than being asked for when the panel opens is that the thread holding
/// the terminal has no provider to ask. So they are declared here, at the one
/// place a model is told what it may send, and read a layer up by
/// [`crate::account`].
///
/// Neither is required, and that is the whole of what keeps them optional in
/// practice too: a call that says nothing about itself gets the panel it would
/// have got before either existed, rather than a panel with a blank where an
/// account of the command should be.
static SCHEMA: LazyLock<String> = LazyLock::new(|| {
    let mut fields = vec![
        Field {
            name: COMMAND,
            about: "The command line to run, as a shell would read it.".into(),
            needed: true,
            shape: Shape::Text,
        },
        Field {
            name: TIMEOUT,
            about: format!(
                "How many seconds to allow before stopping it. Defaults to {SECONDS}, and cannot \
                 exceed {CEILING}. Cannot be sent with background."
            ),
            needed: false,
            shape: Shape::Count(Whole {
                least: 1,
                most: Some(CEILING),
            }),
        },
        Field {
            name: crate::account::LEFT,
            about: "Leave the command running and answer at once, for something with no end of \
                    its own: a dev server, a file watcher, a tunnel. The answer names the number \
                    it is running as and carries whatever it printed in its first moment. A \
                    command that has already exited by then is reported as an ordinary result \
                    instead, so a failure still reaches you now. At most four may run at once."
                .into(),
            needed: false,
            shape: Shape::Flag,
        },
    ];
    fields.extend(crate::account::fields(
        "command",
        "What the command does",
        "Where the line runs more than one command, account for each of them.",
    ));
    Schema {
        about: "Runs a shell command in the workspace root and returns its output and exit \
                status."
            .into(),
        fields,
    }
    .text()
});

/// Runs shell commands in the workspace root.
pub struct Bash {
    workspace: Workspace,
    cancel: Cancel,
    /// Where a command that is left running goes. Empty in a run with nothing
    /// holding the other end — a test, and a probe — and then a call asking to be
    /// left running is refused rather than silently waited for.
    leaving: Option<Background>,
    /// Where the shell is, resolved once and absolute — `None` on a machine
    /// that has none, which is a failure the first call reports.
    shell: Option<std::path::PathBuf>,
    /// The whole of what a command is started with: the names [`environment`]
    /// inherits, with the `env` block laid over them by [`Bash::exporting`].
    env: Vec<(Box<str>, OsString)>,
    /// How long a command asked to be left running is watched before the call
    /// answers. [`FIRST`] unless [`Bash::watching`] says otherwise.
    first: Duration,
}

impl std::fmt::Debug for Bash {
    /// Hand-written because `env` holds configured *values*, and this tool holds
    /// them because it is the one that has to hand them to a child.
    /// `ANTHROPIC_API_KEY`, `GITHUB_TOKEN` and `NPM_TOKEN` are ordinary entries
    /// there. A derive would put every one of them wherever a `{:?}` reaches —
    /// a log line, an assertion message, a panic payload — so the redaction is
    /// the type's, not a rule about who may print it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bash")
            .field("workspace", &self.workspace)
            .field("cancel", &self.cancel)
            .field("leaving", &self.leaving)
            .field("shell", &self.shell)
            .field("env", &Exported(&self.env))
            .field("first", &self.first)
            .finish()
    }
}

/// The exported variables as `Debug` may show them: every name, and a marker
/// where each value would be. Which name is set is what somebody reading this
/// is looking for, and it is all they need.
struct Exported<'a>(&'a [(Box<str>, OsString)]);

impl std::fmt::Debug for Exported<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.0.iter().map(|(name, _)| (name, "<redacted>")))
            .finish()
    }
}

impl Bash {
    /// Runs in `workspace`, and stops when `cancel` says to.
    #[must_use]
    pub fn new(workspace: Workspace, cancel: Cancel) -> Self {
        Self::inheriting(workspace, cancel, |name| std::env::var_os(name))
    }

    /// The same, reading crucible's own environment through `lookup`.
    ///
    /// Read once, here, rather than at every spawn: nothing in this process
    /// writes to its environment — that is `unsafe` in edition 2024 and denied
    /// in this workspace — so the answer cannot have changed by the time a
    /// command runs, and the spawn path is left with nothing to ask.
    ///
    /// The shell is found the same way and for a second reason: a bare name is
    /// resolved wherever it is spawned, and a command here is spawned in the
    /// workspace. [`shell`] says what that costs.
    fn inheriting(
        workspace: Workspace,
        cancel: Cancel,
        lookup: impl Fn(&str) -> Option<OsString>,
    ) -> Self {
        Self {
            workspace,
            cancel,
            leaving: None,
            shell: shell::find(&lookup),
            env: environment::inherited(lookup),
            first: FIRST,
        }
    }

    /// Where commands this tool is asked to leave running are kept.
    ///
    /// Handed in rather than made here, because the binary is what ends them on
    /// the way out and what draws the row saying how many there are — the same
    /// shape the read record has, and for the same reason.
    #[must_use]
    pub fn leaving(mut self, left: Background) -> Self {
        self.leaving = Some(left);
        self
    }

    /// How long a command asked to be left running is watched before the call
    /// answers, in place of `FIRST`.
    ///
    /// The default is the one a reader waits through, and it is a judgement
    /// about them rather than about any machine: long enough for a command that
    /// cannot start to say so now, short enough that a dev server does not read
    /// as a pause. What that judgement cannot do is decide when a shell this
    /// process spawned actually gets to run — a host busy with other work can
    /// take longer to start `sh` than the whole window allows, and then a
    /// command already over reads as one still going.
    ///
    /// So the window is an argument for anything that has to say which of those
    /// two happened. Nothing shipped calls this; the wiring takes the default,
    /// and the number stays where the reasoning for it is.
    #[must_use]
    pub fn watching(mut self, first: Duration) -> Self {
        self.first = first;
        self
    }

    /// Variables every command this tool runs is started with, on top of the
    /// ones the environment boundary inherits.
    ///
    /// Handed to each child rather than set in this process, which is not a
    /// workaround: writing to the environment is `unsafe` in edition 2024 and
    /// this workspace forbids it, and the narrower thing is the right thing
    /// anyway. The model's commands get these; nothing else does, and a thread
    /// reading the environment while another one writes it cannot happen.
    ///
    /// A name given here wins over the one crucible inherited, because somebody
    /// who wrote `PATH` into a configuration file meant it for the commands
    /// crucible runs. Winning is two things together: the inherited entry under
    /// that name is dropped, and what replaces it is appended last — which is
    /// what settles a configured `Path` against an inherited `PATH` on Windows,
    /// where those are one variable and `std` hands over the later of the two.
    #[must_use]
    pub fn exporting<'a>(mut self, vars: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        for (name, value) in vars {
            self.env.retain(|(existing, _)| existing.as_ref() != name);
            self.env.push((name.into(), value.into()));
        }
        self
    }
}

impl Tool for Bash {
    fn name(&self) -> &'static str {
        NAME
    }

    fn schema(&self) -> &'static str {
        SCHEMA.as_str()
    }

    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
        let command =
            match Args::parse(NAME, args).and_then(|args| args.text(COMMAND).map(str::to_owned)) {
                Ok(line) => command::read(&line),
                // A call this malformed will be refused by `run`, but it still has
                // to be given a sensitivity first — and the safe answer to "what is
                // about to run" when nobody can read it is everything that was
                // sent, reported as unreadable.
                Err(_) => crucible_core::Command::Opaque(args.as_str().into()),
            };

        Sensitivity::SpawnsProcess { command }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        summary::field(NAME, args, COMMAND)
    }

    fn run(&self, approved: Approved, watch: &dyn Watch) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, approved.args())?;
        let command = args.text(COMMAND)?;
        let seconds = args.count(TIMEOUT, SECONDS)?;
        let background = args.flag(crate::account::LEFT, false)?;

        if seconds > CEILING {
            return Ok(ToolOutput::failed(format!(
                "timeout must be {CEILING} seconds or less"
            )));
        }

        // Refused rather than one of them ignored. A command left running has no
        // deadline — that is what it is for — so a call that sent both asked for
        // two different things, and answering it with either would be answering a
        // question nobody put.
        if background && args.holds(TIMEOUT) {
            return Ok(ToolOutput::failed(
                "timeout does not apply to a command left running: send one or the other",
            ));
        }

        // What decides whether this call ends up waiting. A run with nothing
        // holding the other end cannot leave a command running and must not
        // quietly wait for a dev server instead, so it says so.
        let leaving = match (background, self.leaving.as_ref()) {
            (true, None) => {
                return Ok(ToolOutput::failed(
                    "this run cannot leave a command running",
                ));
            }
            // Let go of once it has had a moment to fail on the spot. A command
            // that is already over by then was never a background command, and
            // the model gets its failure now rather than in a panel.
            (true, Some(left)) => Some(output::Leaving {
                left,
                after: Some(self.first),
            }),
            // Nothing asked for, and the key can still ask.
            (false, Some(left)) => Some(output::Leaving { left, after: None }),
            (false, None) => None,
        };

        if self.cancel.requested() {
            return Err(ToolError::Cancelled(NAME));
        }

        let shell = self.shell.as_ref().ok_or_else(|| {
            io(
                "no POSIX shell to run it with",
                std::io::Error::new(std::io::ErrorKind::NotFound, shell::ABSENT),
            )
        })?;

        let mut spawning = std::process::Command::new(shell);
        spawning
            .arg("-c")
            .arg(command)
            .current_dir(self.workspace.root())
            // The child's environment is built rather than inherited: what
            // crucible was started with holds the provider credential, and
            // `env` is a command a model runs for ordinary reasons. The
            // `environment` module says what a child gets instead.
            .env_clear()
            .envs(
                self.env
                    .iter()
                    .map(|(name, value)| (name.as_ref(), value.as_os_str())),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(unix)]
        let scope = platform::Scope::new(&mut spawning);
        #[cfg(windows)]
        let scope = platform::Scope::new(&mut spawning)
            .map_err(|source| io("could not prepare command containment", source))?;

        let child = spawning
            .spawn()
            .map_err(|source| io("could not start a shell", source))?;
        #[cfg(windows)]
        let child = {
            if let Err(source) = scope.attach(&child) {
                // Consumed, because the job handle goes with the command it was
                // holding: closing it is what ends anything the shell managed to
                // start before the assignment failed.
                output::discard(child, scope);
                return Err(io("could not contain the command", source));
            }
            child
        };

        // `count` already refused anything but a positive number, and the
        // ceiling above bounds it, so the fallback is unreachable arithmetic
        // rather than a decision.
        let allowed = Duration::from_secs(u64::try_from(seconds).unwrap_or(60));

        let waiting = output::Waiting {
            allowed,
            cancel: &self.cancel,
            watch,
            leaving,
        };

        match output::collect(child, scope, &waiting)? {
            output::Left::Answered(output) => Ok(output),

            // Kept, or refused and ended — the registry owns both, because it is
            // what knows the cap and what would have to end the command anyway.
            output::Left::Running(taking) => {
                let Some(left) = self.leaving.as_ref() else {
                    return Ok(ToolOutput::failed(
                        "this run cannot leave a command running",
                    ));
                };
                let printed = taking.printed();

                match left.keep(command, taking) {
                    Some(number) => Ok(ToolOutput::ok(format!(
                        "{printed}\n\n[left running as #{number}; {LEFT_RUNNING}]"
                    ))),
                    None => Ok(ToolOutput::failed(format!(
                        "{MOST} commands are already running; stop one before leaving another"
                    ))),
                }
            }
        }
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
