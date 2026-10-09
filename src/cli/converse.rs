//! The loop: read a line, take a turn, draw what the turn does.
//!
//! The turn runs as a task on the application's runtime and the terminal stays
//! with this thread. That split is the whole reason a turn can stream while a
//! question is waiting to be answered, and it is why no lock appears anywhere
//! on the render path: the only thread that writes to the terminal is the one
//! running this loop.
//!
//! Raw mode is held for the whole session rather than for each prompt, because
//! the box takes typing while a turn runs: the keyboard cannot be handed back
//! between turns if somebody is still writing in one. So this loop reads keys
//! and the worker's events together — a short wait on the channel, then a look
//! at whatever the keyboard already has, round and round — and a permission
//! question is answered by a key rather than by a line the terminal collected.
//!
//! Two things follow from holding it. The keys that would otherwise be the
//! terminal's arrive here as keys, so this loop is the only thing that can act
//! on them: Esc asks a running turn to stop, and Ctrl-C throws away the line and
//! offers to leave an empty one, mid turn exactly as between turns. And a
//! session with no terminal at either end holds nothing at all and reads whole
//! lines, which is the path every test drives.
//!
//! A turn can also be told to stop from outside the keyboard: a closed window
//! or a `kill` is noted by [`super::ending`] while a turn runs, and this loop
//! reads the note on the same round it reads keys. It stops the turn the way
//! Esc would, and hands back [`Fatal::Ended`] the way a terminal that failed
//! is handed back. Every turn that ends in an error has its session finished
//! here, before the error leaves, so what was said is on the disk whoever else
//! still holds the session. Between turns the same note calls off a wait on
//! the keyboard instead, so that the guards below hand the terminal back
//! before the signal is obeyed, and [`converse`] reads it once they have.
//!
//! The session log is append-only and written as the turn goes, so `--continue`
//! picks the session up from wherever it stopped.
//!
//! Which is also the last thing a session does. A full screen is borrowed and
//! handed back, so the transcript goes with it; a native session leaves it in
//! the reader's scrollback. Either way this loop returns a [`Parting`] saying
//! how to come back to the session, and on a borrowed screen where the log is
//! and whether it kept up, for the caller to report once the terminal is the
//! reader's again.

use std::cell::{Cell, RefCell};
use std::io::BufRead;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use crucible_app::Conversation;
use crucible_app::client::Ended;
use crucible_app::providers::{Lookup, Served, Serving, Sourcing, available};
use crucible_app::startup::ProviderAuth;
use crucible_app::subscription::Subscriptions;
use crucible_auth::Store;
use crucible_builtins::{Background, Ledger, Plan};
use crucible_client_api::{Command, ErrorCode, Prompt, Refusal};
use crucible_context::Room;
use crucible_runner::{Event, Runner, TurnError, Turned};
use crucible_runtime::Cancel;
use crucible_session::Session;
use crucible_tui::{
    Editor, Pasting, Raw, Renderer, Reporting, Screen, ScreenMode, Sending, Spelling, Terminal,
    TerminalError,
};
use crucible_types::{Attachment, Compacting, SessionId, Spend};

use super::draw;
use super::gathering::Gathering;
use super::kept::Kept;
use super::panicked::Panics;
use super::seen::{Asking, CAPACITY, Inbox, Putting, Relay, Seen};
use super::style::Style;
use super::{Fatal, standing};
use answering::{Answers, asked, cramped, read, verdict};
use command::Ran;
use expanding::Standing;
pub(crate) use first::First;
use planning::Planning;
use queueing::{Prompts, Retained};
use recalling::Recalling;
use turning::Turning;
use typing::{Asked, Says};

mod answering;
mod asking;
mod attaching;
pub(crate) mod command;
mod expanding;
mod finding;
mod first;
mod leaving;
mod mode;
mod picking;
mod planning;
mod putting;
mod queueing;
mod recalling;
mod region;
mod replaying;
mod resuming;
mod secret;
mod turning;
mod typing;
mod warning;

/// How long the loop waits on the turn before looking at the keyboard.
///
/// A wake-up rate rather than a spin: the thread is parked in `recv_timeout`
/// for all of it. Short enough that a keystroke appears at once — well inside
/// what a hand notices — and long enough that a turn producing nothing costs
/// sixty wake-ups a second and no work in any of them.
const TICK: Duration = Duration::from_millis(16);

/// How many finished prompts may wait behind a running turn.
const QUEUED_LINES: usize = 64;

/// How many prompt bytes may wait behind a running turn.
///
/// The editor's own ceiling: one prompt is bounded wherever it is taken, so a
/// queue of them is bounded twice over — once here in bytes, once above in
/// lines.
const QUEUED_BYTES: usize = Editor::MAX_BYTES;

/// What every turn in a conversation is taken under.
///
/// Almost all of these are settled before the first prompt and never change:
/// the style comes from the files and the terminal together, and the cancel is
/// the same one the tools were built with. One value rather than three
/// parameters carried down through every turn.
///
/// Which provider is being asked is the exception, and `/login` is why: a run
/// that started with no key for anything is one command away from having one,
/// and what is written down afterwards has to go under the name that key was
/// for.
///
/// The mode is not among them at all. It is the other thing about a session that
/// changes after it has started, so it is read from the engine that holds it
/// every time it is drawn rather than copied here and kept in step.
pub(crate) struct Terms {
    /// Whether to write colour, which characters to draw with, how much of a
    /// tool call to show, and which table of colours to draw with.
    ///
    /// In a cell because two commands change it: `/theme` picks a different
    /// table, and `/settings` a different table, glyph set or tool detail, and
    /// everything drawn after either is drawn with what it chose. Settled once
    /// at startup and again only when somebody says so — never per event, which
    /// is the thing `Style`'s own module doc is about.
    pub(crate) style: Cell<Style>,
    /// Which theme the files named, or `/theme` last took. `None` where no
    /// layer said — which is not the same as `auto`, and is why the panel marks
    /// nothing rather than marking the row `auto` happens to have resolved to.
    pub(crate) chosen: Cell<Option<crucible_config::ThemeChoice>>,
    /// Which syntax theme fenced code is read in, where a layer named one or
    /// `/theme` took one. `None` is "nothing said", and the first fence settles
    /// on whatever this build draws code in unless somebody says otherwise.
    pub(crate) reading: RefCell<Option<String>>,
    /// What `/settings` has written down this session, by key, one entry a
    /// row: the settings above were read at the start and are not read again,
    /// so a row reopened shows what was taken rather than what was there.
    pub(crate) settled: RefCell<Vec<(&'static str, String)>>,
    /// What stops a turn.
    pub(crate) cancel: Cancel,
    /// The application's runtime, which a turn runs on as a task and a
    /// command is waited for on.
    ///
    /// The drawing thread is never inside it, which is what lets this thread
    /// wait on it: for a command's answer, for the backend the sandbox panel
    /// shows, and for a turn that has ended to hand the conversation back.
    pub(crate) runtime: tokio::runtime::Handle,
    /// What the process has been told from outside it, which stops a turn too
    /// and then the run.
    pub(crate) ending: super::ending::Ending,
    /// What a line typed while a turn runs is pushed into, and the turn draws
    /// from between one pass and the next. Held for the session the way the
    /// cancel is: it is made once beside it, and the turn's thread and the loop
    /// that reads the keyboard each hold an end.
    pub(crate) steer: crucible_runtime::Steer,
    /// What a fact the session learned mid-turn is pushed into, and the turn
    /// draws from at the same boundary it draws steering from.
    ///
    /// Held beside the steer for the same reason, and separate from it for a
    /// different one: what goes in here is the harness reporting something,
    /// not the reader asking for something. A command left running that has
    /// exited is the only thing that goes in it today, and the agent was told
    /// not to poll for that — so this is the channel that makes the promise
    /// true.
    pub(crate) aside: crucible_runtime::Aside,
    /// Which files this session has read, which is what `write` asks before it
    /// replaces one.
    ///
    /// Held for the same reason the cancel is: it is made once, beside the
    /// tools that share it, and a command reaches for the same value they were
    /// built with. `/clear` and `/resume` both leave the session those files
    /// were read in, and a record that outlived its session would let `write`
    /// replace a file the session in hand never saw.
    pub(crate) ledger: Ledger,
    /// Which deferred tools this session has looked up. `/clear` empties it for
    /// the reason it empties the plan: what it would otherwise leave is a model
    /// holding tools this conversation never asked for.
    pub(crate) revealed: crucible_tools::Revealed,
    /// Where a tool's questions reach the thread that draws them.
    ///
    /// Held for the reason the ledger and the plan are: it is made once, beside
    /// the tool that was built with it, and the loop holds the other end. What
    /// it lends changes every turn; what holds it does not.
    pub(crate) putting: Putting,
    /// The requests this front end makes of the application, numbered.
    pub(crate) client: super::client::Client,
    /// The plan the agent is working to, which is what stands above the box.
    ///
    /// Held for the same reason the ledger is, and emptied by the same command:
    /// the plan belongs to the session it was written in, and one that outlived
    /// its session would be a panel above the prompt describing work the agent
    /// on the other side of it has no memory of.
    pub(crate) plan: Plan,
    /// Every command left running, which the row under the box counts and the
    /// panel behind it lists.
    ///
    /// Held for the reason the two above are, and emptied by nothing: a running
    /// dev server is a fact about the machine rather than about the context, so
    /// `/clear` leaves it alone where it empties the record and the plan. What
    /// ends these is the run ending.
    pub(crate) leaving: Background,
    /// A model picked mid-turn, held for the turn the loop starts next.
    ///
    /// The runner is on the worker for the running turn's length, so a pick
    /// made then cannot reach it — it is held here and applied as that turn
    /// ends, when the runner is this side's again, so the row between turns
    /// names the model the next one is asked under.
    pub(crate) pending_model: Cell<Option<(Served, String)>>,
    /// A speed taken off `/fast` mid-turn, held the same way.
    pub(crate) pending_speed: Cell<Option<crucible_models::Speed>>,
    /// A mode shift+tab stepped to mid-turn, held for the turn the loop starts
    /// next.
    ///
    /// The runner holding the mode is on the worker for a running turn's
    /// length, so a step made then cannot reach it — it is held here and put
    /// on the runner as that turn ends, when the runner is this side's again.
    /// The row under the box says the step at once, so the press is not dead;
    /// the running turn keeps the mode it began under, and between turns the
    /// row is the runner's own mode again, which a step made there moves from.
    pub(crate) pending_mode: Cell<Option<crucible_tools::Mode>>,
    /// The settled configuration model limits are read from. Kept in memory so
    /// `/model` resolves a new name exactly as startup did without touching a
    /// file on the command path.
    pub(crate) settings: crucible_config::Settings,
    /// The file at home that `/model` writes its answer into. A model is a fact
    /// about who is running crucible rather than about the checkout, so it is
    /// not a project configuration file.
    pub(crate) choosing: PathBuf,
    /// Where a key given to `/login` is written down. Built by the caller from
    /// the same home directory the launch read its keys out of: a store built
    /// here and one built there pointing at different files would be a `/login`
    /// that wrote where nothing reads.
    pub(crate) logins: Store,
    /// Subscription implementations compiled into this binary.
    pub(crate) subscriptions: Subscriptions,
    /// The yes given to each route whose vendor uses what is sent, which the
    /// clients this run sends through ask before a request leaves.
    pub(crate) consent: crucible_app::content_use::Consent,
    /// Sets a provider up the way the launch set this run's up.
    ///
    /// `/login` is what calls it, handing back the keys it just wrote — so what
    /// the session asks from the next turn is what the next run here would ask,
    /// resolved once and out of the same files.
    pub(crate) serving: Serving,
    /// Builds the web sources again for the provider and model a switch takes,
    /// from the credential that provider is set up with.
    pub(crate) sourcing: Sourcing,
    /// Reads a variable from the environment the launch was started in, which
    /// is where a key can be exported rather than stored.
    ///
    /// Handed in beside [`serving`](Self::serving), which reads the same one,
    /// so a session with no environment to offer can say so.
    pub(crate) environment: Lookup,
    /// Where this machine keeps its session logs.
    pub(crate) sessions: PathBuf,
    /// The directory this conversation is about, which is what decides whose
    /// sessions are listed and which of them may be picked up.
    pub(crate) workspace: crucible_workspace::Workspace,
    /// Which press finishes a prompt, and which one opens a line under it.
    ///
    /// Read at startup and again when `/settings` changes it, which is why it
    /// is a `Cell`: the panel is handed these terms and not the editor, so
    /// the loop hands the editor this answer whenever a command returns. Not
    /// part of the style — it is about what arrives from the terminal rather
    /// than about what is drawn to it.
    pub(crate) sending: Cell<Sending>,
    /// How long a running tool call is out before it stands over the row that
    /// says a turn is running, from `output.pinAfterSeconds`.
    ///
    /// A `Cell` for the reason [`sending`](Self::sending) is: `/settings`
    /// changes it, and each turn reads it as it starts.
    pub(crate) pinning: Cell<Duration>,
    /// The commands a `/` line is read against.
    ///
    /// A registry rather than the list itself, because what is in it is a
    /// generation: the built-ins at startup, and whatever is committed beside
    /// them later. A line is read against the snapshot taken as it is read, so
    /// a name it resolves is one that was in force when it was typed.
    pub(crate) commands: crucible_registry::Registry<command::Slash>,
    /// The providers a name is read against, from `--model provider/…` to the
    /// rows `/login` and `/model` draw.
    ///
    /// A registry for the reason the commands are: the built-ins at startup,
    /// and whatever is committed beside them later. Each reader takes its own
    /// snapshot, so a provider it names is one that was in force when it asked.
    pub(crate) providers: crucible_registry::Registry<crucible_app::providers::Arm>,
}

impl Terms {
    /// What the terminal is drawn with right now.
    ///
    /// A method rather than a field read because one command changes it, and
    /// every caller wants whatever is in force at the moment it draws rather
    /// than whatever was in force when the session opened.
    pub(crate) fn style(&self) -> Style {
        self.style.get()
    }

