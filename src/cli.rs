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
mod release;
mod remember;
#[cfg(test)]
mod sample;
mod seen;
mod startup;
mod style;

use std::io::{self, Write as _};
use std::process::ExitCode;

use clap::Parser;
use crucible_config::{ConfigError, Home, Settings, Updates};
use crucible_core::{Cancel, CredentialError, PathError, Workspace};
use crucible_runner::SessionError;
use crucible_tui::{RawError, Renderer, SystemTerminal, TerminalError, Title, TitleError, Welcome};

use crate::cli::choice::Choice;
use crate::cli::converse::Terms;
use crate::cli::draw::Opening;
use crate::cli::startup::{Startup, assemble, served};
use crate::cli::style::Style;

/// The providers this is built with, and where each one's key is read from.
///
/// One list rather than two: the sentence a wrong name gets back is written
/// from it, and so is the check that refuses the name before anything is drawn.
/// [`startup::provider`] has one arm per entry, and adding a provider is an
/// edit to both in the same commit.
///
/// No model is written here, and none may be. A name in this file is a name
/// this build was compiled with, and a model chosen that way is chosen for
/// somebody who never asked for it — it outlives the model, it is asked for
/// with whichever key happens to be set, and the first anyone hears of the
/// mismatch is a refusal from a vendor they did not mean to write to. What to
/// ask for comes from the person running it, through `--model` or through
/// `providers.<name>.model`, and where neither says, crucible asks rather than
/// guesses.
const PROVIDERS: [Served; 2] = [
    Served {
        name: "anthropic",
        key: "ANTHROPIC_API_KEY",
    },
    Served {
        name: "openai",
        key: "OPENAI_API_KEY",
    },
];

/// A provider this build has an arm for, and where its key is read from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Served {
    /// What `--model provider/…` and `providers.<name>` call it.
    pub(crate) name: &'static str,
    /// The variable its key is read from, unless `apiKeyEnv` names another.
    /// The *name* is what is written here; the value is read once, in
    /// [`startup::provider`], and goes no further than the header it signs.
    pub(crate) key: &'static str,
}

/// What crucible says when nothing on this machine is set up to answer.
///
/// Said under the welcome where a run starts with no key for any provider, and
/// again in place of any turn typed before there is one.
pub(crate) const NOTHING_TO_ASK: &str = "Warning: No models available. Use /login or set an API key environment variable. Then use /model to select a model.";

/// What it says when there is a provider to ask and nothing to ask it for.
///
/// A separate sentence rather than the one above, because the one above tells
/// somebody to set a key — and the key is the half they have already done. A
/// warning that names the wrong missing thing is worse than no warning: it
/// sends the reader to check something that was never wrong.
pub(crate) const NO_MODEL_CHOSEN: &str =
    "Warning: No model selected. Use /model to select the model to ask.";

/// Which of the two a session with no model has to say.
///
/// The provider by name rather than by entry, because the loop holds only the
/// name by the time it has to ask this again.
pub(crate) const fn unasked(provider: Option<&str>) -> &'static str {
    match provider {
        Some(_) => NO_MODEL_CHOSEN,
        None => NOTHING_TO_ASK,
    }
}

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
claude-sonnet-5, or openai/gpt-5.6-terra. The provider is whichever of \
ANTHROPIC_API_KEY and OPENAI_API_KEY holds a key — a variable exported empty \
holds none, so it does not compete — and where both do, qualify the name or \
set providers.<name>.model for one of them. The key is read from that \
provider's variable, or from whichever one its apiKeyEnv names.

