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

use std::cell::Cell;
use std::io::{self, Write as _};
use std::process::ExitCode;

use clap::Parser;
use crucible_auth::{Keys, Store};
use crucible_config::{ConfigError, Home, Settings, Updates};
use crucible_core::{Cancel, CredentialError, Effort, PathError, Provider, Workspace};
use crucible_provider::EndpointError;
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
/// The models are what `/model` offers and not what anything asks for. No
/// default is written here and none may be: a model chosen at compile time is
/// chosen for somebody who never asked for it — it outlives the model, it is
/// asked for with whichever key happens to be set, and the first anyone hears
/// of the mismatch is a refusal from a vendor they did not mean to write to.
/// What to ask for comes from the person running it, through `--model` or
/// through `providers.<name>.model`, and where neither says, crucible asks
/// rather than guesses. An offer is how it asks; it is still they who answer.
///
/// So this list going stale costs a shortcut and nothing else. A model retired
/// since the build is one nobody picked without the vendor refusing it by name,
/// and a model released since is typed, which is the path that was there before
/// any of these were written down.
const PROVIDERS: [Served; 3] = [
    Served {
        name: "anthropic",
        shown: "Anthropic",
        key: "ANTHROPIC_API_KEY",
        models: &[
            Model::new("claude-fable-5", EVERY),
            Model::new("claude-opus-5", EVERY),
            Model::new("claude-sonnet-5", EVERY),
            // The one model of this vendor's current three generations that
            // takes no rung: it reasons against a token budget rather than
            // against a word, and the field the other three read is one it has
            // never been served.
            Model::new("claude-haiku-4-5", NONE),
        ],
    },
    Served {
        name: "moonshot",
        shown: "MoonshotAI",
        key: "MOONSHOT_API_KEY",
        // Spelled the way the coding console spells them, that being the one
        // crucible asks. The open platform serves the same models under longer
        // names and does not serve the second of these at all, so a key from
        // there is a `baseUrl` and a typed name rather than a shorter list.
        models: &[
            Model::new("k3", KIMI),
            // The same model held to a quarter of its context. Offered beside
            // it because the choice is a plan's rather than a preference: the
            // full window is what the cheapest tier does not come with.
            Model::new("k3-256k", KIMI),
            // Both coding models think on every turn and are not asked how
            // hard: the field is not theirs to read, and thinking cannot be
            // turned off either.
            Model::new("kimi-for-coding", NONE),
            Model::new("kimi-for-coding-highspeed", NONE),
        ],
    },
    Served {
        name: "openai",
        shown: "OpenAI",
        key: "OPENAI_API_KEY",
        // The `-pro` variants are left off: they answer in one piece rather
        // than streaming, and every turn here is drawn as it arrives.
        models: &[
            Model::new("gpt-5.6-sol", EVERY),
            Model::new("gpt-5.6-terra", EVERY),
            Model::new("gpt-5.6-luna", EVERY),
            // One generation back and one rung short of the others.
            Model::new(
                "gpt-5.5",
                &[Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh],
            ),
        ],
    },
];

/// What a model that takes every rung crucible has is written with.
const EVERY: &[Effort] = &Effort::LADDER;

/// The three rungs the Kimi thinking models serve.
///
/// It maps the two it does not serve onto these rather than refusing them, so
/// this is the one place a narrowed ladder is a courtesy instead of the
/// difference between a turn and an error. It is still narrowed: a rung offered
/// is a rung asked for, and two words that reach the same rung are two words
/// somebody has to be told are the same.
const KIMI: &[Effort] = &[Effort::Low, Effort::High, Effort::Max];

/// What a model that takes none at all is written with.
///
/// Not the same as a model this build has never heard of. That one is offered
/// all five and left to the vendor to refuse, because crucible knows nothing
/// about it either way; this one is a model crucible knows serves no rung, and
/// offering one would be inventing a fact rather than declining to have one.
const NONE: &[Effort] = &[];

/// One model a provider offers, and how hard it can be asked to think.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Model {
    /// What `--model` and `/model` name it, spelled the way the vendor spells
    /// it, because it is the vendor that has to recognise it.
    pub(crate) name: &'static str,
    /// The rungs of [`Effort`] this model serves, weakest first, as its
    /// vendor's documentation had them when this was built.
    ///
    /// Empty is a model that takes no rung at all — several of these serve
    /// none, and two vendors refuse the request outright rather than ignoring
    /// the field. So this is what `/effort` walks rather than the whole ladder:
    /// a rung offered here that the model does not serve is a refusal crucible
    /// walked somebody into, one keystroke after showing them the word that
    /// caused it.
    pub(crate) rungs: &'static [Effort],
}

