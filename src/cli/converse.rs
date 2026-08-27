//! The loop: read a line, take a turn, draw what the turn does.
//!
//! The turn runs on its own thread and the terminal stays with this one. That
//! split is the whole reason a turn can stream while a question is waiting to
//! be answered, and it is why no lock appears anywhere on the render path: the
//! only thread that writes to the terminal is the one running this loop.
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
//! The session log is append-only and written as the turn goes, so `--continue`
//! picks the session up from wherever it stopped.
//!
//! Which is also the last thing a session does. The screen it drew on is
//! borrowed and handed back, so the transcript goes with it — and this loop
//! returns a [`Parting`] saying where the log is and whether it kept up, for
//! the caller to report once the screen is the reader's again.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use crucible_auth::Store;
use crucible_core::{
    Answer as Chosen, Answered, Attachment, Cancel, Compacting, Event, Mode, Post as _, Question,
    Remember, Revealed, Room, Sensitivity, Spend, ToolCall, Verdict, Workspace,
};
use crucible_runner::Runner;
use crucible_tools::{Background, Ledger, Plan};
use crucible_tui::{
    Editor, Key, Pasting, Pressed, Raw, Renderer, Reporting, Screen, Sending, Spelling, Terminal,
};

use super::draw;
use super::kept::Kept;
use super::seen::{Answer, Asking, CAPACITY, Given, Inbox, Putting, Relay, Seen};
use super::style::Style;
use super::subscription::Subscriptions;
use super::unasked;
use super::{Fatal, Served, Serving, standing};
use command::Ran;
use expanding::Standing;
use planning::Planning;
use recalling::Recalling;
use turning::Turning;
use typing::{Asked, Says};

mod asking;
mod attaching;
mod command;
mod expanding;
mod finding;
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
    /// Whether to write colour, how much of a tool call to show, and which
    /// table of colours to draw with.
    ///
    /// In a cell because one command changes it: `/theme` picks a different
    /// table, and everything drawn after it is drawn in that one. Settled once
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
    /// What stops a turn.
    pub(crate) cancel: Cancel,
    /// What a line typed while a turn runs is pushed into, and the turn draws
    /// from between one pass and the next. Held for the session the way the
    /// cancel is: it is made once beside it, and the turn's thread and the loop
    /// that reads the keyboard each hold an end.
    pub(crate) steer: crucible_core::Steer,
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
    pub(crate) revealed: Revealed,
    /// Where a tool's questions reach the thread that draws them.
    ///
    /// Held for the reason the ledger and the plan are: it is made once, beside
    /// the tool that was built with it, and the loop holds the other end. What
    /// it lends changes every turn; what holds it does not.
    pub(crate) putting: Putting,
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
    /// Which provider this session is set up to ask, where a key was found for
    /// one. `/model` writes its answer under this name, and where there is none
    /// there is no name to write it under.
    ///
    /// A cell because `/login` fills it in. Every command is handed these terms
    /// by reference, and taking them mutably for the one that changes this
    /// would put a `&mut` through every arm that does not.
    pub(crate) provider: Cell<Option<&'static str>>,
    /// A model picked mid-turn, held for the turn the loop starts next.
    ///
    /// The runner is on the worker for the running turn's length, so a pick
    /// made then cannot reach it — it is held here and applied at the next
    /// turn's start, when the runner is this side's again. `/clear` takes it
    /// with the rest of the session: a pick made for a session being left is
    /// not one the new one asked for.
    pub(crate) pending_model: Cell<Option<(Served, String)>>,
    /// A mode shift+tab stepped to mid-turn, held for the turn the loop starts
    /// next.
    ///
    /// The runner holding the mode is on the worker for a running turn's
    /// length, so a step made then cannot reach it — it is held here and put
    /// on the runner at the next turn's start, when the runner is this side's
    /// again. The row under the box says the step at once, marked for the next
    /// turn, so the press is not dead and the row is not a lie about the mode
    /// the running turn is decided under.
    pub(crate) pending_mode: Cell<Option<Mode>>,
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
    /// Sets a provider up the way the launch set this run's up.
    ///
    /// `/login` is what calls it, handing back the keys it just wrote — so what
    /// the session asks from the next turn is what the next run here would ask,
    /// resolved once and out of the same files.
    pub(crate) serving: Serving,
    /// Where this machine keeps its session logs.
    pub(crate) sessions: PathBuf,
    /// The directory this conversation is about, which is what decides whose
    /// sessions are listed and which of them may be picked up.
    pub(crate) workspace: Workspace,
    /// Which press finishes a prompt, and which one opens a line under it.
    ///
    /// Read once at startup and never again: it is a fact about the keyboard in
    /// front of somebody, and no command changes it. Not a `Cell` for that
    /// reason, and not part of the style either — it is about what arrives from
    /// the terminal rather than about what is drawn to it.
    pub(crate) sending: Sending,
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
}

/// What a session leaves on the reader's own screen once it has gone.
///
/// Everything a session draws is drawn on a screen this process borrows and
/// hands back, so what a reader scrolls up to afterwards is the shell they
/// started from. This is the one thing written after the handing back: where
/// the rest of it went.
///
/// Decided in [`converse`], because that is the last place the session still
/// exists, and written by the caller, because the screen is the last guard to
/// be given back and nothing may reach the reader's own until it has.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Parting {
    /// Say nothing.
    ///
    /// Either no screen was taken — so the session drew into the reader's own
    /// scrollback and is still sitting there — or nothing was recorded, which
    /// is a run that asked not to be kept and has no session to come back to.
    Nothing,

    /// The transcript went with the screen, and this file holds all of it.
    Kept(PathBuf),

    /// The transcript went with the screen, and this file holds the part of it
    /// that reached the disk before the log stopped recording.
    Lost(PathBuf),
}