    /// What a change of who is asked is decided from, read against one
    /// generation of the providers.
    ///
    /// The generation is the caller's, so that the rows it draws and the
    /// switch it asks for are read against the same one.
    pub(crate) fn switching<'a>(
        &'a self,
        providers: &'a crucible_app::providers::Providers,
    ) -> crucible_app::switching::Switching<'a> {
        crucible_app::switching::Switching {
            providers,
            settings: &self.settings,
            serving: &self.serving,
            sourcing: &self.sourcing,
            logins: &self.logins,
            choosing: &self.choosing,
        }
    }

    /// What a session with no model says, read off what this machine holds now.
    ///
    /// Now rather than at the launch, because a `/logout` can take the last key
    /// away or leave another: the welcome said it for the credentials there
    /// were then, and a prompt saying it again says it for the ones there are.
    pub(crate) fn unasked(&self, provider: Option<&str>) -> &'static str {
        // With a provider chosen the missing piece is the model whatever else
        // is set up, so the credentials are read only when they decide it.
        let any = provider.is_none() && {
            let stored = self.logins.read();
            let auth = ProviderAuth {
                settings: &self.settings,
                from: &*self.environment,
                stored: &stored,
                subscriptions: &self.subscriptions,
            };
            available(&self.providers.snapshot(), auth).next().is_some()
        };
        crucible_app::providers::unasked(provider, any)
    }
}

/// What a session leaves on the reader's own screen once it has gone.
///
/// A full-screen session draws on a screen this process borrows and hands
/// back, so what a reader scrolls up to afterwards is the shell they started
/// from, and this says where the rest of it went. A native session drew in the
/// reader's own buffer, so its transcript is still there to scroll to, and
/// this says only how to come back to it.
///
/// Decided in [`converse`], because that is the last place the session still
/// exists, and written by the caller, because the screen is the last guard to
/// be given back and nothing may reach the reader's own until it has.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Parting {
    /// Say nothing.
    ///
    /// Either the session's input or output was not a terminal, so it ran
    /// with nobody at the keys to tell, or nothing was recorded, which is a run that asked not to be kept and
    /// has no session to come back to.
    Nothing,

    /// The transcript is still in the reader's scrollback, and this file is
    /// the session it came from. Whatever was said about the log while the
    /// session ran is in that scrollback too, so only the way back is said.
    Stayed(PathBuf),

    /// The transcript went with the screen, and this file holds all of it.
    Kept(PathBuf),

    /// The transcript went with the screen, and this file holds the part of it
    /// that reached the disk before the log stopped recording.
    Lost(PathBuf),
}

impl Parting {
    /// What a session that has just ended leaves behind.
    ///
    /// `drew` is where the session drew, `written` is the file the session
    /// was recorded to and is absent in a run that asked not to be kept, and
    /// `problem` is the first write to that file that failed.
    ///
    /// A function of three values rather than three reads at the end of the
    /// loop, because two of them are only ever true on a real terminal: this is
    /// the whole of the decision, and it can be asked without one.
    fn of(drew: Drew, written: Option<PathBuf>, problem: Option<&str>) -> Self {
        let Some(path) = written else {
            return Self::Nothing;
        };
        match drew {
            Drew::Borrowed if problem.is_none() => Self::Kept(path),
            Drew::Borrowed => Self::Lost(path),
            Drew::Scrollback => Self::Stayed(path),
            Drew::Nowhere => Self::Nothing,
        }
    }
}

/// Where a session drew, which is what decides what leaving it has to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drew {
    /// On the alternate screen, which is handed back with the transcript on it.
    Borrowed,
    /// In the reader's own buffer, where the transcript stays.
    Scrollback,
    /// Nowhere, because the session's input or output was not a terminal.
    Nowhere,
}

/// Which of the terminal's modes a session takes, given where it draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Holds {
    /// The alternate screen.
    screen: bool,
    /// Reports of the pointer's buttons, motion and wheel.
    pointer: bool,
}

/// What a session drawing in `mode` takes from the terminal.
///
/// Native mode takes neither: its point is the reader's own buffer, whose
/// scrollback the alternate screen would hide and whose selection and wheel
/// taking the pointer would take.
fn holds(mode: ScreenMode) -> Holds {
    match mode {
        ScreenMode::Fullscreen => Holds {
            screen: true,
            pointer: true,
        },
        ScreenMode::Native => Holds {
            screen: false,
            pointer: false,
        },
    }
}

/// Closes a renderer's live region when dropped.
///
/// A region left open would be closed by the renderer's own drop, after the
/// session has given the terminal back — rewinding over whatever was written
/// below it by then.
struct Closing<'a, T: Terminal>(&'a mut Renderer<T>);

impl<T: Terminal> Deref for Closing<'_, T> {
    type Target = Renderer<T>;

    fn deref(&self) -> &Renderer<T> {
        self.0
    }
}

impl<T: Terminal> DerefMut for Closing<'_, T> {
    fn deref_mut(&mut self) -> &mut Renderer<T> {
        self.0
    }
}

impl<T: Terminal> Drop for Closing<'_, T> {
    fn drop(&mut self) {
        // Nowhere to report it: the terminal that refused is the one a report
        // would be written to.
        let _ = self.0.closes();
    }
}

/// Reads prompts and takes turns until input ends.
///
/// `input` is standard input in a real run. It is a parameter so that a test
/// can drive the loop: the deadlock this file has to avoid is one that only
/// shows up when a whole turn runs, and a hardwired stdin makes that unrunnable.
///
/// A signal noted while the keyboard was waited on between turns is read here,
/// once every guard the session held has handed back what it held, and
/// outranks whatever the session ended with, as it does inside a turn.
pub(crate) fn converse<T: Terminal>(
    conversation: Conversation,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    first: First<'_>,
    input: &mut dyn BufRead,
) -> Result<Parting, Fatal> {
    let conversed = conversing(conversation, renderer, terms, first, input);
    match terms.ending.told() {
        Some(told) => Err(Fatal::Ended(told)),
        None => conversed,
    }
}

