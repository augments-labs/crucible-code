//! Events, turned into what a terminal shows.
//!
//! Text that arrived from somewhere else is committed without colour. The
//! transcript measures what it holds in display columns to know how many rows
//! of the window a line takes, and a colour code in a tool's output is bytes an
//! untrusted string put there — bytes that would be counted as width.
//!
//! What this file composed itself is a different thing and goes out a different
//! door. A call line and the line hanging under it are spans this program built,
//! so they are handed to [`Renderer::present`] as rows and the palette decides
//! their colour at the last moment: the mark and the tool's name in the accent,
//! what the call was about and what came back in the quieter one. A row that
//! arrives already laid out is clipped to the window rather than folded into
//! it, so nothing here is counting its columns a second time.
//!
//! Every mark drawn into a line comes out of [`Glyphs`] rather than out of a
//! literal here. A terminal whose font is missing a corner is missing the one a
//! tool result hangs off as much as the one a box is drawn with, and a second
//! set of marks in this file would be a second answer to a question the
//! configuration asks in one word.
//!
//! What arrives here is a stream of events; what it is drawn as is a column of
//! blocks. What was asked, what was answered, a call and the line hanging under
//! it: each of those is one block, and what separates two of them is a row of
//! nothing. Every block asks [`Renderer::apart`] for that row on its way in
//! rather than leaving one behind — a block cannot know it was the last, and a
//! turn that parted on the way out would hand the shell back a prompt one row
//! lower than it found it.
//!
//! A call that changed a file is drawn with the change under it, inside that
//! same block: the count on the row hanging off the call, and below it the lines
//! that moved, each on a ground saying which way. Which lines those are is not
//! worked out here. It arrives on the result from the tool that held both
//! versions of the file, already bounded in rows and in the length of each of
//! them — so nothing on this side has to decide what to do about a change to a
//! whole file, and a call that sent no block is one that changed nothing rather
//! than one nobody could work out.
//!
//! Two lines are drawn twice. A call stands in the footing with a mark that
//! moves for as long as its tool is out, and commits through [`returned`] once
//! it has answered — the same words, in the same columns, with the motion gone.
//! So what joins the transcript is still free of escape sequences, and a pipe
//! and the session file get the still line rather than a frame of a moving one.
//!
//! The other is the line counting a run of calls that only looked around, which
//! stands over that one while the run goes on and commits through [`gathered`]
//! when it ends. The tense turns as it settles — a run still going is
//! *reading*, one that is over *read* — because a line the reader scrolls back
//! to is a line about something finished.

use std::borrow::Cow;
use std::fmt;
use std::path::Path;

use crucible_core::{
    Attachment, Change, Changed, Compacted, Compacting, Diff, Event, Modality, Question,
    Sensitivity, StopReason, Summary, ToolCall, ToolId, ToolOutput, Workspace, written,
};
use crucible_tools::Ended;
use crucible_tui::{
    Glyphs, Renderer, Row, Slot, Terminal, TerminalError, clip, columns, cut, fold,
};

use super::converse::Parting;
use super::kept::Kept;
use super::style::Style;

pub(crate) mod opening;
pub(crate) mod when;

pub(crate) use opening::{Opening, opening};

/// What every row of a question after the first is written behind.
///
/// The mark that opens a question sits at the first column, and this is what
/// keeps it the only thing there: a question long enough to wrap spills the
/// model's own text onto the rows below, and a row of that reading `? bash
/// wants to run: ls` must not be able to stand where the genuine one stands.
/// The same two columns the reason and the answers are written behind, so the
/// whole question reads as one block.
const UNDER: &str = "  ";

/// The narrowest a block's gutter of line numbers is drawn.
///
/// Three, because that is where a file stops being one screen: below it the
/// numbers say where the reader is looking, and a column that narrowed to fit
/// them would move the whole block sideways between one call and the next.
const NUMBER: usize = 3;

/// What stands where a file's kind stands, on the row saying a request went out
/// without it.
///
/// The content was not carried and the file has not moved, so it can be looked
/// at again. Nothing here names the ceiling: what a reader can act on is asking
/// again, and what they cannot act on is a number.
const AGAIN: &str = "not sent, can be read again";

/// What stands where a file's kind stands, on the row saying the model being
/// asked does not read it.
///
/// It offers nothing, because asking again would produce the same file of the
/// same kind. What a reader can act on is the model, so that is the word the
/// row leaves them holding.
const UNREAD: &str = "not sent, this model does not read it";

