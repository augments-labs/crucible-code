//! Events, turned into what a terminal shows.
//!
//! Committed lines carry no escape sequences. The live tail measures what it
//! holds in display columns to know where to move the cursor back to, and a
//! colour code is bytes it would count as width. Colour therefore goes only
//! where nothing is ever redrawn: the prompt mark, written straight through.

use std::fmt;

use crucible_core::{Event, Sensitivity, StopReason, ToolCall, ToolOutput, Workspace};
use crucible_tui::{Renderer, Terminal, TerminalError};

use super::style::Style;

/// Dim, then back. Only for text that is written once and never redrawn.
const DIM: &str = "\x1b[2m";
const PLAIN: &str = "\x1b[0m";

/// The two lines a session opens with.
///
/// The root is there because every tool path is relative to it, and a user who
/// started crucible in the wrong directory should find out before the first
/// tool call rather than after it.
pub(crate) fn opening<T: Terminal>(
    renderer: &mut Renderer<T>,
    model: &str,
    workspace: &Workspace,
) -> Result<(), TerminalError> {
    renderer.commit(&format!("crucible {} · {model}", env!("CARGO_PKG_VERSION")))?;
    renderer.commit(&workspace.root().display().to_string())?;
    renderer.commit("")
}

/// Draws one event.
pub(crate) fn event<T: Terminal>(
    renderer: &mut Renderer<T>,
    event: Event,
    style: Style,
) -> Result<(), TerminalError> {
    let columns = renderer.columns();

    match event {
        // The turn number is in the title bar, not in the transcript: a line
        // per turn saying which turn it is crowds out the turn itself.
        Event::TurnStarted { .. } => Ok(()),

        Event::Delta { text } => renderer.stream(&text),

        Event::ToolRequested { call } => {
            // Whatever the model was saying is finished; it said it to explain
            // the call that follows.
            renderer.settle()?;
            renderer.commit(&requested(&call, style.args(columns)))
        }

        Event::ToolFinished { output, .. } => {
            renderer.commit(&finished(&output, style.output(columns)))
        }

        // The tail is settled either way; an answer that stopped early is
        // finished text as much as one that ran out of things to say.
        Event::TurnFinished { stop, .. } => {
            renderer.settle()?;
            match notice(stop) {
                Some(said) => renderer.commit(said),
                None => Ok(()),
            }
        }

        // Clipped: a refusal carries up to 8 KiB of the provider's own words,
        // and a response that failed part-way through carries whatever the
        // provider said about why. Neither is this program's text, and neither
        // may become extra rows.
        Event::Failed { error } => {
            renderer.settle()?;
            renderer.commit(&format!("! {}", clipped(error, style.output(columns))))
        }
    }
}

/// Says that the session log stopped recording.
///
/// Worth interrupting for: the writing happens off the turn's thread and fails
/// quietly, so without this the user learns about it the next day, when
/// `--continue` offers half a transcript.
pub(crate) fn trouble<T: Terminal>(
    renderer: &mut Renderer<T>,
    problem: &str,
    style: Style,
) -> Result<(), TerminalError> {
    let width = style.output(renderer.columns());

    renderer.settle()?;
    renderer.commit(&format!(
        "! this session has stopped being recorded: {}",
        clipped(problem, width)
    ))
}

/// Draws a permission question and leaves the cursor where the answer goes.
pub(crate) fn question<T: Terminal>(
    renderer: &mut Renderer<T>,
    call: &ToolCall,
    sensitivity: &Sensitivity,
    style: Style,
) -> Result<(), TerminalError> {
    let width = style.args(renderer.columns());

    renderer.settle()?;
    renderer.commit(&asked(call, sensitivity, width))?;
    mark(renderer, "  [y]es  [a]lways  [n]o › ", style)
}

