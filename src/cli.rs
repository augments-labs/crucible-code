//! Argument parsing, and what the arguments and the files together decided.
//!
//! The command line is read here and resolved against the configuration. What
//! those answers choose is then built in two halves: the terminal, the renderer
//! and the terms every turn runs under are put together here, because they are
//! what a failure has to be reported through; the provider, the tools and the
//! session are put together one module along, in [`startup`]. Everything below
//! is reached as a trait object, which is what leaves every crate free of the
//! others.
//!
//! Nothing above this file knows what an HTTP client is, and nothing below it
//! knows what the command line said.

mod choice;
mod converse;
mod draw;
#[cfg(test)]
mod fake;
mod remember;
#[cfg(test)]
mod sample;
mod seen;
mod startup;
mod style;

use std::io::{self, Write as _};
use std::process::ExitCode;

use clap::Parser;
use crucible_config::{ConfigError, Home, Settings};
use crucible_core::{Cancel, CredentialError, PathError, Workspace};
use crucible_runner::SessionError;
use crucible_tui::{Renderer, SystemTerminal, TerminalError, Title, TitleError, Welcome};

use crate::cli::choice::Choice;
use crate::cli::converse::Terms;
use crate::cli::startup::{Startup, assemble, served};
use crate::cli::style::Style;

/// The model asked when the command line does not name one.
const MODEL: &str = "claude-sonnet-5";

/// The providers this is built with.
///
/// One list rather than two: the sentence a wrong name gets back is written
/// from it, and so is the check that refuses the name before anything is drawn.
/// [`startup::provider`] has one arm per entry, and adding a provider is an
/// edit to both in the same commit.
const PROVIDERS: [&str; 2] = ["anthropic", "openai"];

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

--model takes a model name, optionally qualified by the provider serving it: \
claude-sonnet-5, or openai/gpt-5.2. Unqualified names go to Anthropic. Left \
off, or given as a provider and a bare slash, the model comes from your \
configuration. The key is read from ANTHROPIC_API_KEY or OPENAI_API_KEY, or \
from whichever variable that provider's apiKeyEnv names.

crucible keeps its own files in ~/.crucible, and reads config.json there, then \
.crucible/config.json and .crucible/config.local.json in the directory it was \
started in. Nearer wins; the command line is nearer than all of them.

Sessions are written one file per session, and --continue picks up the most \
recent one for this directory.

Flags, session files and config are unstable for the whole 0.0.x line."
)]
struct Cli {
    /// Carry on the most recent session for this directory.
    #[arg(short, long)]
    r#continue: bool,

    /// The model to ask, optionally as provider/model. Defaults to what your
    /// configuration says, then to claude-sonnet-5.
    #[arg(short, long)]
    model: Option<String>,
}