/// Draws one event.
pub(crate) fn event<T: Terminal>(
    renderer: &mut Renderer<T>,
    event: Event,
    workspace: &Workspace,
    style: Style,
    kept: &mut Kept,
) -> Result<(), TerminalError> {
    let columns = renderer.columns();

    match event {
        // The turn number is in the title bar, not in the transcript: a line
        // per turn saying which turn it is crowds out the turn itself. What it
        // is worth here is the row it parts from the prompt above it — the
        // answer starts under a blank rather than against the line that asked
        // for it.
        //
        // What a turn spent is on the row above the box, which is drawn from the
        // footing rather than committed here — a running total committed to the
        // transcript would be one line per reading, each of them wrong the
        // moment the next arrived.
        Event::TurnStarted { .. } => renderer.apart(),

        // All four are transient live-foot state rather than transcript lines.
        // Spend, retries and compaction shape the working row; carried load
        // refreshes the remaining-window fact in the prompt. Each is true only
        // until its next event, so committing every reading would leave a
        // column of stale state in front of the answer.
        Event::Spent { .. }
        | Event::PromptCache { .. }
        | Event::Sandbox { .. }
        | Event::Retrying
        | Event::Carried { .. }
        | Event::Compacting { .. } => Ok(()),

        // What it came to does not. Room having been made is a thing that
        // happened to the session rather than a state it is in, and the row
        // above the box takes its rows back — so this is written down where the
        // record of the session is, under the turn it happened in.
        Event::Compacted { compacted } => {
            renderer.apart()?;
            renderer.present(&compacted_rows(compacted, columns, style.glyphs()))
        }

        // Kept and not drawn. What a running command prints stands under the
        // call it belongs to, in rows the footing lays out and hands back; what
        // is kept here is the end of it, so the key that stands a result whole
        // stands a call that has not answered yet as well. Committing it would
        // put the tail of a build immediately above the same tail inside the
        // result that follows it.
        Event::Wrote { call, text } => {
            kept.wrote(&call, text.as_str());
            Ok(())
        }

        // Asked on every delta because the first one is the only one worth
        // asking on, and nothing here can tell which that was: a turn opens
        // with deltas, and so does whatever the model says after a tool has
        // answered. The renderer settles the question — a row is owed where
        // nothing is live, and never in the middle of a line still arriving.
        Event::Delta { text } => {
            renderer.apart()?;
            renderer.stream(&text)
        }

        // Whatever the model was saying is finished; it said it to explain the
        // call that follows. The line for the call itself is not written here
        // either way. Where the call has something live about it that line is
        // in the footing until the tool answers, and a line that is still
        // moving is not one the transcript can hold; where it has not, it is
        // handed straight back to be written. Both commit through [`returned`],
        // which is why neither is drawn from this arm.
        Event::ToolRequested { call, summary, .. } => {
            kept.calling(call.id.clone(), called(&call, &summary));
            renderer.settle()
        }

        // A line steered into the running turn, committed as the reader's own
        // words. The stream is settled first so the line lands after whatever
        // was arriving rather than in the middle of it — the same row a prompt
        // is owed, for the same reason.
        Event::Steered { line } => {
            renderer.settle()?;
            renderer.apart()?;
            renderer.landmark();
            prompt(renderer, line, style)
        }

        // One block: the row that says what came back, and under it the lines a
        // call that changed a file moved. No row parts them, because a reader
        // asking what the call did is asking both halves of the same question.
        // Under the answer it is about rather than under the prompt that sent
        // the files: what happened is that this request went out without them,
        // and the reader is being told at the moment it did.
        Event::Aged { files } => without(renderer, &files, AGAIN, workspace, style),
        Event::Unread { files } => without(renderer, &files, UNREAD, workspace, style),

        Event::ToolFinished { call, output, .. } => came_back(renderer, kept, &call, output, style),

        // The tail is settled either way; an answer that stopped early is
        // finished text as much as one that ran out of things to say.
        Event::TurnFinished { stop, .. } => {
            renderer.settle()?;
            match notice(stop) {
                Some(said) => {
                    renderer.apart()?;
                    renderer.commit(said)
                }
                None => Ok(()),
            }
        }

        // Wrapped rather than clipped: a refusal carries up to 8 KiB of the
        // provider's own words, and a response that failed part-way through
        // carries whatever the provider said about why. Every word of it is
        // what the reader has to go on, and the type that carried it bounded
        // it. The breaks are this program's, though: a newline in the
        // provider's text is folded into a space first, so where one row ends
        // and the next begins is never the provider's to choose, and a row
        // of its shaped like a question of ours cannot be made on purpose.
        // No mark in front of it either. The mark hangs a result off the line
        // that asked for it, and a failed turn was asked for by nobody's line
        // — it stands on its own, in the one colour kept for it.
        Event::Failed { error } => {
            renderer.settle()?;
            renderer.apart()?;
            let said = error.to_string();
            let flat = said.split_whitespace().collect::<Vec<_>>().join(" ");
            let rows: Vec<Row> = fold(&flat, columns)
                .into_iter()
                .map(|row| Row::new().then(Slot::Trouble, row))
                .collect();
            renderer.present(&rows)
        }
    }
}

/// Writes the line a command that ended on its own leaves behind.
///
/// Named apart from [`ended`], which is about the session: one word for two
/// endings is what the vocabulary table exists to prevent.
///
/// A line rather than a block, and the only thing in this program written to the
/// transcript outside a turn. It has to be written: the row under the box counts
/// what is running, and a count that quietly went down with nothing said would
/// leave a reader — and a model — believing a server is up. What it says is what
/// the reader cannot ask for afterwards, since the command is gone: which one,
/// how it ended, and how much it had printed.
///
/// Which one, in the words the call accounted for itself in where it gave any.
/// This row arrives long after the call that started it has scrolled off, and
/// what somebody agreed to on the panel was that sentence — handing back the
/// shell line it expanded to asks them to match the two up again. A call that
/// said nothing about itself is named by its command, because that is what
/// there is and nothing may be invented in its place.
///
/// And the status as a number in every case. The mark already says whether it
/// went well; the number is the part a reader takes to whatever was waiting on
/// it, and this is the only line that will mention this command again.
///
/// # Errors
///
/// [`TerminalError::Io`] if the terminal could not be written to.
pub(crate) fn gone<T: Terminal>(
    renderer: &mut Renderer<T>,
    ended: &Ended,
    style: Style,
) -> Result<(), TerminalError> {
    let glyphs = style.glyphs();
    let (mark, how) = match ended.code {
        Some(0) => (glyphs.done(), "exit status 0".to_owned()),
        Some(code) => (glyphs.failed(), format!("exit status {code}")),
        None => (glyphs.failed(), "killed".to_owned()),
    };

    // The command is what gives up room, and the words after it are what the
    // row is for. A command is as long as somebody typed it, so clipping the
    // sentence whole is clipping the end of it — and the end is how it ended
    // and how much it printed, which is the part nobody can go back and ask
    // for. So the tail is measured first and kept, and the command is cut to
    // what is left.
    let tail = format!(
        " ended on its own {} {how} {} {} lines",
        glyphs.dot(),
        glyphs.dot(),
        ended.lines
    );

    // The account where there is one, and no brackets around it: `Bash(…)` is
    // how this program spells a call, and a sentence inside that shape would
    // read as a command somebody could run.
    let named = if ended.said.trim().is_empty() {
        spelled(ended.tool, &ended.called)
    } else {
        ended.said.trim().to_owned()
    };

    let room = style.output(renderer.columns()).saturating_sub(2);
    let called = clipped(named, room.saturating_sub(columns(&tail)), glyphs);

    // Clipped again, because a window narrower than the tail alone has room
    // for neither and the row still may not be wider than the window it is
    // drawn on.
    let said = clipped(format!("{called}{tail}"), room, glyphs);

    renderer.settle()?;
    renderer.apart()?;
    renderer.commit(&format!("{mark} {said}"))
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
) -> Result<(), TerminalError> {
    let columns = renderer.columns();
    let rows: Vec<Row> = fold(said, columns)
        .into_iter()
        .map(|row| Row::new().then(Slot::Strong, row))
        .collect();

    renderer.commit("")?;
    renderer.present(&rows)?;
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
    // The same row a typed line is owed, for the same reason: what was asked is
    // a block, and a block is parted from the one above it. A line queued during
    // a turn lands under the answer that turn produced, which is exactly where
    // the blank belongs.
    renderer.apart()?;
    renderer.landmark();
    prompt(renderer, said.to_owned(), style)
}