impl Parting {
    /// What a session that has just ended leaves behind.
    ///
    /// `borrowed` is whether a screen was taken, `written` is the file the
    /// session was recorded to and is absent in a run that asked not to be
    /// kept, and `problem` is the first write to that file that failed.
    ///
    /// A function of three values rather than three reads at the end of the
    /// loop, because two of them are only ever true on a real terminal: this is
    /// the whole of the decision, and it can be asked without one.
    fn of(borrowed: bool, written: Option<PathBuf>, problem: Option<&str>) -> Self {
        match written {
            Some(path) if borrowed && problem.is_none() => Self::Kept(path),
            Some(path) if borrowed => Self::Lost(path),
            _ => Self::Nothing,
        }
    }
}

/// Reads prompts and takes turns until input ends.
///
/// `input` is standard input in a real run. It is a parameter so that a test
/// can drive the loop: the deadlock this file has to avoid is one that only
/// shows up when a whole turn runs, and a hardwired stdin makes that unrunnable.
pub(crate) fn converse<T: Terminal>(
    mut runner: Runner,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    opening: &draw::opening::Standing,
    input: &mut dyn BufRead,
) -> Result<Parting, Fatal> {
    // Named once here because the prompt asks for it every frame: it is what the
    // row under the box counts, and what that loop wakes on a clock for while
    // there is anything left to end.
    let left = &terms.leaving;

    // First of the guards, so that it is the last of them given back: raw mode
    // is left while this is still held, and the sequence that leaves it goes to
    // the screen that is about to stop existing rather than to the reader's own.
    //
    // Taken here rather than where the renderer was built, because everything
    // between the two can still refuse to start a session — an unreadable
    // configuration, a provider nobody named, a home directory that would not
    // be made private — and a refusal written to a screen that is handed back
    // in the same breath is one nobody reads.
    let screen = Screen::take()?;

    // Whether the transcript is about to be taken away with the screen it was
    // drawn on, which is the whole of what decides if there is anything to say
    // on the way out. Read here rather than at the end, because it is a fact
    // about the start of the session and the binding above outlives the answer.
    let borrowed = screen.is_some();

    // Held for the whole session and dropped on the way out however this
    // returns. Between turns it is what draws the box; during one it is what
    // lets the box go on being typed into. `None` is a session with no terminal
    // at one end or the other, which reads whole lines instead.
    let raw = Raw::enter()?;
    let keys = raw.is_some();

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
    // emulator's own selection instead of this one.
    let _pointer = Reporting::on()?;

    // Everything the session keeps between turns and hands to each of them:
    // the line being typed, the lines finished behind it, what a result had no
    // room to say, the view over it, the plan, and where an answer comes from.
    // Held in one value for the reason its own prose gives.
    let mut held = Held::new(
        terms.plan.clone(),
        terms.sending,
        Answers { input, keys },
        opening,
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
    opening.commit(renderer)?;

    // What the session already said, before what it will be asked about it: a
    // resumed session is one the model can see and the reader cannot, and the
    // screen it is being read on was opened empty a moment ago.
    //
    // Before the first prompt, because a session picked up on the command line
    // reaches this loop the same way one picked up by `/resume` does, and the
    // question is about the session rather than about how it was reached.
    replaying::replayed(
        renderer,
        &runner,
        &terms.workspace,
        &mut held.kept,
        terms.style(),
    )?;

    // Answered rather than acted on. Making room is a request, and a request is
    // run the one way this file runs one — on a worker, with the box live under
    // it — which is the loop below. Carried in as a value so that the panel
    // above the loop and the command inside it reach the same code.
    let mut making = resuming::asked(renderer, &mut runner, terms, keys)?;

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

        // The fixed foot — the transcript-map door. Said here rather than
        // once at startup because a session that reopens the screen — a view,
        // a resize — is one that has to be told again.
        renderer.foots()?;

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

        // And a queue opened during it, for the same reason and one of its own:
        // the lines in it are the reader's until they close it, and the loop
        // below would otherwise commit the first of them while they were still
        // going over the rest.
        queueing::stand(
            renderer,
            style,
            queueing::Reading {
                queue: &mut held.queued,
                editor: &mut held.editor,
                steer: &terms.steer,
            },
            &mut held.viewing,
        )?;

        // Whatever the turn that just ended never reached is the queue's alone
        // now. The two hold the same lines while a turn runs — one to steer it,
        // one to answer once it is over — and a line left here is worked into
        // the *next* turn as well as being that turn's own prompt. After the
        // view above, because a queue still open is a queue still held.
        drop(terms.steer.take());

        // Before the queue, because a prompt typed while room was being made is
        // a prompt about a session that has had room made: sending it first
        // would spend the whole window this is here to free.
        if let Some(why) = making.take() {
            let (back, leaving) = ran(runner, renderer, terms, Work::Room(why), &mut held)?;
            runner = back;

            if leaving {
                break;
            }
            continue;
        }

        // The lines queued during the last turn are the next turn, all of them
        // at once: the oldest is its prompt and the rest are offered to it. They
        // are committed here rather than where they were typed: at that moment
        // the answer above them was still arriving, and a line written into the
        // middle of one is a line in the wrong place.
        if let Some(said) = batched(&mut held.queued, &terms.steer) {
            draw::queued(renderer, &said, style)?;

            let attached = attaching::beside(
                renderer,
                &runner,
                &terms.workspace,
                attaching::Sent {
                    prompt: &said,
                    images: &held.images,
                },
                style,
            )?;
            let work = Work::Turn(said, attached);
            let (back, leaving) = ran(runner, renderer, terms, work, &mut held)?;
            runner = back;

            // Left after the trouble `ran` says rather than instead of it: a
            // log that stopped recording is worth hearing about on the way out
            // as much as on the way through.
            if leaving {
                break;
            }
            continue;
        }

        let between = typing::Between {
            runner: &mut runner,
            editor: &mut held.editor,
            planning: &mut held.planning,
            recalling: &mut held.recalling,
            images: &mut held.images,
            left,
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

            Asked::Untyped => match unboxed(renderer, &runner, style, held.answers.input)? {
                Some(said) => (said, true),
                None => break,
            },
        };

        // Before the turn, because a command is not one: it is answered here,
        // on this thread, and costs the provider nothing. Nothing of it reaches
        // the transcript either — what the model is told about a session is
        // what was said to it, and `/help` was not.
        if local && let Some(wanted) = command::wanted(&prompt) {
            match command::run(wanted, renderer, &mut runner, &mut held, terms)? {
                Ran::Again => continue,
                Ran::Leave => break,
                Ran::Room(why) => {
                    making = Some(why);
                    continue;
                }
            }
        }

        if prompt.trim().is_empty() {
            continue;
        }

        // Before the turn and not inside it, because a turn with no model is
        // not a turn: the prompt would be recorded, a request would go out
        // naming nothing, and the vendor's refusal would describe a model name
        // that was never typed. `/model` is what changes this answer, so it is
        // said again here rather than only under the welcome the session opened
        // with — by now that has scrolled away.
        if runner.model().is_empty() {
            let said = unasked(terms.provider.get());

            // Down a pipe there is nobody to type `/model`, so carrying on
            // reads every remaining line and answers none of them — and ends
            // `Ok`, which is the one thing a script looks at. Said and failed
            // rather than said and shrugged.
            if !renderer.is_terminal() {
                return Err(Fatal::Unanswerable(said));
            }

            draw::unconfigured(renderer, said)?;
            continue;
        }

        let attached = attaching::beside(
            renderer,
            &runner,
            &terms.workspace,
            attaching::Sent {
                prompt: &prompt,
                images: &held.images,
            },
            terms.style(),
        )?;
        let work = Work::Turn(prompt, attached);
        let (back, leaving) = ran(runner, renderer, terms, work, &mut held)?;
        runner = back;

        if leaving {
            break;
        }
    }

    // Read before the drain below, which consumes the session. A session that
    // recorded nothing has no name and no file, and that is the same answer as
    // a session nothing was hidden from: there is nowhere to send the reader.
    let session = runner.into_session();
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
        draw::trouble(renderer, problem, terms.style())?;
    }

    renderer.settle()?;

    // Whatever was said about the log while the screen was still up went with
    // it, which is why the failure reaches here at all: pointing a reader at a
    // file and calling it the transcript would be the last thing crucible said
    // and false.
    Ok(Parting::of(borrowed, written, problem.as_deref()))
}

