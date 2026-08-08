//! The loop: read a line, take a turn, draw what the turn does.
//!
//! The turn runs on its own thread and the terminal stays with this one. That
//! split is the whole reason a turn can stream while a question is waiting to
//! be answered, and it is why no lock appears anywhere on the render path: the
//! only thread that writes to the terminal is the one running this loop.
//!
//! Standard input is left in cooked mode for 0.0.1. The consequence worth
//! knowing: Ctrl-C during a turn ends the process, because catching a signal
//! would need `unsafe`, which this workspace forbids. The session log is
//! append-only and written as the turn goes, so `--continue` picks the
//! session up from wherever it stopped.

use std::io::BufRead;
use std::sync::mpsc::channel;
use std::thread;

use crucible_core::{Cancel, Event, Post as _, Verdict};
use crucible_runner::Runner;
use crucible_tui::{Renderer, Terminal};

use super::Fatal;
use super::draw;
use super::seen::{Asking, Relay, Seen};

/// What the user types after.
const MARK: &str = "› ";

/// Reads prompts and takes turns until input ends.
///
/// `input` is standard input in a real run. It is a parameter so that a test
/// can drive the loop: the deadlock this file has to avoid is one that only
/// shows up when a whole turn runs, and a hardwired stdin makes that unrunnable.
pub(crate) fn converse<T: Terminal>(
    mut runner: Runner,
    renderer: &mut Renderer<T>,
    cancel: &Cancel,
    input: &mut dyn BufRead,
) -> Result<(), Fatal> {
    // Said once. The log does not start working again, and a line under every
    // turn from here on would bury the turns.
    let mut told = false;

    loop {
        // The window may have changed while the last turn was streaming.
        // Noticed here rather than as it happens because catching the signal a
        // resize sends needs `unsafe`, so a prompt is the only moment there is:
        // what a resize costs in 0.0.1 is the turn it lands in, not the session.
        renderer.resized()?;
        draw::mark(renderer, MARK)?;

        let Some(prompt) = read(input)? else { break };
        if prompt.trim().is_empty() {
            continue;
        }

        runner = take(runner, renderer, cancel, input, prompt)?;

        if !told && let Some(problem) = runner.session().trouble() {
            draw::trouble(renderer, &problem)?;
            told = true;
        }
    }

    // The writer thread is still holding the last turn when the loop ends, so
    // the poll above cannot have seen a failure recorded during it. Draining
    // here is what stops the one turn most likely to matter from being the one
    // nobody is told about.
    if let Some(problem) = runner.into_session().finish()
        && !told
    {
        draw::trouble(renderer, &problem)?;
    }

    renderer.settle()?;
    Ok(())
}

/// One turn, start to finish.
///
/// The runner goes to the worker and comes back, which is what makes the
/// transcript and the permission memory survive a turn without being shared
/// between threads.
fn take<T: Terminal>(
    runner: Runner,
    renderer: &mut Renderer<T>,
    cancel: &Cancel,
    input: &mut dyn BufRead,
    prompt: String,
) -> Result<Runner, Fatal> {
    // Both channels are made fresh for this turn. A reply channel that outlived
    // its turn could hand the next question an answer meant for the last one.
    let (post, seen) = channel();
    let (reply, hear) = channel();

    let mut asking = Asking::new(post.clone(), hear);
    let relay = Relay::new(post);
    let running = cancel.clone();

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
    for one in seen {
        match one {
            Seen::Turn(event) => draw::event(renderer, event)?,
            Seen::Question { call, sensitivity } => {
                draw::question(renderer, &call, &sensitivity)?;
                // A worker that stopped waiting has already denied itself.
                let _ = reply.send(verdict(read(input)?.as_deref()));
            }
        }
    }

    working.join().map_err(|_| Fatal::Lost)
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
/// Anything unrecognised is a refusal, and so is end of input. The two ways to
/// say yes are both explicit; everything else, including a typo and a closed
/// pipe, leaves the tool unrun.
fn verdict(answer: Option<&str>) -> Verdict {
    match answer.map(str::trim) {
        Some("y" | "Y" | "yes") => Verdict::AllowOnce,
        Some("a" | "A" | "always") => Verdict::AllowSession,
        _ => Verdict::Deny,
    }
}

#[cfg(test)]
mod tests;
