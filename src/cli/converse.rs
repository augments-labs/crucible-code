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

use std::cell::Cell;
use std::collections::VecDeque;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel, sync_channel};
use std::thread;
use std::time::Duration;

use crucible_auth::Store;
use crucible_core::{
    Answer as Chosen, Answered, Cancel, Event, Post as _, Question, Remember, Revealed,
    Sensitivity, ToolCall, Verdict, Workspace,
};
use crucible_runner::Runner;
use crucible_tools::{Background, Ledger, Plan};
use crucible_tui::{Editor, Key, Pressed, Raw, Renderer, Reporting, Terminal, pressed};

use super::draw;
use super::kept::Kept;
use super::seen::{Answer, Asking, CAPACITY, Given, Inbox, Putting, Relay, Seen};
use super::style::Style;
use super::subscription::Subscriptions;
use super::unasked;
use super::{Fatal, Serving, standing};
use command::Ran;
use expanding::Standing;
use planning::Planning;
use turning::Turning;
use typing::Asked;
use typing::between;

mod asking;
mod command;
mod expanding;
mod leaving;
mod mode;
mod picking;
mod planning;
mod putting;
mod region;
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
    /// Whether to write colour, and how much of a tool call to show.
    pub(crate) style: Style,
    /// What stops a turn.
    pub(crate) cancel: Cancel,
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
    input: &mut dyn BufRead,
) -> Result<(), Fatal> {
    let style = terms.style;

    // Named once here because the prompt asks for it every frame: it is what the
    // row under the box counts, and what that loop wakes on a clock for while
    // there is anything left to end.
    let left = &terms.leaving;

    // Held for the whole session and dropped on the way out however this
    // returns. Between turns it is what draws the box; during one it is what
    // lets the box go on being typed into. `None` is a session with no terminal
    // at one end or the other, which reads whole lines instead.
    let raw = Raw::enter()?;
    let keys = raw.is_some();

    // Held beside it and for as long, because a click means something at both
    // ends of a turn: it puts the cursor where the pointer is in the box
    // between turns, and it expands a cut result up in the transcript while one
    // runs. The rows worth clicking are written by a turn, so a guard taken per
    // prompt would hand the pointer back exactly where it has something to
    // point at.
    //
    // The wheel is what that costs. A terminal forwarding buttons is not using
    // them itself, so scrolling back over the transcript stops working for as
    // long as this is held, and `output.mouse` left alone is how a reader who
    // scrolls more than they expand keeps it.
    let _pointer = style.clicks().then(Reporting::on).transpose()?.flatten();

    // One line for the whole session rather than one per prompt. What was typed
    // while a turn ran is still there when it ends, and the allocation the last
    // line grew to is the one the next starts in.
    let mut editor = Editor::new();

    // Lines finished while a turn was still running. They are the next prompts,
    // in the order they were typed, and each is taken without asking for
    // another.
    let mut queued = Prompts::default();

    // What the transcript had no room to say, waiting for Ctrl+O. Held for the
    // whole session rather than for a turn: the row offering the key is read
    // after the turn that drew it has ended, which is when there is time to read
    // anything.
    let mut kept = Kept::default();

    // Whether that view is standing. Held beside what it stands over rather
    // than by either loop that draws it: a view opened while a turn ran is
    // still open when the turn ends, and the reader who opened it is reading.
    let mut opened = Standing::default();

    // This side of the plan the tools were built with. Made once for the
    // session, because what it holds is a copy of the plan and the setting of
    // the key that opens it, and both outlive every turn.
    let mut planning = Planning::new(terms.plan.clone());

    // Whether the log's trouble has been said. Once is all it is worth, for
    // the reason `troubled` gives.
    let mut told = false;

    loop {
        // The window may have changed while the last turn was streaming. The
        // box notices a resize as it happens, because in raw mode the terminal
        // reports one; between turns there is nobody reading, so it is noticed
        // here instead.
        renderer.resized()?;

        // A view opened during the last turn is still open, and it was standing
        // in the rows the box is about to take. So it moves into the region
        // here and reads keys of its own until it is closed, and what comes
        // after it is the box with the line still in it. Nothing was written
        // into the transcript on either side of it.
        //
        // The one door for both halves of the key: what Ctrl+O opens at the
        // prompt is stood here too, so the view a reader closes is the same
        // view whichever press put it up.
        expanding::stand(renderer, style, &kept, &mut opened)?;

        // A line queued during the last turn is the prompt, and nothing is
        // asked. It is committed here rather than where it was typed: at that
        // moment the answer above it was still arriving, and a line written
        // into the middle of one is a line in the wrong place.
        if let Some(said) = queued.pop() {
            draw::queued(renderer, &said, style)?;

            let took = take(
                runner,
                renderer,
                terms,
                said,
                Taking {
                    editor: &mut editor,
                    queued: &mut queued,
                    kept: &mut kept,
                    opened: &mut opened,
                    planning: &mut planning,
                    answers: Answers { input, keys },
                },
            )?;
            runner = took.runner;
            troubled(renderer, &runner, style, &mut told)?;

            // Said after the trouble above rather than instead of it: a log
            // that stopped recording is worth hearing about on the way out as
            // much as on the way through.
            if matches!(took.meanwhile, typing::Meanwhile::Leaving) {
                break;
            }
            continue;
        }

        let between = between(&mut runner, &mut editor, &mut planning, left, keys);
        let asked = typing::ask(renderer, style, between)?;

        // Answered by the state that holds what it stands over, because the loop
        // that read the key holds neither. The box comes back either way, with the
        // line still in it.
        if opened.asked(&asked, &kept) {
            continue;
        }

        let prompt = match asked {
            Asked::Said(said) => said,
            Asked::Ended => break,

            // Taken above, by the state that holds what it stands over.
            Asked::Expand | Asked::Clicked(_) => continue,

            Asked::Untyped => match unboxed(renderer, &runner, style, input)? {
                Some(said) => said,
                None => break,
            },
        };

        // Before the turn, because a command is not one: it is answered here,
        // on this thread, and costs the provider nothing. Nothing of it reaches
        // the transcript either — what the model is told about a session is
        // what was said to it, and `/help` was not.
        if let Some(wanted) = command::wanted(&prompt) {
            match command::run(wanted, renderer, &mut runner, terms, keys)? {
                Ran::Again => continue,
                Ran::Leave => break,
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

            draw::unconfigured(renderer, said, style)?;
            continue;
        }

        let took = take(
            runner,
            renderer,
            terms,
            prompt,
            Taking {
                editor: &mut editor,
                queued: &mut queued,
                kept: &mut kept,
                opened: &mut opened,
                planning: &mut planning,
                answers: Answers { input, keys },
            },
        )?;
        runner = took.runner;
        troubled(renderer, &runner, style, &mut told)?;

        if matches!(took.meanwhile, typing::Meanwhile::Leaving) {
            break;
        }
    }

    // The writer thread is usually still holding the last turn when the loop
    // ends, so the poll above cannot be relied on to have seen a failure
    // recorded during it. Draining here is what stops the one turn most likely
    // to matter from being the one nobody is told about.
    //
    // Its own statement, and not the first half of the condition below, because
    // the drain is what puts the last turn on the disk: it has to happen
    // whether or not anything has already been said, and a condition is
    // something a later edit can reorder into not happening at all.
    let problem = runner.into_session().finish();

    if let Some(problem) = problem
        && !told
    {
        draw::trouble(renderer, &problem, style)?;
    }

    renderer.settle()?;
    Ok(())
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
    draw::mark(renderer, &format!("{} {mark} ", runner.mode()), style)?;

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

/// One turn, start to finish.
///
/// The runner goes to the worker and comes back, which is what makes the
/// transcript and the permission memory survive a turn without being shared
/// between threads. It is also why a failure on this side is held to the end of
/// the turn rather than returned where it happens: the worker owns the runner,
/// the runner owns the session, and the session's log is finished by a thread
/// its `Drop` waits for. Leaving early would drop the join handle and detach
/// all three, and the process would exit over a log still being written.
fn take<T: Terminal>(
    mut runner: Runner,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    prompt: String,
    mut taking: Taking<'_>,
) -> Result<Took, Fatal> {
    let (post, seen) = sync_channel(CAPACITY);
    let (answering, hear) = Answering::new(&terms.putting, &post);
    let mut seen = Inbox::new(seen);

    let mut asking = Asking::new(post.clone(), hear);
    let relay = Relay::new(post, terms.putting.clone());
    let running = terms.cancel.clone();

    // The box stands under the turn, where it stands under the prompt the rest
    // of the time, and the mode stands under the box. A turn is the longest a
    // session goes without a prompt on screen, and it is the stretch the mode is
    // deciding things over: what a tool call arriving in the middle of it costs
    // is exactly which mode is in force, and reading that off the screen must
    // not mean remembering it.
    //
    // Read here because the runner is about to leave, and nothing changes the
    // mode while it is away. Drawn below rather than here, where a failure would
    // be a turn that never ran.
    //
    // The model beside it for the same two reasons: the row says it, and only
    // `/model` and `/effort` change it — neither of which can be run while the
    // turn they would change is the one running.
    let mut says = typing::under(&runner, terms.style.glyphs());

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
    let mut turning = Turning::started();

    // A turn can start with prompts already behind it: the loop above took one
    // of them and left the rest. Read before the first frame, so the row naming
    // what is coming is right on the frame it first appears in.
    turning.queueing(taking.queued.waiting(), renderer.columns(), terms.style);

    let working = thread::Builder::new()
        .name("turn".to_owned())
        .spawn(move || {
            // The runner reports what happened and returns why it stopped;
            // nothing else has posted the failure, so this is where it becomes
            // visible.
            if let Err(problem) = runner.turn(prompt.trim(), &mut asking, &relay, &running) {
                relay.post(Event::Failed { error: problem });
            }

            runner
        })
        .map_err(Fatal::Worker)?;

    // The first thing drawn, and held like everything drawn after it: the runner
    // is with the worker now, so a terminal that failed here has to be carried
    // to the end of the turn rather than returned from the middle of one.
    let mut held = stop_if_failed(
        typing::stand(
            renderer,
            taking.editor,
            typing::Footing {
                turning: &turning,
                planning: taking.planning,
            },
            &says,
            terms.style,
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
    loop {
        match seen.recv_timeout(TICK) {
            Ok(one) => {
                // Before it is drawn, because drawing consumes it. The row
                // above the box says what the turn is doing, and this is the
                // only place that can be read off.
                let mut returned = None;
                if let Seen::Turn(event) = &one {
                    returned = turning.saw(event);
                }

                // And the line of a call whose tool has answered is written
                // before the event that ended it is drawn, so that the result
                // hangs under the call it answers. It goes out through its own
                // door rather than through `shown`, which is already at the
                // arguments this project allows one function.
                if held.is_ok()
                    && let Some(said) = returned
                {
                    held = stop_if_failed(
                        draw::returned(renderer, &said, terms.style).map_err(Fatal::from),
                        &terms.cancel,
                    );

                    // And the same line is what a result too long for its row
                    // is held under, since that is how the reader knows which
                    // call the text they asked for answers.
                    taking.kept.calling(said);
                }

                if held.is_ok() {
                    held = stop_if_failed(
                        shown(one, renderer, terms, &mut taking, &answering),
                        &terms.cancel,
                    );
                } else if matches!(one, Seen::Question { .. } | Seen::Asked { .. }) {
                    // Nothing is drawn and nothing is read once the terminal
                    // has failed, and both kinds of question still have to be
                    // answered or the worker waits for ever. A refusal and
                    // nobody-answered are what a drawing thread that has
                    // stopped means, said out loud rather than by going quiet.
                    let _ = answering.reply.send(verdict(None));
                    let _ = answering.give.send(None);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        // Reaped and counted before the box is drawn again, because the row under
        // it says how many commands are still running and a command that has
        // exited is not one of them. A number that only moved when something else
        // on the row did would be exactly the stale fact this row exists to
        // report.
        drop(terms.leaving.reap());
        says.running = terms.leaving.count();

        // After the event rather than before it, so what the turn said is on
        // screen before the box is drawn back underneath it. A line finished
        // here is kept for the loop above: running it now would start a second
        // turn inside this one.
        // Not read once the session is leaving. The turn is still stopping and
        // this loop still has to drain it, but nothing typed into a box on its
        // way off the screen can change where the session goes.
        if held.is_ok() && taking.answers.keys && matches!(meanwhile, typing::Meanwhile::Nothing) {
            match typing::during(
                renderer,
                typing::During {
                    background: &terms.leaving,
                    editor: taking.editor,
                    queued: taking.queued,
                    turning: &mut turning,
                    planning: taking.planning,
                    kept: taking.kept,
                    opened: taking.opened,
                    says: &says,
                    style: terms.style,
                    cancel: &terms.cancel,
                    leaving: &mut leaving,
                },
            ) {
                // Kept rather than acted on. The turn has been asked to stop
                // and this loop is what notices it has: leaving here would drop
                // the join handle below and take the process out over a session
                // log still being written.
                Ok(asked) => meanwhile = asked,
                Err(problem) => held = stop_if_failed(Err(problem), &terms.cancel),
            }
        }
    }

    // The turn is over, so what stood under it is taken back — the box, or the
    // view if Ctrl+O was pressed while the turn ran. What comes back next is
    // the same one of them, live this time, and the two on screen together
    // would be one of them drawn twice.
    if held.is_ok() {
        held = renderer
            .under(&[], None, terms.style.palette())
            .map_err(Fatal::from);
    }

    let runner = working.join().map_err(|_| Fatal::Lost)?;
    held.map(|()| Took { runner, meanwhile })
}

/// A turn, and what the keyboard asked for while it ran.
struct Took {
    /// The runner, back from the worker that held it for the turn.
    runner: Runner,
    /// Whether anything pressed during the turn ends the session with it.
    meanwhile: typing::Meanwhile,
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
    fn waiting(&self) -> Option<&str> {
        self.lines.front().map(String::as_str)
    }

    /// Takes the oldest waiting prompt and releases its byte reservation.
    fn pop(&mut self) -> Option<String> {
        let prompt = self.lines.pop_front()?;
        self.bytes = self.bytes.saturating_sub(prompt.len());
        Some(prompt)
    }
}

/// What one turn borrows for as long as it runs, beyond the runner and the
/// terminal.
///
/// Everything here outlives the turn and belongs to the loop above it — the
/// line being typed, the lines finished behind it, what its results had no room
/// to say, where the answer to a question comes from. A struct because a call
/// with four references in a row is one nobody can read; the prompt is not among
/// them because the prompt is what the turn is about rather than something it
/// hands back.
struct Taking<'a> {
    /// The line being written while the turn runs. It outlives the turn, so
    /// what was typed during one is still in the box after it.
    editor: &'a mut Editor,
    /// The prompts waiting behind this turn, which this one adds to as lines
    /// are finished in the box under it. The loop above takes them.
    queued: &'a mut Prompts,
    /// What this turn's results had no room to say, for the reader who asks
    /// afterwards. It outlives the turn the same way the line being typed does.
    kept: &'a mut Kept,
    /// Whether the reader is standing that under the turn, and where over it.
    /// It outlives the turn too: what was opened under one is still open after
    /// it, in the region the box comes back to.
    opened: &'a mut Standing,
    /// The plan above the box. A turn is when it changes — the tool that writes
    /// it runs on the worker thread — and it outlives the turn the same way
    /// everything else here does.
    planning: &'a mut Planning,
    /// Where the answer to a permission question comes from.
    answers: Answers<'a>,
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
        let said = match heard(pressed()?) {
            Heard::Said(said) => said,

            // The renderer is holding a size the screen no longer has, and
            // everything it draws after this rewinds by the row count that size
            // implies — for the rest of the turn, over rows it never drew.
            // Nothing is redrawn here: the question is committed, so it is the
            // terminal's to reflow, and the row the answer goes on is one the
            // renderer counts none of and therefore leaves alone.
            Heard::Resized => {
                renderer.resized()?;
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
fn heard(arrived: Pressed) -> Heard {
    match arrived {
        Pressed::Key(Key::Char(letter)) => Heard::Said(letter.to_string()),
        Pressed::Key(Key::Interrupt | Key::Eof | Key::Enter) | Pressed::Escape => {
            Heard::Said(String::new())
        }

        Pressed::Resized => Heard::Resized,

        // An arrow, a click, a mode step, a key that means nothing here — the key
        // about what is already running among them, since this question is what
        // decides whether the command runs at all. None of them is an answer, and
        // none of them may be read as one.
        //
        // Ctrl+E is here for now rather than because it belongs here: the
        // question is committed to scrollback a row at a time, and there is no
        // second shape of it to toggle into until the panel is what draws it.
        //
        // Ctrl+O is here because this is the question with nowhere to stand: it
        // was put a row at a time precisely because there was no room for a
        // panel, and a view of what was cut needs more room than the panel did.
        //
        // Ctrl+T for the same reason as Ctrl+E: the question took the rows the
        // plan stands in, so there is no panel on screen for the key to open.
        Pressed::Key(_)
        | Pressed::Up
        | Pressed::Down
        | Pressed::Background
        | Pressed::Cycle
        | Pressed::Explain
        | Pressed::Expand
        | Pressed::Plan
        | Pressed::Clicked { .. }
        | Pressed::Ignored => Heard::Ignored,
    }
}

/// Draws one thing the worker sent, and answers it if it was a question.
fn shown<T: Terminal>(
    one: Seen,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    taking: &mut Taking<'_>,
    answering: &Answering,
) -> Result<(), Fatal> {
    let Answering { reply, give } = answering;
    let style = terms.style;

    match one {
        Seen::Turn(event) => draw::event(renderer, event, style, taking.kept)?,
        Seen::Question { call, sensitivity } => {
            // A durable rule cannot live in either project configuration file:
            // both names can arrive with a checkout, whatever an ignore rule
            // says. Until policy has a per-workspace store outside the checkout,
            // the prompt offers only answers this process can honour.
            let answer = asked(renderer, &call, &sensitivity, &mut taking.answers, style);
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
            if !taking.answers.keys {
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

        // Nothing is drawn under it. The panel stood in the live region and the
        // region was given back, so a call that was allowed reads exactly like
        // one nothing asked about — which is what the reader is looking at
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

        let Some(taken) = took(question)? else {
            return Ok(None);
        };

        draw::answered(renderer, taken.answer())?;
        given.push(Answered::new([taken.answer()]));
    }

    Ok(Some(given))
}

/// Reads one key as one of `question`'s answers, or nothing where it means to
/// leave.
fn took(question: &Question) -> Result<Option<Chosen>, Fatal> {
    loop {
        let arrived = pressed()?;
        let Pressed::Key(Key::Char(typed)) = arrived else {
            if arrived == Pressed::Resized {
                continue;
            }
            return Ok(None);
        };

        let at = usize::try_from(typed.to_digit(10).unwrap_or(0))
            .ok()
            .and_then(|digit| digit.checked_sub(1));

        if let Some(answer) = at.and_then(|at| question.answers().nth(at)) {
            return Ok(Some(answer.clone()));
        }

        return Ok(None);
    }
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
