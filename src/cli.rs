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

mod branching;
mod browser;
mod choice;
mod converse;
mod draw;
#[cfg(test)]
mod fake;
mod kept;
mod models;
mod release;
mod remember;
#[cfg(test)]
mod sample;
mod seen;
mod standing;
mod startup;
mod style;
mod subscription;

use std::cell::{Cell, RefCell};
use std::fmt;
use std::io::{self, Write as _};
use std::process::ExitCode;

use clap::Parser;
use crucible_auth::{Store, StoredCredentials};
use crucible_config::{ConfigError, Home, Settings};
use crucible_core::{
    Cancel, CredentialError, Effort, Modalities, PathError, Provider, Revealed, SessionId,
    ToolsetError, Workspace,
};
use crucible_provider::EndpointError;
use crucible_runner::SessionError;
use crucible_tools::{Background, Ledger, Plan};
use crucible_tui::{
    RawError, Renderer, ScreenError, SystemTerminal, TerminalError, Title, TitleError, Welcome,
};

use crate::cli::choice::Choice;
use crate::cli::converse::Terms;
use crate::cli::draw::Opening;
use crate::cli::startup::{Startup, assemble, served};
use crate::cli::style::Style;
use crate::cli::subscription::Subscriptions;

/// How long the terminal is given to say what colour its background is.
///
/// **This is blocking I/O on the startup path, which `performance-budgets.md`
/// forbids outright.** It is written down here rather than argued away: the
/// rule is absolute and this is an exception to it. What makes the exception
/// defensible is that the cost is bounded by this constant rather than by the
/// terminal — eight milliseconds of a twenty-millisecond budget, worst case,
/// once per run — and `bench-first-frame` measures the whole path, so the
/// budget is the thing that decides whether it stays affordable rather than
/// this comment.
///
/// A terminal that implements the question answers in about a millisecond. One
/// that does not costs this much once and is never asked again, and what it
/// loses is the band behind the prompt rather than anything it needs. Terminals
/// where the answer would be slow are not asked at all — see `RELAYED` in
/// `crucible_tui::ground`, and the reason there is about a late reply becoming
/// a keystroke rather than about the wait.
///
/// The way out, when it is worth the machinery: ask on a thread and let the
/// answer land in `Terms::style` at the next prompt, which the loop already
/// re-reads per turn. That trades this exception for a second writer to the
/// terminal, which is its own rule.
const PATIENCE: std::time::Duration = std::time::Duration::from_millis(8);

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
            Model::shown("k3", "K3", KIMI),
            // The same model held to a quarter of its context. Offered beside
            // it because the smaller context is a distinct provider offering
            // rather than a local preference.
            Model::shown("k3-256k", "K3-256k", KIMI),
            // The coding models are known by their product names; the wire
            // identifier stays the one the console serves them under.
            Model::shown("kimi-for-coding", "K2.7 Coding", KIMI),
            Model::shown("kimi-for-coding-highspeed", "K2.7 Coding Highspeed", KIMI),
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

/// What may be put in front of this model, on this provider.
///
/// Two halves, and neither is the answer alone: a model reads what it reads
/// whichever protocol carries it, and a protocol carries what it has a shape
/// for whatever the model would do with it. Offering the union would send a
/// video to a request with no word for one.
///
/// `None` is not the empty set. It is the table never having heard of this
/// model, which is a different thing from the model reading nothing, and the
/// caller says so in different words.
pub(crate) fn attachable(provider: &dyn Provider, model: &str) -> Option<Modalities> {
    facts(provider.name(), model).map(|facts| facts.accepts.intersection(provider.spells()))
}