impl Model {
    /// One entry of the table above.
    const fn new(name: &'static str, rungs: &'static [Effort]) -> Self {
        Self { name, rungs }
    }
}

/// A provider this build has an arm for, and where its key is read from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Served {
    /// What `--model provider/…` and `providers.<name>` call it.
    pub(crate) name: &'static str,
    /// How the vendor spells it, for a list somebody is reading down rather
    /// than typing at. It reaches no argument, no config file and no request:
    /// a name that is capitalised in one place and lowercase in another is one
    /// somebody eventually types the wrong way round, so only [`Served::name`]
    /// is ever matched against.
    pub(crate) shown: &'static str,
    /// The variable its key is read from, unless `apiKeyEnv` names another.
    /// The *name* is what is written here; the value is read once, in
    /// [`startup::provider`], and goes no further than the header it signs.
    pub(crate) key: &'static str,
    /// The models `/model` offers for it, newest first, at most five.
    ///
    /// An offer and not a rung: nothing here is ever asked for unless somebody
    /// chose it, and a name that is not on the list is typed the way it always
    /// was. Which is what keeps this from being the model built into the build —
    /// the list is a shortcut past the vendor's documentation, and the vendor
    /// remains the authority on what it serves.
    ///
    /// That last sentence binds the rungs beside each name too. They are read
    /// off the same documentation and go stale the same way, and what a stale
    /// one costs is a rung missing from a panel rather than a wrong request:
    /// `--effort` and `/effort <rung>` both go straight to the vendor, which is
    /// the path that was there before any of this was written down.
    pub(crate) models: &'static [Model],
}

/// What one provider is set up with, once there is a key for it.
///
/// The three answers a launch reaches before the first prompt, in one value so
/// that `/login` can reach them again from the prompt. A key is what all three
/// waited on: a file that chose a model for a provider this machine could not
/// reach was a file saying nothing about this run, and the moment a key arrives
/// it is saying something.
pub(crate) struct Resolved {
    /// What a request is written to.
    pub(crate) provider: Box<dyn Provider>,
    /// The model the files name for it, where they name one.
    pub(crate) model: Option<Box<str>>,
    /// The rung they name for it, where they name one.
    pub(crate) effort: Option<Effort>,
}

/// Sets one provider up from the keys in hand, the way the launch set this run's
/// up.
///
/// Boxed rather than borrowed because what it closes over are borrows of the
/// launch, and a lifetime here would follow the value that holds it into the
/// signature of everything a command is handed.
pub(crate) type Serving = Box<dyn Fn(Served, &Keys) -> Result<Resolved, Fatal>>;

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

/// The rungs `model` serves, as far as this build knows.
///
/// All five for a name the table does not hold, which is the answer for every
/// model released since the build and every one typed rather than picked.
/// Nothing is known about it either way, and the choice is between offering a
/// rung its vendor may refuse and withholding one it serves — the first is a
/// sentence back from the vendor, the second is crucible deciding what a model
/// it has never heard of can do.
pub(crate) fn rungs(provider: &str, model: &str) -> &'static [Effort] {
    PROVIDERS
        .into_iter()
        .filter(|one| one.name == provider)
        .flat_map(|one| one.models)
        .find(|one| one.name == model)
        .map_or(EVERY, |one| one.rungs)
}

/// The command-line surface.
///
/// Unstable for the whole 0.x line: flags may be renamed or removed in any
/// 0.x release without a deprecation period.
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
ANTHROPIC_API_KEY, MOONSHOT_API_KEY and OPENAI_API_KEY holds a key — a \
variable exported empty holds none, so it does not compete — and where more \
than one does, qualify the name or set providers.<name>.model for one of them. \
The key is read from that provider's variable, or from whichever one its \
apiKeyEnv names.

MoonshotAI issues a key against one of two consoles and refuses it at the \
other, and nothing in the key says which. crucible asks the coding console; a \
key from the open platform sets providers.moonshot.baseUrl to \
https://api.moonshot.ai/v1.

--effort says how hard to think, as low, medium, high, xhigh or max, on every \
turn of the session. Left off, it is providers.<name>.effort, and where nothing \
says either, whatever the vendor's own default is for the model being asked. \
Not every model takes one, and a rung named for a model that does not is \
refused by its vendor rather than dropped.

