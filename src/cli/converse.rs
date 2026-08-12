//! The loop: read a line, take a turn, draw what the turn does.
//!
//! The turn runs on its own thread and the terminal stays with this one. That
//! split is the whole reason a turn can stream while a question is waiting to
//! be answered, and it is why no lock appears anywhere on the render path: the
//! only thread that writes to the terminal is the one running this loop.
//!
//! A prompt is typed into the box [`typing`] draws, and everything else on this
//! thread is read as a line the terminal collected. Raw mode is therefore held
//! only while a line is being written, which is what leaves the rest of the
//! session as it was: Ctrl-C during a turn ends the process, because catching a
//! signal would need `unsafe`, which this workspace forbids. The session log is
//! append-only and written as the turn goes, so `--continue` picks the session
//! up from wherever it stopped.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::thread;

use crucible_core::{Cancel, Event, Minted, Mode, Post as _, Remember, Verdict, narrowest};
use crucible_runner::Runner;
use crucible_tui::{Renderer, Terminal, TerminalError};

use super::Fatal;
use super::draw;
use super::remember;
use super::seen::{Answer, Asking, Relay, Seen};
use super::style::Style;
use typing::Asked;

mod typing;

/// What the user types after, where there is no box to type into.
const MARK: &str = "› ";

/// What every turn in a conversation is taken under.
///
/// All of these are settled before the first prompt and none changes at one:
/// the style comes from the files and the terminal together, the mode from the
/// same resolution the engine was built with, and the cancel is the same one
/// the tools were built with. One value rather than three parameters carried
/// down through every turn.
pub(crate) struct Terms {
    /// Whether to write colour, and how much of a tool call to show.
    pub(crate) style: Style,
    /// The mode the permission engine is in, written on every prompt line.
    pub(crate) mode: Mode,
    /// What stops a turn.
    pub(crate) cancel: Cancel,
    /// The file an answer of `always` writes its rule into.
    pub(crate) remembering: PathBuf,
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

    // The mode in force, spelled the way configuration spells it. It is on
    // screen every time rather than said once at the top because the moment it
    // matters is hours in, when the top has scrolled away — a `fullAccess`
    // session must not be distinguishable from an `ask` one only by what the
    // user remembers starting. The box says it under itself; a redirected run
    // has only the row the prompt is read on, so it says it there.
    let mode = terms.mode.to_string();
    let mark = format!("{mode} {MARK}");

    // Said once. The log does not start working again, and a line under every
    // turn from here on would bury the turns.
    let mut told = false;

    loop {
        // The window may have changed while the last turn was streaming. The
        // box notices a resize as it happens, because in raw mode the terminal
        // reports one; between turns there is nobody reading, so it is noticed
        // here instead.
        renderer.resized()?;

        let prompt = match typing::ask(renderer, style, &mode)? {
            Asked::Said(said) => said,
            Asked::Ended => break,

            // Nothing to type into: no terminal, or one at only one end. The
            // line is read the way every other answer on this thread is.
            Asked::Untyped => {
                draw::mark(renderer, &mark, style)?;

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

        if prompt.trim().is_empty() {
            continue;
        }

        runner = take(runner, renderer, terms, input, prompt)?;

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
    input: &mut dyn BufRead,
    prompt: String,
) -> Result<Runner, Fatal> {
    // Both channels are made fresh for this turn. A reply channel that outlived
    // its turn could hand the next question an answer meant for the last one.
    let (post, seen) = channel();
    let (reply, hear) = channel();

    let mut asking = Asking::new(post.clone(), hear);
    let relay = Relay::new(post);
    let running = terms.cancel.clone();

    let working = thread::spawn(move || {
        let mut runner = runner;

        // The runner reports what happened and returns why it stopped; nothing
        // else has posted the failure, so this is where it becomes visible.
        if let Err(problem) = runner.turn(prompt.trim(), &mut asking, &relay, &running) {
            relay.post(Event::Failed { error: problem });
        }

        runner
    });

    // Ends when the worker drops both senders, which happens when the turn is
    // over. No sentinel event, and no way to leave the loop early and miss the
    // last delta.
    let mut held = Ok(());

    for one in seen {
        if held.is_ok() {
            held = shown(one, renderer, terms, input, &reply);
        } else if matches!(one, Seen::Question { .. }) {
            // Nothing is drawn and nothing is read once the terminal or the
            // input has failed, but a question still has to be answered: the
            // worker waits on the reply channel, and this loop waits on the
            // worker. A refusal is what a drawing thread that has stopped
            // already means, said out loud rather than by going quiet.
            let _ = reply.send(verdict(None, false));
        }
    }

    let runner = working.join().map_err(|_| Fatal::Lost)?;
    held.map(|()| runner)
}

/// Draws one thing the worker sent, and answers it if it was a question.
fn shown<T: Terminal>(
    one: Seen,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    input: &mut dyn BufRead,
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
            let answer = verdict(read(input)?.as_deref(), rule.is_some());

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