/// What this build knows about a model's limits, if it knows anything.
///
/// Keyed on the name exactly as it was asked for. A name this build has never
/// heard of answers `None` rather than the nearest thing it resembles: two
/// models one word apart can differ five-fold in what they accept, and a
/// session run against the wrong one of them throws away most of itself before
/// anybody notices.
pub(crate) fn facts(provider: &str, model: &str) -> Option<models::Facts> {
    models::FACTS
        .iter()
        .find(|facts| facts.provider == provider && facts.model == model)
        .copied()
}

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
    /// What a picker row calls it, where the product name differs from the wire
    /// identifier a configuration or command must carry.
    pub(crate) shown: &'static str,
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
        Self {
            name,
            shown: name,
            rungs,
        }
    }

    /// One entry whose product name and wire identifier differ.
    const fn shown(name: &'static str, shown: &'static str, rungs: &'static [Effort]) -> Self {
        Self { name, shown, rungs }
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

/// What one provider is set up with, once it has a credential.
///
/// The two answers a launch reaches before the first prompt, in one value so
/// that `/login` can reach them again from the prompt. A usable credential is
/// what both waited on: a file that chose a model for a provider this machine
/// could not reach was a file saying nothing about this run, and the moment a
/// credential arrives it is saying something.
pub(crate) struct Resolved {
    /// What a request is written to.
    pub(crate) provider: Box<dyn Provider>,
    /// Which non-secret source supplied its credential.
    pub(crate) source: CredentialSource,
}

/// Where the active credential came from, without any credential bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CredentialSource {
    /// An environment variable, named but never read back through this value.
    Environment(Box<str>),
    /// An API key written by `/login`.
    StoredKey,
    /// A renewable account login written by `/login`.
    Subscription,
}

impl fmt::Display for CredentialSource {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment(name) => write!(out, "environment variable {name}"),
            Self::StoredKey => out.write_str("a stored API key"),
            Self::Subscription => out.write_str("a stored account login"),
        }
    }
}

/// Sets one provider up from the credentials in hand, the way the launch set
/// this run's up.
///
/// Boxed rather than borrowed because what it closes over are borrows of the
/// launch, and a lifetime here would follow the value that holds it into the
/// signature of everything a command is handed.
pub(crate) type Serving = Box<dyn Fn(Served, &StoredCredentials) -> Result<Resolved, Fatal>>;

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

/// What it says when several providers are authenticated and none was chosen.
///
/// Authentication made models reachable; it did not choose which vendor may
/// receive the next prompt. `/model` is the explicit joint provider/model
/// choice, so telling somebody to log in again would name the wrong missing
/// thing.
pub(crate) const NO_PROVIDER_CHOSEN: &str =
    "Warning: No provider selected. Use /model to select a provider and model.";

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

/// The startup warning after credential discovery has distinguished zero from
/// several available providers.
const fn opening_unasked(provider: Option<Served>, any_credential: bool) -> &'static str {
    match (provider, any_credential) {
        (Some(_), _) => NO_MODEL_CHOSEN,
        (None, true) => NO_PROVIDER_CHOSEN,
        (None, false) => NOTHING_TO_ASK,
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
claude-sonnet-5, or openai/gpt-5.6-terra. The provider is whichever holds a \
usable credential — a key in one of ANTHROPIC_API_KEY, MOONSHOT_API_KEY and \
OPENAI_API_KEY (a variable exported empty holds none, so it does not compete), \
or one stored by /login, whether an API key or an account login. Where more \
than one is usable, qualify the name or set provider for one of them. \
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
recent one for this directory. --resume picks up the exact session an id \
names instead; a quitting session prints its own id on the way out, and \
/resume inside a session lists the rest.

Flags, session files and config are unstable for the whole 0.x line."
)]
struct Cli {
    /// Carry on the most recent session for this directory.
    #[arg(short, long)]
    r#continue: bool,

    /// Pick up the exact session this id names, from the parting message or
    /// the /resume picker.
    #[arg(short, long, value_name = "SESSION_ID", conflicts_with = "continue")]
    resume: Option<String>,

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

    /// The configured tool roster was invalid or could not be materialized.
    #[error(transparent)]
    Toolset(#[from] ToolsetError),

    /// `--resume` named a session this workspace has no record of.
    ///
    /// Its own sentence rather than the session crate's, because the id came
    /// from the command line a moment ago: what the user needs to hear is that
    /// the address is wrong here, not which file was looked for.
    #[error("no session {0} in this workspace")]
    NoSession(Box<str>),

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

    /// The terminal would not hand over a screen to draw on.
    #[error(transparent)]
    Screen(#[from] ScreenError),

    /// Sensitive local state could not be made owner-only.
    #[error("{file} could not be protected: {source}")]
    Private {
        /// The directory or file whose boundary could not be established.
        file: Box<str>,
        /// What the platform refused.
        source: crucible_privacy::PrivacyError,
    },

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

    /// A renewable token was paired with an API-key endpoint setting.
    #[error(
        "providers.{provider}.baseUrl cannot be used with a subscription login; \
         export an API key to use that address"
    )]
    SubscriptionAddress {
        /// The provider whose fixed subscription audience was selected.
        provider: Box<str>,
    },

