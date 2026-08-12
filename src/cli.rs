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
use crucible_tui::{RawError, Renderer, SystemTerminal, TerminalError, Title, TitleError, Welcome};

use crate::cli::choice::Choice;
use crate::cli::converse::Terms;
use crate::cli::startup::{Startup, assemble, served};
use crate::cli::style::Style;

/// The providers this is built with, and what each is asked for when nothing
/// else names a model.
///
/// One list rather than three: the sentence a wrong name gets back is written
/// from it, so is the check that refuses the name before anything is drawn, and
/// so is the model a run lands on with no flag and no configuration.
/// [`startup::provider`] has one arm per entry, and adding a provider is an
/// edit to both in the same commit.
///
/// The model belongs to the provider rather than to the build. One name for all
/// of them is a name only one of them serves, and the other finds that out
/// after the key has been read and the request sent.
const PROVIDERS: [Served; 2] = [
    Served {
        name: "anthropic",
        model: "claude-sonnet-5",
        key: "ANTHROPIC_API_KEY",
    },
    Served {
        name: "openai",
        model: "gpt-5.6-terra",
        key: "OPENAI_API_KEY",
    },
];

/// A provider this build has an arm for, the model it answers with, and where
/// its key is read from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Served {
    /// What `--model provider/…` and `providers.<name>` call it.
    pub(crate) name: &'static str,
    /// What to ask it for when neither the flag nor a file named a model.
    pub(crate) model: &'static str,
    /// The variable its key is read from, unless `apiKeyEnv` names another.
    /// The *name* is what is written here; the value is read once, in
    /// [`startup::provider`], and goes no further than the header it signs.
    pub(crate) key: &'static str,
}

/// The provider an unqualified model name is served by, and the one a machine
/// holding no key — or every key — lands on.
///
/// Named rather than taken from the head of the list, so reordering the entries
/// above cannot quietly move where a bare `crucible` sends its first turn.
const FALLBACK: &str = "anthropic";

/// The provider names, for the sentence a name outside them gets back.
fn names() -> String {
    PROVIDERS.map(|one| one.name).join(", ")
}

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
claude-sonnet-5, or openai/gpt-5.6-terra. Unqualified names go to Anthropic. \
Left off, the provider is whichever of ANTHROPIC_API_KEY and OPENAI_API_KEY \
holds a key, and Anthropic when both or neither does — a variable exported \
empty holds none, so it does not compete. Left off, or \
given as a provider and a bare slash, the model comes from your configuration, \
and failing that from the one this build pairs with that provider. The key is \
read from that provider's variable, or from whichever one its apiKeyEnv names.

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
    /// configuration says, then to the one this build pairs with that provider.
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

    /// The terminal would not hand over the keys as they are pressed.
    #[error(transparent)]
    Raw(#[from] RawError),

    /// The command line named a provider this is not built with.
    #[error("no provider called {named}; this build has {}", names())]
    Provider {
        /// What was asked for.
        named: Box<str>,
    },

    /// The command line put nothing before the slash.
    #[error("--model needs a provider before the slash, as in --model openai/gpt-5.6-terra")]
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

    // A flag that is present names a provider, even when it names one by saying
    // a bare model name and letting the unqualified form answer. A flag left off
    // names nothing at all, and then the only evidence on the machine is which
    // key is set.
    let choice = match cli.model.as_deref() {
        Some(named) => Choice::parse(named).ok_or(Fatal::Providerless)?,
        None => Choice::serving(keyed(&settings, &from)),
    };

    // The name on its own, here rather than in `assemble`, because the banner
    // below names a model and the provider that would serve it: a run that
    // cannot start should not first announce that it has. Only the name — the
    // key, the agent and the session stay where they are, after the banner,
    // since the first frame is measured to its first word.
    //
    // What it found comes back rather than being thrown away, because the model
    // to fall back on is a fact about the provider this proved. Looking the name
    // up twice is what would let the two answers be about different providers.
    let serving = served(&choice.provider)?;
    let model = wanted(&choice, &settings, serving);

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
    // This is where a session starts and not what it stays at: the engine below
    // takes it, and from then on the engine is the only thing that holds it, so
    // the mode on screen cannot drift from the mode in force.
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
        cancel: cancel.clone(),

        // The layer git ignores, resolved from the root the project's own
        // files were read from — so what an answer of `always` writes is what
        // the next crucible started here reads back.
        remembering: crucible_config::local(workspace.root()),

        // The two `/resume` reads a directory of logs with. Both are settled
        // here for the same reason everything else in `Terms` is: the session
        // being picked up is one of this directory's, and which directory that
        // is was decided before the first prompt.
        sessions: home.sessions().to_owned(),
        workspace: workspace.clone(),
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

/// Which provider to ask when nothing named one.
///
/// `crucible` on its own says nothing about which provider is wanted, so the
/// only evidence there is is which key the machine holds. Somebody who has
/// exported one key has set up one provider, and answering them with a refusal
/// about the *other* provider's variable names a thing they never meant to set.
///
/// Where the file says `apiKeyEnv`, that is the variable looked for: a key is
/// configured by the name of the variable holding it, and this reads the name
/// rather than the value — nothing here learns what any key is.
///
/// Several keys, or none, is [`FALLBACK`]. That is not a guess about which was
/// meant; it is the same answer for the same machine every run, rather than one
/// that turns on which variables a shell happened to export.
///
/// A variable exported empty is looked at twice, and the order is the whole
/// point. It is not a key — the lookup that reads one refuses a blank — so it
/// loses to a variable that holds one, and a shell carrying `ANTHROPIC_API_KEY=`
/// to turn that provider *off* does not outvote the key beside it. Where nothing
/// holds a key it counts after all, so a machine set up with one blank variable
/// is refused by the name already in the shell rather than by one the user has
/// never typed.
fn keyed(settings: &Settings, from: &dyn Fn(&str) -> Option<String>) -> &'static str {
    sole(settings, from, |value| !value.trim().is_empty())
        .or_else(|| sole(settings, from, |_| true))
        .unwrap_or(FALLBACK)
}

/// The one provider whose variable `holds`, or `None` where that is not exactly
/// one of them.
///
/// The value is read and asked a question about; nothing keeps it and nothing
/// learns what it was.
fn sole(
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
    holds: impl Fn(&str) -> bool,
) -> Option<&'static str> {
    let mut found = PROVIDERS.into_iter().filter(|one| {
        from(settings.api_key_env(one.name).unwrap_or(one.key)).is_some_and(|value| holds(&value))
    });

    match (found.next(), found.next()) {
        (Some(one), None) => Some(one.name),
        _ => None,
    }
}

/// Which model to ask for, once the command line and the files have both spoken.
///
/// The flag, then the configuration for that provider, then the name that
/// provider is built with. `--model openai/` naming a provider and no model is
/// what makes the middle rung reachable: without it every way of choosing a
/// provider names a model in the same breath, and `providers.openai.model`
/// could never be the answer to anything.
///
/// The bottom rung is `serving` rather than a name of its own, so every rung is
/// about the provider the run is going to. A rung that was not would send one
/// vendor another vendor's model name.
fn wanted(choice: &Choice, settings: &Settings, serving: Served) -> Box<str> {
    choice
        .model
        .clone()
        .or_else(|| settings.model(&choice.provider).map(Into::into))
        .unwrap_or_else(|| serving.model.into())
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