There is no model built in. Left off, or given as a provider and a bare slash, \
the model comes from your configuration; where nothing says, crucible starts \
and asks rather than picking one, and /model writes your answer down.

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

    /// The model to ask, optionally as provider/model. Left off, it is
    /// whatever your configuration chose for the provider whose key is set.
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

    /// More than one provider has a key, and nothing said which of them to ask.
    #[error(
        "more than one provider holds a key ({held}), so which to ask is not decided; \
         qualify the name as --model provider/model, or set providers.<name>.model \
         for one of them"
    )]
    Ambiguous {
        /// The variables that hold one, named so the answer is a shell command
        /// away rather than a search through the documentation.
        held: Box<str>,
    },

    /// A prompt arrived that could not be answered, on a run with no terminal
    /// to fix it from.
    ///
    /// Interactively this is a warning and the session carries on, because
    /// `/model` is a key away. Down a pipe there is nobody to type it, so the
    /// prompt is unanswerable and a run that returned nothing must say so in
    /// the one place a script reads: the exit code. Ending with `Ok` there is
    /// the "it does nothing" report, arriving as success.
    #[error("{0} No turn was taken.")]
    Unanswerable(&'static str),

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

    // Only the flag names a provider outright. Everything else is evidence
    // rather than an instruction — which key this machine holds, which provider
    // a file already chose a model for — and it is read here, before anything is
    // drawn, so a run that cannot start does not first announce that it has.
    //
    // Both halves may come back missing, and neither is filled in with a guess.
    // A provider chosen without a key is one whose refusal arrives after the
    // first prompt; a model chosen without being asked for is one vendor's name
    // sent to whichever vendor the key belongs to.
    let choice = match cli.model.as_deref() {
        Some(named) => Choice::parse(named).ok_or(Fatal::Providerless)?,
        None => Choice::default(),
    };

    let serving = match &choice.provider {
        Some(named) => Some(served(named)?),
        None => chosen(&choice, &settings, &from)?,
    };
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
            style::Output {
                color: settings.color(),
                glyphs: settings.glyphs(),
                detail: settings.tool_detail(),
                mouse: settings.mouse(),
            },
            renderer.is_terminal(),
            &from,
        ),
        cancel: cancel.clone(),

        // The layer git ignores, resolved from the root the project's own
        // files were read from — so what an answer of `always` writes is what
        // the next crucible started here reads back.
        remembering: crucible_config::local(workspace.root()),

        // Who `/model` is choosing for, and where it writes the choice down.
        // Both are settled here because both are facts about how this run was
        // set up, and the prompt is not the place to work out either again.
        provider: serving.map(|one| one.name),
        choosing: crucible_config::user(&home),

        // The two `/resume` reads a directory of logs with. Both are settled
        // here for the same reason everything else in `Terms` is: the session
        // being picked up is one of this directory's, and which directory that
        // is was decided before the first prompt.
        sessions: home.sessions().to_owned(),
        workspace: workspace.clone(),
    };

    // Said once, now that the style is settled. It is what decides whether the
    // markers in the model's markdown are read or left where they are.
    renderer.wears(terms.style.palette());

    // What was worked on here before. This is on the startup path, which is
    // budgeted at twenty milliseconds, so it is bounded at both ends: the
    // component says how many rows it can use, and the scan reads names to
    // put a directory in time order and opens only the newest few files it
    // finds there. A directory nobody has worked in costs one read and draws
    // the heading with nothing under it.
    let sessions = crucible_runner::recent(home.sessions(), &workspace, Welcome::WANTED);

    // Off the disk, so no socket is opened on the path the first frame is
    // measured on. Asking again happens after the frame is drawn, on a thread
    // nobody waits for, and what it finds is what the next run says. Nothing
    // said is asking: a release check is the sort of thing somebody turns off,
    // and one that has to be turned *on* is one nobody has.
    let asking = settings.updates().is_none_or(Updates::wanted);
    let update = asking
        .then(|| release::newer(home.path(), env!("CARGO_PKG_VERSION")))
        .flatten();

    draw::opening(
        &mut renderer,
        &Opening {
            model: model.as_deref(),
            unasked: unasked(serving.map(|one| one.name)),
            workspace: &workspace,
            sessions: &sessions,
            update: update.as_ref(),
            style: terms.style,
        },
    )?;

    if asking {
        release::refresh(home.path());
    }

    let runner = assemble(&Startup {
        provider: serving,
        model: model.as_deref(),
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

/// Which provider to ask when the flag named none, or `None` where this machine
/// has nothing set up to ask.
///
/// The only evidence a flagless run leaves is which key the machine holds.
/// Somebody who has exported one key has set up one provider, and that is the
/// provider — whatever model name they went on to type. This is the rung that
/// stops `OPENAI_API_KEY` from being read beside a model only Anthropic serves:
/// there is no provider written into this build for a bare name to fall back
/// on, so the key decides, and a key that decides nothing is a run with no
/// provider rather than a run against a guess.
///
/// Where the file says `apiKeyEnv`, that is the variable looked for: a key is
/// configured by the name of the variable holding it, and this reads the name
/// rather than the value — nothing here learns what any key is. A variable
/// exported empty holds no key, the same way the lookup that reads one sees it,
/// so `ANTHROPIC_API_KEY=` turns that provider off rather than competing.
///
/// Several keys and a model already chosen for exactly one of them is that one:
/// a `providers.openai.model` written at home is an answer to this question,
/// and the run after `/model` is the one it has to keep answering. Several keys
/// and no such answer is [`Fatal::Ambiguous`] — two providers set up and nothing
/// choosing between them is a question, and picking one would send a turn to a
/// vendor over a coin toss.
fn chosen(
    choice: &Choice,
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Served>, Fatal> {
    let mut holding = keyed(settings, from);
    let (Some(first), second) = (holding.next(), holding.next()) else {
        return Ok(None);
    };
    if second.is_none() {
        return Ok(Some(first));
    }

    // The flag having named the model is what makes configuration unable to
    // answer: `providers.<name>.model` is a choice of model, and the flag has
    // already overruled it, so reading it here would pick a provider by a name
    // this run is not going to ask for.
    let mut answered = keyed(settings, from)
        .filter(|one| choice.model.is_none() && settings.model(one.name).is_some());

    match (answered.next(), answered.next()) {
        (Some(one), None) => Ok(Some(one)),
        _ => Err(Fatal::Ambiguous {
            held: keyed(settings, from)
                .map(|one| settings.api_key_env(one.name).unwrap_or(one.key))
                .collect::<Vec<_>>()
                .join(", ")
                .into(),
        }),
    }
}

/// Every provider whose variable holds a key, in the order they are declared.
fn keyed<'a>(
    settings: &'a Settings,
    from: &'a dyn Fn(&str) -> Option<String>,
) -> impl Iterator<Item = Served> + 'a {
    PROVIDERS.into_iter().filter(move |one| {
        from(settings.api_key_env(one.name).unwrap_or(one.key))
            .is_some_and(|value| !value.trim().is_empty())
    })
}

/// Which model to ask for, once the command line and the files have both spoken.
///
/// The flag, then the configuration for the provider this is going to, then
/// nothing. `--model openai/` naming a provider and no model is what makes the
/// middle rung reachable: without it every way of choosing a provider names a
/// model in the same breath, and `providers.openai.model` could never be the
/// answer to anything.
///
/// There is no bottom rung, and that is the point. A name written into this
/// build would be asked for on behalf of somebody who never chose it, and it
/// would be asked of whichever provider the key belongs to.
///
/// A key written into a file and left empty is a file that said nothing, not a
/// request for a model called nothing. Sent as it stands it would reach a
/// vendor as a name with no characters in it.
fn wanted(choice: &Choice, settings: &Settings, serving: Option<Served>) -> Option<Box<str>> {
    if let Some(named) = choice.model.clone() {
        return Some(named);
    }

    let configured = settings.model(serving?.name)?.trim();

    (!configured.is_empty()).then(|| configured.into())
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
