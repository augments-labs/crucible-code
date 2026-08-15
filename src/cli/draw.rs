//! Events, turned into what a terminal shows.
//!
//! Committed lines carry no escape sequences. The live tail measures what it
//! holds in display columns to know where to move the cursor back to, and a
//! colour code is bytes it would count as width. Colour therefore goes only
//! where nothing is ever redrawn: the prompt mark, written straight through.

use std::fmt;

use crucible_core::{Event, Sensitivity, StopReason, ToolCall, ToolOutput};
use crucible_tui::{Renderer, Row, Slot, Terminal, TerminalError, cut, fold};

use super::style::Style;

mod opening;
pub(crate) mod when;

pub(crate) use opening::{Opening, opening};

/// Dim, then back. Only for text that is written once and never redrawn.
const DIM: &str = "\x1b[2m";
const PLAIN: &str = "\x1b[0m";

/// What every row of a question after the first is written behind.
///
/// The mark that opens a question sits at the first column, and this is what
/// keeps it the only thing there: a question long enough to wrap spills the
/// model's own text onto the rows below, and a row of that reading `? bash
/// wants to run: ls` must not be able to stand where the genuine one stands.
/// The same two columns the reason and the answers are written behind, so the
/// whole question reads as one block.
const UNDER: &str = "  ";

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

/// Says that there is nothing to ask, where a prompt was typed anyway.
///
/// The same words the session opened with, and drawn the same way: bold and in
/// the accent, wrapped rather than clipped because the half that says what to
/// do about it is the second half. A blank row on each side, so it reads as an
/// answer to what was just typed rather than as the start of a turn.
pub(crate) fn unconfigured<T: Terminal>(
    renderer: &mut Renderer<T>,
    said: &str,
    style: Style,
) -> Result<(), TerminalError> {
    let columns = renderer.columns();
    let rows: Vec<Row> = fold(said, columns)
        .into_iter()
        .map(|row| Row::new().then(Slot::Strong, row))
        .collect();

    renderer.commit("")?;
    renderer.present(&rows, style.palette())?;
    renderer.commit("")
}

/// Puts a prompt that was queued during a turn where a typed one would have
/// gone.
///
/// A line finished while the last answer was still arriving belongs above the
/// turn it starts rather than in the middle of the one it was typed during, so
/// it is committed here, on the way in.
pub(crate) fn queued<T: Terminal>(
    renderer: &mut Renderer<T>,
    said: &str,
    style: Style,
) -> Result<(), TerminalError> {
    renderer.present(
        &[crucible_tui::Prompt::committed(said, style.glyphs())],
        style.palette(),
    )
}

/// Writes the letter that answered a question, and ends the row.
///
/// Nothing echoed it: the answer arrived as a key rather than as a line the
/// terminal collected. Without this the record shows a question with no reply
/// under it, and the next thing drawn lands on the same row.
pub(crate) fn answered<T: Terminal>(
    renderer: &mut Renderer<T>,
    said: &str,
) -> Result<(), TerminalError> {
    renderer.prompt(&format!("{said}\r\n"))
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
/// The window is what bounds this rather than the compact ceiling a tool call
/// and its result are drawn to. Those two are reports of something that is
/// happening; this is the moment somebody decides whether it may, and a
/// decision taken on a command's leading columns is a decision about the
/// padding in front of the payload.
pub(crate) fn question<T: Terminal>(
    renderer: &mut Renderer<T>,
    call: &ToolCall,
    sensitivity: &Sensitivity,
    style: Style,
) -> Result<(), TerminalError> {
    let columns = renderer.columns();

    renderer.settle()?;
    for row in asked(call, sensitivity, columns) {
        renderer.commit(&row)?;
    }

    mark(renderer, answers(), style)
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

        // The answer is not over, and 0.x has no way to resume it. Saying so
        // is what turns it into something the user can act on — the same prompt
        // again picks up from a transcript that already holds this much.
        StopReason::Paused => Some("! unfinished: the provider paused this turn; ask it to go on"),

        StopReason::Cancelled => Some("! stopped"),

        // The provider named a reason this build has not heard of. Whatever it
        // was, it was not one of the two endings above that mean the answer is
        // whole — so it is reported as unfinished rather than passed over,
        // which is the one way a truncated answer reaches the reader looking
        // complete.
        StopReason::Unknown => {
            Some("! unfinished: the provider stopped for a reason this build does not know")
        }
    }
}