/// Why crucible could not run, or could not carry on.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Fatal {
    /// The directory crucible was started in could not be read.
    #[error("the directory crucible was started in could not be read: {0}")]
    Here(io::Error),

    /// The working directory is not one that can be worked in.
    #[error(transparent)]
    Workspace(#[from] PathError),

    /// The session could not be recorded or continued.
    #[error(transparent)]
    Session(#[from] SessionError),

    /// crucible's own files could not be found or read.
    #[error(transparent)]
    Config(#[from] ConfigError),

    /// There is no key to authenticate with.
    #[error(transparent)]
    Credential(#[from] CredentialError),

    /// The terminal could not be drawn on.
    #[error(transparent)]
    Terminal(#[from] TerminalError),

    /// The terminal would not take a title.
    #[error(transparent)]
    Title(#[from] TitleError),

    /// The command line named a provider this is not built with.
    #[error("no provider called {named}; this build has {}", PROVIDERS.join(", "))]
    Provider {
        /// What was asked for.
        named: Box<str>,
    },

    /// The command line put nothing before the slash.
    #[error("--model needs a provider before the slash, as in --model openai/gpt-5.2")]
    Providerless,

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
    let here = std::env::current_dir().map_err(Fatal::Here)?;
    let workspace = Workspace::open(here)?;
    let cancel = Cancel::new();
    let from = |name: &str| std::env::var(name).ok();

    // Where crucible keeps its own files, read from the environment here and
    // handed down as a path — no crate below this one asks where anything is.
    // Then the files themselves, once, before anything that could want them.
    let home = Home::find(&|name| std::env::var_os(name))?;
    let settings = Settings::read(&home, workspace.root())?;

    // Widened after the files are read because the root is what found them:
    // `.crucible/config.json` is looked for in the directory crucible was
    // started in, so the workspace has to exist before it can be told what else
    // to reach. Once, here, and never again — nothing in a turn may widen it.
    let workspace = workspace.reaching(settings.extra_directories())?;

    // An absent flag parses as "the default provider, no model named", so the
    // resolution below has one path through it rather than two.
    let choice =
        Choice::parse(cli.model.as_deref().unwrap_or_default()).ok_or(Fatal::Providerless)?;
    let model = wanted(&choice, &settings);

    // The name on its own, here rather than in `assemble`, because the banner
    // below names a model and the provider that would serve it: a run that
    // cannot start should not first announce that it has. Only the name — the
    // key, the agent and the session stay where they are, after the banner,
    // since the first frame is measured to its first word.
    served(&choice.provider)?;

    // Set before the session is started, because a session writes a file and
    // this does not: a failure here leaves the disk as it found it. The guard
    // restores the title on the way out of this function however it is left, so
    // a failure between here and the loop does not leave a tab named after a
    // process that is gone. A redirected run holds nothing, which is the guard
    // saying there was no tab to name.
    //
    // Reentrant on this thread, which is the only one that writes here: the
    // renderer holds the lock for its whole life, and the title borrows the
    // same handle to set a tab name and hand it back on the way out.
    let held = Title::set()?;

    // Before the renderer exists, which is the only moment this is reachable:
    // `Renderer` takes the terminal by value, so a clear cannot be mistaken for
    // a frame later and the rules about what a frame may write stay as strict
    // as they are. Off unless asked for — crucible draws inline, so the rows
    // already on screen are somebody's own work.
    let mut terminal = SystemTerminal::stdout();
    if settings.clear_screen(&from)?.wanted() {
        crucible_tui::clear(&mut terminal)?;
    }
    let mut renderer = Renderer::new(terminal);

    // The mode the files named, or the one that asks. `None` is "no layer
    // said", which is a different thing from a layer that said `ask` — but the
    // answer is the same, and the distinction is the command line's to use.
    // Resolved once and handed to the prompt line and the engine both, so the
    // mode on screen cannot drift from the mode in force.
    let mode = settings.mode().unwrap_or_default();

    // Settled once, here, from the files and the terminal together. Nothing on
    // the render path may ask either of them again.
    let terms = Terms {
        style: Style::resolve(
            settings.color(),
            settings.glyphs(),
            settings.tool_detail(),
            renderer.is_terminal(),
            &from,
        ),
        mode,
        cancel: cancel.clone(),

        // The layer git ignores, resolved from the root the project's own
        // files were read from — so what an answer of `always` writes is what
        // the next crucible started here reads back.
        remembering: crucible_config::local(workspace.root()),
    };

    // What was worked on here before. This is on the startup path, which is
    // budgeted at twenty milliseconds, so it is bounded at both ends: the
    // component says how many rows it can use, and the scan reads names to
    // put a directory in time order and opens only the newest few files it
    // finds there. A directory nobody has worked in costs one read and draws
    // the heading with nothing under it.
    let sessions = crucible_runner::recent(home.sessions(), &workspace, Welcome::WANTED);

    draw::opening(&mut renderer, &model, &workspace, &sessions, terms.style)?;

    let runner = assemble(&Startup {
        provider: &choice.provider,
        model: &model,
        resuming: cli.r#continue,
        mode,
        settings: &settings,
        sessions: home.sessions(),
        workspace: &workspace,
        cancel: &cancel,
        from: &from,
    })?;
    let outcome = converse::converse(runner, &mut renderer, &terms, &mut io::stdin().lock());

    drop(held);
    outcome
}

/// Which model to ask for, once the command line and the files have both spoken.
///
/// The flag, then the configuration for that provider, then the name this is
/// built with. `--model openai/` naming a provider and no model is what makes
/// the middle rung reachable: without it every way of choosing a provider names
/// a model in the same breath, and `providers.openai.model` could never be the
/// answer to anything.
fn wanted(choice: &Choice, settings: &Settings) -> Box<str> {
    choice
        .model
        .clone()
        .or_else(|| settings.model(&choice.provider).map(Into::into))
        .unwrap_or_else(|| MODEL.into())
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

#[cfg(test)]
mod tests;