    /// Provider construction and source resolution disagreed.
    #[error("no credential is available for {provider}; use /login or set its API key variable")]
    Authentication {
        /// The provider that could not be authenticated.
        provider: Box<str>,
    },

    /// The command line put nothing before the slash.
    #[error("--model needs a provider before the slash, as in --model openai/gpt-5.6-terra")]
    Providerless,

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

    /// A redirected input line would exceed the retained prompt ceiling.
    #[error("what you typed is longer than 1 MiB; no prompt was accepted")]
    InputTooLong,

    /// The operating system could not create the thread that takes a turn.
    #[error("the turn could not start: {0}")]
    Worker(io::Error),

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

/// What the terminal says its background is, where the answer would be used.
///
/// Asked only where a run is going to write colour at all. Colour off is not a
/// slower answer, it is no question: `Palette::resolve` takes `Depth::Off`, the
/// band resolves to nothing, and the reply is discarded — so asking first and
/// discarding after spends the patience out of a twenty-millisecond budget on a
/// value nobody reads. `NO_COLOR` and a redirected run are both that case, and
/// so is the startup probe, which runs with `NO_COLOR` against a pty nothing
/// answers on and would otherwise have paid the whole wait twenty times over.
fn asked(
    settings: &crucible_config::Settings,
    terminal: bool,
    from: &dyn Fn(&str) -> Option<String>,
) -> Option<(u8, u8, u8)> {
    style::writes_colour(settings.color(), terminal, from)
        .then(|| crucible_tui::asked(PATIENCE, from))
        .flatten()
}

/// What the `output` block said, gathered out of the settled layers.
///
/// Its own function because it is the one value in `run` that is only a list:
/// five answers read out of one block, none of them decided here.
fn drawn(settings: &crucible_config::Settings) -> style::Output {
    style::Output {
        color: settings.color(),
        glyphs: settings.glyphs(),
        detail: settings.tool_detail(),
        theme: settings.theme(),
        syntax: settings.syntax_theme().map(str::to_owned),
    }
}

/// Which press the `input` block said sends a prompt.
///
/// The translation from what a document may say to what the editor understands,
/// and the only place the two spellings meet. Nothing said is Return sending,
/// which is what almost every terminal makes possible and every reader expects.
fn sends(settings: &crucible_config::Settings) -> crucible_tui::Sending {
    match settings.sending() {
        Some(crucible_config::Sending::AltEnter) => crucible_tui::Sending::AltEnter,
        Some(crucible_config::Sending::Enter) | None => crucible_tui::Sending::Enter,
    }
}

/// Builds everything, then hands over to the loop.
fn run(cli: &Cli) -> Result<(), Fatal> {
    let here = std::env::current_dir().map_err(Fatal::Here)?;
    let workspace = Workspace::open(here)?;
    let cancel = Cancel::new();

    // Made here rather than beside the tools that share it, because a third
    // thing holds one: `/clear` and `/resume` empty it when they leave the
    // session those files were read in.
    let ledger = Ledger::new();
    let revealed = Revealed::new();

    // And beside it for the same reason again: the tool that writes the plan is
    // one holder, the panel above the box is a second, and `/clear` is the
    // third.
    let plan = Plan::new();

    // Made here for a fourth reason on top of theirs: this is what ends every
    // command left running, and it ends them by being dropped. Held by the
    // outermost scope there is, so the last thing that happens in this process is
    // the processes it started going with it.
    let leaving = Background::new();

    // The other end of the panel a model's questions stand in. One value shared
    // rather than copied, so a question put on the worker thread is one the
    // thread that draws meets on its next frame — the same bargain the plan and
    // the read record are made under.
    let putting = seen::Putting::new();
    let from = |name: &str| std::env::var(name).ok();

    // Where crucible keeps its own files, read from the environment here and
    // handed down as a path — no crate below this one asks where anything is.
    // Then the files themselves, once, before anything that could want them.
    let home = Home::find(&|name| std::env::var_os(name))?;
    protect_user_config(&home)?;
    let settings = Settings::read(&home, workspace.root())?;

    // What was logged in with, read once and from the same directory. A store
    // that cannot be read comes back empty with a sentence rather than as an
    // error, and the sentence is drawn under the welcome: a file that is only
    // ever an alternative to an exported variable must not be what ends a run
    // that never needed it.
    let keys = Store::in_home(home.path()).read();
    let subscriptions = Subscriptions::production();

    // Widened after the files are read because the root is what found them:
    // `.crucible/config.json` is looked for in the directory crucible was
    // started in, so the workspace has to exist before it can be told what else
    // to reach. Once, here, and never again — nothing in a turn may widen it.
    let workspace = workspace.reaching(settings.extra_directories())?;

    // Flags, configuration and the usable credential set are read into one
    // answer before anything is drawn. Neither provider nor model is guessed:
    // a provider chosen without a credential is one whose refusal arrives after
    // the first prompt, and a model chosen without being asked for is one
    // vendor's name sent to whichever vendor the credential belongs to.
    let launch = launch(cli, &settings, &from, &keys, &subscriptions)?;

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

    let mut renderer = Renderer::new(SystemTerminal::stdout());

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
        style: Cell::new(Style::resolve(
            drawn(&settings),
            renderer.is_terminal(),
            // What the terminal says its own background is. Asked once, here,
            // because a palette is settled once and this is what it is settled
            // from — and asked with a short patience because this is the
            // startup path: an answer that arrives after the budget is worth
            // less than the budget is. A terminal that will not say leaves the
            // variable it set at launch, which says which way its ground goes
            // and not what colour it is; that is enough to pick a table and not
            // enough to blend a band off.
            asked(&settings, renderer.is_terminal(), &from),
            crucible_tui::ground::seeded(&from),
            &from,
        )),
        // What the files named, so `/theme` opens with each mark on the row
        // already in force rather than on the first one. `None` here would make
        // a reader who configured a theme look at a panel that says they have
        // not chosen.
        chosen: Cell::new(settings.theme()),
        // Which press sends. Asked rather than worked out: a terminal that
        // keeps Shift and Return for itself reports nothing this program could
        // have read, and the reader is the one who can see that happening.
        sending: sends(&settings),
        reading: RefCell::new(settings.syntax_theme().map(str::to_owned)),
        cancel: cancel.clone(),
        steer: crucible_core::Steer::new(),
        aside: crucible_core::Aside::new(),
        ledger: ledger.clone(),
        revealed: revealed.clone(),
        plan: plan.clone(),
        putting: putting.clone(),
        leaving: leaving.clone(),

        // Who `/model` is choosing for, and where it writes the choice down.
        // The second is a fact about how this run was set up and the prompt is
        // not the place to work it out again; the first is the one thing here a
        // command can change, because `/login` is what fills it in on a machine
        // that started with nothing.
        provider: Cell::new(launch.serving.map(|one| one.name)),
        // Nothing is picked mid-turn at startup: the slot is empty until a
        // `/model` over a running turn fills it.
        pending_model: Cell::new(None),
        // And no mode is stepped to mid-turn: the slot is empty until a
        // shift+tab over a running turn fills it.
        pending_mode: Cell::new(None),
        settings: settings.clone(),
        choosing: crucible_config::user(&home),

        // What `/login` sets a session up with, answered the way this launch
        // answered it for itself — so a credential given at the prompt leaves
        // the session asking what the next run here would ask, rather than what
        // a second reading of the same files happened to say.
        serving: re_serving(settings.clone(), subscriptions.clone()),

        // The two `/resume` reads a directory of logs with. Both are settled
        // here for the same reason everything else in `Terms` is: the session
        // being picked up is one of this directory's, and which directory that
        // is was decided before the first prompt.
        // The same directory the keys above were read from.
        logins: Store::in_home(home.path()),
        // The account logins `/login` can start, the same registry the launch
        // resolved stored subscriptions through.
        subscriptions: subscriptions.clone(),
        sessions: home.sessions().to_owned(),
        workspace: workspace.clone(),
    };