/// The session itself, behind [`converse`].
fn conversing<T: Terminal>(
    mut conversation: Conversation,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    first: First<'_>,
    input: &mut dyn BufRead,
) -> Result<Parting, Fatal> {
    // Named once here because the prompt asks for it every frame: it is what the
    // row under the box counts, and what that loop wakes on a clock for while
    // there is anything left to end.
    let left = &terms.leaving;

    // Named before every guard, so that it is the last thing given back: what
    // panicked on another thread while the terminal was held, and was never
    // said on it, is written out once the screen is the reader's own again.
    // Taken only once raw mode is, below; a session that holds no terminal
    // lets a panic be written the way it always is.
    let mut panics = None;

    // First of the guards, so that it is the last of them given back: raw mode
    // is left while this is still held, and the sequence that leaves it goes to
    // the screen that is about to stop existing rather than to the reader's own.
    //
    // Taken here rather than where the renderer was built, because everything
    // between the two can still refuse to start a session — an unreadable
    // configuration, a provider nobody named, a home directory that would not
    // be made private — and a refusal written to a screen that is handed back
    // in the same breath is one nobody reads.
    let holds = holds(renderer.screen());
    let screen = if holds.screen { Screen::take()? } else { None };

    // Whether the transcript is about to be taken away with the screen it was
    // drawn on, which with the keys below is the whole of what decides what
    // there is to say on the way out. Read here rather than at the end, because
    // it is a fact about the start of the session and the binding above
    // outlives the answer.
    let borrowed = screen.is_some();

    // Held for the whole session and dropped on the way out however this
    // returns. Between turns it is what draws the box; during one it is what
    // lets the box go on being typed into. `None` is a session with no terminal
    // at one end or the other, which reads whole lines instead.
    let raw = Raw::enter()?;
    let keys = raw.is_some();

    // Raw mode is held only where both ends are a terminal, as the screen is
    // taken: a native session holding it drew in the reader's own buffer, and
    // a run without it was not watched by anyone to tell on the way out.
    let drew = if borrowed {
        Drew::Borrowed
    } else if keys {
        Drew::Scrollback
    } else {
        Drew::Nowhere
    };
    if keys {
        panics = Some(Panics::kept());
        // A signal between turns is obeyed where it lands, and that would be
        // with the keys still raw. While the keyboard is waited on, it is
        // noted instead, and the wait called off, so the guards above hand
        // the terminal back before the signal is obeyed.
        if let Some(recall) = terms.ending.recall() {
            renderer.recalled_by(recall);
        }
    }

    // Held the same way and for the same length, and asked for unconditionally
    // rather than from a setting: the older key encoding has no room for the
    // modifier on Shift+Return, so without this the editor's answer to it is
    // never reached. A terminal that does not implement the newer spelling
    // discards the request and loses nothing, which is why there is no
    // capability to consult — and why there is no query either, since an answer
    // would arrive in the queue the prompt is about to read keys from.
    let _spelling = Spelling::distinct()?;

    // And the other half of what the old encoding cannot carry. Pasted text is
    // just bytes, so every line break in it is the byte Return sends: without
    // this, pasting three lines into the box sends the first as a turn and
    // leaves the other two typed into the next prompt. Bracketed, the block
    // arrives whole and its newlines stay newlines.
    let _pasting = Pasting::bracketed()?;

    // And the pointer, for the whole session rather than for as long as
    // something stands. The wheel is what scrolls the transcript, and the
    // transcript is on screen the whole time — a pointer taken only while a
    // list is up would be a wheel that works in the one place a reader is least
    // likely to reach for it.
    //
    // The drag is answered here too, for the same reason: a terminal
    // forwarding buttons is not using them itself, so a selection has to be
    // this program's or nobody's. Shift is still the way past a program
    // holding the pointer, and stays the answer for a reader who wanted their
    // emulator's own selection instead of this one — or native mode, which
    // leaves the pointer to the terminal altogether.
    let _pointer = if holds.pointer {
        Reporting::on()?
    } else {
        None
    };

    // Last of the guards, so that it is the first given back: in the
    // terminal's own buffer the live region is closed however this returns —
    // by a quit, by an error, or unwinding — while the modes are still held,
    // and before anything kept for the way out is written below it. Nothing
    // on a screen of crucible's own.
    let mut closing = Closing(renderer);
    let renderer = &mut *closing;

    // Everything the session keeps between turns and hands to each of them:
    // the line being typed, the lines finished behind it, what a result had no
    // room to say, the view over it, the plan, and where an answer comes from.
    // Held in one value for the reason its own prose gives.
    let mut held = Held::new(
        terms.plan.clone(),
        terms.sending.get(),
        Answers { input, keys },
        first.card,
    );

    // What this directory has been asked before, read once here rather than at
    // the first arrow. It is one small file and this is before the first frame
    // either way; reading it under the key would put a disk between a press and
    // the line it puts in the box.
    held.recalling = Recalling::new(terms.sessions.clone(), terms.workspace.clone());

    // The opening is the first thing in the transcript, which is where a
    // reader scrolls back to find it. Written down rather than stood over the
    // box: the band it lands in is the one that scrolls, so the card keeps its
    // place under whatever is said next instead of being drawn again over it
    // every frame until the first prompt goes.
    first.drawn(renderer)?;

    attaching::refresh_store(&mut held, importing(conversation.session()));

    // The session already has model context and recorded display history.
    // Put that history onto the newly opened screen before asking what to do
    // next, independently of how much context the model currently retains.
    //
    // Before the first prompt, because a session picked up on the command line
    // reaches this loop the same way one picked up by `/resume` does, and the
    // question is about the session rather than about how it was reached.
    // Taken from the session here and dropped at the end of the walk: it is the
    // room a pruning gave back, and holding it for the length of the run would
    // be this screen undoing what that pruning was run to do.
    let pruned = conversation.session().take_pruned();
    let against = replaying::Replay::of(conversation.runner(), terms, &pruned);
    replaying::replayed(renderer, &against, conversation.session(), &mut held.kept)?;
    drop(pruned);

    // Answered rather than acted on. Making room is a request, and a request is
    // run the one way this file runs one — on a worker, with the box live under
    // it — which is the loop below. Carried in as a value so that the panel
    // above the loop and the command inside it reach the same code.
    let mut making = resuming::asked(
        renderer,
        conversation.runner(),
        conversation.session(),
        terms,
        keys,
    )?;

    loop {
        // Read here rather than before the loop, because one command changes
        // it. `/theme` runs between turns, which is inside this loop, and a
        // style captured above it would go on drawing the box in whatever was
        // in force when the session opened — the transcript would follow the
        // new theme and the box, the one thing the reader is looking straight
        // at while they choose it, would not.
        //
        // Once per turn and not per frame: a `Cell::get` of a `Copy` value is
        // a read, and what is between here and the next prompt is a whole turn.
        let style = terms.style();

        // The window may have changed while the last turn was streaming. The
        // box notices a resize as it happens, because in raw mode the terminal
        // reports one; between turns there is nobody reading, so it is noticed
        // here instead.
        renderer.resized()?;

        // What panicked on another thread since the last pass, said here
        // rather than written past the screen by whichever thread it was.
        if let Some(panics) = &panics {
            let (said, unkept) = panics.take();
            if let Err(error) = draw::panicked(renderer, &said, unkept) {
                panics.put_back(said, unkept);
                return Err(error.into());
            }
        }

        // A view opened during the last turn is still open, and it was standing
        // in the rows the box is about to take. So it moves into the region
        // here and reads keys of its own until it is closed, and what comes
        // after it is the box with the line still in it. Nothing was written
        // into the transcript on either side of it.
        //
        // The one door for both halves of the key: what Ctrl+O opens at the
        // prompt is stood here too, so the view a reader closes is the same
        // view whichever press put it up.
        expanding::stand(renderer, style, &held.kept, &mut held.opened)?;

        // Whatever the turn that just ended never reached is the queue's alone
        // now. The two hold the same lines while a turn runs — one to steer it,
        // one to answer once it is over — and a line left here is worked into
        // the *next* turn as well as being that turn's own prompt.
        drop(terms.steer.take());

        // Before the queue, because a prompt typed while room was being made is
        // a prompt about a session that has had room made: sending it first
        // would spend the whole window this is here to free.
        if let Some(why) = making.take() {
            let (back, leaving) = ran(conversation, renderer, terms, Work::Room(why), &mut held)?;
            conversation = back;

            if leaving {
                break;
            }
            continue;
        }

        // The lines queued during the last turn are the next turn, before the
        // box is asked for another — unless that turn stopped on a used-up
        // plan, when they wait for the reader instead.
        let (back, taken) = queueing::taken(conversation, renderer, terms, &mut held, style)?;
        conversation = back;

        // Left after the trouble `ran` says rather than instead of it: a log
        // that stopped recording is worth hearing about on the way out as much
        // as on the way through.
        match taken {
            Some(true) => break,
            Some(false) => continue,
            None => {}
        }

        let commands = terms.commands.snapshot();
        let between = typing::Between {
            commands: &commands,
            conversation: &mut conversation,
            terms,
            attachment_store: held
                .attachment_store
                .as_ref()
                .map(|(path, id)| (path.as_path(), id)),
            editor: &mut held.editor,
            planning: &mut held.planning,
            recalling: &mut held.recalling,
            images: &mut held.images,
            clipboard: &mut held.clipboard,
            left,
            aside: &terms.aside,
            queued: &mut held.queued,
            keys,
        };
        let asked = typing::ask(renderer, style, between)?;

        // Answered by the state that holds what it stands over, because the loop
        // that read the key holds neither. The box comes back either way, with the
        // line still in it.
        if held.opened.asked(&asked, &held.kept) {
            continue;
        }

        let (prompt, local) = match asked {
            // Through the same door as a typed one, on purpose: a woken turn
            // still needs a model to be asked of, and the guards below are
            // where that is answered.
            Asked::Said(said) => said.into_parts(),
            Asked::Woke(said) => (said, false),
            Asked::Ended => break,

            // Taken above, by the state that holds what it stands over.
            Asked::Expand | Asked::Clicked(_) => continue,

            Asked::Untyped => {
                match unboxed(renderer, conversation.runner(), style, held.answers.input)? {
                    Some(said) => (said, true),
                    None => break,
                }
            }
        };

        // Before the turn, because a command is not one: it is answered here,
        // on this thread, and costs the provider nothing. Nothing of it reaches
        // the transcript either — what the model is told about a session is
        // what was said to it, and `/help` was not.
        if local && let Some(wanted) = command::wanted(&terms.commands.snapshot(), &prompt) {
            let ran = command::run(wanted, renderer, &mut conversation, &mut held, terms)?;
            // `/settings` may have changed which press sends.
            held.editor.send_with(terms.sending.get());
            attaching::refresh_store(&mut held, importing(conversation.session()));
            match ran {
                Ran::Again => continue,
                Ran::Leave => break,
                // Asked as a prompt is, before the question about a vendor
                // a recap with nobody to ask would never reach. Only down a
                // pipe: at a terminal the command said the warning itself, as
                // its reply under the line that asked, and asked for nothing.
                Ran::Room(Compacting::Asked) if !answerable(&conversation) => {
                    unanswered(&conversation, renderer, terms)?;
                    continue;
                }
                Ran::Room(why) => {
                    making = Some(why);
                    continue;
                }
            }
        }

        if prompt.trim().is_empty() {
            continue;
        }

        if !answerable(&conversation) {
            unanswered(&conversation, renderer, terms)?;
            continue;
        }

        let imported = attaching::imported(&held);
        let attached = attaching::beside(
            renderer,
            attaching::Asking::of(conversation.runner(), imported.as_deref()),
            &terms.workspace,
            attaching::Sent {
                prompt: &prompt,
                images: &held.images,
            },
            terms.style(),
        )?;
        let work = Work::Turn(prompt, attached);
        let (back, leaving) = ran(conversation, renderer, terms, work, &mut held)?;
        conversation = back;

        if leaving {
            break;
        }
    }

    // Read before the drain below. A session that recorded nothing has no name
    // and no file, and that is the same answer as a session nothing was hidden
    // from: there is nowhere to send the reader.
    //
    // The conversation's, because `/clear` and `/resume` put a different
    // session there and closed the one they replaced: this is whichever one
    // the loop ended on.
    let session = Arc::clone(conversation.session());
    let written = session.id().is_some().then(|| session.path().to_path_buf());

    // The writer thread is usually still holding the last turn when the loop
    // ends, so the poll above cannot be relied on to have seen a failure
    // recorded during it. Draining here is what stops the one turn most likely
    // to matter from being the one nobody is told about.
    //
    // Its own statement, and not the first half of the condition below, because
    // the drain is what puts the last turn on the disk: it has to happen
    // whether or not anything has already been said, and a condition is
    // something a later edit can reorder into not happening at all.
    let problem = session.finish();

    if let Some(problem) = &problem
        && !held.told
    {
        draw::trouble(renderer, problem)?;
    }

    renderer.settle()?;

    // In the terminal's own buffer the live region is closed while the modes
    // are still held, so what they and any held-back panic write on the way
    // out lands below it. Nothing on a screen of crucible's own. Closed here
    // so a terminal that refuses is reported; `closing` covers every other
    // way out, and does nothing once this has run.
    renderer.closes()?;

    // Whatever was said about the log while the screen was still up went with
    // it, which is why the failure reaches here at all: pointing a reader at a
    // file and calling it the transcript would be the last thing crucible said
    // and false.
    Ok(Parting::of(drew, written, problem.as_deref()))
}

