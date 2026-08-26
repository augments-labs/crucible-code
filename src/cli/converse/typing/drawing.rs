//! The box on screen, and what a click on it landed on.
//!
//! One owner for both, because they are the same arithmetic read in opposite
//! directions: a click read against a box laid out any other way lands
//! somewhere nobody pointed at. Everything here is about a width and a number
//! of rows — what the keys mean is next door.

use std::io;

use crucible_tui::{
    Aimed, Caret, CommandCount, Draft, Editor, Prompt, PromptRows, Recalled, Remaining, Renderer,
    Row, Terminal, TerminalError, Typed,
};

use crate::cli::Fatal;
use crate::cli::style::Style;

use crate::cli::converse::planning::Planning;

use super::{Opened, Says};

/// What is drawn around the box between turns.
///
/// Four facts in one value because a call taking them all beside the renderer,
/// the editor and the style is a call nobody can read — which is what the
/// argument ceiling is there to stop.
#[derive(Clone, Copy)]
pub(super) struct Around<'a> {
    /// The plan the agent is working to, above everything else.
    pub(super) planning: &'a Planning,
    /// The commands a leading slash is offering, between the plan and the box.
    pub(super) open: &'a Opened,
    /// What the rows under the box say.
    pub(super) says: &'a Says,
    /// Where in the retained prompts the line came from, which the top border
    /// of the box says while an arrow is what put it there.
    pub(super) history: Recalled,
}

/// The four of them at one call.
///
/// A function rather than a literal at each place that draws: the names are
/// worth writing once, and the call that draws the box is worth keeping on one
/// line.
pub(super) fn around<'a>(
    planning: &'a Planning,
    open: &'a Opened,
    says: &'a Says,
    history: Recalled,
) -> Around<'a> {
    Around {
        planning,
        open,
        says,
        history,
    }
}

/// Puts the box on screen with the cursor where the line was typed to, and
/// whatever the line has opened directly over it.
///
/// The box goes into the band that is held to a share of the window, because
/// that share is a rule about how much of the screen a long prompt may take
/// from what it is answering. Everything above it — the list and the plan —
/// goes into the band above, which has no share: a list is what the reader is
/// looking at while it is open, so the transcript is what gives way to it
/// rather than the box being pushed off the screen. Both bands reach the
/// renderer together so the prompt, status and map control come from one frame.
///
/// The list takes its share of the window before the plan is asked for any. It
/// is the shorter of the two and it was opened by the character last typed,
/// which is a stronger claim on the rows than a panel that was already there.
/// Neither is counted against the opening: a list opened over it is what the
/// reader is looking at, and shrinking it to keep a card that has already been
/// read is the wrong way round.
pub(super) fn draw<T: Terminal>(
    renderer: &mut Renderer<T>,
    editor: &Editor,
    style: Style,
    around: Around<'_>,
) -> Result<(), Fatal> {
    let columns = renderer.columns();
    let bordering = Bordering {
        left: around.says.left,
        history: around.history,
    };
    let boxed = boxing(renderer, editor, around.says, bordering, style);

    // What is left for a list once the box and the blank row that keeps it off
    // the box have taken theirs.
    let room = renderer.rows().saturating_sub(boxed.rows.len() + 1);

    let mut listed = around.open.rows(columns, room, style.glyphs());

    // The row that keeps the box off whatever was last committed. A list opens
    // with its own, so this is only owed where there is no list -- and the box
    // is owed one either way, because a border drawn against the last line of
    // an answer reads as part of it.
    if listed.is_empty() {
        listed.push(Row::new());
    }

    let mut over = Vec::new();
    over.append(&mut around.planning.rows(
        columns,
        room.saturating_sub(listed.len()),
        style.glyphs(),
    ));
    over.append(&mut listed);

    let pointed = boxed.pointed.as_ref().map(|(at, row)| (*at, row));
    let prompt = replacement(&boxed.rows, boxed.caret, pointed)?;
    renderer.replace(prompt, &over, style.palette())?;
    Ok(())
}

/// Validates the prompt rows before they become one renderer replacement.
pub(super) fn replacement<'a>(
    rows: &'a [Row],
    caret: Caret,
    pointed: Option<(usize, &'a Row)>,
) -> Result<PromptRows<'a>, Fatal> {
    PromptRows::new(rows, caret, pointed).ok_or_else(|| {
        Fatal::from(TerminalError::from(io::Error::new(
            io::ErrorKind::InvalidData,
            "the prompt replacement disagrees with its rows",
        )))
    })
}

/// The box rows in their resting state and the sole pointable row contrasted.
pub(super) struct Boxed {
    pub(super) rows: Vec<Row>,
    pub(super) pointed: Option<(usize, Row)>,
    pub(super) caret: Caret,
}