    // Said once, now that the style is settled. It is what decides whether the
    // markers in the model's markdown are read or left where they are.
    renderer.wears(terms.style().palette());

    // And beside it, for the marker the reader above drops rather than reads:
    // a bullet and a quote bar are drawn out of the same set as every border
    // and mark on screen, so a font missing one is missing all of them.
    renderer.draws(terms.style().glyphs());

    // And how far one notch of the wheel moves the transcript. Read here rather
    // than where the wheel is answered, because it is answered on the render
    // path and the render path opens no file — and because a wheel is hardware
    // whose notch means whatever its owner's system has been told it means,
    // which is a thing only its owner can say.
    renderer.rolls(settings.scroll_speed(&from)?.rows());

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
    let asking = settings.updates().unwrap_or_default().wanted();
    let update = asking
        .then(|| release::newer(home.path(), env!("CARGO_PKG_VERSION")))
        .flatten();

    let opening = draw::opening(
        &mut renderer,
        &Opening {
            credential: launch.credential.as_ref(),
            model: launch.model.as_deref(),
            provider: terms.provider.get(),
            unasked: launch.unasked,
            trouble: keys.trouble(),
            workspace: &workspace,
            sessions: &sessions,
            update: update.as_ref(),
            style: terms.style(),
        },
    )?;