/// Where `session` keeps the copies of what is attached to it: beside its log,
/// under its name. `None` for a session that records nothing, which has
/// neither.
fn importing(session: &Session) -> Option<(PathBuf, SessionId)> {
    session
        .id()
        .cloned()
        .map(|id| (session.path().to_owned(), id))
}

/// Says once that the session log stopped recording.
///
/// Once per session rather than once per turn: the log does not start working
/// again, so a line under every turn from here on would bury the turns it is
/// about. `told` is the loop's own memory of having said it, which is why it is
/// passed rather than read back off anything.
fn troubled<T: Terminal>(
    renderer: &mut Renderer<T>,
    session: &Session,
    told: &mut bool,
) -> Result<(), Fatal> {
    if !*told && let Some(problem) = session.trouble() {
        draw::trouble(renderer, &problem)?;
        *told = true;
    }

    Ok(())
}

/// Reads one line on a run with no box to type it into.
///
/// `None` where input ended, which ends the session.
///
/// The mode in force is spelled the way configuration spells it, in front of
/// the line rather than under a box there is none of. It is on screen every
/// time rather than said once at the top because the moment it matters is hours
/// in, when the top has scrolled away — a `fullAccess` session must not be
/// distinguishable from an `ask` one only by what the user remembers starting.
///
/// The mark after it is the one a line is typed after everywhere else, taken
/// from the same setting: this is the prompt on a run that has no box to draw
/// one in.
fn unboxed<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &Runner,
    style: Style,
    input: &mut dyn BufRead,
) -> Result<Option<String>, Fatal> {
    let mark = style.glyphs().caret();
    draw::mark(renderer, &format!("{} {mark} ", runner.mode()))?;

    let Some(said) = read(input)? else {
        // The mark is still the last thing on its row, and nothing but this
        // ends it. Without it, whatever comes next is drawn on top of `ask › `
        // — a report below, or the shell's own prompt once crucible is gone,
        // which is every ordinary exit. The box needs none of this: it takes
        // its own rows back before it returns.
        draw::ended(renderer)?;
        return Ok(None);
    };

    Ok(Some(said))
}

/// Whether a prompt can be a turn at all.
///
/// Asked before the turn and not inside it, because a turn with no model is
/// not a turn: the prompt would be recorded, a request would go out naming
/// nothing, and the vendor's refusal would describe a model name that was never
/// typed. A model named with nobody to ask it of, as `--model foo` is on a
/// machine with nothing set up, is the same session: the provider standing in
/// would refuse the turn, but only after the prompt was recorded as said to a
/// model nobody asked. A typed prompt, a queued one and `/compact` are asked.
///
/// The application decides it, so a client with no terminal is refused the
/// same turns this one is.
fn answerable(conversation: &Conversation) -> bool {
    conversation.missing().is_none()
}

/// Says that what was asked has nobody to ask, where [`answerable`] said
/// so: a prompt typed at the box or queued, `/compact` down a pipe (at a
/// terminal the command says it itself), and room asked for with no model
/// to make a recap, a picked-up session's or the application's refusal.
///
/// At a terminal it draws the warning. Down a pipe it fails instead: there
/// is nobody to type `/model`, so carrying on reads every remaining line
/// and answers none of them.
///
/// `/model` is what changes this answer, so it is said again here rather than
/// only under the welcome the session opened with: by now that has scrolled
/// away.
fn unanswered<T: Terminal>(
    conversation: &Conversation,
    renderer: &mut Renderer<T>,
    terms: &Terms,
) -> Result<(), Fatal> {
    let said = terms.unasked(conversation.serving());

    // Down a pipe there is nobody to type `/model`, so carrying on reads every
    // remaining line and answers none of them, and ends `Ok`, which is the one
    // thing a script looks at. Said and failed rather than said and shrugged.
    if !renderer.is_terminal() {
        return Err(Fatal::Unanswerable(said));
    }

    draw::unconfigured(renderer, said)?;
    Ok(())
}

/// Hangs the one-line reply written since `command` under the line that
/// asked, and parts the next block from the pair; nothing where nobody asked.
fn replied<T: Terminal>(
    renderer: &mut Renderer<T>,
    command: Option<usize>,
    style: Style,
) -> Result<(), Fatal> {
    if let Some(from) = command {
        renderer.subordinate(from, style.glyphs())?;
        renderer.commit("")?;
    }
    Ok(())
}

/// Runs one piece of work and settles what came back.
///
/// The three places a turn or a compaction starts do the same things after it:
/// take the runner back, say once where the log has stopped recording, say
/// what a request for room that changed nothing came to, find out whether
/// something pressed while it ran ends the session, and put a model picked and
/// a mode stepped to while it ran on the runner. `true` is the session leaving.
fn ran<T: Terminal>(
    conversation: Conversation,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    work: Work,
    held: &mut Held<'_>,
) -> Result<(Conversation, bool), Fatal> {
    // Room made because a picked-up session was taken as notes reaches here
    // without passing the box, so it is asked here as `/compact` is there.
    if matches!(work, Work::Room(_)) && !answerable(&conversation) {
        unanswered(&conversation, renderer, terms)?;
        return Ok((conversation, false));
    }
    // Only a line somebody typed has a reply to hang under it, which is why
    // this asks who asked rather than what ran: room made because the window
    // filled, or because a resumed session was picked up as notes, was nobody's
    // command and has no line above it to hang from.
    //
    // Counted before anything is asked or sent, because the line that asked
    // left no blank under it: the reply, whichever it turns out to be, is the
    // next row, and the blank after it is written here once it is.
    let command = matches!(work, Work::Room(Compacting::Asked)).then(|| renderer.lines());
    let style = terms.style();
    if warning::held(&conversation, renderer, terms, &work, held)? {
        replied(renderer, command, style)?;
        return Ok((conversation, false));
    }
    let took = take(conversation, renderer, terms, work, held)?;

    troubled(renderer, took.conversation.session(), &mut held.told)?;

    // Neither of the two that changed nothing posted anything, because neither
    // took anything: a compaction reports what it replaced, and these replaced
    // nothing. So this is the only place either can be said, and a command that
    // appears to run and changes nothing is one somebody types again.
    //
    // A refused turn is the third: a prompt a guardrail turned away before any
    // request was made posted no event at all, so without this the reader is
    // handed a fresh prompt and no word on why nothing answered.
    match &took.did {
        Did::Reported | Did::UsedUp => {}
        Did::Refused(turned) => draw::refused(renderer, turned)?,
        Did::Nothing => draw::unmade(renderer)?,
        Did::Stopped => draw::stopped(renderer)?,
        Did::Unsent(refusal) => renderer.commit(&format!("! {refusal}"))?,
        // Answered as a prompt with nobody to ask is, down a pipe too.
        Did::Unasked => unanswered(&took.conversation, renderer, terms)?,
    }

    // And only a one-line reply is a reply. A compaction that ran posts the
    // ruled record instead, which is true of the session rather than of the
    // line that asked for it — a rule is drawn from the first column, and a
    // mark shoved in front of one reads as a result that lost its start. The
    // blank above that record is the one it asks for on its way in.
    if !matches!(took.did, Did::Reported | Did::UsedUp) {
        replied(renderer, command, style)?;
    }

    // Asked of every piece of work that ran, so the first one to end any other
    // way lets the queue go again.
    held.used_up = matches!(took.did, Did::UsedUp);

    let leaving = matches!(took.meanwhile, typing::Meanwhile::Leaving);
    let mut conversation = took.conversation;

    // A speed, a model and a mode taken while the work ran are put on the
    // runner now, the moment it is this side's again, so the row between turns
    // and the next turn agree, and a step made between them moves from the
    // mode the row shows. After the reply above, which answered the line that
    // asked for this work. The speed goes first: its price was the model in
    // force's, and another model taken over the same turn resets it. A session
    // leaving drops all three, a confirmed `/model` pick included: it has no
    // next turn, and the pick is not written down.
    let model = terms.pending_model.take();
    let mode = terms.pending_mode.take();
    let speed = terms.pending_speed.take();
    if !leaving {
        if let Some(speed) = speed {
            command::apply_speed(renderer, &mut conversation, terms, speed)?;
        }
        if let Some((provider, name)) = model {
            command::apply_model(renderer, &mut conversation, terms, provider, &name)?;
        }
        if let Some(mode) = mode {
            let asked = Command::SetMode(crucible_app::client::mode(mode));
            terms.perform(&mut conversation, asked);
        }
    }

    Ok((conversation, leaving))
}