/// What the frame around the line says, which it reads off neither of them.
///
/// Two numbers from two owners — the window reading is the turn's and the place
/// in the history is the walk's — and the box is where they are drawn. Together
/// because they travel together through every call that lays the box out, and a
/// sixth loose argument is one more thing for a caller to put in the wrong
/// order.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Bordering {
    /// How much of the window is left, where the turn has said.
    pub(super) left: Option<u8>,
    /// Where in the retained prompts the line came from, where an arrow is what
    /// put it there.
    pub(super) history: Recalled,
}

/// Lays out the prompt rows once and composes only its pointable row a second time.
pub(super) fn boxing<T: Terminal>(
    renderer: &Renderer<T>,
    editor: &Editor,
    says: &Says,
    bordering: Bordering,
    style: Style,
) -> Boxed {
    let room = Prompt::room(renderer.rows());
    let columns = renderer.columns();
    let glyphs = style.glyphs();
    let prompt = writing(editor, says, bordering, false, room);
    let (rows, pointed) = prompt.rows_with_pointed(columns, glyphs);
    let caret = prompt.caret(columns);

    Boxed {
        rows,
        pointed,
        caret,
    }
}

/// The box as it is being typed into.
///
/// One place the fields are named, because [`draw`] and the click that follows
/// it have to lay the same component out: a click read against a box drawn any
/// other way lands somewhere nobody pointed at.
pub(super) fn writing<'a>(
    editor: &'a Editor,
    says: &'a Says,
    bordering: Bordering,
    running_pointed: bool,
    room: usize,
) -> Prompt<'a> {
    Prompt {
        draft: Draft::projected(editor.projection()),
        left: Remaining::new(bordering.left),
        history: bordering.history,
        mode: says.mode.as_ref(),
        tone: says.tone,
        hint: &says.keys,
        model: says.model.as_str(),
        provider: says.provider,
        effort: says.effort,
        asking: says.asking.as_deref(),
        commands: CommandCount::new(says.running, running_pointed),
        room,
    }
}

/// Where a click landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Pointed {
    /// The screen row the pointer was on.
    pub(super) row: usize,
    /// How many columns from the left of it.
    pub(super) column: usize,
}

/// What a click landed on.
///
/// Three answers rather than two, because a click that landed on nothing is not
/// the same as one that landed on the line: the first owes no frame, and the
/// second has already moved the cursor to where the pointer was.
pub(super) enum Landed {
    /// This row of the record, which is above the box entirely. What was cut is
    /// held by the loop that owns the transcript, so the answer is there.
    Record(usize),
    /// The line being typed, which now has the cursor where the pointer was.
    Line,
    /// The row under the box naming what is still running, which is the one thing
    /// on it that is an offer rather than a fact.
    Counted,
    /// The border, a blank row, the shell's own output from before crucible
    /// started — or a terminal that would not say where its cursor is.
    Nothing,
}

/// Reads where a click landed, moving the cursor where it landed on the line.
///
/// What is under a window row is the renderer's answer: it shares the window
/// out and it holds the transcript, so a row of one band and a line of the
/// session come out of the same reading. What is left here is where the box
/// sits inside what is standing, which the frame that drew it said. Leaving the
/// cursor where it is is the right answer to a click that did not land on the
/// line.
pub(super) fn landed<T: Terminal>(
    renderer: &Renderer<T>,
    editor: &mut Editor,
    says: &Says,
    at: Pointed,
) -> Landed {
    let row = match renderer.aimed(at.row) {
        Some(Aimed::Line(line)) => return Landed::Record(line),
        Some(Aimed::Boxed(row)) => row,
        // A list or a plan standing over the box. Answered where it is drawn,
        // by the loop that opened it, and not here.
        Some(Aimed::Stood(_)) | None => return Landed::Nothing,
    };

    // Laid out with no place in the history, because the label the border
    // carries is set into a rule that was already there: it takes no row, so
    // the arithmetic a click is read against is the same either way, and the
    // frame that drew it is not here to be asked which it was.
    let bordering = Bordering {
        left: says.left,
        history: Recalled::default(),
    };
    let prompt = writing(
        editor,
        says,
        bordering,
        false,
        Prompt::room(renderer.rows()),
    );

    // Asked of the box, because which row the count came out on is the box's own
    // arithmetic at this width. The column is not asked about: the row is the
    // affordance, and nothing else on it is one, so a click anywhere along it
    // means the one thing it could mean.
    if prompt.counting(renderer.columns(), row) {
        return Landed::Counted;
    }

    // A line and a column within it, which is what the box's arithmetic knows:
    // a newline makes a bare column into the whole text ambiguous, so the editor
    // is placed by the pair rather than by one number.
    let Some((line, into)) = prompt.clicked(renderer.columns(), row, at.column) else {
        return Landed::Nothing;
    };

    if editor.place_at(line, into) == Typed::Changed {
        Landed::Line
    } else {
        Landed::Nothing
    }
}