/// Writes something the user is expected to type after.
///
/// Straight through the terminal rather than through `commit`, because what is
/// wanted here is a line with no newline on the end.
pub(crate) fn mark<T: Terminal>(
    renderer: &mut Renderer<T>,
    text: &str,
    style: Style,
) -> Result<(), TerminalError> {
    renderer.settle()?;

    let coloured = style.color();
    let terminal = renderer.terminal();
    if coloured {
        terminal.write(DIM)?;
        terminal.write(text)?;
        terminal.write(PLAIN)?;
    } else {
        terminal.write(text)?;
    }
    terminal.flush()
}

/// The line for a call about to run.
///
/// The arguments are shown as the model wrote them. Parsing them again here to
/// pull out a path would be a second reading of a schema the tool already owns,
/// and the two would drift.
fn requested(call: &ToolCall, width: usize) -> String {
    format!("· {} {}", call.name, clipped(call.args.as_str(), width))
}

/// The line for a call that finished.
fn finished(output: &ToolOutput, width: usize) -> String {
    let text = output.text();
    let mut lines = text.lines();
    let first = clipped(lines.next().unwrap_or_default(), width);
    let rest = lines.count();

    let mark = if output.is_failed() { "  ✗ " } else { "  " };
    match rest {
        0 => format!("{mark}{first}"),
        more => format!("{mark}{first} (+{more} lines)"),
    }
}

/// What to say about a turn that ended, if anything.
///
/// Every variant is named rather than caught by a rest pattern: a stop reason
/// this file has not considered is exactly the one that would go unmentioned,
/// and the whole point of the set being closed is that a new one cannot.
fn notice(stop: StopReason) -> Option<&'static str> {
    match stop {
        // The ordinary ending, and the one taken every time. The answer speaks
        // for itself; a line under each turn saying it finished is noise.
        StopReason::Yielded | StopReason::WantsTools => None,

        StopReason::OutOfTokens => Some("! unfinished: the answer reached the token ceiling"),

        // Named apart from the ceiling because the remedy is the opposite: a
        // shorter request buys nothing, so a user told the wrong reason retries
        // in the one way that cannot work.
        StopReason::Filtered => Some("! unfinished: the provider's filter cut the answer short"),

        // The answer is not over, and 0.0.x has no way to resume it. Saying so
        // is what turns it into something the user can act on — the same prompt
        // again picks up from a transcript that already holds this much.
        StopReason::Paused => Some("! unfinished: the provider paused this turn; ask it to go on"),

        StopReason::Cancelled => Some("! stopped"),
    }
}

/// What the user is being asked to allow.
fn asked(call: &ToolCall, sensitivity: &Sensitivity, width: usize) -> String {
    match sensitivity {
        // Never reached through the permission engine, which allows or refuses
        // a read and never asks about one. Here so that a tool reclassified
        // later still has a question to show rather than a blank one.
        Sensitivity::ReadOnly { target } => {
            format!("? {} wants to read: {}", call.name, clipped(target, width))
        }

        Sensitivity::MutatesFile { target } => format!(
            "? {} wants to change: {}",
            call.name,
            clipped(target, width)
        ),

        // Clipped like the others, and for a stronger reason. What runs is the
        // model's text to choose. Unclipped, a newline in it commits a second
        // row, and the last two rows on screen become a question the attacker
        // wrote and the genuine answer mark underneath it.
        Sensitivity::SpawnsProcess { command } => {
            format!("? {} wants to run: {}", call.name, clipped(command, width))
        }
    }
}

/// One line, at most `width` characters of it.
///
/// Control characters become spaces: a newline inside JSON arguments would
/// otherwise break the line in two, and the tail would be counting rows that
/// the caller did not mean to write.
fn clipped(text: impl fmt::Display, width: usize) -> String {
    let text = text.to_string();
    let mut clipped: String = text
        .trim()
        .chars()
        .take(width)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();

    if text.trim().chars().nth(width).is_some() {
        clipped.push('…');
    }
    clipped
}

#[cfg(test)]
mod tests;