/// What the user is being asked to allow, in the rows a window `columns` wide
/// needs for it.
///
/// Wrapped rather than clipped, unlike everything else this file draws. What is
/// named here is the thing being consented to — the whole command, the whole
/// path — and a name with its tail cut off is one the reader agrees to without
/// having read it. Rows are what that costs, and rows are affordable: the
/// renderer measures every line it commits, so a question three rows tall is
/// three rows it knows it drew.
fn asked(call: &ToolCall, sensitivity: &Sensitivity, columns: usize) -> Vec<String> {
    let said = match sensitivity {
        // Never reached through the permission engine, which allows or refuses
        // a read and never asks about one. Here so that a tool reclassified
        // later still has a question to show rather than a blank one.
        Sensitivity::ReadOnly { target } => format!("? {} wants to read: {target}", call.name),

        Sensitivity::MutatesFile { target } => format!("? {} wants to change: {target}", call.name),

        Sensitivity::SpawnsProcess { command } => {
            format!("? {} wants to run: {command}", call.name)
        }
    };

    wrapped(&said, columns)
}

/// One question, folded to the window and written behind [`UNDER`] from the
/// second row on.
///
/// Both halves of that are load-bearing, because the text being folded is the
/// model's. The flattening comes first: a newline in it would otherwise break a
/// row nothing here counted, and the renderer would be moving the cursor back
/// over rows it did not write. The indent is what the flattening alone does not
/// buy — a command long enough to wrap puts the model's text on rows of its
/// own, and one of them saying `? bash wants to run: ls` directly above the
/// answer mark is consent for something nobody read.
///
/// Every row is folded to the width of the narrowest, so the indent cannot push
/// one past the last column. The first row pays two columns it did not have to;
/// a question is the wrong place to spend care on that.
fn wrapped(said: &str, columns: usize) -> Vec<String> {
    // A window with no room for the indent and something after it is folded
    // without one. Two columns of margin on a window three wide would leave a
    // row of one character, and rows wider than the window are rows the
    // terminal wraps itself — which is the one thing this may not produce.
    let under = if columns > UNDER.len() { UNDER } else { "" };

    // Never zero, or a window too narrow to fold into would leave the question
    // with no rows at all — and a mark with nothing above it is one somebody
    // answers blind.
    let room = columns.saturating_sub(under.len()).max(1);

    fold(&flattened(said), room)
        .into_iter()
        .enumerate()
        .map(|(row, text)| match row {
            0 => text.to_owned(),
            _ => format!("{under}{text}"),
        })
        .collect()
}

/// The answers on offer, and the mark to type one after.
///
/// Durable rules have no trusted per-workspace store yet. The prompt therefore
/// offers only answers whose lifetime it can honour.
fn answers() -> &'static str {
    "  [y]es  [s]ession  [n]o › "
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
/// Flattened before it is measured, for the reason [`flattened`] gives: what is
/// counted has to be what will be drawn.
fn clipped(text: impl fmt::Display, width: usize) -> String {
    let mut line = flattened(text);

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

/// `text` with everything the terminal would not draw turned into a space.
///
/// How many rows a line costs is this file's to decide and the renderer's to
/// count, so a control character in text that arrived from somewhere else is
/// the one thing that could take that decision away: a newline in a tool's
/// arguments is a second row nobody wrote. A space rather than nothing because
/// that is what the row will show — [`cut`] and [`fold`] drop what a terminal
/// will not draw, and this has already decided to draw something.
fn flattened(text: impl fmt::Display) -> String {
    text.to_string()
        .trim()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests;