/// Commits one prompt from source that can be laid out again after a resize.
fn prompt<T: Terminal>(
    renderer: &mut Renderer<T>,
    said: String,
    style: Style,
) -> Result<(), TerminalError> {
    renderer.responsive(
        said.len(),
        Box::new(move |columns| {
            crucible_tui::Prompt::committed(&said, columns, style.glyphs(), style.palette().bands())
        }),
    )
}

/// Counts the files a prompt sent, on rows under it.
///
/// Under rather than beside: what was asked is one block and the files went
/// with it, so they belong inside that block rather than in one of their own.
/// Called with the block still open, straight after the line.
///
/// Counts rather than names. The marker is the whole row because the marker is
/// the name — the prompt above uses it to point at the file, and a reader who
/// chose that file off their own disk a moment ago learns nothing from being
/// told where it was. Against that, a pasted screen grab is named by whatever
/// wrote it, so the path is long, shared with nothing else on the screen, and
/// frequently somebody's home directory read out to whoever is watching.
///
/// Files that stayed behind are named in full by [`without`], where which one
/// is the entire point of the row.
///
/// Reached from both places a committed prompt is drawn, which is why it is
/// here rather than in either of them. A row a live prompt gets and a replayed
/// one does not is not a transcript that remembers, it is two transcripts.
pub(crate) fn attached<T: Terminal>(
    renderer: &mut Renderer<T>,
    attachments: &[Attachment],
    style: Style,
) -> Result<(), TerminalError> {
    if attachments.is_empty() {
        return Ok(());
    }

    let mut numbers = Numbers::default();
    let named: Vec<String> = attachments
        .iter()
        .map(|one| {
            let number = numbers.next(one.modality);
            format!("[{} #{number}]", label(one.modality))
        })
        .collect();

    rows(renderer, &named, style)
}

/// Names the files a request went out without, where its answer arrives.
///
/// Said as it happens rather than kept: the answer that follows had less to
/// look at than the one before it, and this is the row that says which file it
/// stopped seeing. The word beside each name is the caller's, because the two
/// reasons a file stays behind leave a reader holding different moves and the
/// row has one line to say which.
///
/// It names no ceiling and asks for nothing — a file left behind is the design
/// working, and what to do about it is the reader's.
pub(crate) fn without<T: Terminal>(
    renderer: &mut Renderer<T>,
    attachments: &[Attachment],
    word: &str,
    workspace: &Workspace,
    style: Style,
) -> Result<(), TerminalError> {
    if attachments.is_empty() {
        return Ok(());
    }

    let named: Vec<String> = attachments
        .iter()
        .map(|one| format!("{} {word}", names(one, workspace)))
        .collect();

    // Settled first because a retry posts this again, and the second reading
    // must land after whatever the first answer had already put on the row.
    renderer.settle()?;
    rows(renderer, &named, style)
}

/// Names a file the way every other path this program prints is named:
/// relative to the root where it is under it, whole where it is not.
///
/// A row is clipped from its end, so an absolute path would spend the width on
/// the part every file in the workspace shares and cut the part that says which
/// one.
fn names(attachment: &Attachment, workspace: &Workspace) -> String {
    let path = Path::new(&*attachment.path);
    if let Ok(relative) = path.strip_prefix(workspace.root()) {
        return written(relative);
    }

    // `Attachment::path` is persisted with forward slashes. Compare that
    // normalized spelling on Windows too, where a `Path` built from it does not
    // necessarily share native components with the canonical workspace root.
    let path = written(path);
    let root = written(workspace.root());
    path.strip_prefix(&root)
        .and_then(|relative| relative.strip_prefix('/'))
        .unwrap_or(&path)
        .to_owned()
}

/// Separate per-kind sequence numbers for the files one prompt sent.
#[derive(Default)]
struct Numbers {
    text: usize,
    image: usize,
    pdf: usize,
    video: usize,
    audio: usize,
}

impl Numbers {
    fn next(&mut self, modality: Modality) -> usize {
        let one = match modality {
            Modality::Text => &mut self.text,
            Modality::Image => &mut self.image,
            Modality::Pdf => &mut self.pdf,
            Modality::Video => &mut self.video,
            Modality::Audio => &mut self.audio,
        };
        *one += 1;
        *one
    }
}

/// The modality word as it starts an attachment label.
fn label(modality: Modality) -> &'static str {
    match modality {
        Modality::Text => "Text",
        Modality::Image => "Image",
        Modality::Pdf => "PDF",
        Modality::Video => "Video",
        Modality::Audio => "Audio",
    }
}