There is no model built in. Left off, or given as a provider and a bare slash, \
the model comes from your configuration; where nothing says, crucible starts \
and asks rather than picking one, and /model writes your answer down.

crucible keeps its own files in ~/.crucible, and reads config.json there, then \
.crucible/config.json and .crucible/config.local.json in the directory it was \
started in. Nearer wins; the command line is nearer than all of them.

Sessions are written one file per session, and --continue picks up the most \
recent one for this directory.

Flags, session files and config are unstable for the whole 0.x line."
)]
struct Cli {
    /// Carry on the most recent session for this directory.
    #[arg(short, long)]
    r#continue: bool,

    /// The model to ask, optionally as provider/model. Left off, it is
    /// whatever your configuration chose for the provider whose key is set.
    #[arg(short, long)]
    model: Option<String>,

    /// How hard to think: low, medium, high, xhigh or max. Left off, it is
    /// what your configuration chose for this provider, and where nothing
    /// says, the vendor's own default for the model.
    #[arg(short, long, value_name = "RUNG")]
    effort: Option<Effort>,
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

    /// `providers.<name>.baseUrl` is not an address requests can be sent to.
    ///
    /// Fatal rather than a warning that carries on at the vendor's address:
    /// somebody who set this has a reason not to reach the vendor, and sending
    /// there anyway would be a refusal that took the key with it.
    #[error("providers.{provider}.baseUrl: {source}")]
    Address {
        /// Which provider was pointed somewhere it could not go.
        provider: Box<str>,
        /// What was wrong with the address.
        source: EndpointError,
    },

    /// The command line put nothing before the slash.
    #[error("--model needs a provider before the slash, as in --model openai/gpt-5.6-terra")]
    Providerless,

    /// More than one provider has a key, and nothing said which of them to ask.
    #[error(
        "more than one provider holds a key ({held}), so which to ask is not decided; \
         qualify the name as --model provider/model, or set provider to one of them"
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

    // What was logged in with, read once and from the same directory. A store
    // that cannot be read comes back empty with a sentence rather than as an
    // error, and the sentence is drawn under the welcome: a file that is only
    // ever an alternative to an exported variable must not be what ends a run
    // that never needed it.
    let keys = Store::in_home(home.path()).read();

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
        None => chosen(&settings, &from, &keys)?,
    };
    let model = wanted(&choice, &settings, serving);
    let effort = thinking(cli.effort, &settings, serving);

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
        // The second is a fact about how this run was set up and the prompt is
        // not the place to work it out again; the first is the one thing here a
        // command can change, because `/login` is what fills it in on a machine
        // that started with nothing.
        provider: Cell::new(serving.map(|one| one.name)),
        choosing: crucible_config::user(&home),

        // What `/login` sets a session up with, answered the way this launch
        // answered it for itself — so a key given at the prompt leaves the
        // session asking what the next run here would ask, rather than what a
        // second reading of the same files happened to say.
        //
        // The settings are cloned because this outlives every borrow in this
        // function, and a lifetime on `Terms` would follow it into the
        // signature of everything a command is handed. They are the files this
        // run already read: nothing in them grows with the transcript.
        serving: {
            let settings = settings.clone();

            Box::new(move |named: Served, stored: &Keys| {
                Ok(Resolved {
                    provider: startup::provider(
                        Some(named),
                        &settings,
                        &|name| std::env::var(name).ok(),
                        stored,
                    )?,

                    // No flag on either, and that is the whole of the
                    // difference from the launch: what `--model` and `--effort`
                    // said has already been applied, and re-applying it over
                    // what the session has since been told would be this
                    // answering a question somebody else already answered.
                    model: wanted(&Choice::default(), &settings, Some(named)),
                    effort: thinking(None, &settings, Some(named)),
                })
            })
        },

        // The two `/resume` reads a directory of logs with. Both are settled
        // here for the same reason everything else in `Terms` is: the session
        // being picked up is one of this directory's, and which directory that
        // is was decided before the first prompt.
        // The same directory the keys above were read from.
        logins: Store::in_home(home.path()),
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
            unasked: unasked(terms.provider.get()),
            trouble: keys.trouble(),
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
        effort,
        resuming: cli.r#continue,
        mode,
        settings: &settings,
        sessions: home.sessions(),
        workspace: &workspace,
        cancel: &cancel,
        from: &from,
        stored: &keys,
    })?;
    let outcome = converse::converse(runner, &mut renderer, &terms, &mut io::stdin().lock());

    drop(held);
    outcome
}

