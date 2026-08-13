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
//! Two things follow from holding it. Ctrl-C is a key here rather than a signal
//! the terminal raises, so this loop is the only thing that can act on it: mid
//! turn it asks the turn to stop, which is what it always meant. And a session
//! with no terminal at either end holds nothing at all and reads whole lines,
//! which is the path every test drives.
//!
//! The session log is append-only and written as the turn goes, so `--continue`
//! picks the session up from wherever it stopped.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::Duration;

use crucible_core::{Cancel, Event, Minted, Post as _, Remember, Verdict, Workspace, narrowest};
use crucible_runner::Runner;
use crucible_tui::{
    Editor, Key, Pressed, Raw, Renderer, Reporting, Terminal, TerminalError, pressed,
};

use super::Fatal;
use super::draw;
use super::remember;
use super::seen::{Answer, Asking, Relay, Seen};
use super::style::Style;
use super::unasked;
use command::Ran;
use typing::Asked;

mod command;
mod mode;
mod typing;

/// What the user types after, where there is no box to type into.
const MARK: &str = "› ";

/// How long the loop waits on the turn before looking at the keyboard.
///
/// A wake-up rate rather than a spin: the thread is parked in `recv_timeout`
/// for all of it. Short enough that a keystroke appears at once — well inside
/// what a hand notices — and long enough that a turn producing nothing costs
/// sixty wake-ups a second and no work in any of them.
const TICK: Duration = Duration::from_millis(16);

/// What every turn in a conversation is taken under.
///
/// All of these are settled before the first prompt and none changes at one:
/// the style comes from the files and the terminal together, and the cancel is
/// the same one the tools were built with. One value rather than three
/// parameters carried down through every turn.
///
/// The mode is not among them. It is the one thing about a session that changes
/// after it has started, so it is read from the engine that holds it every time
/// it is drawn rather than copied here and kept in step.
pub(crate) struct Terms {
    /// Whether to write colour, and how much of a tool call to show.
    pub(crate) style: Style,
    /// What stops a turn.
    pub(crate) cancel: Cancel,
    /// The file an answer of `always` writes its rule into.
    pub(crate) remembering: PathBuf,
    /// Which provider this session is set up to ask, where a key was found for
    /// one. `/model` writes its answer under this name, and where there is none
    /// there is no name to write it under.
    pub(crate) provider: Option<&'static str>,
    /// The file at home that `/model` writes its answer into. A model is a fact
    /// about who is running crucible rather than about the checkout, so it is
    /// not the file beside `remembering`.
    pub(crate) choosing: PathBuf,
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

    // Only where a layer asked for it, because the wheel is a button: a
    // terminal forwarding buttons to crucible is one whose wheel no longer
    // scrolls the scrollback this program's transcript lives in.
    let _pointer = (keys && style.clicks())
        .then(Reporting::on)
        .transpose()?
        .flatten();

    // One line for the whole session rather than one per prompt. What was typed
    // while a turn ran is still there when it ends, and the allocation the last
    // line grew to is the one the next starts in.
    let mut editor = Editor::new();