/// One row per already-composed file display.
fn rows<T: Terminal>(
    renderer: &mut Renderer<T>,
    named: &[String],
    style: Style,
) -> Result<(), TerminalError> {
    let files: Vec<&str> = named.iter().map(String::as_str).collect();
    let columns = renderer.columns();
    renderer.present(&crucible_tui::Prompt::attached(
        &files,
        columns,
        style.glyphs(),
        style.palette().bands(),
    ))
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
    renderer.prompt(Slot::Plain, &format!("{said}\n"))
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

/// Says how to come back, on the screen the session did not run on.
///
/// The last thing crucible writes. A quit that was asked for gets exactly two
/// lines — the command that resumes this session — because that command names
/// an id nothing on the screen has said, and everything else about the session
/// is already in the scrollback the shell just got back. A log that stopped
/// recording earns one more line first: that failure was said while the session
/// ran, on a screen that no longer exists, so it is said once more or a reader
/// later opens a truncated transcript believing it whole.
///
/// Through [`Renderer::parting`] rather than [`Renderer::commit`], because
/// there is no picture left to commit under — the frame this renderer is
/// holding describes a screen that no longer exists.
pub(crate) fn parting<T: Terminal>(
    renderer: &mut Renderer<T>,
    parting: &Parting,
    style: Style,
) -> Result<(), TerminalError> {
    let (path, lost) = match parting {
        Parting::Nothing => return Ok(()),
        Parting::Kept(path) => (path, false),
        Parting::Lost(path) => (path, true),
    };

    let mut rows = vec![Row::new()];

    // The path is written whole. The sentence would survive losing its tail;
    // this is an address, and one clipped to the window is a reader sent
    // nowhere.
    if lost {
        rows.push(
            Row::new()
                .then(
                    Slot::Quiet,
                    format!(
                        "{} the log stopped recording — the transcript up to then is in ",
                        style.glyphs().dot()
                    ),
                )
                .then(Slot::Plain, path.display().to_string()),
        );
    }

    // The id is the file's stem because the file is named by it; a log kept
    // somewhere unnameable has no command worth printing.
    if let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) {
        rows.push(Row::new().then(Slot::Quiet, "Resume this session with:"));
        rows.push(Row::new().then(Slot::Plain, format!("  crucible --resume {id}")));
    }

    if rows.len() == 1 {
        return Ok(());
    }

    renderer.parting(&rows)
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

    mark(renderer, &answers(style.glyphs()))
}

/// One question, written into the transcript where there was no room to stand a
/// panel in.
///
/// Wrapped rather than clipped, for the reason the permission question above is:
/// half a question is a question about something else, and rows are affordable
/// because the renderer measures every line it commits.
///
/// Only the answers the call offered are numbered here. The panel adds two more
/// — one to write an answer nobody offered, one to leave — and the first of them
/// wants a line editor, which is exactly what a window this small has no room
/// for. Leaving is still offered, by the key that always means it.
pub(crate) fn asking<T: Terminal>(
    renderer: &mut Renderer<T>,
    question: &Question,
    at: usize,
    of: usize,
    style: Style,
) -> Result<(), TerminalError> {
    let columns = renderer.columns();

    renderer.settle()?;
    let counted = if of > 1 {
        format!("? {} ({} of {of})", question.question(), at + 1)
    } else {
        format!("? {}", question.question())
    };
    for row in wrapped(&counted, columns) {
        renderer.commit(&row)?;
    }

    for (number, answer) in question.answers().enumerate() {
        let said = if answer.says().is_empty() {
            format!("{UNDER}{}. {}", number + 1, answer.answer())
        } else {
            format!(
                "{UNDER}{}. {} {} {}",
                number + 1,
                answer.answer(),
                style.glyphs().dash(),
                answer.says()
            )
        };
        for row in wrapped(&said, columns) {
            renderer.commit(&row)?;
        }
    }

    mark(renderer, UNDER)
}

/// Writes something the user is expected to type after.
///
/// Through `prompt` rather than `commit`, because what is wanted here is a line
/// with no ending on it: the answer goes on the same row. The tone is a slot
/// rather than an escape sequence the caller wrote, so the palette decides it
/// at the moment the row is drawn and a theme chosen later repaints it.
pub(crate) fn mark<T: Terminal>(
    renderer: &mut Renderer<T>,
    text: &str,
) -> Result<(), TerminalError> {
    renderer.prompt(Slot::Quiet, text)
}