/// One turn, start to finish.
///
/// The runner goes to the worker and comes back, which is what makes the
/// transcript and the permission memory survive a turn without being shared
/// between threads. It is also why a failure on this side is held to the end of
/// the turn rather than returned where it happens: the worker owns the runner,
/// the runner owns the session, and the session's log is finished by a thread
/// its `Drop` waits for. Leaving early would drop the join handle and detach
/// all three, and the process would exit over a log still being written.
/// One turn, on the thread that draws it.
///
/// The pieces the loop over a turn's events needs, in one value so a slash
/// command opened mid-turn can drive the same loop: a panel that keeps the
/// transcript rendering behind it calls [`Turn::step`] between the keys it
/// reads, and each step is one pass of what the loop below does — drain what
/// the worker reported, then look at the keyboard.
struct Turn<'a, 'h> {
    /// What the turn says is happening, row by row.
    turning: &'a mut Turning,
    /// The session's held state, lent for the turn.
    held: &'a mut Held<'h>,
    /// The row under the box, kept current as the count under it moves.
    says: &'a mut Says,
    /// Where the worker's events arrive.
    seen: &'a mut Inbox,
    /// Whether the terminal is still being written to, or the last write
    /// failed and the rest of the turn is only being drained.
    drawn: &'a mut Result<(), Fatal>,
    /// What the keys read while the turn ran asked for.
    meanwhile: &'a mut typing::Meanwhile,
    /// When Ctrl-C was last pressed against an empty line, if it is still the
    /// last key pressed.
    leaving: &'a mut Option<Instant>,
    /// The terms every turn is taken on.
    terms: &'a Terms,
    /// Which provider is answering, by its name in the registry, as it stood
    /// when the turn began: the conversation that knows is on the worker.
    serving: Option<&'static str>,
}

impl Turn<'_, '_> {
    /// One pass over the turn: drain one event, then look at the keyboard.
    ///
    /// `false` where the worker has closed the channel and the turn is over.
    fn step<T: Terminal>(&mut self, renderer: &mut Renderer<T>) -> bool {
        if !self.drain(renderer) {
            return false;
        }
        self.keys(renderer);
        true
    }

