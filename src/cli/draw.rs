//! Events, turned into what a terminal shows.
//!
//! Committed lines carry no escape sequences. The live tail measures what it
//! holds in display columns to know where to move the cursor back to, and a
//! colour code is bytes it would count as width. Colour therefore goes only
//! where nothing is ever redrawn: the prompt mark, written straight through.
//!
//! Every mark drawn into a line comes out of [`Glyphs`] rather than out of a
//! literal here. A terminal whose font is missing a corner is missing the one a
//! tool result hangs off as much as the one a box is drawn with, and a second
//! set of marks in this file would be a second answer to a question the
//! configuration asks in one word.
//!
//! One line is drawn twice. A call stands in the footing with a mark that moves
//! for as long as its tool is out, and commits through [`returned`] once it has
//! answered — the same words, in the same columns, with the motion gone. So
//! what reaches scrollback is still free of escape sequences, and a pipe and
//! the session file get the still line rather than a frame of a moving one.

use std::fmt;

use crucible_core::{Event, Sensitivity, StopReason, Summary, ToolCall, ToolOutput};
use crucible_tui::{Glyphs, Renderer, Row, Slot, Terminal, TerminalError, columns, cut, fold};

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
        //
        // What a turn spent is on the row above the box, which is drawn from the
        // footing rather than committed here — a running total committed to
        // scrollback would be one line per reading, each of them wrong the
        // moment the next arrived.
        Event::TurnStarted { .. } | Event::Spent { .. } => Ok(()),

        Event::Delta { text } => renderer.stream(&text),

        // Whatever the model was saying is finished; it said it to explain the
        // call that follows. The line for the call itself is not written here:
        // it is live until the tool answers, standing in the footing with a
        // mark that moves, and a line the renderer moves back over cannot also
        // be in scrollback. It commits through [`returned`].
        Event::ToolRequested { .. } => renderer.settle(),

        Event::ToolFinished { output, .. } => {
            renderer.commit(&finished(&output, style.output(columns), style.glyphs()))
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
            renderer.commit(&format!(
                "! {}",
                clipped(error, style.output(columns), style.glyphs())
            ))
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
        clipped(problem, width, style.glyphs())
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

/// What a call's line says: the tool, and what the call is about.
///
/// The words in the brackets arrive on the event, worked out by the tool that
/// owns the arguments. Reading those here to pull out a path would be a second
/// reading of a schema the tool already owns, and the two would drift. A call
/// nobody could read is drawn as the bare name, because empty brackets would
/// claim it was about nothing.
///
/// Whole, and without the mark: this is what the footing holds for as long as
/// the tool is out, and the footing draws the mark itself because the mark is
/// the part that moves.
pub(crate) fn called(call: &ToolCall, summary: &Summary) -> String {
    let name = pascal(&call.name);

    if summary.is_empty() {
        name
    } else {
        format!("{name}({})", summary.as_str())
    }
}

/// A call line's words, in the columns a window this wide leaves them.
///
/// One place, because the line is drawn twice — live in the footing, then
/// committed — and a line that changed shape at the moment it stopped moving
/// would read as a second call. The mark and the space after it are the
/// window's rather than the words', so they come off the room before the
/// ceiling does: a line as wide as the window with a mark still in front of it
/// is a row the terminal wraps and the live tail never counted.
///
/// Nothing comes back where the window has room for neither. Both callers draw
/// the mark alone then — it still says a call was made, which is the half of
/// this line the result hanging under it cannot say for itself.
pub(crate) fn words(said: &str, window: usize, style: Style) -> String {
    let glyphs = style.glyphs();
    let room = style
        .args(window)
        .min(window.saturating_sub(columns(glyphs.called()) + 1));

    clipped(said, room, glyphs)
}

/// Commits the line of a call that has stopped being live.
///
/// The same words the footing was drawing, in the same columns, with the motion
/// gone — so what reaches scrollback carries no escape sequence, and the result
/// that follows hangs under a line that is already there.
pub(crate) fn returned<T: Terminal>(
    renderer: &mut Renderer<T>,
    said: &str,
    style: Style,
) -> Result<(), TerminalError> {
    let window = renderer.columns();
    let mark = style.glyphs().called();
    let said = words(said, window, style);

    renderer.settle()?;

    if said.is_empty() {
        renderer.commit(mark)
    } else {
        renderer.commit(&format!("{mark} {said}"))
    }
}

/// A tool's name as a row writes it: `web_fetch` becomes `WebFetch`.
///
/// The model's name for a tool is the wire's, chosen so a schema can carry it.
/// A row is read by a person, and the capital is what separates the name from
/// the words beside it at a glance.
fn pascal(name: &str) -> String {
    let mut written = String::with_capacity(name.len());

    for word in name.split('_') {
        let mut letters = word.chars();
        if let Some(first) = letters.next() {
            written.extend(first.to_uppercase());
            written.push_str(letters.as_str());
        }
    }

    written
}

/// The line for a call that finished, hung under the call it answers.
fn finished(output: &ToolOutput, width: usize, glyphs: Glyphs) -> String {
    let text = output.text();
    let mut lines = text.lines();
    let first = clipped(lines.next().unwrap_or_default(), width, glyphs);
    let rest = lines.count();

    let under = gutter(output.is_failed(), glyphs);
    match rest {
        0 => format!("{under}{first}"),
        more => format!("{under}{first} (+{more} lines)"),
    }
}

/// What a result row opens with, before whatever the tool said.
///
/// The corner sits one column past the mark that opened the call, and where
/// that column is is measured off the mark rather than counted out: the two are
/// then one decision instead of two that agree until somebody changes the mark.
/// A corner under the tool's name rather than under its mark reads as a second
/// call rather than as the first one's answer.
///
/// A failure is marked here and nowhere else. The call line above it stands as
/// it was — a call that was made is a call that was made, whatever came back —
/// and one thing says the answer was a failure: the row that says what it was.
fn gutter(failed: bool, glyphs: Glyphs) -> String {
    let indent = " ".repeat(columns(glyphs.called()) + 1);
    let cross = if failed {
        format!("{} ", glyphs.failed())
    } else {
        String::new()
    };

    format!("{indent}{} {cross}", glyphs.hangs())
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
fn clipped(text: impl fmt::Display, width: usize, glyphs: Glyphs) -> String {
    let mut line = flattened(text);

    if cut(&line, width).is_none() {
        return line;
    }

    // The ellipsis is columns of the row rather than columns past its end, so
    // what is kept has to leave room for it — the reason for asking twice, since
    // the first answer is also what says whether anything is owed at all. The
    // offset falls on a character boundary and never between one and the
    // selector that widens it, which is [`cut`]'s to guarantee and what this
    // leans on.
    //
    // Measured rather than assumed to be one: the ascii set spells it `...`,
    // and a line that reserved a single column for it would be committed three
    // columns wider than the window. The terminal wraps that itself, which is a
    // row the live tail never counted and a cursor a row off on every frame
    // after it.
    let more = glyphs.ellipsis();

    // A window with no room for the mark that says there is more has no room to
    // say it either: what goes out is the line's own first columns, cut where
    // they run out. Anything else is a mark three columns wide announcing that
    // a one-column window was too narrow — the overflow it exists to prevent,
    // committed by the thing preventing it.
    if columns(more) > width {
        if let Some(kept) = cut(&line, width) {
            line.truncate(kept);
        }

        return line;
    }

    if let Some(kept) = cut(&line, width.saturating_sub(columns(more))) {
        line.truncate(kept);
    }

    line.push_str(more);
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