    // A line finished while a turn was still running. It is the next prompt,
    // and it is taken without asking for another.
    let mut queued: Option<String> = None;

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
        if let Some(said) = queued.take() {
            draw::queued(renderer, &said, style)?;

            let (back, over) = take(
                runner,
                renderer,
                terms,
                Taking {
                    prompt: said,
                    editor: &mut editor,
                    answers: Answers { input, keys },
                },
            )?;
            runner = back;
            queued = over;

            if !told && let Some(problem) = runner.session().trouble() {
                draw::trouble(renderer, &problem, style)?;
                told = true;
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
                draw::mark(renderer, &format!("{} {MARK}", runner.mode()), style)?;

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
            match command::run(wanted, renderer, &mut runner, terms)? {
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
            draw::unconfigured(renderer, unasked(terms.provider), style)?;
            continue;
        }

        let (back, over) = take(
            runner,
            renderer,
            terms,
            Taking {
                prompt,
                editor: &mut editor,
                answers: Answers { input, keys },
            },
        )?;
        runner = back;
        queued = over;

        if !told && let Some(problem) = runner.session().trouble() {
            draw::trouble(renderer, &problem, style)?;
            told = true;
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
    runner: Runner,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    mut taking: Taking<'_>,
) -> Result<(Runner, Option<String>), Fatal> {
    // Both channels are made fresh for this turn. A reply channel that outlived
    // its turn could hand the next question an answer meant for the last one.
    let (post, seen) = channel();
    let (reply, hear) = channel();

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
    let mode = runner.mode();
    let prompt = taking.prompt;

    let working = thread::spawn(move || {
        let mut runner = runner;

        // The runner reports what happened and returns why it stopped; nothing
        // else has posted the failure, so this is where it becomes visible.
        if let Err(problem) = runner.turn(prompt.trim(), &mut asking, &relay, &running) {
            relay.post(Event::Failed { error: problem });
        }

        runner
    });

    // The first thing drawn, and held like everything drawn after it: the runner
    // is with the worker now, so a terminal that failed here has to be carried
    // to the end of the turn rather than returned from the middle of one.
    let mut held = typing::stand(renderer, taking.editor, mode, terms.style);
    let mut queued = None;

    // Ends when the worker drops both senders, which happens when the turn is
    // over. The wait is bounded rather than blocking so that the keyboard is
    // looked at between deltas; nothing is skipped either way, because every
    // event is still taken from the same channel in the order it was sent.
    loop {
        match seen.recv_timeout(TICK) {
            Ok(one) => {
                if held.is_ok() {
                    held = shown(one, renderer, terms, &mut taking.answers, &reply);
                } else if matches!(one, Seen::Question { .. }) {
                    // Nothing is drawn and nothing is read once the terminal or
                    // the input has failed, but a question still has to be
                    // answered: the worker waits on the reply channel, and this
                    // loop waits on the worker. A refusal is what a drawing
                    // thread that has stopped already means, said out loud
                    // rather than by going quiet.
                    let _ = reply.send(verdict(None, false));
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        // After the event rather than before it, so what the turn said is on
        // screen before the box is drawn back underneath it. A line finished
        // here is kept for the loop above: running it now would start a second
        // turn inside this one.
        if held.is_ok() && taking.answers.keys {
            match typing::during(renderer, taking.editor, mode, terms.style, &terms.cancel) {
                Ok(said) => queued = said.or(queued),
                Err(problem) => held = Err(problem),
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
    held.map(|()| (runner, queued))
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
/// Anything that is not one of the letters is a refusal, which is what an
/// unrecognised line already meant. Escape and Ctrl-C are spelled out among
/// them so that the way out of a question is the way out of everything else.
fn answered<T: Terminal>(
    renderer: &mut Renderer<T>,
    answers: &mut Answers<'_>,
    writable: bool,
) -> Result<Answer, Fatal> {
    if !answers.keys {
        return Ok(verdict(read(answers.input)?.as_deref(), writable));
    }

    loop {
        let said = match pressed()? {
            Pressed::Key(Key::Char(letter)) => letter.to_string(),
            Pressed::Key(Key::Interrupt | Key::Eof | Key::Enter) | Pressed::Escape => String::new(),

            // A resize, an arrow, a click. None of them is an answer, and none
            // of them may be read as one.
            _ => continue,
        };

        draw::answered(renderer, &said)?;
        return Ok(verdict(Some(&said), writable));
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
            // Minted once and used twice. What the question offers has to be
            // the rule that gets written, or the user agreed to one thing and
            // crucible wrote down another.
            let rule = narrowest(&call, &sensitivity);

            draw::question(renderer, &call, &sensitivity, rule.as_ref(), style)?;
            let answer = answered(renderer, answers, rule.is_some())?;

            // Before the answer goes back, so the file is written by the time
            // the tool it allowed runs. `always` is only ever answered where a
            // rule was minted, so the two arriving together is one fact twice
            // rather than a case that has to be handled.
            if let (Some(rule), Remember::Always) = (&rule, answer.1) {
                keep(renderer, &terms.remembering, rule, style)?;
            }

            // A worker that stopped waiting has already denied itself.
            let _ = reply.send(answer);
        }
    }

    Ok(())
}

/// Writes one rule down, and says what happened either way.
///
/// A failure here does not end the turn and does not change the answer. The
/// engine treats `always` as at least a session's worth on its own, so what a
/// failed write costs is the part that outlives the process — and the line
/// drawn says which rule that was, so it can be added by hand.
fn keep<T: Terminal>(
    renderer: &mut Renderer<T>,
    file: &Path,
    rule: &Minted,
    style: Style,
) -> Result<(), TerminalError> {
    let outcome = remember::allowing(file, rule);

    draw::remembered(
        renderer,
        rule,
        match &outcome {
            Ok(()) => Ok(file),
            Err(problem) => Err(problem),
        },
        style,
    )
}

/// Reads one line, or `None` at end of input.
fn read(input: &mut dyn BufRead) -> Result<Option<String>, Fatal> {
    let mut line = String::new();

    match input.read_line(&mut line) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(line)),
        Err(problem) => Err(Fatal::Input(problem)),
    }
}

/// What an answer to a permission question means.
///
/// Anything unrecognised is a refusal, and so is end of input. Every way to say
/// yes is explicit; everything else, including a typo and a closed pipe, leaves
/// the tool unrun.
///
/// `writable` is whether a rule could be minted for this call. Where none can,
/// `always` is neither offered nor accepted: an answer that cannot be written
/// down would last a session while reading as though it lasted for ever, and
/// the difference would only show up the next time crucible started.
fn verdict(answer: Option<&str>, writable: bool) -> Answer {
    match answer.map(str::trim) {
        Some("y" | "Y" | "yes") => (Verdict::Allow, Remember::Never),
        Some("s" | "S" | "session") => (Verdict::Allow, Remember::Session),
        Some("a" | "A" | "always") if writable => (Verdict::Allow, Remember::Always),
        _ => (Verdict::Deny, Remember::Never),
    }
}

#[cfg(test)]
mod tests;
