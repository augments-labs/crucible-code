//! Argument parsing, and what the arguments and the files together decided.
//!
//! The command line is read here and resolved against the configuration. What
//! those answers choose is then built in two halves: the terminal, the renderer
//! and the terms every turn runs under are put together here, because they are
//! what a failure has to be reported through; the provider, the tools and the
//! session are put together by the application crate, in
//! [`crucible_app::startup`], which hands back the conversation the loop then
//! drives. Everything below is reached as a trait object, which is what leaves
//! every crate free of the others.
//!
//! Nothing above this file knows what an HTTP client is, and nothing below it
//! knows what the command line said.

mod browser;
mod choice;
mod converse;
mod counting;
mod draw;
#[cfg(test)]
mod fake;
mod gathering;
mod kept;
mod release;
#[cfg(test)]
mod sample;
mod seen;
mod standing;
mod style;

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::io::{self, Write as _};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use crucible_app::AppError;
use crucible_app::providers::{
    Providers, Served, available, chosen, opening_unasked, providers, re_serving,
};
use crucible_app::startup::{self, Startup, assemble, served};
use crucible_app::subscription::Subscriptions;
use crucible_auth::Store;
use crucible_builtins::{Background, Ledger, Plan};
use crucible_config::{Home, Settings};
use crucible_core::{Cancel, Effort, Revealed, SessionId, Workspace};
use crucible_tui::{
    RawError, Renderer, ScreenError, SystemTerminal, TerminalError, Title, TitleError, Welcome,
};

use crate::cli::choice::Choice;
use crate::cli::converse::Terms;
use crate::cli::draw::Opening;
use crate::cli::style::Style;

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
usable credential — a key in one of ANTHROPIC_API_KEY, GEMINI_API_KEY, MOONSHOT_API_KEY and \
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

--extensions lists what is installed in ~/.crucible/extensions, with what each \
manifest asks to be allowed to do and the digest crucible took over its bytes, \
and stops. Nothing installed is run to produce that list, which is the point of \
being able to read it.

--sandbox prints the confinement a command in this directory would run under — \
which backend enforces it, what that backend can and cannot hold, the reach and \
ceilings a command would get, and anything given up along the way — and stops. \
No command is run to produce it, and every path in it is a digest.

--with-mcp names a server written down under mcp.servers and hosts it for this \
run, and may be repeated. A configuration file is a list of servers you could \
run; nothing is started until a run names one. What a hosted server offers is \
called as mcp:<server>/<tool>, and it runs confined the way a command does.

Flags, session files and config are unstable for the whole 0.x line.",
    args_conflicts_with_subcommands = true
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

    /// Host an MCP server written down under mcp.servers, by name. Repeat it
    /// for each one. Nothing is hosted unless it is named here.
    #[arg(long = "with-mcp", value_name = "NAME")]
    with_mcp: Vec<String>,

    /// List what is installed in crucible's extensions directory and stop,
    /// without running any of it.
    #[arg(
        long,
        conflicts_with_all = ["continue", "resume", "model", "effort", "with_mcp"]
    )]
    extensions: bool,

    /// Print the confinement a command in this directory would run under and
    /// stop, without running one.
    #[arg(
        long,
        conflicts_with_all = ["continue", "resume", "model", "effort", "with_mcp", "extensions"]
    )]
    sandbox: bool,

    /// Explicit one-time operating-system maintenance commands.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Provision or remove native sandbox support.
    Sandbox {
        #[command(subcommand)]
        action: SandboxMaintenance,
    },
}

#[derive(Debug, Subcommand)]
enum SandboxMaintenance {
    /// Provision or repair the native Windows sandbox. Run once from an
    /// Administrator PowerShell; ordinary Crucible runs stay unelevated.
    Setup {
        /// User account to provision. Left off, it is the elevated process
        /// owner.
        #[arg(long, value_name = "ACCOUNT")]
        owner: Option<OsString>,
    },
    /// Remove the native Windows account, network policy, and setup record.
    Uninstall {
        /// User account whose setup is removed. Left off, it is the elevated
        /// process owner.
        #[arg(long, value_name = "ACCOUNT")]
        owner: Option<OsString>,
    },
}

/// Why crucible could not run, or could not carry on.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Fatal {
    /// The directory crucible was started in could not be read.
    #[error("the directory crucible was started in could not be read: {0}")]
    Here(io::Error),

    /// The run could not be assembled, or a session could not carry on: the
    /// application's own sentence, said as it wrote it.
    #[error(transparent)]
    App(#[from] AppError),

    /// Native sandbox provisioning or removal could not complete.
    #[error("Windows sandbox maintenance failed: {0}")]
    SandboxMaintenance(io::Error),

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

/// Says a failure the command line met on its own way in the application's
/// words, so one failure has one sentence whichever side of the boundary
/// reached it first.
macro_rules! through_the_application {
    ($($problem:ty),+ $(,)?) => {$(
        impl From<$problem> for Fatal {
            fn from(problem: $problem) -> Self {
                Self::App(problem.into())
            }
        }
    )+};
}

through_the_application!(
    crucible_core::PathError,
    crucible_config::ConfigError,
    crucible_core::RegistryError,
    crucible_session::SessionError,
    crucible_app::providers::ArmError,
);

/// Reads the command line and does what it says.
pub(crate) fn start() -> ExitCode {
    let cli = Cli::parse();

    let done = match (&cli.command, cli.extensions, cli.sandbox) {
        (Some(Command::Sandbox { action }), _, _) => maintain_sandbox(action),
        (None, true, _) => listed(),
        (None, _, true) => confined(),
        (None, _, _) => run(&cli),
    };

    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => fail(&problem),
    }
}