    /// The events half of a pass, on its own so a panel standing mid-turn can
    /// keep the transcript moving while the keyboard is the panel's: what the
    /// worker reported is drawn, what ended is reaped and counted, and the row
    /// under the box is kept current — but the keyboard is not looked at, which
    /// is the panel's to read.
    ///
    /// `false` where the worker has closed the channel and the turn is over.
    fn drain<T: Terminal>(&mut self, renderer: &mut Renderer<T>) -> bool {
        // Read once a pass, which is what a signal noted while this turn runs
        // is waiting for. It ends the turn by the road a terminal that stopped
        // taking writes ends it — nothing more is drawn, the turn is asked to
        // stop, and this loop goes on draining it until the worker has written
        // down what it had. It outranks a terminal failure already held: a
        // window that closed fails the next write as well, and of the two it is
        // the hang-up that says how the process should be seen to have ended.
        if !matches!(self.drawn, Err(Fatal::Ended(_)))
            && let Some(told) = self.terms.ending.told()
        {
            *self.drawn = stop_if_failed(Err(Fatal::Ended(told)), &self.terms.cancel);
        }

        match self.seen.recv_timeout(TICK) {
            Ok(one) => {
                // Before it is drawn, because drawing consumes it. The row
                // above the box says what the turn is doing, and this is the
                // only place that can be read off.
                let mut returned = Vec::new();
                let mut terminal = false;
                if let Seen::Turn(event) = &one {
                    terminal = matches!(event, Event::TurnFinished { .. } | Event::Failed { .. });
                    returned = self.turning.saw(event);
                }

                // And a line the turn says it worked in stops waiting behind
                // it. The turn is the only side that knows which lines it
                // reached — one typed a moment too late is still queued and
                // still owed its own turn — so the panel is corrected here,
                // where the turn says what it took, and nowhere earlier.
                if let Seen::Turn(Event::Steered { line }) = &one
                    && self.held.queued.steered(line)
                {
                    self.turning.redraw();
                }

                // And the line of a call whose tool has answered is written
                // before the event that ended it is drawn, so that the result
                // hangs under the call it answers. It goes out through its own
                // door rather than through `shown`, which is already at the
                // arguments this project allows one function.
                for settled in returned {
                    let (call, said) = (settled.call, settled.said);

                    // A call that only looked around joins the run being
                    // counted above it instead of taking a row of its own.
                    // Never after a terminal event: the reader is being told
                    // what was out when the turn stopped, and a count is not an
                    // answer to that.
                    if let Some(looking) = settled.looking.filter(|_| !terminal) {
                        if let Some(alone) = self.held.gathering.took(call, looking, said) {
                            // The run has a second call, so the first one will
                            // not be a row of its own after all and what it
                            // came back with belongs where the rest of the
                            // run's results are.
                            match alone.output {
                                Some(output) => {
                                    self.held
                                        .kept
                                        .gathered(&alone.call, output.into_text(), None);
                                }
                                None => self.held.kept.abandoned(&alone.call),
                            }
                        }
                        continue;
                    }

                    if terminal {
                        // No result follows a terminal event. Remove the exact
                        // retained live call after committing its heading so it
                        // cannot leak into a later expansion.
                        self.held.kept.abandoned(&call);
                    }
                    if self.drawn.is_ok() {
                        // The run above this row ends here, whatever it came
                        // to: this call did something, and a line counting what
                        // was only looked at may not close over it.
                        *self.drawn = stop_if_failed(
                            settling(renderer, self.held, self.terms.style())
                                .and_then(|()| draw::returned(renderer, &said, self.terms.style()))
                                .map_err(Fatal::from),
                            &self.terms.cancel,
                        );
                    }
                }

                // And it ends here for everything that is not another call: the
                // model saying something, a question being put, the turn
                // ending. Each of those is a thing the reader is being shown,
                // and the count of what was looked at belongs above it.
                if breaks(&one) && self.drawn.is_ok() {
                    *self.drawn = stop_if_failed(
                        settling(renderer, self.held, self.terms.style()).map_err(Fatal::from),
                        &self.terms.cancel,
                    );
                }

                if self.drawn.is_ok() {
                    *self.drawn = stop_if_failed(
                        shown(one, renderer, self.terms, self.held),
                        &self.terms.cancel,
                    );
                } else {
                    // Nothing is drawn and nothing is read once the terminal
                    // has failed, and both kinds of question still have to be
                    // answered on the channel they arrived with, or the
                    // worker waits for ever. A refusal and nobody-answered
                    // are what a drawing thread that has stopped means, said
                    // out loud rather than by going quiet.
                    match one {
                        Seen::Question { reply, .. } => {
                            let _ = reply.send(verdict(None));
                        }
                        Seen::Asked { reply, .. } => {
                            let _ = reply.send(None);
                        }
                        Seen::Turn(_) => {}
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                // An explicit compaction can finish and close the channel with
                // its completion events already queued. Keep driving ordinary
                // live frames until the factual 100% dwell has elapsed; sleeping
                // only one tick preserves keyboard and resize responsiveness.
                let now = Instant::now();
                if let Some(wait) = self.turning.completion_wait(now) {
                    if wait.is_zero() {
                        self.turning.finished_frame(now);
                        return false;
                    }
                    thread::sleep(wait.min(TICK));
                } else {
                    return false;
                }
            }
        }

        // Reaped and counted before the box is drawn again, because the row under
        // it says how many commands are still running and a command that has
        // exited is not one of them. A number that only moved when something else
        // on the row did would be exactly the stale fact this row exists to
        // report.
        //
        // And said, the way one that ended between turns is said: a command that
        // finished while the turn ran is otherwise a count that moved with no
        // line to say why, and the reader is owed the line the moment there is
        // room for it rather than after the turn. The model is told separately,
        // just below, and out of a different queue — the reader reads a screen
        // and the model reads a transcript, and neither is the other's copy.
        if self.drawn.is_ok() {
            for ended in self.terms.leaving.reap() {
                *self.drawn = stop_if_failed(
                    draw::gone(renderer, &ended, self.terms.style()).map_err(Fatal::from),
                    &self.terms.cancel,
                );
            }
        } else {
            drop(self.terms.leaving.reap());
        }
        self.says.running = self.terms.leaving.count();

        // And the model, now rather than at the top of a turn it may be waiting
        // to reach. The tool result promised that completion would be reported
        // and told it not to poll; this is the half of that promise that comes
        // due while it is still working, and without it an agent that believed
        // the promise waits for something nothing was going to send.
        //
        // Taken from `reported`, which is the same take-once queue the note
        // between turns is taken from, so whichever gets there first says it and
        // the other says nothing. What the turn does not take before it ends is
        // still in the aside, and the next turn starts by reading it back.
        if let Some(said) = standing::said(&self.terms.leaving.reported()) {
            self.terms.aside.say(said);
        }

        true
    }

    /// The keyboard half of a pass: the loop over the keys pressed while the
    /// turn ran. On its own so `step` is `drain` and this, and a panel never
    /// reaches it — while a panel stands, the keyboard is the panel's.
    fn keys<T: Terminal>(&mut self, renderer: &mut Renderer<T>) {
        // After the event rather than before it, so what the turn said is on
        // screen before the box is drawn back underneath it. A line finished
        // here is kept for the loop above: running it now would start a second
        // turn inside this one.
        // Not read once the session is leaving. The turn is still stopping and
        // this loop still has to drain it, but nothing typed into a box on its
        // way off the screen can change where the session goes.
        if self.drawn.is_ok()
            && self.held.answers.keys
            && matches!(*self.meanwhile, typing::Meanwhile::Nothing)
        {
            // Read before the box borrows the rest of `held`, and read every
            // frame: the numbers in it move while the run goes on, and a line
            // held from the frame before would be saying the run had stalled.
            let counting = self.held.gathering.doing();

            match typing::during(
                renderer,
                typing::During {
                    counting: &counting,
                    background: &self.terms.leaving,
                    editor: &mut self.held.editor,
                    images: &mut self.held.images,
                    clipboard: &mut self.held.clipboard,
                    attachment_store: self
                        .held
                        .attachment_store
                        .as_ref()
                        .map(|(path, id)| (path.as_path(), id)),
                    queued: &mut self.held.queued,
                    turning: self.turning,
                    planning: &mut self.held.planning,
                    kept: &mut self.held.kept,
                    opened: &mut self.held.opened,
                    recalling: &mut self.held.recalling,
                    opened_list: &mut self.held.opened_list,
                    listing: &mut self.held.listing,
                    says: self.says,
                    style: self.terms.style(),
                    cancel: &self.terms.cancel,
                    steer: &self.terms.steer,
                    terms: self.terms,
                    leaving: self.leaving,
                },
            ) {
                // Kept rather than acted on. The turn has been asked to stop
                // and this loop is what notices it has: leaving here would drop
                // the join handle below and take the process out over a session
                // log still being written.
                Ok(typing::Meanwhile::Leaving) => *self.meanwhile = typing::Meanwhile::Leaving,
                // A slash command is run here rather than in the keyboard loop:
                // the panel it stands is the turn's to keep rendering under, and
                // this is where the turn is. Running it hands the keyboard to
                // the panel for its length, which is why it is not `during`'s.
                Ok(typing::Meanwhile::Command(command)) => {
                    if self.drawn.is_ok() {
                        let ran = self.command(renderer, &command);
                        *self.drawn = stop_if_failed(ran, &self.terms.cancel);
                    }
                }
                Ok(typing::Meanwhile::Nothing) => {}
                Err(problem) => *self.drawn = stop_if_failed(Err(problem), &self.terms.cancel),
            }
        }
    }

    /// Runs a slash command finished mid-turn, with the turn rendering behind
    /// whatever it stands. Which of the three it may do is the command's own
    /// say; the panel is stood from here, where the turn's drain can be run
    /// between the keys it reads.
    fn command<T: Terminal>(
        &mut self,
        renderer: &mut Renderer<T>,
        command: &command::Owned,
    ) -> Result<(), Fatal> {
        // A panel waits on the keyboard with no clock, and the pass that would
        // read a noted signal is run from inside that wait.
        let _unclocked = self.terms.ending.unclocked()?;

        match command.class() {
            command::MidTurn::Live => self.live(renderer, command),
            command::MidTurn::Deferred => self.deferred(renderer, command),
            command::MidTurn::Refused(why) => {
                command::refused(renderer, command, why, self.terms.style()).map(|_| ())
            }
        }
    }

    /// Runs a slash command that moves nothing but the screen, with the turn
    /// still rendering behind it.
    ///
    /// The panel owns the keyboard while it stands — `keys` is not reached —
    /// and the transcript is kept moving by draining the turn between the keys
    /// the panel reads. A permission or asked question is the one thing not
    /// drained: it has paused the turn already, so it is held for the panel's
    /// close rather than drawn over it, and the loop above answers it the
    /// moment the keyboard is the box's again.
    fn live<T: Terminal>(
        &mut self,
        renderer: &mut Renderer<T>,
        command: &command::Owned,
    ) -> Result<(), Fatal> {
        // The transcript advances while the panel stands, so the drain is run
        // once a pass. The keyboard is not the turn's here, which is why this
        // is `drain` rather than `step` — and why the hook is handed the
        // renderer rather than closing over it.
        let counted = command::Counted {
            usage: crucible_app::client::usage(
                &self.says.model,
                &self.turning.breakdown(),
                &self.turning.totals(),
                self.turning.limits().as_ref(),
                self.serving,
            ),
            serving: self.serving,
            mode: self.says.running_mode,
            session: self
                .held
                .attachment_store
                .as_ref()
                .map(|(_, id)| id.clone()),
        };
        let ran = command::live(renderer, self.terms, command, &counted, &mut |renderer| {
            self.drain(renderer);
            Ok(())
        });
        // `/settings` may have changed which press sends.
        self.held.editor.send_with(self.terms.sending.get());
        ran
    }

    /// Runs a command whose pick is held for the turn started next.
    ///
    /// `/model` is the one of these. The picker opens over the running turn,
    /// the consequence is said and agreed to, and the pick is held — the
    /// runner is on the worker for this turn's length, so nothing of it can
    /// change now. The loop applies it when the turn ends and the runner is
    /// this side's again.
    fn deferred<T: Terminal>(
        &mut self,
        renderer: &mut Renderer<T>,
        command: &command::Owned,
    ) -> Result<(), Fatal> {
        // Which model is in force is read off the row under the box: the
        // runner that would answer is on the worker, and the row was written
        // from it before the turn began.
        // The mode is a ladder shift+tab steps mid-turn; `/mode` is that step
        // made by name, held the same way.
        if matches!(command.command(), command::Command::Mode) {
            let next = self
                .terms
                .pending_mode
                .get()
                .unwrap_or(self.says.running_mode)
                .next();
            self.terms.pending_mode.set(Some(next));
            self.says.cycling(next);
            return Ok(());
        }

        let model = self.says.model.clone();
        let current = command::Asked {
            provider: self.serving,
            model: &model,
            effort: self.says.effort,
            pace: self.says.pace,
        };
        let picked = command::deferred(renderer, self.terms, current, command, &mut |renderer| {
            self.drain(renderer);
            Ok(())
        })?;
        match picked {
            Some(command::Kept::Model(provider, name)) => {
                self.terms.pending_model.set(Some((provider, name)));
            }
            Some(command::Kept::Speed(speed)) => self.terms.pending_speed.set(Some(speed)),
            None => {}
        }
        Ok(())
    }
}

fn take<T: Terminal>(
    conversation: Conversation,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    work: Work,
    held: &mut Held<'_>,
) -> Result<Took, Fatal> {
    let (post, seen) = sync_channel(CAPACITY);
    terms.putting.open(post.clone());
    let mut seen = Inbox::new(seen);

    let asking = Asking::new(post.clone(), terms.client.clone());
    let relay = Relay::new(post, terms.putting.clone());
    let running = terms.cancel.clone();

    // The box stands under the turn, where it stands under the prompt the rest
    // of the time, and the mode stands under the box. A turn is the longest a
    // session goes without a prompt on screen, and it is the stretch the mode is
    // deciding things over: what a tool call arriving in the middle of it costs
    // is exactly which mode is in force, and reading that off the screen must
    // not mean remembering it.
    //
    // Read here because the runner is about to leave. The mode this turn uses
    // stays with it; a Shift-Tab while it is away updates this row immediately
    // and is held for the next turn. Drawn below rather than here, where a
    // failure would be a turn that never ran.
    //
    // The model beside it for the same two reasons: the row says it, and only
    // `/model` and `/effort` change it — neither of which can be run while the
    // turn they would change is the one running.
    let mut says = typing::under(conversation.runner()).naming(&terms.commands.snapshot());
    let serving = conversation.serving();

    // And what ended while nothing was running goes into the aside rather than
    // into what is above, which is the one place a fact like this can be put
    // that does not decide how long it lasts. The prompt is written again every
    // turn, so a note put there is said once and then gone; the aside is drained
    // into the transcript by the turn itself, which is where the same note goes
    // when a command ends while a turn is running. One fact, one lifetime,
    // whichever of the two moments it arrives in.
    if let Some(said) = standing::said(&terms.leaving.reported()) {
        terms.aside.say(said);
    }

    // Whatever stopped the last turn is spent, and this is the last moment at
    // which clearing it can be certain of that: from the next line on there are
    // two threads, one of them reading the keyboard. A press arriving after
    // this is a press about the turn below, which is what the turn does with a
    // flag it finds raised.
    terms.cancel.reset();

    // From here until the worker has been joined, a hang-up or a termination
    // is noted for the loop below rather than obeyed where it lands: obeyed,
    // it would take the answer on screen with it.
    let heeding = terms.ending.turn();

    // Started before the worker rather than on the first thing it reports, so
    // that what the clock measures is what somebody is waiting for. A turn that
    // spends its first ten seconds connecting has spent them.
    let runner = conversation.runner();
    let mut turning = Turning::started(runner.breakdown())
        .using(runner.totals(), runner.plan_limits())
        .pinning(terms.pinning.get());

    attaching::refresh_store(held, importing(conversation.session()));
    let working = sent(
        &terms.runtime,
        conversation,
        work,
        asking,
        relay,
        running,
        terms.steer.clone(),
        terms.aside.clone(),
    );

    // The first thing drawn, and held like everything drawn after it: the runner
    // is with the worker now, so a terminal that failed here has to be carried
    // to the end of the turn rather than returned from the middle of one.
    let mut drawn = stop_if_failed(
        typing::stand(
            renderer,
            &held.editor,
            typing::Footing {
                turning: &turning,
                planning: &mut held.planning,
                counting: "",
                opened_list: &held.opened_list,
                // A turn can start with prompts already behind it: room is
                // made before the queue is read, so a line typed during the
                // last turn is still waiting when this one is about making
                // room for it, and its panel stands from the first frame.
                queued: &held.queued,
                history: held.recalling.place(),
            },
            &says,
            terms.style(),
        ),
        &terms.cancel,
    );

    // Both of these outlive one look at the keyboard because the gesture they
    // belong to does: the offer to leave is made on one press and taken on the
    // next, and the two can land either side of a delta arriving.
    let mut leaving = None;
    let mut meanwhile = typing::Meanwhile::Nothing;

    // Ends when the worker drops both senders, which happens when the turn is
    // over. The wait is bounded rather than blocking so that the keyboard is
    // looked at between deltas. The queue itself is bounded too: adjacent
    // deltas already waiting are drawn together, and a provider that outruns a
    // slow terminal meets backpressure instead of growing process memory.
    let mut turn = Turn {
        turning: &mut turning,
        held,
        says: &mut says,
        seen: &mut seen,
        drawn: &mut drawn,
        meanwhile: &mut meanwhile,
        leaving: &mut leaving,
        terms,
        serving,
    };
    while turn.step(renderer) {}

    // The turn is over, so what stood under it is taken back — the box, or the
    // view if Ctrl+O was pressed while the turn ran. What comes back next is
    // the same one of them, live this time, and the two on screen together
    // would be one of them drawn twice.
    if drawn.is_ok() {
        drawn = renderer
            .under(&[], None, terms.style().palette())
            .map_err(Fatal::from);
    }

    // The task has dropped both senders, so it has ended or is ending: this
    // waits only for it to hand back what it took.
    let (conversation, did) = terms.runtime.block_on(working).map_err(|_| Fatal::Lost)?;

    // Written down before the signal it was held back from is let through,
    // and a turn that failed written down whoever else holds the session:
    // [`Stretch::over`] says why, and in which order.
    let drawn = heeding.over(drawn, || {
        let _ = conversation.session().finish();
    });

    drawn.map(|()| Took {
        conversation,
        meanwhile,
        did,
    })
}

/// Sends the work away as a task on `runtime`, with the runner.
///
/// The runner goes with it and comes back beside what it found to do, which is
/// what makes the transcript and the permission memory survive a turn without
/// being shared between threads. Nothing on this side waits on the provider,
/// which is what keeps the box under the turn live while it runs. The turn
/// holds a worker only while it is polled: what it waits on — the model, a
/// question, a terminal slow to take what it reports — hands the worker back
/// while it lasts.
// Every one of these has to cross into the task as a value it owns or clones;
// the run that bundles four of them borrows, so it can only be made inside it.
// The lint counts to five; what has to travel is seven, and where it goes.
#[allow(clippy::too_many_arguments)]
fn sent(
    runtime: &tokio::runtime::Handle,
    mut conversation: Conversation,
    work: Work,
    mut asking: Asking,
    relay: Relay,
    running: Cancel,
    steer: crucible_runtime::Steer,
    aside: crucible_runtime::Aside,
) -> tokio::task::JoinHandle<(Conversation, Did)> {
    runtime.spawn(async move {
        // One run for the whole of what this worker was sent to do, and
        // the identity every event of it carries. Minted here rather than
        // inside the runner because the failure below is this side's to
        // post: a `TurnError` is handed back rather than reported, and a
        // failure stamped with a run of its own would say the turn that
        // failed was somebody else's.
        let run = conversation
            .runner()
            .starting(&relay, &running, &steer, &aside);
        let reporting = run.reporting();

        // What somebody asked for is asked of the application as the
        // command it is. Room made because the window filled, or because
        // a session was picked up as notes, is the host's own doing and
        // no client's to ask for.
        //
        // A turn and a compaction are the same shape, and that is the
        // whole of why this is one function: one request, answered over
        // seconds, reporting as it goes. Everything the loop that draws
        // does for a turn — the bar, the clock, the box taking the next
        // prompt, the key that stops it — is what a reader waiting on a
        // compaction needs, and none of it is about a turn.
        let ended = match work {
            Work::Turn(prompt, attached) => match Prompt::new(&prompt) {
                Ok(prompt) => {
                    let asked = Command::Prompt(prompt);
                    asking.turn(&mut conversation, asked, attached, &run).await
                }
                Err(refusal) => Ended::Refused(refusal),
            },
            Work::Room(Compacting::Asked) => {
                asking
                    .turn(&mut conversation, Command::Compact, Box::default(), &run)
                    .await
            }
            // No turn is running, so the reading starts at nothing and
            // what it comes to is the recap request's own cost — posted
            // on the way, which is all the row above the box asks.
            Work::Room(why) => {
                Ended::Room(conversation.compact(why, &run, &mut Spend::default()).await)
            }
        };

        // What a turn came to is read rather than dropped: one that ran
        // reported itself as it went, and one a guardrail refused may
        // have reported nothing at all.
        let did = match ended {
            Ended::Turn(Ok(Turned::Ran(_))) | Ended::Room(Ok(Room::Made(_))) => Did::Reported,
            Ended::Turn(Ok(refused)) => Did::Refused(refused),
            Ended::Room(Ok(Room::Nothing)) => Did::Nothing,
            Ended::Room(Ok(Room::Stopped)) => Did::Stopped,
            Ended::Turn(Err(problem)) | Ended::Room(Err(problem)) => {
                let used_up = matches!(problem, TurnError::PlanLimit { .. });
                reporting.post(Event::Failed { error: problem });
                if used_up { Did::UsedUp } else { Did::Reported }
            }
            Ended::Refused(refusal) => Did::Unsent(refusal),
            // Not reached from here: a prompt or `/compact` with no model to
            // ask is answered by [`answerable`] before any work is sent.
            Ended::Unasked(_) => Did::Unasked,
            // Not reached from here: every route holding the send is asked
            // about on the drawing thread before the work is sent, each yes
            // is written down there, and nothing on the worker takes one out.
            Ended::Warned(_) | Ended::Unrecorded(_) => Did::Unsent(ErrorCode::Abandoned.into()),
        };

        (conversation, did)
    })
}

/// What a worker is sent away to do.
///
/// Two things rather than one, because there are two and they are the same
/// shape: a request goes out, it answers over seconds, and what it reports has
/// to reach a screen somebody is watching. What told them apart before was
/// which of three loops was drawing, and two of the three were worse.
enum Work {
    /// One prompt, the files it named, and everything that follows from it
    /// until the agent yields.
    Turn(String, Box<[Attachment]>),
    /// Room, made for the reason it names.
    Room(Compacting),
}

/// What the worker found to do.
enum Did {
    /// It happened, and everything about it was reported as it happened.
    Reported,
    /// It was reported as [`Self::Reported`] is, and it stopped on a used-up
    /// plan. Told apart because the queue waits for the reader after it: the
    /// lines in it, run as the next turn, would stop again or go to a vendor
    /// that has said the plan is spent.
    UsedUp,
    /// A guardrail turned the turn away, or could not decide about it. Carried
    /// back whole because the refusal may be the only thing the turn produced:
    /// a prompt refused before any request was made posts no event.
    Refused(Turned),
    /// Room was asked for and there was none to make — a session with nothing
    /// behind the turns it keeps whole. Nothing was spent and nothing was
    /// posted, so this is the only place it can be said, and a command that
    /// appears to run and changes nothing is one somebody types again.
    Nothing,
    /// Room was being made and the key that stops a turn stopped it. Told apart
    /// from [`Self::Nothing`] because the two leave the same session behind and
    /// mean opposite things to whoever is reading: one says this session has no
    /// middle to replace, and the other says the one thing the reader already
    /// knows they did. Nothing was posted for this either — a compaction
    /// reports what it took, and this one took nothing.
    Stopped,
    /// The application would not take it: a prompt longer than any front end
    /// may send. Nothing ran and nothing was posted.
    Unsent(Refusal),
    /// Room was asked for with no model to ask for a recap. Nothing was
    /// recorded, sent or posted.
    Unasked,
}

/// A turn, and what the keyboard asked for while it ran.
struct Took {
    /// The conversation, back from the worker that held it for the turn.
    conversation: Conversation,
    /// Whether anything pressed during the turn ends the session with it.
    meanwhile: typing::Meanwhile,
    /// What the worker found to do.
    did: Did,
}

/// Raises cancellation the first time the drawing side can no longer proceed.
///
/// The caller still drains the event channel and joins the worker, preserving
/// the original failure while letting a quiet provider observe that nobody can
/// use its answer any more.
fn stop_if_failed<T>(result: Result<T, Fatal>, cancel: &Cancel) -> Result<T, Fatal> {
    if result.is_err() {
        cancel.request();
    }
    result
}

/// What the session holds between turns and lends to each one, beyond the
/// runner and the terminal.
///
/// Everything here outlives the turn — the line being typed, the lines finished
/// behind it, what its results had no room to say, where the answer to a
/// question comes from. One value rather than six locals because every turn is
/// handed all of it: a call with six references in a row is one nobody can
/// read, and what a turn needs next is added here rather than at each of the
/// three places one starts. The prompt is not among them, because the prompt is
/// what the turn is about rather than something it hands back.
///
/// A command is lent it too, and for the same reason. A command runs between
/// turns, on this thread, and the ones that reach in here reach for what the
/// session is holding rather than for anything of their own: `/resume` drops
/// what the session it is leaving had held, and every command that opens
/// something asks first whether there is a keyboard to answer it with.
struct Held<'a> {
    /// The line being written, one for the whole session rather than one per
    /// prompt: what was typed while a turn ran is still in the box when it
    /// ends, and the allocation the last line grew to is the one the next
    /// starts in.
    editor: Editor,
    /// The prompts waiting behind a turn, which the turn adds to as lines are
    /// finished in the box under it. They are the next turn, in the order they
    /// were typed: the whole of the queue goes to one turn rather than a turn
    /// each, which is what [`queueing::batched`] does with it.
    queued: Prompts,
    /// Whether the last work stopped on a used-up plan, which holds the queue
    /// until the reader sends something: [`queueing::taken`] runs nothing
    /// while it is set.
    used_up: bool,
    /// What the transcript had no room to say, waiting for Ctrl+O. Held for the
    /// whole session rather than for a turn: the row offering the key is read
    /// after the turn that drew it has ended, which is when there is time to
    /// read anything.
    kept: Kept,
    /// The run of calls that only looked around, counted rather than named.
    /// For a turn rather than a session — a run is broken by the first thing
    /// that is not one of them, and the end of a turn is such a thing — but it
    /// is held beside the rest because the loop that draws it is this one.
    gathering: Gathering,
    /// Whether the reader is standing that under the turn, and where over it. A
    /// view opened while a turn ran is still open when the turn ends, in the
    /// region the box comes back to, and the reader who opened it is reading.
    opened: Standing,
    /// The command list a line typed mid-turn has open above the box, empty
    /// while the line is a prompt.
    opened_list: typing::Opened,
    /// The list of what is still running, stood by a click on the count under
    /// the box. Held for the session like the two standings beside it: the box
    /// a turn is drawn over is the same one the click is read against, so the
    /// mark in it belongs to the session rather than to a turn.
    listing: leaving::Leaving,
    /// The plan above the box. A turn is when it changes — the tool that writes
    /// it runs on the worker thread — and what this holds is a copy of the plan
    /// and the setting of the key that opens it, both of which outlive it.
    planning: Planning,
    /// The prompts this directory has already been asked, and where an arrow
    /// has walked back to in them.
    ///
    /// For the session and past it: the list came off a file when the session
    /// opened and goes back to it as each line is finished, so a walk reaches
    /// through what was asked here yesterday. `/clear` does not empty it —
    /// forgetting a conversation is not forgetting how somebody phrased the
    /// question they are about to ask again.
    recalling: Recalling,
    /// The images pasted at the prompt, in the order they were pasted. The
    /// paste puts `[Image #N]` in the line and the path of the Nth here, and a
    /// prompt saying the marker sends the image. For the session rather than
    /// for a prompt, so a later prompt can still say an earlier number.
    images: Vec<Box<str>>,
    /// The desktop clipboard connection used by every image paste.
    ///
    /// Opened lazily, then kept so repeated Ctrl+V presses reuse the platform
    /// clipboard connection. An opening failure leaves `None` and is retried.
    clipboard: Option<arboard::Clipboard>,
    /// Durable identity copied before a turn lends the runner to its worker.
    ///
    /// Clipboard image import needs these while the box remains live under that
    /// turn. `None` in a session configured not to record, where there is nowhere
    /// to keep an imported image safely.
    attachment_store: Option<(PathBuf, SessionId)>,
    /// Whether the log's trouble has been said. Once is all it is worth, for
    /// the reason [`troubled`] gives.
    told: bool,
    /// Where the answer to a permission question comes from.
    answers: Answers<'a>,
    /// The card the session opened with, which `/clear` and `/resume` write
    /// again at the top of the record they start over.
    opening: &'a draw::opening::Standing,
}

impl<'a> Held<'a> {
    /// What a session starts with: nothing typed, nothing queued, nothing kept
    /// and nothing said, over the plan the tools were built with.
    fn new(
        plan: Plan,
        sending: Sending,
        answers: Answers<'a>,
        opening: &'a draw::opening::Standing,
    ) -> Self {
        Self {
            // The one editor that takes a newline: a prompt is a paragraph, not
            // a line, and the box grows a row for each. Every other editor — a
            // permission note, a secret, a name — stays one line, so this is
            // also the one that has a second press to give away.
            editor: Editor::new().multiline().sends(sending),
            queued: Prompts::default(),
            used_up: false,
            kept: Kept::default(),
            gathering: Gathering::default(),
            opened: Standing::default(),
            opened_list: typing::Opened::default(),
            listing: leaving::Leaving::default(),
            planning: Planning::new(plan),
            // Nothing to reach back through and nowhere to write. The session
            // that has a directory to read one out of puts it here itself:
            // every other holder of a `Held` is a test of something else.
            recalling: Recalling::default(),
            images: Vec::new(),
            clipboard: None,
            attachment_store: None,
            told: false,
            answers,
            opening,
        }
    }
}

/// Whether this is a thing the run of counted calls may not close over.
///
/// Every variant is named rather than caught by a rest arm, for the reason
/// [`Turning::saw`](turning::Turning::saw) names its own: an event added later
/// either belongs inside a run of calls that only looked around or ends one,
/// and that is a decision to make here rather than one to inherit.
fn breaks(one: &Seen) -> bool {
    match one {
        // Both are the reader being shown something, and a count of what was
        // looked at belongs above it rather than around it.
        Seen::Question { .. } | Seen::Asked { .. } => true,

        Seen::Turn(event) => match event {
            // The calls themselves, and what arrives while they are out. None
            // of it is a row, so none of it parts one call from the next.
            //
            // A result is here as well, and it has to be: the event that folded
            // a call into the run is the one before this, and breaking on the
            // result would end every run at one call.
            Event::ToolRequested { .. }
            | Event::ToolFinished { .. }
            | Event::Wrote { .. }
            | Event::Spent { .. }
            | Event::PromptCache { .. }
            | Event::Sandbox { .. }
            | Event::Used { .. }
            | Event::PlanLimits { .. }
            | Event::Retrying => false,

            // Everything else is a row, or is about to be one. The model
            // speaking is the plainest case and the one a reader feels: what
            // it says next is about what it just looked at, so the looking is
            // counted and closed first.
            //
            // `Carried` is the one that is not a row and breaks anyway. It is
            // posted once a round trip, when every call of the batch has been
            // answered and the agent is going back for more, and that is the
            // smallest part of a turn worth a line. A run held open past it
            // would be held open for the whole turn — and a turn that only
            // looks around can go on for minutes, leaving the reader watching
            // an empty transcript with a number over the box: nothing to
            // scroll back through, nothing to point at, and the line they were
            // told they could open not yet a line at all.
            Event::TurnStarted { .. }
            | Event::Carried { .. }
            | Event::Delta { .. }
            | Event::Compacting { .. }
            | Event::Compacted { .. }
            | Event::Steered { .. }
            | Event::Aged { .. }
            | Event::Unread { .. }
            | Event::FastRefused { .. }
            | Event::TurnFinished { .. }
            | Event::Failed { .. } => true,
        },
    }
}

/// Ends the run of calls being counted, writing whatever it came to.
///
/// Nothing where the run is empty, which is most of the time. One row where it
/// gathered enough to be worth folding. And the call's own row where it did
/// not: a run of one is not folded, so the row it was always going to have is
/// written now that it is known no second call was coming.
fn settling<T: Terminal>(
    renderer: &mut Renderer<T>,
    held: &mut Held<'_>,
    style: Style,
) -> Result<(), TerminalError> {
    let mut gathering = held.gathering.taken();

    if gathering.folds() {
        // Swept rather than pointed one at a time: what the turn gathered and
        // has not yet offered is exactly this run, because every other result
        // was pointed at its own row as it went down.
        let at = draw::gathered(renderer, &gathering.did(), style)?;
        held.kept.onto(at);
        return Ok(());
    }

    let Some(alone) = gathering.alone() else {
        return Ok(());
    };

    draw::returned(renderer, &alone.said, style)?;

    let Some(output) = alone.output else {
        held.kept.abandoned(&alone.call);
        return Ok(());
    };

    draw::came_back(
        renderer,
        &mut held.kept,
        &alone.call,
        draw::Shown::live(output),
        style,
    )
}

/// Refuses a pending action already off the channel: the drain cannot meet
/// the same [`Seen::Question`] or [`Seen::Asked`] again, so silence must
/// still carry a refusal, or the worker waits forever beside it.
fn refuse<T>(reply: oneshot::Sender<T>, refusal: T, problem: Fatal) -> Result<(), Fatal> {
    let _ = reply.send(refusal);
    Err(problem)
}

/// Draws one thing the worker sent, and answers it if it was a question.
fn shown<T: Terminal>(
    one: Seen,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    held: &mut Held<'_>,
) -> Result<(), Fatal> {
    let style = terms.style();

    match one {
        // A call the run above it is counting. No row is drawn for it — the
        // count is its row — but what it came back with is kept either way,
        // because the line counting the run is the door to all of it.
        Seen::Turn(Event::ToolFinished { call, output, .. }) if held.gathering.holds(&call) => {
            if let Some(output) = held.gathering.answered(&call, output) {
                held.kept.gathered(&call, output.into_text(), None);
            }
        }
        Seen::Turn(event) => {
            draw::event(renderer, event, &terms.workspace, style, &mut held.kept)?;
        }
        Seen::Question {
            call,
            sensitivity,
            reply,
        } => {
            // A durable rule cannot live in either project configuration file:
            // both names can arrive with a checkout, whatever an ignore rule
            // says. Until policy has a per-workspace store outside the checkout,
            // the prompt offers only answers this process can honour.
            // The question stands until somebody decides, with no clock on
            // it, so nothing would read a signal noted while it stood.
            let answer = terms.ending.unclocked().and_then(|_unclocked| {
                asked(renderer, &call, &sensitivity, &mut held.answers, style)
            });
            match answer {
                Ok(answer) => {
                    // A worker that stopped waiting has already denied itself.
                    let _ = reply.send(answer);
                }
                Err(problem) => return refuse(reply, verdict(None), problem),
            }
        }
        Seen::Asked { questions, reply } => {
            // A loop reading lines rather than keys has nobody to put a panel
            // to, and neither has one whose raw mode never came up. The tool is
            // not registered in either, so this is the belt rather than the
            // braces — but a panel that read keys nobody is at would wait for
            // ever, and waiting for ever is the one failure this loop may not
            // have.
            if !held.answers.keys {
                let _ = reply.send(None);
                return Ok(());
            }

            // The same wait as a permission question's: as long as the panel stands.
            let unclocked = match terms.ending.unclocked() {
                Ok(unclocked) => unclocked,
                Err(problem) => return refuse(reply, None, problem),
            };

            let given = match putting::put(renderer, style, &questions) {
                Ok(given) => given,
                Err(problem) => return refuse(reply, None, problem),
            };

            let given = match given {
                putting::Put::Said(answered) => Some(answered),
                putting::Put::Left => None,

                // Nothing was drawn and no key was read, so the questions still
                // have to be put: a window this small is not somebody saying no.
                putting::Put::Cramped => match cramped(renderer, &questions, style) {
                    Ok(given) => given,
                    Err(problem) => return refuse(reply, None, problem),
                },
            };

            drop(unclocked);
            let _ = reply.send(given);
        }
    }

    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