/// Says once that the session log stopped recording.
///
/// Once per session rather than once per turn: the log does not start working
/// again, so a line under every turn from here on would bury the turns it is
/// about. `told` is the loop's own memory of having said it, which is why it is
/// passed rather than read back off anything.
fn troubled<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &Runner,
    style: Style,
    told: &mut bool,
) -> Result<(), Fatal> {
    if !*told && let Some(problem) = runner.session().trouble() {
        draw::trouble(renderer, &problem, style)?;
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

/// Runs one piece of work and settles what came back.
///
/// The three places a turn or a compaction starts do the same things after it:
/// take the runner back, say once where the log has stopped recording, say
/// what a request for room that changed nothing came to, and find out whether
/// something pressed while it ran ends the session. `true` is the session
/// leaving.
fn ran<T: Terminal>(
    mut runner: Runner,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    work: Work,
    held: &mut Held<'_>,
) -> Result<(Runner, bool), Fatal> {
    // A model picked mid-turn is applied now, before the turn starts: the
    // runner is this side's again, and the pick was made for the request about
    // to go out rather than for the one already answered.
    if let Some((provider, name)) = terms.pending_model.take() {
        command::apply_model(renderer, &mut runner, terms, provider, &name)?;
    }

    // A mode stepped to mid-turn is put on the runner now, before the turn
    // starts: the runner is this side's again, and the step was made for the
    // requests about to go out rather than for the one already decided.
    if let Some(mode) = terms.pending_mode.take() {
        runner.switch(mode);
    }

    let took = take(runner, renderer, terms, work, held)?;
    let style = terms.style();

    troubled(renderer, &took.runner, style, &mut held.told)?;

    // Neither of the two that changed nothing posted anything, because neither
    // took anything: a compaction reports what it replaced, and these replaced
    // nothing. So this is the only place either can be said, and a command that
    // appears to run and changes nothing is one somebody types again.
    match took.did {
        Did::Reported => {}
        Did::Nothing => draw::unmade(renderer)?,
        Did::Stopped => draw::stopped(renderer)?,
    }

    Ok((
        took.runner,
        matches!(took.meanwhile, typing::Meanwhile::Leaving),
    ))
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
    /// The two channels a question is answered down.
    answering: &'a Answering,
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
                    self.turning.queueing(
                        self.held.queued.waiting_all(),
                        renderer.columns(),
                        self.terms.style(),
                    );
                }

                // And the line of a call whose tool has answered is written
                // before the event that ended it is drawn, so that the result
                // hangs under the call it answers. It goes out through its own
                // door rather than through `shown`, which is already at the
                // arguments this project allows one function.
                for (call, said) in returned {
                    if terminal {
                        // No result follows a terminal event. Remove the exact
                        // retained live call after committing its heading so it
                        // cannot leak into a later expansion.
                        self.held.kept.abandoned(&call);
                    }
                    if self.drawn.is_ok() {
                        *self.drawn = stop_if_failed(
                            draw::returned(renderer, &said, self.terms.style())
                                .map_err(Fatal::from),
                            &self.terms.cancel,
                        );
                    }
                }

                if self.drawn.is_ok() {
                    *self.drawn = stop_if_failed(
                        shown(one, renderer, self.terms, self.held, self.answering),
                        &self.terms.cancel,
                    );
                } else if matches!(one, Seen::Question { .. } | Seen::Asked { .. }) {
                    // Nothing is drawn and nothing is read once the terminal
                    // has failed, and both kinds of question still have to be
                    // answered or the worker waits for ever. A refusal and
                    // nobody-answered are what a drawing thread that has
                    // stopped means, said out loud rather than by going quiet.
                    let _ = self.answering.reply.send(verdict(None));
                    let _ = self.answering.give.send(None);
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
        // at the top of its next turn.
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
            match typing::during(
                renderer,
                typing::During {
                    background: &self.terms.leaving,
                    editor: &mut self.held.editor,
                    queued: &mut self.held.queued,
                    turning: self.turning,
                    planning: &mut self.held.planning,
                    kept: &mut self.held.kept,
                    opened: &mut self.held.opened,
                    viewing: &mut self.held.viewing,
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
        match command.class() {
            command::MidTurn::Live => self.live(renderer, command),
            command::MidTurn::Deferred => self.deferred(renderer, command),
            command::MidTurn::Refused(why) => {
                command::refused(renderer, command.command(), why, self.terms.style()).map(|_| ())
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
        command::live(renderer, self.terms, command, &mut |renderer| {
            self.drain(renderer);
            Ok(())
        })
    }

    /// Runs a command whose pick the turn started next applies.
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

        let current = self.says.model.clone();
        let picked = command::deferred(renderer, self.terms, &current, command, &mut |renderer| {
            self.drain(renderer);
            Ok(())
        })?;
        if let Some(command::Kept::Model(provider, name)) = picked {
            self.terms.pending_model.set(Some((provider, name)));
        }
        Ok(())
    }
}

fn take<T: Terminal>(
    mut runner: Runner,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    work: Work,
    held: &mut Held<'_>,
) -> Result<Took, Fatal> {
    let (post, seen) = sync_channel(CAPACITY);
    let (answering, hear) = Answering::new(&terms.putting, &post);
    let mut seen = Inbox::new(seen);

    let asking = Asking::new(post.clone(), hear);
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
    let mut says = typing::under(&runner);

    // Read here for the same reason and at the same moment: half of what a turn
    // is asked under is about the session — which model is answering, and how
    // hard it was asked to think — and a model can find out neither for itself.
    // Written once at startup it would go on describing the session the first
    // turn was taken in, so it is written again for each.
    // Drained here rather than when it happened: the reader was told at the
    // moment, and this is the other audience being told at the one moment there
    // is room for it. A turn already in flight has nowhere to put a new fact.
    runner.telling(&standing::under(
        runner.model(),
        runner.effort(),
        &terms.workspace,
        &terms.leaving.reported(),
    ));

    // Whatever stopped the last turn is spent, and this is the last moment at
    // which clearing it can be certain of that: from the next line on there are
    // two threads, one of them reading the keyboard. A press arriving after
    // this is a press about the turn below, which is what the turn does with a
    // flag it finds raised.
    terms.cancel.reset();

    // Started before the worker rather than on the first thing it reports, so
    // that what the clock measures is what somebody is waiting for. A turn that
    // spends its first ten seconds connecting has spent them.
    let mut turning = Turning::started(says.left);

    // A turn can start with prompts already behind it: room is made before the
    // queue is read, so a line typed during the last turn is still waiting when
    // this one is about making room for it. Read before the first frame, so the
    // panel naming what is coming is right on the frame it first appears in.
    turning.queueing(held.queued.waiting_all(), renderer.columns(), terms.style());

    let working = sent(runner, work, asking, relay, running, terms.steer.clone())?;

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
                opened_list: &held.opened_list,
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
        answering: &answering,
        drawn: &mut drawn,
        meanwhile: &mut meanwhile,
        leaving: &mut leaving,
        terms,
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

    let (runner, did) = working.join().map_err(|_| Fatal::Lost)?;
    drawn.map(|()| Took {
        runner,
        meanwhile,
        did,
    })
}

/// Sends the work away on its own thread, with the runner.
///
/// The runner goes with it and comes back beside what it found to do, which is
/// what makes the transcript and the permission memory survive a turn without
/// being shared between threads. Nothing on this side waits on the provider,
/// which is what keeps the box under the turn live while it runs.
// The cancel and the steer are both read off the worker the turn runs on, and
// bundling them would drag the two through every signature the cancel crosses.
// The lint counts to five; the worker needs six.
#[allow(clippy::too_many_arguments)]
fn sent(
    mut runner: Runner,
    work: Work,
    mut asking: Asking,
    relay: Relay,
    running: Cancel,
    steer: crucible_core::Steer,
) -> Result<thread::JoinHandle<(Runner, Did)>, Fatal> {
    thread::Builder::new()
        .name("turn".to_owned())
        .spawn(move || {
            // The runner reports what happened and returns why it stopped;
            // nothing else has posted the failure, so this is where it becomes
            // visible.
            let did = match work {
                Work::Turn(prompt, attached) => {
                    if let Err(problem) =
                        runner.turn(&prompt, attached, &mut asking, &relay, &running, &steer)
                    {
                        relay.post(Event::Failed { error: problem });
                    }
                    Did::Reported
                }

                // The same shape, and that is the whole of why this is one
                // function: making room is one request, answered over seconds,
                // reporting as it goes. Everything the loop that draws does for
                // a turn — the bar, the clock, the box taking the next prompt,
                // the key that stops it — is what a reader waiting on a
                // compaction needs, and none of it is about a turn.
                // No turn is running, so the reading starts at nothing and
                // what it comes to is the recap request's own cost — posted
                // on the way, which is all the row above the box asks.
                Work::Room(why) => {
                    match runner.compact(why, &relay, &running, &mut Spend::default()) {
                        Ok(Room::Made(_)) => Did::Reported,
                        Ok(Room::Nothing) => Did::Nothing,
                        Ok(Room::Stopped) => Did::Stopped,
                        Err(problem) => {
                            relay.post(Event::Failed { error: problem });
                            Did::Reported
                        }
                    }
                }
            };

            (runner, did)
        })
        .map_err(Fatal::Worker)
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
}

/// A turn, and what the keyboard asked for while it ran.
struct Took {
    /// The runner, back from the worker that held it for the turn.
    runner: Runner,
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

/// Prompts finished while a turn is still running.
///
/// Kept in order, and every one of them kept — the other two answers were both
/// wrong for the same reason: keeping one line and dropping the rest loses
/// something the user typed, watched the box take, and never sees again, and
/// joining them into one prompt puts a message in the transcript nobody wrote.
///
/// Lines and bytes are both bounded: one-byte prompts cannot choose an
/// unbounded number of allocations, and full-sized prompts cannot choose an
/// unbounded retained buffer. Refusal leaves the editor untouched, so a prompt
/// is never silently dropped after the box appeared to accept it.
#[derive(Debug, Default)]
struct Prompts {
    lines: VecDeque<String>,
    bytes: usize,
}

/// Whether a finished line moved from the editor into [`Prompts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retained {
    /// The line is waiting for its turn.
    Accepted,
    /// A line or byte ceiling left it in the editor.
    Refused,
}

impl Prompts {
    /// Takes the editor whole where both ceilings have room.
    fn accept(&mut self, editor: &mut Editor) -> Retained {
        let bytes = editor.text().len();
        if self.lines.len() >= QUEUED_LINES || bytes > QUEUED_BYTES.saturating_sub(self.bytes) {
            return Retained::Refused;
        }

        self.bytes += bytes;
        self.lines.push_back(editor.take());
        Retained::Accepted
    }

    /// The prompt the next turn will be taken from, where one is waiting.
    ///
    /// Read while the turn ahead of it is still running, for the row that says
    /// what is coming after it. A line that went into the box and vanished is
    /// the thing this exists to stop: the queue is the only place it is, and
    /// until it is named there is nothing on screen to say it was kept.
    #[cfg(test)]
    fn waiting(&self) -> Option<&str> {
        self.lines.front().map(String::as_str)
    }

    /// Every prompt waiting, oldest first.
    ///
    /// The panel above the box is drawn from these: the second and third are as
    /// much queued as the first, and a list that named only the front one said
    /// the rest were not there.
    fn waiting_all(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }

    /// How many prompts are waiting.
    fn waiting_count(&self) -> usize {
        self.lines.len()
    }

    /// Drops the prompt `at` places back, releasing its byte reservation.
    ///
    /// What the queue's full view removes one with: a line typed and not yet
    /// sent is the reader's to take back until the turn takes it. `None` where
    /// there is no such place.
    fn drop(&mut self, at: usize) -> Option<String> {
        let prompt = self.lines.remove(at)?;
        self.bytes = self.bytes.saturating_sub(prompt.len());
        Some(prompt)
    }

    /// Drops the oldest waiting prompt that says `line`, and answers whether
    /// there was one.
    ///
    /// What a turn taking a line to steer by leaves behind. From the moment it
    /// is taken the line is in the transcript, so a panel that goes on naming
    /// it says the reader is owed a turn they have already had — and the count
    /// beside it says so twice. Matched on what the line says rather than on
    /// where it sat, because the reader may have taken an earlier one back
    /// between the turn reading the queue and saying what it read.
    fn steered(&mut self, line: &str) -> bool {
        let Some(at) = self.lines.iter().position(|waiting| waiting == line) else {
            return false;
        };

        self.drop(at).is_some()
    }

    /// Takes the oldest waiting prompt and releases its byte reservation.
    fn pop(&mut self) -> Option<String> {
        let prompt = self.lines.pop_front()?;
        self.bytes = self.bytes.saturating_sub(prompt.len());
        Some(prompt)
    }
}

/// Takes the whole queue for one turn: the oldest line is the prompt, and every
/// line behind it is offered to that same turn.
///
/// A burst typed behind a turn is one thing the reader wanted said. Taken a line
/// per turn, the first was answered before the model had read the second, so the
/// agent worked to a question the reader had already added to — and three turns
/// went by answering what was asked once. Handed over together, the runner
/// records the batch at the first boundary of the turn this starts, which is
/// before it asks anything: the model reads all of it and then answers all of
/// it.
///
/// The lines behind the prompt go through the steer rather than into it,
/// because joining them would put a message in the transcript nobody wrote.
/// Each reaches it as the line it was typed as; what they share is the turn.
///
/// `None` where nothing is waiting, which is the ordinary case.
fn batched(queued: &mut Prompts, steer: &crucible_core::Steer) -> Option<String> {
    let said = queued.pop()?;

    while let Some(behind) = queued.pop() {
        steer.say(behind);
    }

    Some(said)
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
    /// each, which is what [`batched`] does with it.
    queued: Prompts,
    /// What the transcript had no room to say, waiting for Ctrl+O. Held for the
    /// whole session rather than for a turn: the row offering the key is read
    /// after the turn that drew it has ended, which is when there is time to
    /// read anything.
    kept: Kept,
    /// Whether the reader is standing that under the turn, and where over it. A
    /// view opened while a turn ran is still open when the turn ends, in the
    /// region the box comes back to, and the reader who opened it is reading.
    opened: Standing,
    /// The command list a line typed mid-turn has open above the box, empty
    /// while the line is a prompt.
    opened_list: typing::Opened,
    /// Whether the queue above is standing open to be gone over, and where the
    /// mark is down it.
    ///
    /// Held here for the reason beside it, and for one more: while it stands
    /// the turn takes none of the lines it names, so a view outliving the turn
    /// that was under it is what keeps the queue from being committed out from
    /// under a reader who was halfway through it.
    viewing: queueing::Standing,
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
    /// paste puts `[image N]` in the line and the path of the Nth here, and a
    /// prompt saying the marker sends the image. For the session rather than
    /// for a prompt, so a later prompt can still say an earlier number.
    images: Vec<Box<str>>,
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
            kept: Kept::default(),
            opened: Standing::default(),
            opened_list: typing::Opened::default(),
            viewing: queueing::Standing::default(),
            listing: leaving::Leaving::default(),
            planning: Planning::new(plan),
            // Nothing to reach back through and nowhere to write. The session
            // that has a directory to read one out of puts it here itself:
            // every other holder of a `Held` is a test of something else.
            recalling: Recalling::default(),
            images: Vec::new(),
            told: false,
            answers,
            opening,
        }
    }
}

/// How a permission question gets answered.
struct Answers<'a> {
    /// Standard input, for a session with no terminal to read keys from.
    input: &'a mut dyn BufRead,
    /// Whether keys are being read rather than lines.
    keys: bool,
}

/// Where the two kinds of answer go back.
///
/// Two channels rather than one carrying both: a verdict and an ask's answers
/// are different types, and one channel would need a union nothing keeps apart.
/// One value rather than two arguments, because they are made together, handed
/// over together, and neither is ever the other's.
struct Answering {
    /// Where a permission verdict goes.
    reply: Sender<Answer>,
    /// Where an ask's answers go.
    give: Sender<Given>,
}

impl Answering {
    /// Both reply channels for one turn, with the ask's end already lent to
    /// whoever asks.
    ///
    /// Fresh for each turn, because a reply channel that outlived its turn could
    /// hand the next question an answer meant for the last one. Making the
    /// channel and lending its far end are one act here, so there is no state in
    /// between where one exists and the other has not been given away.
    fn new(putting: &Putting, post: &SyncSender<Seen>) -> (Self, Receiver<Answer>) {
        let (reply, hear) = channel();
        let (give, given) = channel();
        putting.open(post.clone(), given);

        (Self { reply, give }, hear)
    }
}

/// One answer to one question.
///
/// A key where there is a keyboard, because raw mode is held for the whole
/// session now and a line-reading terminal is not collecting one. The letter is
/// written out afterwards, since nothing echoed it: an answer that left no mark
/// would leave the record showing a question and no reply.
///
/// This is the one place in the session that waits on a key with no clock on
/// it: the question stands until somebody decides. So it is also the longest a
/// window can change without anybody noticing, which is why the key that says
/// it did is acted on here rather than passed over with the arrows.
fn answered<T: Terminal>(
    renderer: &mut Renderer<T>,
    answers: &mut Answers<'_>,
) -> Result<Answer, Fatal> {
    if !answers.keys {
        return Ok(verdict(read(answers.input)?.as_deref()));
    }

    loop {
        let Some(arrived) = renderer.pressed()? else {
            continue;
        };
        let said = match heard(arrived) {
            Heard::Said(said) => said,

            // The renderer is holding a size the screen no longer has, and
            // every band it shares the window out into is measured against it.
            // Taking the new one is the whole of the answer: the question is
            // committed, so its rows are folded again at the width the window
            // has now, and the frame that follows puts them back.
            Heard::Resized => {
                renderer.resized()?;
                continue;
            }

            // The question is committed too, which is what makes the wheel
            // worth answering here: a reader deciding whether to allow a call
            // is reading what was said above it to decide.
            Heard::Scrolled { back } => {
                renderer.notched(back)?;
                continue;
            }

            Heard::Ignored => continue,
        };

        draw::answered(renderer, &said)?;
        return Ok(verdict(Some(&said)));
    }
}

/// What one key pressed at a question means.
///
/// Three answers rather than two, and the third is the whole reason this is a
/// function: a key that is not an answer is not therefore nothing. Written apart
/// from the loop above because that loop cannot be driven from a test — the
/// keyboard it reads is the process's own — and this much of the reading can be.
enum Heard {
    /// What was typed, to be read as a verdict. Empty is a refusal.
    Said(String),
    /// The window changed under the question.
    Resized,
    /// The wheel turned, so the transcript the question stands at the foot of
    /// moves. `back` is towards the top of the session.
    Scrolled {
        /// Whether the notch was towards the top of the session.
        back: bool,
    },
    /// Not an answer and not news. Wait for the next key.
    Ignored,
}

/// Reads one key as an answer, as news, or as neither.
///
/// Anything that is not one of the letters is a refusal, which is what an
/// unrecognised line already meant. Escape and Ctrl-C are spelled out among
/// them so that the way out of a question is the way out of everything else.
///
/// Every remaining key is named rather than caught by a rest arm. A key that
/// arrives while a permission question is on screen is either an answer or
/// something this has decided to ignore, and a new one added to `Pressed` must
/// be decided about here rather than silently join the second group.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
fn heard(arrived: Pressed) -> Heard {
    match arrived {
        Pressed::Key(Key::Char(letter)) => Heard::Said(letter.to_string()),
        Pressed::Key(Key::Interrupt | Key::Eof | Key::Enter) | Pressed::Escape => {
            Heard::Said(String::new())
        }

        Pressed::Resized => Heard::Resized,
        Pressed::Scrolled { back } => Heard::Scrolled { back },

        // An arrow, a click, a mode step, a key that means nothing here — the key
        // about what is already running among them, since this question is what
        // decides whether the command runs at all. None of them is an answer, and
        // none of them may be read as one.
        //
        // Ctrl+E is here for now rather than because it belongs here: the
        // question joins the transcript a row at a time, and there is no second
        // shape of it to toggle into until the panel is what draws it.
        //
        // Ctrl+O is here because this is the question with nowhere to stand: it
        // was put a row at a time precisely because there was no room for a
        // panel, and a view of what was cut needs more room than the panel did.
        //
        // Ctrl+T for the same reason as Ctrl+E: the question took the rows the
        // plan stands in, so there is no panel on screen for the key to open.
        //
        // And Ctrl+Y, which copies the line in the box: this question is what
        // the reader is answering, and no line is being typed behind it.
        //
        // And the key that crosses regions, which needs regions to cross: this
        // question was put a row at a time precisely because there was no room
        // for a panel, and a row at a time is not divided into anything.
        Pressed::Key(_)
        | Pressed::Up
        | Pressed::Down
        | Pressed::Background
        | Pressed::Cycle
        | Pressed::Tab
        | Pressed::Explain
        | Pressed::Expand
        | Pressed::Plan
        | Pressed::Pasted(_)
        | Pressed::Clicked { .. }
        | Pressed::Queue
        | Pressed::Copy
        | Pressed::PasteImage
        | Pressed::Rename
        | Pressed::Dragged { .. }
        | Pressed::Hovered { .. }
        | Pressed::Released { .. }
        | Pressed::Ignored => Heard::Ignored,
    }
}

/// Draws one thing the worker sent, and answers it if it was a question.
fn shown<T: Terminal>(
    one: Seen,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    held: &mut Held<'_>,
    answering: &Answering,
) -> Result<(), Fatal> {
    let Answering { reply, give } = answering;
    let style = terms.style();

    match one {
        Seen::Turn(event) => {
            draw::event(renderer, event, &terms.workspace, style, &mut held.kept)?;
        }
        Seen::Question { call, sensitivity } => {
            // A durable rule cannot live in either project configuration file:
            // both names can arrive with a checkout, whatever an ignore rule
            // says. Until policy has a per-workspace store outside the checkout,
            // the prompt offers only answers this process can honour.
            let answer = asked(renderer, &call, &sensitivity, &mut held.answers, style);
            let answer = match answer {
                Ok(answer) => answer,
                Err(problem) => {
                    // This question has already left the channel, so the drain
                    // cannot encounter and refuse it again. Silence must still
                    // be a refusal or the worker waits forever beside the
                    // terminal failure this returns.
                    let _ = reply.send(verdict(None));
                    return Err(problem);
                }
            };

            // A worker that stopped waiting has already denied itself.
            let _ = reply.send(answer);
        }
        Seen::Asked { questions } => {
            // A loop reading lines rather than keys has nobody to put a panel
            // to, and neither has one whose raw mode never came up. The tool is
            // not registered in either, so this is the belt rather than the
            // braces — but a panel that read keys nobody is at would wait for
            // ever, and waiting for ever is the one failure this loop may not
            // have.
            if !held.answers.keys {
                let _ = give.send(None);
                return Ok(());
            }

            let given = putting::put(renderer, style, &questions);
            let given = match given {
                Ok(given) => given,
                Err(problem) => {
                    // This ask has already left the channel, so the drain
                    // cannot meet it again. Nobody answered is what the worker
                    // has to hear, or it waits for ever beside the terminal
                    // failure this returns.
                    let _ = give.send(None);
                    return Err(problem);
                }
            };

            let given = match given {
                putting::Put::Said(answered) => Some(answered),
                putting::Put::Left => None,

                // Nothing was drawn and no key was read, so the questions still
                // have to be put: a window this small is not somebody saying no.
                putting::Put::Cramped => match cramped(renderer, &questions, style) {
                    Ok(given) => given,
                    Err(problem) => {
                        let _ = give.send(None);
                        return Err(problem);
                    }
                },
            };

            let _ = give.send(given);
        }
    }

    Ok(())
}

/// Puts one question and waits for its answer.
///
/// The panel where there is a keyboard and room to stand one; otherwise the
/// question a row at a time, which is what a redirected run and a window with
/// four rows both get.
fn asked<T: Terminal>(
    renderer: &mut Renderer<T>,
    call: &ToolCall,
    sensitivity: &Sensitivity,
    answers: &mut Answers<'_>,
    style: Style,
) -> Result<Answer, Fatal> {
    if answers.keys {
        // Read here rather than carried from the worker, because the panel is
        // the only thing that shows it: the row a call gets in the transcript
        // is drawn from what the tool made of the arguments, and this is the
        // model's own sentence about them.
        let account = crucible_tools::account(&call.args);

        // Nothing is drawn under it. The panel stood over the transcript and
        // the rows it covered are back, so a call that was allowed reads exactly
        // like one nothing asked about — which is what the reader is looking at
        // anyway, since the result of the call is the row underneath.
        match asking::ask(renderer, style, call, sensitivity, &account)? {
            asking::Answered::Said(answer) => return Ok(answer),

            // Nothing was drawn and no key was read, so the question still has
            // to be put. Falling through rather than refusing: a window this
            // small is not somebody saying no.
            asking::Answered::Cramped => {}
        }
    }

    draw::question(renderer, call, sensitivity, style)?;
    answered(renderer, answers)
}

/// Puts the questions a row at a time, where there was no room to stand a panel.
///
/// One key per question, which is what a window this small can offer: the panel
/// adds an answer somebody writes themselves, and writing one wants a line
/// editor and the rows to draw it in. Anything that is not one of the numbers
/// leaves the whole ask, the way escape does on the panel.
fn cramped<T: Terminal>(
    renderer: &mut Renderer<T>,
    questions: &[Question],
    style: Style,
) -> Result<Given, Fatal> {
    let mut given = Vec::new();

    for (at, question) in questions.iter().enumerate() {
        draw::asking(renderer, question, at, questions.len(), style)?;

        let Some(taken) = took(renderer, question)? else {
            return Ok(None);
        };

        draw::answered(renderer, taken.answer())?;
        given.push(Answered::new([taken.answer()]));
    }

    Ok(Some(given))
}

/// Reads one key as one of `question`'s answers, or nothing where it means to
/// leave.
fn took<T: Terminal>(
    renderer: &mut Renderer<T>,
    question: &Question,
) -> Result<Option<Chosen>, Fatal> {
    loop {
        let Some(arrived) = renderer.pressed()? else {
            continue;
        };

        match numbered(arrived, question) {
            Numbered::Chose(answer) => return Ok(Some(answer)),
            Numbered::Left => return Ok(None),

            // The renderer is holding a size the screen no longer has, and the
            // question after this one would be drawn against it — a row folded
            // for a window that has gone. Taking the new size is the whole of
            // the answer, the same as it is one question above: the rows already
            // committed are folded again at the width the window has now, and
            // the next frame puts them back.
            Numbered::Resized => renderer.resized()?,
        }
    }
}

/// What one key pressed at a question put a row at a time means.
///
/// Three answers for the reason [`Heard`] has three: a key that is not one of
/// the numbers is not therefore nothing, and the one that says the window
/// changed has to be told apart from the one that says leave. Written apart
/// from the loop above because that loop cannot be driven from a test — the
/// keyboard it reads is the process's own — and this much of the reading can
/// be.
enum Numbered {
    /// The answer whose number was typed.
    Chose(Chosen),
    /// The window changed under the question.
    Resized,
    /// Anything else, which leaves the whole ask.
    Left,
}

/// Reads one key as one of `question`'s numbered answers.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
fn numbered(arrived: Pressed, question: &Question) -> Numbered {
    if arrived == Pressed::Resized {
        return Numbered::Resized;
    }

    let Pressed::Key(Key::Char(typed)) = arrived else {
        return Numbered::Left;
    };

    let at = usize::try_from(typed.to_digit(10).unwrap_or(0))
        .ok()
        .and_then(|digit| digit.checked_sub(1));

    at.and_then(|at| question.answers().nth(at))
        .map_or(Numbered::Left, |answer| Numbered::Chose(answer.clone()))
}

/// Reads one bounded line, or `None` at end of input.
///
/// `fill_buf` exposes each source block before any of it is copied, so a pipe
/// with no newline can cross the prompt bound without making this process
/// retain the rest first.
fn read(input: &mut dyn BufRead) -> Result<Option<String>, Fatal> {
    let mut line = Vec::new();

    loop {
        let available = input.fill_buf().map_err(Fatal::Input)?;
        if available.is_empty() {
            break;
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |at| at + 1);
        if take > QUEUED_BYTES.saturating_sub(line.len()) {
            return Err(Fatal::InputTooLong);
        }

        line.extend_from_slice(available.get(..take).unwrap_or_default());
        input.consume(take);
        if newline.is_some() {
            break;
        }
    }

    if line.is_empty() {
        return Ok(None);
    }

    String::from_utf8(line)
        .map(Some)
        .map_err(|problem| Fatal::Input(io::Error::new(io::ErrorKind::InvalidData, problem)))
}

/// What an answer to a permission question means.
///
/// Anything unrecognised is a refusal, and so is end of input. Every way to say
/// yes is explicit; everything else, including a typo and a closed pipe, leaves
/// the tool unrun.
///
/// Durable rules have no trusted per-workspace store yet, so `always` is not an
/// answer. Treating it as a session answer would promise more than was kept.
fn verdict(answer: Option<&str>) -> Answer {
    match answer.map(str::trim) {
        Some("y" | "Y" | "yes") => (Verdict::Allow, Remember::Never),
        Some("s" | "S" | "session") => (Verdict::Allow, Remember::Session),
        _ => (Verdict::Deny, Remember::Never),
    }
}

#[cfg(test)]
mod tests;