    if asking {
        release::refresh(home.path());
    }

    let runner = assemble(&Startup {
        leaving: &leaving,
        provider: launch.serving,
        unasked: launch.unasked,
        model: launch.model.as_deref(),
        effort: launch.effort,
        resuming: resuming(cli)?,
        mode,
        settings: &settings,
        sessions: home.sessions(),
        workspace: &workspace,
        cancel: &cancel,
        ledger: &ledger,
        revealed: &revealed,
        plan: &plan,
        putting: &putting,
        terminal: renderer.is_terminal(),
        from: &from,
        stored: &keys,
        subscriptions: &subscriptions,
    })?;
    let outcome = converse::converse(
        runner,
        &mut renderer,
        &terms,
        &opening,
        &mut io::stdin().lock(),
    );

    drop(held);

    // After every guard the loop was holding has been given back, the screen
    // among them. What this writes goes to the reader's own scrollback rather
    // than to the one the session ran on, which is the only reason it says
    // anything worth keeping.
    draw::parting(&mut renderer, &outcome?, terms.style())?;

    Ok(())
}

/// Protects the user configuration before any value can be read from it.
///
/// Tightens the permission bits of crucible's home directory and the
/// configuration file in it to owner-only; the file's *contents* are never
/// written here. A missing file is the ordinary case — nothing has been
/// configured yet — and anything else the platform refuses ends the run before
/// a secret a wider audience could read is treated as private.
fn protect_user_config(home: &Home) -> Result<(), Fatal> {
    let private = |file: &std::path::Path, source| Fatal::Private {
        file: file.display().to_string().into(),
        source,
    };
    crucible_privacy::directory(home.path()).map_err(|source| private(home.path(), source))?;

    let config = crucible_config::user(home);
    match crucible_privacy::tighten(&config) {
        Ok(_) => Ok(()),
        Err(problem) if problem.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(private(&config, source)),
    }
}

/// What the launch resolved, before anything was drawn.
struct Launch {
    serving: Option<Served>,
    model: Option<Box<str>>,
    effort: Option<Effort>,
    credential: Option<CredentialSource>,
    unasked: &'static str,
}

/// Reads the flags, the files and the usable credential set into one answer.
fn launch(
    cli: &Cli,
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
    credentials: &StoredCredentials,
    subscriptions: &Subscriptions,
) -> Result<Launch, Fatal> {
    let choice = match cli.model.as_deref() {
        Some(named) => Choice::parse(named).ok_or(Fatal::Providerless)?,
        None => Choice::default(),
    };
    let serving = match &choice.provider {
        Some(named) => Some(served(named)?),
        None => chosen(settings, from, credentials, subscriptions)?,
    };
    Ok(Launch {
        model: wanted(&choice, settings, serving),
        effort: thinking(cli.effort, settings, serving),
        credential: serving
            .and_then(|named| credential_source(named, settings, from, credentials, subscriptions)),
        unasked: opening_unasked(
            serving,
            available(settings, from, credentials, subscriptions)
                .next()
                .is_some(),
        ),
        serving,
    })
}

/// The `Serving` `/login` and `/logout` re-resolve a provider through.
///
/// The settings are cloned because the closure outlives every borrow in `run`,
/// and a lifetime on `Terms` would follow it into the signature of everything
/// a command is handed. They are the files this run already read: nothing in
/// them grows with the transcript.
fn re_serving(settings: Settings, subscriptions: Subscriptions) -> Serving {
    Box::new(move |named: Served, stored: &StoredCredentials| {
        let source = credential_source(
            named,
            &settings,
            &|name| std::env::var(name).ok(),
            stored,
            &subscriptions,
        )
        .ok_or_else(|| Fatal::Authentication {
            provider: named.name.into(),
        })?;

        // A provider resolved here is one a credential was just found for, so
        // the sentence the `None` arm refuses with is never reached; it is
        // spelled the way the launch would spell it for this provider anyway.
        Ok(Resolved {
            provider: startup::provider(
                Some(named),
                unasked(Some(named.name)),
                startup::ProviderAuth {
                    settings: &settings,
                    from: &|name| std::env::var(name).ok(),
                    stored,
                    subscriptions: &subscriptions,
                },
            )?,
            source,
        })
    })
}

