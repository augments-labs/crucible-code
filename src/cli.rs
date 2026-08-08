//! Argument parsing, and the wiring those arguments choose.
//!
//! This is the only place concrete types meet. A provider, six tools, a session
//! log and a renderer are built here and handed to each other as trait objects,
//! which is what leaves every crate below free of the others.
//!
//! Nothing above this file knows what an HTTP client is, and nothing below it
//! knows what the command line said.

mod converse;
mod draw;
#[cfg(test)]
mod fake;
mod seen;

use std::io::{self, Write as _};
use std::process::ExitCode;

use clap::Parser;
use crucible_core::{ApiKey, Cancel, CredentialError, HeaderKey, PathError, Workspace};
use crucible_provider::{Anthropic, Https};
use crucible_runner::{Model, Runner, Session, SessionError, Tools};
use crucible_tools::{Bash, Edit, Glob, Grep, Read, Write};
use crucible_tui::{Renderer, SystemTerminal, TerminalError, Title, TitleError};

/// The model asked when the command line does not name one.
const MODEL: &str = "claude-sonnet-5";

/// The variable the key is read from. The *name* is what is configured here;
/// the value never appears in this repository or in a session file.
const KEY: &str = "ANTHROPIC_API_KEY";

/// Ceiling on one response, in tokens.
const MAX_TOKENS: u32 = 8192;

/// The standing instructions every turn carries.
///
/// Written for this harness. It says how to work and how to answer, and leaves
/// what the tools do to the tools' own schemas — a system prompt that also
/// describes each tool is a second place for that description to go stale.
const SYSTEM: &str = "\
You are crucible, a coding agent working in a terminal beside a developer.

Work from what the code says rather than what it probably says: read a file \
before changing it, and search before concluding something is not there. \
Prefer the smallest change that finishes the job, and match the conventions of \
the file you are editing rather than your own habits.

Answer in plain prose, briefly. The developer is reading a terminal: put the \
conclusion first, skip the preamble, and do not read a file's contents back \
after editing it — say what changed and why.

Ask when the answer would change what you build. Otherwise decide, say which \
way you decided, and carry on.";

/// The command-line surface.
///
/// Unstable for the whole 0.0.x line: flags may be renamed or removed in any
/// 0.0.x release without a deprecation period.
///
/// `long_about` is spelled out rather than left to this doc comment, which clap
/// would otherwise print: what a contributor needs to know about this struct is
/// not what a user needs to know about the program.
#[derive(Debug, Parser)]
#[command(
    name = "crucible",
    version,
    about = "The harness where agents are forged.",
    long_about = "The harness where agents are forged.

A coding agent that works in the terminal. Type a prompt; it reads, searches, \
edits and runs things in the current directory, and asks before anything that \
changes a file or starts a process.

Authenticates with the key in ANTHROPIC_API_KEY. Sessions are written under \
the data directory, one file per session, and --continue picks up the most \
recent one for this directory.

Flags, session files and config are unstable for the whole 0.0.x line."
)]
struct Cli {
    /// Carry on the most recent session for this directory.
    #[arg(short, long)]
    r#continue: bool,

    /// The model to ask.
    #[arg(short, long, default_value = MODEL)]
    model: String,
}

/// Why crucible could not run, or could not carry on.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Fatal {
    /// The working directory is not one that can be worked in.
    #[error(transparent)]
    Workspace(#[from] PathError),

    /// The session could not be recorded or continued.
    #[error(transparent)]
    Session(#[from] SessionError),

    /// There is no key to authenticate with.
    #[error(transparent)]
    Credential(#[from] CredentialError),

    /// The terminal could not be drawn on.
    #[error(transparent)]
    Terminal(#[from] TerminalError),

    /// The terminal would not take a title.
    #[error(transparent)]
    Title(#[from] TitleError),

    /// Standard input could not be read.
    #[error("could not read what you typed: {0}")]
    Input(io::Error),

    /// The thread running the turn ended without returning it.
    #[error("the turn ended unexpectedly")]
    Lost,
}

/// Reads the command line and does what it says.
pub(crate) fn start() -> ExitCode {
    let cli = Cli::parse();

    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => fail(&problem),
    }
}

/// Builds everything, then hands over to the loop.
fn run(cli: &Cli) -> Result<(), Fatal> {
    let here = std::env::current_dir().map_err(Fatal::Input)?;
    let workspace = Workspace::open(here)?;
    let cancel = Cancel::new();

    let directory = Session::directory()?;
    let (session, earlier) = if cli.r#continue {
        let (session, transcript) = Session::resume(&directory, &workspace)?;
        (session, Some(transcript))
    } else {
        (Session::start(&directory, &workspace)?, None)
    };

    let provider = Anthropic::new(
        Box::new(HeaderKey::new(ApiKey::from_env(KEY)?, "x-api-key", "")),
        Box::new(Https::new()),
    );

    let mut runner = Runner::new(
        Box::new(provider),
        tools(&workspace, &cancel),
        model(&cli.model, &workspace),
        session,
    );
    if let Some(transcript) = earlier {
        runner = runner.resuming(transcript);
    }

    // Reentrant on this thread, which is the only one that writes here: the
    // renderer holds the lock for its whole life, and the title borrows the
    // same handle to set a tab name and hand it back on the way out.
    let held = Title::set(io::stdout())?;
    let mut renderer = Renderer::new(SystemTerminal::stdout())?;

    draw::opening(&mut renderer, &cli.model, &workspace)?;
    let outcome = converse::converse(runner, &mut renderer, &cancel, &mut io::stdin().lock());

    drop(held);
    outcome
}

/// Everything the model may call.
///
/// The order is the order they are advertised in, which is the order a model
/// tends to reach for them: read before write, search before either.
fn tools(workspace: &Workspace, cancel: &Cancel) -> Tools {
    let mut tools = Tools::new();

    tools.add(Box::new(Read::new(workspace.clone())));
    tools.add(Box::new(Grep::new(workspace.clone())));
    tools.add(Box::new(Glob::new(workspace.clone())));
    tools.add(Box::new(Edit::new(workspace.clone())));
    tools.add(Box::new(Write::new(workspace.clone())));
    tools.add(Box::new(Bash::new(workspace.clone(), cancel.clone())));

    tools
}

/// Which model to ask, and what it is standing on.
///
/// The root goes in the system prompt because every tool takes paths relative
/// to it, and a model that has to guess the root spends its first tool call
/// finding out.
fn model(name: &str, workspace: &Workspace) -> Model {
    let system = format!(
        "{SYSTEM}\n\nThe workspace root is {}. Every tool path is relative to it.",
        workspace.root().display()
    );

    Model {
        name: name.into(),
        max_tokens: MAX_TOKENS,
        system: Some(system.into()),
    }
}

/// Writes a fatal error where the user will see it.
///
/// Straight to standard error rather than through the renderer: the renderer is
/// one of the things that can fail here, and by this point there is no live
/// region left to protect.
fn fail(problem: &Fatal) -> ExitCode {
    let mut line = String::from("crucible: ");
    line.push_str(&problem.to_string());
    line.push('\n');

    let _ = io::stderr().write_all(line.as_bytes());
    ExitCode::FAILURE
}