/// Which provider to ask when the flag named none, or `None` where this machine
/// has nothing set up to ask.
///
/// A key says a provider can be *reached*. Only a statement about providers
/// chooses one, and `provider` in the configuration is the only statement there
/// is — everything under `providers.<name>` is a subordinate clause about a
/// provider already being asked. So the top rung here is that key, and it is
/// read before any variable: which key a shell happens to carry is a fact about
/// that shell, and a turn sent to the wrong vendor costs money there and leaves
/// the prompt behind.
///
/// Below it, exactly one provider holding a key is that provider. That is not a
/// choice between competitors — it is the absence of anything to choose, which
/// is what lets a first run work with one exported key and no configuration at
/// all. Where the file says `apiKeyEnv`, that is the variable looked for: a key
/// is configured by the name of the variable holding it, and this reads the name
/// rather than the value, so nothing here learns what any key is. A variable
/// exported empty holds no key, so `ANTHROPIC_API_KEY=` turns that provider off
/// rather than competing.
///
/// Several keys and nothing choosing between them is [`Fatal::Ambiguous`]. No
/// key outranks another and no order between vendors is written down anywhere,
/// so there is nothing here to break the tie with; picking one would send a turn
/// to a vendor over a coin toss.
fn chosen(
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
    keys: &Keys,
) -> Result<Option<Served>, Fatal> {
    // Refused here where a name this build has nothing for is a sentence naming
    // the ones it has, rather than carried as "nobody chose" into a session that
    // would then look set up by a key nobody named.
    if let Some(named) = settings.provider() {
        return served(named).map(Some);
    }

    let mut holding = keyed(settings, from, keys);
    let (Some(first), second) = (holding.next(), holding.next()) else {
        return Ok(None);
    };
    if second.is_none() {
        return Ok(Some(first));
    }

    Err(Fatal::Ambiguous {
        held: keyed(settings, from, keys)
            .map(|one| held(one, settings, from))
            .collect::<Vec<_>>()
            .join(", ")
            .into(),
    })
}

/// Every provider crucible holds a key for, in the order they are declared.
///
/// Two places to look and one entry either way. A provider whose key is both
/// exported and written down is one provider: listed twice it would be two to
/// the question above, and somebody who exported the key they had already
/// logged in with would be asked to choose between a provider and itself.
fn keyed<'a>(
    settings: &'a Settings,
    from: &'a dyn Fn(&str) -> Option<String>,
    keys: &'a Keys,
) -> impl Iterator<Item = Served> + 'a {
    PROVIDERS
        .into_iter()
        .filter(move |one| exported(*one, settings, from) || keys.get(one.name).is_some())
}

/// Whether this provider's variable holds a key.
///
/// Blank is not one. `ANTHROPIC_API_KEY=` is how a shell turns a provider off,
/// and a variable that is set and empty used to count as one held.
fn exported(one: Served, settings: &Settings, from: &dyn Fn(&str) -> Option<String>) -> bool {
    from(settings.api_key_env(one.name).unwrap_or(one.key))
        .is_some_and(|value| !value.trim().is_empty())
}

/// How a provider holding a key is named back, in the sentence that asks which
/// of them to ask.
///
/// The variable, where that is where the key is: unsetting it is what the
/// reader would go and do about it. The provider's own name where the key was
/// written down instead — there is no variable to unset, and naming one that
/// was never set sends them to look at the wrong thing.
fn held(one: Served, settings: &Settings, from: &dyn Fn(&str) -> Option<String>) -> String {
    if exported(one, settings, from) {
        return settings.api_key_env(one.name).unwrap_or(one.key).to_owned();
    }

    one.name.to_owned()
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

/// How hard to think, once the command line and the files have both spoken.
///
/// The flag, then `providers.<name>.effort` for the provider this run is going
/// to, then nothing — the rungs `--model` is resolved down, and the bottom one
/// is missing for the same reason. A run nobody said anything to about effort
/// is one the vendor's own default applies to, and that default is per model:
/// answering `high` here on everybody's behalf would send the field to a model
/// that does not take it, and turn a session nobody configured into a refusal.
///
/// A provider with no key set is a run with no provider, and a file that chose
/// a rung for one it is not going to is a file that said nothing about this
/// run — the same reading `--model openai/` gets.
fn thinking(asked: Option<Effort>, settings: &Settings, serving: Option<Served>) -> Option<Effort> {
    asked.or_else(|| settings.effort(serving?.name))
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