/// Which provider to ask when the flag named none, or `None` where this machine
/// has nothing set up to ask.
///
/// A credential says a provider can be *reached*. Only a statement about
/// providers chooses one, and `provider` in the configuration is the only
/// statement there is — everything under `providers.<name>` is a subordinate
/// clause about a provider already being asked. The remembered statement is
/// active only while that provider remains reachable. Otherwise the session
/// opens with no provider so `/login` can repair it; falling through to a
/// different credential would send a turn to a vendor nobody chose.
///
/// Below it, exactly one provider holding a credential is that provider. That
/// is not a choice between competitors — it is the absence of anything to
/// choose, which is what lets a first run work with one credential and no
/// configuration at all. A stored account login counts beside an exported
/// variable and a written-down key: it is read through [`credential_source`],
/// the same answer construction would act on, so discovery and construction
/// cannot disagree about what this machine holds.
///
/// Several credentials and nothing choosing between them leaves the provider
/// open rather than failing the launch. `/model` settles both halves
/// explicitly; picking one here would send a turn to a vendor over the
/// declaration order, and refusing to start would strand a machine that is one
/// command away from a choice.
fn chosen(
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
    credentials: &StoredCredentials,
    subscriptions: &Subscriptions,
) -> Result<Option<Served>, Fatal> {
    // Refused here where a name this build has nothing for is a sentence naming
    // the ones it has, rather than carried as "nobody chose" into a session that
    // would then look set up by a credential nobody named.
    if let Some(named) = settings.provider() {
        let one = served(named)?;
        return Ok(
            credential_source(one, settings, from, credentials, subscriptions)
                .is_some()
                .then_some(one),
        );
    }

    let mut holding = available(settings, from, credentials, subscriptions);
    let (Some(first), second) = (holding.next(), holding.next()) else {
        return Ok(None);
    };
    Ok(second.is_none().then_some(first))
}

/// Every provider crucible holds a usable credential for, in declaration order.
///
/// Two places to look and one entry either way. A provider whose credential is
/// both exported and written down is one provider: listed twice it would be two
/// to the question above, and somebody who exported the key they had already
/// logged in with would be asked to choose between a provider and itself.
fn available<'a>(
    settings: &'a Settings,
    from: &'a dyn Fn(&str) -> Option<String>,
    stored: &'a StoredCredentials,
    subscriptions: &'a Subscriptions,
) -> impl Iterator<Item = Served> + 'a {
    PROVIDERS
        .into_iter()
        .filter(move |one| credential_source(*one, settings, from, stored, subscriptions).is_some())
}

/// The source provider construction will select, without reading a secret out.
///
/// The order is the one [`startup::provider`] resolves in, and the two must
/// agree: `/logout` names what remains after a stored credential is removed,
/// and a source this computes that construction would not select is a sentence
/// that lies.
fn credential_source(
    one: Served,
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
    stored: &StoredCredentials,
    subscriptions: &Subscriptions,
) -> Option<CredentialSource> {
    if settings.base_url(one.name).is_none()
        && subscriptions.supports(one.name)
        && stored.has_subscription(one.name)
    {
        return Some(CredentialSource::Subscription);
    }
    let variable = settings.api_key_env(one.name).unwrap_or(one.key);
    if from(variable).is_some_and(|value| !value.trim().is_empty()) {
        return Some(CredentialSource::Environment(variable.into()));
    }
    if stored.has_key(one.name) {
        return Some(CredentialSource::StoredKey);
    }
    None
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

/// Which earlier session the command line asked for, parsed at the boundary.
///
/// An identifier that does not parse names no session anywhere, so it gets the
/// same sentence an unknown one does rather than a parser's complaint: either
/// way, nothing recorded here answers to it.
fn resuming(cli: &Cli) -> Result<startup::Resuming, Fatal> {
    use std::str::FromStr as _;

    match &cli.resume {
        Some(text) => SessionId::from_str(text)
            .map(startup::Resuming::Exact)
            .map_err(|_| Fatal::NoSession(text.as_str().into())),
        None if cli.r#continue => Ok(startup::Resuming::Newest),
        None => Ok(startup::Resuming::No),
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

#[cfg(test)]
mod tests;
