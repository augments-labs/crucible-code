//! Events, turned into what a terminal shows.
//!
//! Committed lines carry no escape sequences. The live tail measures what it
//! holds in display columns to know where to move the cursor back to, and a
//! colour code is bytes it would count as width. Colour therefore goes only
//! where nothing is ever redrawn: the prompt mark, written straight through.

use std::fmt;
use std::path::Path;

use crucible_core::{Event, Minted, Sensitivity, StopReason, ToolCall, ToolOutput};
use crucible_tui::{Renderer, Terminal, TerminalError, cut};

use super::remember::RememberError;
use super::style::Style;

mod opening;

pub(crate) use opening::opening;

/// Dim, then back. Only for text that is written once and never redrawn.
const DIM: &str = "\x1b[2m";
const PLAIN: &str = "\x1b[0m";

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
///
/// `rule` is what an answer of `always` would write down, and `None` where this
/// call is one no rule can be minted for.
pub(crate) fn question<T: Terminal>(
    renderer: &mut Renderer<T>,
    call: &ToolCall,
    sensitivity: &Sensitivity,
    rule: Option<&Minted>,
    style: Style,
) -> Result<(), TerminalError> {
    let width = style.args(renderer.columns());

    renderer.settle()?;
    renderer.commit(&asked(call, sensitivity, width))?;
    mark(renderer, answers(rule.is_some()), style)
}

/// Says what an answer of `always` left behind, or why it left nothing.
///
/// Drawn after the answer rather than offered before it: what a user needs is
/// the rule and the file it went into, so they can find it again to take the
/// permission back.
pub(crate) fn remembered<T: Terminal>(
    renderer: &mut Renderer<T>,
    rule: &Minted,
    outcome: Result<&Path, &RememberError>,
    style: Style,
) -> Result<(), TerminalError> {
    let width = style.output(renderer.columns());

    renderer.settle()?;
    renderer.commit(&kept(rule, outcome, width))
}

/// Writes something the user is expected to type after.
///
/// Through `prompt` rather than `commit`, because what is wanted here is a line
/// with no newline on the end. `prompt` settles first and then writes verbatim,
/// so the colour has to be in the text handed to it — which is the arrangement
/// that makes it safe: escape bytes cost no column in a row no frame will move
/// back over.
pub(crate) fn mark<T: Terminal>(
    renderer: &mut Renderer<T>,
    text: &str,
    style: Style,
) -> Result<(), TerminalError> {
    if !style.color() {
        return renderer.prompt(text);
    }

    let mut marked = String::with_capacity(DIM.len() + text.len() + PLAIN.len());
    marked.push_str(DIM);
    marked.push_str(text);
    marked.push_str(PLAIN);

    renderer.prompt(&marked)
}

/// Ends the row a prompt mark was left on.
///
/// The mark carries no line ending while it is live, which is also what leaves
/// the renderer no row to settle: nothing else can end it, so this does, the
/// same way and for the same reason. A terminal gets the carriage return with
/// it, as every other row written to one does; a pipe gets the byte on its own,
/// because a pipe has no column to return to and the carriage return would end
/// up in whatever kept the output.
///
/// Through the same door the mark went out of, since it is the same row: a
/// verbatim write over a live region that has already been settled. The settle
/// `prompt` does first has nothing left to do here, so the ending is all that
/// reaches the terminal.
pub(crate) fn ended<T: Terminal>(renderer: &mut Renderer<T>) -> Result<(), TerminalError> {
    let ending = if renderer.is_terminal() { "\r\n" } else { "\n" };

    renderer.prompt(ending)
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

/// The answers on offer, and the mark to type one after.
///
/// `always` is missing where nothing can be written down. Offering a yes that
/// outlives the process when no rule can be minted would be a promise crucible
/// cannot keep, and the user would find out by the same question coming back
/// tomorrow.
fn answers(writable: bool) -> &'static str {
    if writable {
        "  [y]es  [s]ession  [a]lways  [n]o › "
    } else {
        "  [y]es  [s]ession  [n]o › "
    }
}

/// The line for a rule that was written down, or that was not.
///
/// The rule is named either way, and clipped either way: it is minted from the
/// call the model asked for, so its text is the model's, and an unclipped
/// newline in it is a row the renderer did not count.
fn kept(rule: &Minted, outcome: Result<&Path, &RememberError>, width: usize) -> String {
    match outcome {
        // The file as well as the rule, because between them they are what
        // somebody opens and deletes to take the permission back.
        Ok(file) => format!(
            "· remembered {} in {}",
            clipped(rule, width),
            file.display()
        ),

        // The turn carries on and the engine still holds the answer for this
        // session, so this line is the whole of what the user is told: the rule
        // is here to be typed by hand, and the problem is why it has to be.
        Err(problem) => format!(
            "! {} was not remembered: {}",
            clipped(rule, width),
            clipped(problem, width)
        ),
    }
}

/// One line, at most `width` display columns of it.
///
/// Columns rather than characters, and counted by the renderer's own [`cut`]
/// rather than here: a wide character takes two of them and a narrow one
/// followed by the emoji presentation selector takes two between them, so a
/// line measured by counting characters is a line clipped to the wrong place in
/// either direction. Sharing the counting is what keeps this and the tail that
/// wraps the result from disagreeing about how wide the same string is.
///
/// Control characters become spaces first, before anything is counted: a
/// newline inside JSON arguments would otherwise break the line in two, and the
/// tail would be counting rows that the caller did not mean to write. A space
/// rather than nothing because that is what the row will show — [`cut`] drops
/// what a terminal will not draw, and this has already decided to draw
/// something.
fn clipped(text: impl fmt::Display, width: usize) -> String {
    let mut line: String = text
        .to_string()
        .trim()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();

    if cut(&line, width).is_none() {
        return line;
    }

    // The ellipsis is a column of the row rather than one past its end, so what
    // is kept has to leave room for it — the reason for asking twice, since the
    // first answer is also what says whether anything is owed at all. The offset
    // falls on a character boundary and never between one and the selector that
    // widens it, which is [`cut`]'s to guarantee and what this leans on.
    if let Some(kept) = cut(&line, width.saturating_sub(1)) {
        line.truncate(kept);
    }

    line.push('…');
    line
}

#[cfg(test)]
mod tests;