#[cfg(target_os = "windows")]
fn maintain_sandbox(action: &SandboxMaintenance) -> Result<(), Fatal> {
    let result = match action {
        SandboxMaintenance::Setup { owner } => {
            crucible_sandbox_broker::setup_windows_sandbox(owner.as_deref())
        }
        SandboxMaintenance::Uninstall { owner } => {
            crucible_sandbox_broker::uninstall_windows_sandbox(owner.as_deref())
        }
    };
    let message = result.map_err(Fatal::SandboxMaintenance)?;
    let _ = writeln!(io::stdout().lock(), "{message}");
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn maintain_sandbox(_action: &SandboxMaintenance) -> Result<(), Fatal> {
    Err(Fatal::SandboxMaintenance(io::Error::new(
        io::ErrorKind::Unsupported,
        "the native Windows sandbox is available only on Windows",
    )))
}

/// Writes what is installed to standard output, and stops.
///
/// Answered here rather than inside [`run`] so that it is answered before
/// anything is built: the flag exists so somebody can read what crucible found
/// *before* deciding whether any of it should ever run, and a listing that had
/// opened a workspace, read a credential or started a session on the way would
/// be a poor thing to reach for when an extension is the suspect.
///
/// A write that fails is dropped the way [`fail`] drops one. Standard output
/// closing early is a `head` on the other end of a pipe, and there is nothing
/// left of this run to report it to.
fn listed() -> Result<(), Fatal> {
    let home = Home::find(&|name| std::env::var_os(name))?;
    let found = crucible_app::extensions::installed(&home, env!("CARGO_PKG_VERSION"))?;

    let _ = io::stdout().write_all(found.as_bytes());
    Ok(())
}

/// Writes the confinement a command here would run under, and stops.
///
/// What the report is made of, and why a backend's refusal is an answer rather
/// than a failure, is [`crucible_app::sandbox::confinement`]'s to say. A write
/// that fails is dropped for the reason [`listed`] drops one.
fn confined() -> Result<(), Fatal> {
    let here = std::env::current_dir().map_err(Fatal::Here)?;
    let home = Home::find(&|name| std::env::var_os(name))?;
    let said = crucible_app::sandbox::confinement(&here, &home)?;

    let _ = io::stdout().write_all(said.as_bytes());
    Ok(())
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
    startup::protected(&home)?;
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
    let providers = providers()?;
    let launch = launch(
        cli,
        &providers.snapshot(),
        startup::ProviderAuth {
            settings: &settings,
            from: &from,
            stored: &keys,
            subscriptions: &subscriptions,
        },
    )?;

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
        commands: converse::command::builtins(&settings.sandbox().enablement())?,
        providers,
        reading: RefCell::new(settings.syntax_theme().map(str::to_owned)),
        cancel: cancel.clone(),
        steer: crucible_core::Steer::new(),
        aside: crucible_core::Aside::new(),
        ledger: ledger.clone(),
        revealed: revealed.clone(),
        plan: plan.clone(),
        putting: putting.clone(),
        leaving: leaving.clone(),

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
        serving: re_serving(
            settings.clone(),
            subscriptions.clone(),
            Box::new(|name| std::env::var(name).ok()),
        ),

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

    // And which repository a number in the answer is counted against, so `#487`
    // is somewhere the reader can go rather than four characters to carry to a
    // browser by hand. Read here for the reason the branch is: it comes out of
    // a file in the checkout, the checkout does not change while the session
    // runs, and a session with no forge behind it simply draws what it drew
    // before.
    renderer.counts(counting::forge(workspace.root()));

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
    let sessions = crucible_session::recent(home.sessions(), &workspace, Welcome::WANTED);

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
            model: launch.model.as_deref(),
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

    // The generation the launch above read its provider out of, taken once so
    // the arm that was resolved and the model record its limits come from are
    // the same generation. A registration committed after this point is for the
    // next reader to see, not for a runner already built.
    let catalogue = terms.providers.snapshot();
    let conversation = assemble(&Startup {
        leaving: &leaving,
        providers: &catalogue,
        provider: launch.serving,
        unasked: launch.unasked,
        model: launch.model.as_deref(),
        effort: launch.effort,
        resuming: resuming(cli)?,
        mode,
        settings: &settings,
        sessions: home.sessions(),
        workspace: &workspace,
        ledger: &ledger,
        revealed: &revealed,
        plan: &plan,
        asking: std::sync::Arc::new(putting.clone()),
        hosting: &cli.with_mcp,
        terminal: renderer.is_terminal(),
        from: &from,
        stored: &keys,
        subscriptions: &subscriptions,
    })?
    .remembering_caches_in(home.path());
    let outcome = converse::converse(
        conversation,
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

/// What the launch resolved, before anything was drawn.
struct Launch {
    serving: Option<Served>,
    model: Option<Box<str>>,
    effort: Option<Effort>,
    unasked: &'static str,
}

/// Reads the flags, the files and the usable credential set into one answer.
fn launch(
    cli: &Cli,
    providers: &Providers,
    auth: startup::ProviderAuth<'_>,
) -> Result<Launch, Fatal> {
    let choice = match cli.model.as_deref() {
        Some(named) => Choice::parse(named).ok_or(Fatal::Providerless)?,
        None => Choice::default(),
    };
    let serving = match &choice.provider {
        Some(named) => Some(served(providers, named)?),
        None => chosen(providers, auth)?,
    };
    Ok(Launch {
        model: wanted(&choice, auth.settings, serving),
        effort: thinking(cli.effort, auth.settings, serving),
        unasked: opening_unasked(serving, available(providers, auth).next().is_some()),
        serving,
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
            .map_err(|_| AppError::NoSession(text.as_str().into()).into()),
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