/// Ends the row a prompt mark was left on.
///
/// The mark carries no line ending while it is live, and nothing else can end
/// it: what comes next would otherwise be written on the same row, which is
/// every ordinary exit as well as the next thing said.
pub(crate) fn ended<T: Terminal>(renderer: &mut Renderer<T>) -> Result<(), TerminalError> {
    renderer.prompt(Slot::Plain, "\n")
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
/// The tool's name is in the accent and what the call is about is in the quieter
/// colour, so a column of calls reads as the tools that ran with their arguments
/// beside them rather than as a paragraph. They are told apart here, after the
/// clipping and not before it, because how much of the line a narrow window
/// leaves is decided on the whole of it — a name cut off before its bracket is a
/// row with no arguments on it, and then there is nothing to tell apart.
///
/// An empty row comes back where the window has room for neither. Both callers
/// draw the mark alone then — it still says a call was made, which is the half
/// of this line the result hanging under it cannot say for itself.
pub(crate) fn words(said: &str, window: usize, style: Style) -> Row {
    let glyphs = style.glyphs();
    let room = style
        .args(window)
        .min(window.saturating_sub(columns(glyphs.called()) + 1));

    let said = clipped(said, room, glyphs);

    match said.split_once('(') {
        Some((name, about)) => Row::new()
            .then(Slot::Strong, name)
            .then(Slot::Quiet, format!("({about}")),
        None => Row::new().then(Slot::Strong, said),
    }
}

/// Writes the line of a call that has stopped being live.
///
/// The same words the footing was drawing, in the same columns and the same
/// colours, with the motion gone — the mark stops pulsing and settles on the
/// accent, and the result that follows hangs under a line that is already there.
pub(crate) fn returned<T: Terminal>(
    renderer: &mut Renderer<T>,
    said: &str,
    style: Style,
) -> Result<(), TerminalError> {
    let window = renderer.columns();
    let words = words(said, window, style);

    renderer.settle()?;
    renderer.apart()?;

    let mut row = Row::new().then(Slot::Accent, style.glyphs().called());
    if !words.is_empty() {
        row.push(Slot::Plain, " ");
        row = row.join(words);
    }

    renderer.present(&[row])
}

/// Writes the one line a folded run of calls comes to.
///
/// The mark a call row wears and, beside it, what the run did — in the slot
/// that says a row has something behind it, so the whole of the sentence lights
/// under the pointer and a click opens every call it counted.
///
/// No offer of a key. The key stands *everything* that was cut, which is a
/// different question from this row, and a row that named it would be telling a
/// reader to press something that answers a wider one. The line itself is the
/// door, which is what the slot it is written in means everywhere else.
///
/// Hands back the record row it went down on. Pointing the run's results at
/// that row is the caller's, because the two callers reach it differently: a
/// turn sweeps up everything it gathered without a row, and a walk through a
/// session already knows which results belong to which run.
///
/// # Errors
///
/// [`TerminalError::Io`] if the terminal could not be written to.
pub(crate) fn gathered<T: Terminal>(
    renderer: &mut Renderer<T>,
    said: &str,
    style: Style,
) -> Result<usize, TerminalError> {
    let glyphs = style.glyphs();
    let window = renderer.columns();
    let mark = glyphs.called();
    let room = window.saturating_sub(columns(mark) + 1);

    renderer.settle()?;
    renderer.apart()?;

    let row = Row::new()
        .then(Slot::Accent, mark)
        .then(Slot::Plain, " ")
        .then(Slot::Cut, clipped(said, room, glyphs));

    renderer.present(&[row])?;

    // Read after the row and not before it, because the row a result answers to
    // is the one that has just gone down.
    Ok(renderer.lines().saturating_sub(1))
}

/// A call as a row spells it, from the tool's own name and what the call was
/// about.
///
/// The shape [`called`] makes out of a `ToolCall`, for the places that hold the two
/// halves separately instead — a command still running, which has outlived the call
/// that carried them together.
pub(crate) fn spelled(tool: &str, about: &str) -> String {
    let name = pascal(tool);

    if about.is_empty() {
        name
    } else {
        format!("{name}({about})")
    }
}

/// A tool's name as a row writes it: `web_fetch` becomes `WebFetch`.
///
/// The model's name for a tool is the wire's, chosen so a schema can carry it.
/// A row is read by a person, and the capital is what separates the name from
/// the words beside it at a glance.
pub(crate) fn pascal(name: &str) -> String {
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

/// The row for a call that finished, hung under the call it answers.
///
/// The corner sits one column past the mark that opened the call, and where
/// that column is is measured off the mark rather than counted out: the two are
/// then one decision instead of two that agree until somebody changes the mark.
/// A corner under the tool's name rather than under its mark reads as a second
/// call rather than as the first one's answer.
///
/// Quiet, corner and words together. The line above already says what was done;
/// this is the detail under it, and a column of calls is read by its marks and
/// its names rather than by what each of them happened to return.
///
/// A failure is marked here and nowhere else, and the cross is the one thing on
/// the row in the reader's own foreground — the call line above it stands as it
/// was, because a call that was made is a call that was made whatever came back,
/// so this row is the only place the eye can be sent.
///
/// A call that changed a file says so in the change's own arithmetic instead of
/// in its answer to the model. Both are true of the same call, and only one is
/// about the file: how many replacements `edit` made is a fact about the
/// instruction it was sent, and the reader is looking at what is in the file.
fn finished(output: &ToolOutput, beyond: usize, window: usize, style: Style) -> Row {
    let glyphs = style.glyphs();
    let mut row = Row::new().then(Slot::Plain, " ".repeat(columns(glyphs.called()) + 1));
    row.push_structural(Slot::Quiet, glyphs.hangs());
    row.push(Slot::Quiet, " ");

    if output.is_failed() {
        row.push(Slot::Plain, format!("{} ", glyphs.failed()));
    }

    // Measured off the row itself rather than worked out from the glyphs a
    // second time: the corner, the space after it and the cross a failure wears
    // are the window's columns rather than the words', and a row that spends
    // them without taking them off is one the terminal wraps and this process
    // never counted. How much of a wide screen a result may take is the style's
    // answer; how much is there at all is the window's, and it wins.
    let room = style
        .output(window)
        .min(window.saturating_sub(row.columns()));

    if let Some(counts) = changed(output) {
        counted(
            &mut row,
            counts,
            output.diff().map_or(0, Diff::dropped),
            room,
        );
        return row;
    }

    let said = summary(output.text());

    if beyond == 0 {
        row.push(Slot::Quiet, clipped(said, room, glyphs));
        return row;
    }

    // The tail comes off the room before the line does. It is the part of the
    // row a reader is looking for — how much was cut, and the key that gives it
    // back — so a first line long enough to push it past the window would be
    // hiding the offer behind the thing being offered.
    let (counted, opens, shut) = offer(beyond, glyphs);
    let tail = columns(&counted)
        .saturating_add(columns(opens))
        .saturating_add(columns(shut));

    // And where the window is too narrow for even that, the line is what the
    // row keeps: an offer alone says nothing about what came back, and the key
    // it names works whether or not the row had room to mention it. It keeps
    // the cut slot with it, because what that slot says is that the result was
    // cut, and a narrow window did not make it whole.
    if tail >= room {
        row.push(Slot::Cut, clipped(said, room, glyphs));
        return row;
    }

    row.push(Slot::Cut, clipped(said, room.saturating_sub(tail), glyphs));
    row.push(Slot::Quiet, counted);
    row.push(Slot::Accent, opens);
    row.push(Slot::Quiet, shut);

    row
}

/// One human-facing line from a tool's complete result.
///
/// A record is summarised by the record. `gh pr view --json …` comes back
/// pretty-printed over thirty rows, and the first content line of one is
/// whichever key happens to sort first — `"assignees": []` says nothing about
/// a pull request, and a row that says it has spent the reader's only line on
/// nothing. So a result that opens like a record is squeezed back onto one
/// line and as much of it as the window holds is shown, in the order it
/// arrived.
///
/// Everything else keeps the rule it always had: pretty-printers put braces and
/// brackets on lines of their own, and an output that starts with one takes its
/// first line of content instead. Deliberately narrower than JSON parsing: a
/// shell command may print fragments, diagnostics and any other text, and a
/// first line such as `[exit status 3]` remains exactly what it said.
fn summary(text: &str) -> Cow<'_, str> {
    if structured(text) {
        return Cow::Owned(squeezed(text));
    }

    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();

    if structural(first) {
        Cow::Borrowed(
            lines
                .find(|line| !line.trim().is_empty() && !structural(line))
                .unwrap_or_default(),
        )
    } else {
        Cow::Borrowed(first)
    }
}

/// Whether `text` opens the way a record does.
///
/// Two characters, not a parse. A brace or a bracket says a record might be
/// starting and the character after it says whether one is: a record holds
/// names, records, lists, numbers or nothing, and `[exit status 3]` and
/// `{ pattern } matched nothing` hold a word — which is how a sentence
/// somebody wrote in brackets keeps its spaces.
fn structured(text: &str) -> bool {
    let text = text.trim_start();
    let mut characters = text.chars();
    let opens = characters.next();
    let next = characters.as_str().trim_start().chars().next();

    match (opens, next) {
        (Some('{'), Some('"' | '}'))
        | (Some('['), Some('{' | '[' | '"' | ']' | '-' | 't' | 'f' | 'n')) => true,
        (Some('['), Some(digit)) => digit.is_ascii_digit(),
        _ => false,
    }
}

/// How much of a record is worth squeezing onto one line.
///
/// Wider than any window, because the row is clipped against the real one a
/// moment later and a record is not read to its end here. The bound is what
/// keeps a result the size of a repository from being copied to draw one row.
const SQUEEZED: usize = 1024;

/// `text` with the layout between its parts dropped and the words inside them
/// kept.
///
/// Whitespace outside a value is a pretty-printer's indentation; whitespace
/// inside one is somebody's sentence, and `"feat(session): add journal"` has to
/// come back with its spaces. One space after a comma or a colon, because that
/// is how a record reads when it is written on one line rather than thirty.
fn squeezed(text: &str) -> String {
    let mut squeezed = String::new();
    let mut inside = false;
    let mut escaped = false;

    for character in text.chars() {
        if squeezed.len() >= SQUEEZED {
            break;
        }

        if inside {
            squeezed.push(character);
            // A quote after a backslash is a quote in the value rather than the
            // end of it, and a backslash after a backslash is a backslash.
            inside = character != '"' || escaped;
            escaped = character == '\\' && !escaped;
            continue;
        }

        match character {
            '"' => {
                inside = true;
                escaped = false;
                squeezed.push(character);
            }
            ',' | ':' => {
                squeezed.push(character);
                squeezed.push(' ');
            }
            // The layout itself, and the space this puts back after a comma or
            // a colon means the one already there would be a second.
            _ if character.is_whitespace() => {}
            _ => squeezed.push(character),
        }
    }

    squeezed
}

/// Whether a line says only the nesting a pretty-printer put around content.
fn structural(line: &str) -> bool {
    matches!(line.trim().trim_end_matches(','), "{" | "}" | "[" | "]")
}

/// The two halves of what a cut result offers: how much it cut, and the door.
///
/// Parted so they can be lit apart. The count is a fact about what came back
/// and the key is the way to the rest of it, and a click on this row opens the
/// same door the key does — so the accent goes on the half that answers one,
/// the way the count of what is still running is lit under the box and the
/// mark parting it from the mode is not.
///
/// The words in front of both are in the cut slot rather than the quiet one the
/// rest of this row is in. At rest the two look the same, which is right: a row
/// of the transcript is a row of the transcript. With the pointer on this result
/// the slot turns to the reader's own foreground, on every row of this result
/// and on no row of another — what lights and what a click opens have to be the
/// same result, or the reader is shown one and given the other. No ground,
/// because a ground would answer which row rather than which result.
fn offer(beyond: usize, glyphs: Glyphs) -> (String, &'static str, &'static str) {
    (
        format!(" (+{beyond} lines {} ", glyphs.dot()),
        "ctrl+o to expand",
        ")",
    )
}

/// Every row one answered call comes back as.
///
/// The row that says what came back, and under it the lines a call that changed
/// a file moved. No row parts them, because a reader asking what the call did is
/// asking both halves of the same question.
///
/// One builder rather than two, for the reason [`words`] is one: this is drawn
/// when the call answers and again when the session is put back on the screen,
/// and a result that read one way live and another way on the way back in is two
/// results as far as a reader is concerned.
pub(crate) fn finished_rows(output: &ToolOutput, window: usize, style: Style) -> Vec<Row> {
    let glyphs = style.glyphs();
    let mut rows = vec![finished(output, beyond(output), window, style)];

    if let Some(diff) = output.diff().filter(|diff| !diff.is_empty()) {
        rows.extend(block(diff, window, glyphs));
    }

    rows
}

/// One result, on the screen and held where the key that opens it can find it.
///
/// The one door a result goes through, and it is one because both paths take
/// it: the turn drawing what has just come back, and the replay putting a
/// session back on the screen. A result that lit and expanded live and did
/// neither on the way back in would be the same result behaving as two, which
/// is what a reader picking a session up would find strange first.
///
/// # Errors
///
/// [`TerminalError::Io`] if the terminal could not be written to.
pub(crate) fn came_back<T: Terminal>(
    renderer: &mut Renderer<T>,
    kept: &mut Kept,
    call: &ToolId,
    output: ToolOutput,
    style: Style,
) -> Result<(), TerminalError> {
    let beyond = beyond(&output);
    let rows = if changed(&output).is_some() && renderer.is_terminal() {
        let retained = output.clone();
        let rows = finished_rows(&retained, renderer.columns(), style);
        let bytes = retained.diff().map_or(0, Diff::retained);
        renderer.responsive(
            bytes,
            Box::new(move |columns| finished_rows(&retained, columns, style)),
        )?;
        rows
    } else {
        let rows = finished_rows(&output, renderer.columns(), style);
        renderer.present(&rows)?;
        rows
    };

    // After the rows and not before them, because this is what the rows could
    // not say: a result the row said the whole of is a result nobody has to be
    // offered, and one held anyway would be an offer to show somebody what they
    // are already looking at.
    //
    // Where the offer went is the block's first row, which is the one that names
    // the key — the lines under it are a change, and a change is cut where it is
    // built rather than here, so it offers nothing. Counted back from the end
    // because the rows have already gone.
    if beyond > 0 {
        let at = renderer.lines().saturating_sub(rows.len());
        kept.finished(call, output.into_text(), at);
    } else {
        // Even when nothing needs retaining, the call and any live tail are
        // over. Leaving them would show a completed call as live.
        kept.answered(call);
    }

    Ok(())
}

/// How many lines of a result its row has no room to say.
///
/// Zero for a call that changed a file. That result is drawn as the change
/// itself and the block under the row is where its lines are; what the block
/// could not fit was cut where the change was built rather than here, so there
/// is nothing left over to offer.
///
/// Zero on the way back in too, where the counts came off the log and the lines
/// did not. There is even less to offer then — the key would open a result whose
/// text is the tool's sentence about the call, which the row already says.
fn beyond(output: &ToolOutput) -> usize {
    if changed(output).is_some() {
        return 0;
    }

    output.text().lines().count().saturating_sub(1)
}

/// What a call changed, from whichever of the two still knows.
///
/// Live, the lines are here and the counts are read off them. On the way back in
/// they are not: a diff is the reader's alone and never reaches the log, so what
/// a resumed session has is the two numbers the result carried down beside it.
/// One question, asked once, so the header a reader met live is the header they
/// meet again — a row that counted lines one way live and another way on the way
/// back in is the same call behaving as two.
///
/// A change of nothing is no change: a call that left the file as it was has a
/// header to draw only if `Added 0 lines` is worth a row, and it is not.
fn changed(output: &ToolOutput) -> Option<Changed> {
    output
        .diff()
        .map(|diff| Changed::new(diff.added(), diff.removed()))
        .or_else(|| output.changed())
        .filter(|counts| !counts.is_empty())
}

/// What a change did, counted.
///
/// The numbers are what the row is read for, so they are the part of it that
/// stands out; the words around them are as quiet as the rest of the line.
///
/// What the block below could not fit is said here rather than at its foot,
/// because this is the row that claims a number — a reader told nine lines went
/// in and shown six of them has been told the truth about the call, and a block
/// that stopped without saying so reads as the whole of what happened.
///
/// `dropped` is what the block below is leaving out, and it is passed in rather
/// than read off the counts because it is a fact about lines being drawn. A
/// resumed session has the counts and no lines, so nothing is being left out of
/// anything, and a row that said otherwise would be counting rows nobody is
/// looking at.
fn counted(row: &mut Row, counts: Changed, dropped: usize, room: usize) {
    let before = row.columns();

    match (counts.added(), counts.removed()) {
        (0, removed) => count(row, "Removed ", removed),
        (added, 0) => count(row, "Added ", added),
        (added, removed) => {
            count(row, "Added ", added);
            row.push(Slot::Quiet, ", ");
            count(row, "removed ", removed);
        }
    }

    // Left off a window with no room for it rather than clipped, and the
    // numbers above are never either: half of "(5 of them not shown)" says
    // nothing, and a clipped count says something untrue.
    let said = format!(" ({dropped} of them not shown)");
    if dropped > 0 && row.columns() - before + columns(&said) <= room {
        row.push(Slot::Quiet, said);
    }
}

/// `word`, then a number of lines with the number emphasised.
fn count(row: &mut Row, word: &str, lines: usize) {
    row.push(Slot::Quiet, word);
    row.push(Slot::Strong, lines.to_string());
    row.push(Slot::Quiet, if lines == 1 { " line" } else { " lines" });
}

/// The narrowest column the numbers down the left of a block all fit in.
///
/// Never under [`NUMBER`], so two calls in a column start their lines in the
/// same place whatever part of a file each of them touched.
fn gutter(diff: &Diff) -> usize {
    let widest = diff
        .lines()
        .iter()
        .map(|line| line.number().checked_ilog10().unwrap_or(0))
        .max()
        .unwrap_or(0);

    usize::try_from(widest)
        .unwrap_or(0)
        .saturating_add(1)
        .max(NUMBER)
}

/// The lines a call moved, under the row that counted them.
///
/// Each row is the number the reader would find that line at, the sign saying
/// which way it went, and the line itself. The two that moved are drawn on a
/// ground of their own carried to the last column of the window: a block of
/// colour is what the eye finds before it reads anything, and one that stopped
/// where its text stopped would have a ragged edge that means nothing.
///
/// The lines around them are on the reader's own ground, because they did not
/// move. They are here so the ones that did have something to be read against,
/// and a context line painted like the rest of the terminal is one the eye skips
/// on its way to the change — which is what it is for.
///
/// Clipped to the window rather than to the ceiling a result line is held to.
/// That ceiling exists to keep one event to about one row, and these rows are
/// the event: a line of code cut off at the same column on a wide terminal would
/// be hiding the change the block was drawn to show.
fn block(diff: &Diff, window: usize, glyphs: Glyphs) -> Vec<Row> {
    let left = columns(glyphs.called()) + 4;
    let gutter = gutter(diff);

    // A space each side of the number, the sign, and two more before the text:
    // the columns the row spends before it says anything.
    let room = window.saturating_sub(left + gutter + 5);

    diff.lines()
        .iter()
        .map(|line| {
            let (ground, marked, sign) = match line.change() {
                Change::Kept => (Slot::Plain, Slot::Quiet, ' '),
                Change::Removed => (Slot::Removed, Slot::RemovedNumber, '-'),
                Change::Added => (Slot::Added, Slot::AddedNumber, '+'),
            };

            let mut row = Row::new().then(Slot::Plain, " ".repeat(left));
            row.push(marked, format!(" {:>gutter$} {sign}  ", line.number()));
            row.push(ground, indented(line.text(), room, glyphs));

            if !matches!(line.change(), Change::Kept) {
                row.fill(ground, window);
            }

            row
        })
        .collect()
}

/// The record of room having been made, as it joins the transcript.
///
/// Ruled above and below, like anything else that is true of the session rather
/// than of the row above it. It says what caused it before what it came to,
/// because the cause is the part a reader can do something about.
pub(super) fn compacted_rows(compacted: Compacted, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let rule = || Row::new().then(Slot::Quiet, glyphs.horizontal().repeat(columns));
    let why = match compacted.why {
        Compacting::Asked => "you asked",
        Compacting::Resumed => "you chose notes over carrying this session whole",
        Compacting::Full => "the window was full",
        Compacting::Refused => "the model would not take another request this size",
    };
    let turns = if compacted.kept == 1 { "turn" } else { "turns" };
    let result = if compacted.replaced == 0 {
        format!(
            " old tool output was cleared {} {} → {} carried {} {} {turns} kept whole",
            glyphs.dot(),
            tokens(compacted.before),
            tokens(compacted.after),
            glyphs.dot(),
            compacted.kept,
        )
    } else {
        format!(
            " {} messages became a recap {} {} → {} carried {} {} {turns} kept whole",
            compacted.replaced,
            glyphs.dot(),
            tokens(compacted.before),
            tokens(compacted.after),
            glyphs.dot(),
            compacted.kept,
        )
    };

    vec![
        rule(),
        Row::new()
            .then(Slot::Plain, " compacted ")
            .then(Slot::Quiet, format!("{} {why}", glyphs.dot())),
        Row::new().then(Slot::Quiet, result),
        rule(),
    ]
}

/// What a request to make room says when there was none to make.
///
/// A session with nothing behind the turns it keeps whole has no middle for a
/// recap to stand in place of, and compaction spends nothing on it. Said out
/// loud rather than passed over in silence: somebody who asked for room, or
/// chose notes over carrying a session whole, is owed the answer either way.
///
/// # Errors
///
/// [`TerminalError::Io`] if the terminal could not be written to.
pub(crate) fn unmade<T: Terminal>(renderer: &mut Renderer<T>) -> Result<(), TerminalError> {
    let window = renderer.columns();
    let rows = [Row::new().then(Slot::Quiet, clip(NOTHING, window))];

    renderer.present(&rows)
}

/// And what it says.
const NOTHING: &str = "there is nothing behind this turn worth replacing yet";

/// What a request to make room says when the key that stops a turn stopped it.
///
/// Told apart from [`unmade`] because the two leave the same session behind and
/// mean opposite things to the person reading: that one says this session has
/// no middle worth replacing, and this one says the thing they already know
/// they did. A reader who pressed the key and was told the session was too
/// short would go looking for what was wrong with their session.
///
/// The words are [`notice`]'s, for the reason a stopped turn gets them: it is
/// the same event — a request that was going and is not any more — and two
/// spellings of it would come apart.
///
/// # Errors
///
/// [`TerminalError::Io`] if the terminal could not be written to.
pub(crate) fn stopped<T: Terminal>(renderer: &mut Renderer<T>) -> Result<(), TerminalError> {
    let window = renderer.columns();
    let said = notice(StopReason::Cancelled).unwrap_or_default();
    let rows = [Row::new().then(Slot::Quiet, clip(said, window))];

    renderer.present(&rows)
}

/// A token count, as a row says it.
pub(crate) fn tokens(tokens: u64) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{}k", tokens / 1_000),
        _ => format!("{}.{}M", tokens / 1_000_000, (tokens % 1_000_000) / 100_000),
    }
}

