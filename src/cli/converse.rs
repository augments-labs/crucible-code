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
use std::sync::mpsc::{RecvTimeoutError, Sender, channel, sync_channel};
use std::thread;
use std::time::Duration;

use crucible_auth::Store;
use crucible_core::{Cancel, Event, Post as _, Remember, Verdict, Workspace};
use crucible_runner::Runner;
use crucible_tools::Ledger;
use crucible_tui::{Editor, Key, Pressed, Raw, Renderer, Terminal, pressed};

use super::draw;
use super::seen::{Answer, Asking, CAPACITY, Inbox, Relay, Seen};
use super::style::Style;
use super::subscription::Subscriptions;
use super::unasked;
use super::{Fatal, Serving, standing};
use command::Ran;
use turning::Turning;
use typing::Asked;

mod command;
mod mode;
mod picking;
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

    // Held for the whole session and dropped on the way out however this
    // returns. Between turns it is what draws the box; during one it is what
    // lets the box go on being typed into. `None` is a session with no terminal
    // at one end or the other, which reads whole lines instead.
    let raw = Raw::enter()?;
    let keys = raw.is_some();

    // One line for the whole session rather than one per prompt. What was typed
    // while a turn ran is still there when it ends, and the allocation the last
    // line grew to is the one the next starts in.
    let mut editor = Editor::new();

    // Lines finished while a turn was still running. They are the next prompts,
    // in the order they were typed, and each is taken without asking for
    // another.
    let mut queued = Prompts::default();

    // Said once. The log does not start working again, and a line under every
    // turn from here on would bury the turns.
    let mut told = false;

    loop {
        // The window may have changed while the last turn was streaming. The
        // box notices a resize as it happens, because in raw mode the terminal
        // reports one; between turns there is nobody reading, so it is noticed
        // here instead.
        renderer.resized()?;

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
                Taking {
                    prompt: said,
                    editor: &mut editor,
                    queued: &mut queued,
                    answers: Answers { input, keys },
                },
            )?;
            runner = took.runner;

            if !told && let Some(problem) = runner.session().trouble() {
                draw::trouble(renderer, &problem, style)?;
                told = true;
            }

            // Said after the trouble above rather than instead of it: a log
            // that stopped recording is worth hearing about on the way out as
            // much as on the way through.
            if matches!(took.meanwhile, typing::Meanwhile::Leaving) {
                break;
            }
            continue;
        }

        let prompt = match typing::ask(renderer, style, &mut runner, &mut editor, keys)? {
            Asked::Said(said) => said,
            Asked::Ended => break,

            // Nothing to type into: no terminal, or one at only one end. The
            // line is read the way every other answer on this thread is.
            Asked::Untyped => {
                // The mode in force, spelled the way configuration spells it,
                // in front of the line rather than under a box there is none
                // of. It is on screen every time rather than said once at the
                // top because the moment it matters is hours in, when the top
                // has scrolled away — a `fullAccess` session must not be
                // distinguishable from an `ask` one only by what the user
                // remembers starting.
                //
                // The mark after it is the one a line is typed after
                // everywhere else, taken from the same setting: this is the
                // prompt on a run that has no box to draw one in.
                let mark = style.glyphs().caret();
                draw::mark(renderer, &format!("{} {mark} ", runner.mode()), style)?;

                let Some(said) = read(input)? else {
                    // The mark is still the last thing on its row, and nothing
                    // but this ends it. Without it, whatever comes next is
                    // drawn on top of `ask › ` — a report below, or the shell's
                    // own prompt once crucible is gone, which is every ordinary
                    // exit. The box needs none of this: it takes its own rows
                    // back before it returns.
                    draw::ended(renderer)?;
                    break;
                };

                said
            }
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
            Taking {
                prompt,
                editor: &mut editor,
                queued: &mut queued,
                answers: Answers { input, keys },
            },
        )?;
        runner = took.runner;

        if !told && let Some(problem) = runner.session().trouble() {
            draw::trouble(renderer, &problem, style)?;
            told = true;
        }

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
    mut taking: Taking<'_>,
) -> Result<Took, Fatal> {
    // Both channels are made fresh for this turn. A reply channel that outlived
    // its turn could hand the next question an answer meant for the last one.
    let (post, seen) = sync_channel(CAPACITY);
    let (reply, hear) = channel();
    let mut seen = Inbox::new(seen);

    let mut asking = Asking::new(post.clone(), hear);
    let relay = Relay::new(post);
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
    let says = typing::under(&runner, terms.style.glyphs());
    let prompt = taking.prompt;

    // Read here for the same reason and at the same moment: half of what a turn
    // is asked under is about the session — which model is answering, and how
    // hard it was asked to think — and a model can find out neither for itself.
    // Written once at startup it would go on describing the session the first
    // turn was taken in, so it is written again for each.
    runner.telling(&standing::under(
        runner.model(),
        runner.effort(),
        &terms.workspace,
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
        typing::stand(renderer, taking.editor, &turning, &says, terms.style),
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
                }

                if held.is_ok() {
                    held = stop_if_failed(
                        shown(one, renderer, terms, &mut taking.answers, &reply),
                        &terms.cancel,
                    );
                } else if matches!(one, Seen::Question { .. }) {
                    // Nothing is drawn and nothing is read once the terminal or
                    // the input has failed, but a question still has to be
                    // answered: the worker waits on the reply channel, and this
                    // loop waits on the worker. A refusal is what a drawing
                    // thread that has stopped already means, said out loud
                    // rather than by going quiet.
                    let _ = reply.send(verdict(None));
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

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
                    editor: taking.editor,
                    queued: taking.queued,
                    turning: &mut turning,
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

    // The turn is over, so what stood under it is taken back. What comes back
    // next is the same box, live this time, and the two on screen together
    // would be one box drawn twice.
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

    /// Takes the oldest waiting prompt and releases its byte reservation.
    fn pop(&mut self) -> Option<String> {
        let prompt = self.lines.pop_front()?;
        self.bytes = self.bytes.saturating_sub(prompt.len());
        Some(prompt)
    }
}

/// What one turn is being taken with, beyond the runner and the terminal.
///
/// A struct because the line and where an answer comes from both outlive the
/// turn, and a call with five references in a row is one nobody can read.
struct Taking<'a> {
    /// What was asked.
    prompt: String,
    /// The line being written while the turn runs. It outlives the turn, so
    /// what was typed during one is still in the box after it.
    editor: &'a mut Editor,
    /// The prompts waiting behind this turn, which this one adds to as lines
    /// are finished in the box under it. The loop above takes them.
    queued: &'a mut Prompts,
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

        // An arrow, a click, a mode step, a key that means nothing here. None
        // of them is an answer, and none of them may be read as one.
        Pressed::Key(_)
        | Pressed::Up
        | Pressed::Down
        | Pressed::Cycle
        | Pressed::Clicked { .. }
        | Pressed::Ignored => Heard::Ignored,
    }
}

/// Draws one thing the worker sent, and answers it if it was a question.
fn shown<T: Terminal>(
    one: Seen,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    answers: &mut Answers<'_>,
    reply: &Sender<Answer>,
) -> Result<(), Fatal> {
    let style = terms.style;

    match one {
        Seen::Turn(event) => draw::event(renderer, event, style)?,
        Seen::Question { call, sensitivity } => {
            // A durable rule cannot live in either project configuration file:
            // both names can arrive with a checkout, whatever an ignore rule
            // says. Until policy has a per-workspace store outside the checkout,
            // the prompt offers only answers this process can honour.
            let answer = draw::question(renderer, &call, &sensitivity, style)
                .map_err(Fatal::from)
                .and_then(|()| answered(renderer, answers));
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
    }

    Ok(())
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