/// What to say about a turn that ended, if anything.
///
/// Every variant is named rather than caught by a rest pattern: a stop reason
/// this file has not considered is exactly the one that would go unmentioned,
/// and the whole point of the set being closed is that a new one cannot.
pub(crate) fn notice(stop: StopReason) -> Option<&'static str> {
    match stop {
        // The ordinary ending, and the one taken every time. The answer speaks
        // for itself; a line under each turn saying it finished is noise.
        StopReason::Yielded | StopReason::WantsTools => None,

        StopReason::OutOfTokens => Some("! unfinished: the answer reached the token ceiling"),

        // The request never fit, so there is no answer to be unfinished. Said
        // in the words of the remedy rather than of the failure: what the
        // reader can do about it is make the session smaller, and nothing about
        // asking again unchanged will help.
        StopReason::WindowExceeded => {
            Some("! the session no longer fits this model's window; try /compact")
        }

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
        // Reached only through an `ask` rule somebody wrote about exactly
        // this path — no mode asks about a read inside the workspace.
        Sensitivity::ReadOnly { target } => format!("? {} wants to read: {target}", call.name),

        // The target is absolute, which already says it is nobody's file in
        // this workspace; the caption says so in words because the path alone
        // asks the reader to notice.
        Sensitivity::ReadsOutside { target } => format!(
            "? {} wants to read, outside the workspace: {target}",
            call.name
        ),

        Sensitivity::MutatesFile { target } => format!("? {} wants to change: {target}", call.name),

        // The line as it was sent, never the spelling a rule is matched
        // against. That one names the commands and drops the operators between
        // them, and the operators are the question: `a && b` is two commands if
        // the first worked, and consent to a list is consent to something
        // nobody sent.
        Sensitivity::SpawnsProcess { command } => {
            format!("? {} wants to run: {}", call.name, command.sent())
        }

        // Both facts, because neither alone is the question. The host is what a
        // rule is written about; what is *sent* is the thing that leaves the
        // machine, and for a search that is the query rather than any address.
        Sensitivity::ReachesNetwork { host } => {
            format!("? {} wants to reach {host}: {}", call.name, host.sent())
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
///
/// The mark is the one the prompt is typed after, because this is the prompt:
/// a question with a line typed under it, standing where no box could — the
/// answer decides whether a tool runs, so it is asked on the thread the tool
/// would run on.
fn answers(glyphs: Glyphs) -> String {
    format!("  [y]es  [s]ession  [n]o {} ", glyphs.caret())
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
pub(crate) fn clipped(text: impl fmt::Display, width: usize, glyphs: Glyphs) -> String {
    within(flattened(text), width, glyphs)
}

/// One line of a file, or of a command's output, at most `width` display columns
/// of it.
///
/// Everything else here is a sentence written for a row, and [`flattened`] tidies
/// a stray space off each end of it. A line of a file is read against the line
/// above it, and what that comparison is made of first is where each of them
/// starts — so this one keeps its indentation and loses only the end, where a
/// carriage return the file was saved with would otherwise become a space the
/// row is padded by. A build's output is read the same way, and indents its own
/// lines to say what belongs to what.
pub(crate) fn indented(text: &str, width: usize, glyphs: Glyphs) -> String {
    let line = text
        .trim_end()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();

    within(line, width, glyphs)
}

/// `line`, cut to `width` with a mark saying it was cut.
fn within(mut line: String, width: usize, glyphs: Glyphs) -> String {
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
pub(crate) fn flattened(text: impl fmt::Display) -> String {
    text.to_string()
        .trim()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests;
