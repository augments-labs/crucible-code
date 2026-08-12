//! The prompt component: the box a line is typed in, and the row under it.
//!
//! Three rows in every state, and a fourth under them that is not part of the
//! box. The border is coloured by the mode in force and the row underneath says
//! what that colour means in words, which is the arrangement that keeps the
//! colour from being the only thing that says it — a terminal with no colour at
//! all still reads the mode off the screen.
//!
//! The status sits below the frame rather than on its bottom edge. A frame is a
//! container, and everything drawn on one reads as belonging to what is inside
//! it; the mode is a fact about the session rather than about the line being
//! typed. Outside the frame it is a row like any other, so a later release
//! putting a second fact beside it moves nothing and re-borders nothing.
//!
//! Like [`crate::Welcome`] this returns [`Row`]s and draws nothing, so every
//! width is asserted with no terminal attached. Unlike it, the rows are live:
//! they are redrawn where they stand as the line changes, which is why the
//! component also says where the cursor goes.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::render::Caret;
use crate::row::Row;
use crate::width;

/// The narrowest terminal that gets a frame.
///
/// Below it the border costs a quarter of the screen to say what the caret
/// already says, so it goes and the caret and the status row are left — which
/// is the same shape a run with no terminal to draw a box on gets.
const FRAMED_AT: usize = 24;

/// What stands before the line on a framed row: an edge, a space, the caret,
/// and the space after it.
const FRAMED: usize = 4;

/// What stands after it: a space and the edge on the other side.
const CLOSING: usize = 2;

/// What stands before the line where there is no frame.
const BARE: usize = 2;

/// Which row of a framed prompt the line is typed on.
const FRAMED_ROW: usize = 1;

/// What the prompt says, and where the cursor is in it.
///
/// Every field is already spelled the way it will be drawn. The mode is a
/// sentence rather than an enum and the tone is a slot rather than a hue,
/// because this crate names no domain type and settles no colour.
#[derive(Debug, Clone, Copy)]
pub struct Prompt<'a> {
    /// The line being typed.
    pub said: &'a str,
    /// How many display columns into it the cursor sits.
    pub column: usize,
    /// What the status row says the mode in force is.
    pub mode: &'a str,
    /// The colour that mode is drawn in, on the border and on its own sentence.
    pub tone: Slot,
    /// What is said quietly after it — the keys that change the mode. Nothing
    /// is drawn in its place when there is none.
    pub hint: &'a str,
}

impl Prompt<'_> {
    /// The component, drawn for a terminal `columns` wide.
    ///
    /// Exactly four rows where there is a frame and two where there is not.
    /// Fixed either way: a box that grew a row as a line got longer would push
    /// everything above it up the screen on a keystroke.
    #[must_use]
    pub fn rows(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        if columns < FRAMED_AT {
            return vec![self.bare(columns, glyphs), self.status(columns)];
        }

        let bar = glyphs.horizontal();
        let (open, opened) = glyphs.top();
        let (close, closed) = glyphs.bottom();
        let across = bar.repeat(columns.saturating_sub(2));

        vec![
            Row::new().then(self.tone, format!("{open}{across}{opened}")),
            self.typed(columns, glyphs),
            Row::new().then(self.tone, format!("{close}{across}{closed}")),
            self.status(columns),
        ]
    }

    /// Where the cursor goes, counted from the top of what [`Prompt::rows`]
    /// returned.
    ///
    /// The cursor is the terminal's own rather than a glyph this draws. A glyph
    /// would have to be inserted where the cursor is, which would shift every
    /// character after it one column right — so the line would move as the
    /// cursor moved through it.
    #[must_use]
    pub fn caret(&self, columns: usize) -> Caret {
        let (row, before) = if columns < FRAMED_AT {
            (0, BARE)
        } else {
            (FRAMED_ROW, FRAMED)
        };

        Caret {
            row,
            column: before + self.window(inner(columns)).1,
        }
    }

    /// The line as it is left in scrollback once it has been typed.
    ///
    /// The caret again, so the record reads the way the box did. Not clipped:
    /// nothing is ever drawn over a settled row, so a line longer than the
    /// terminal is wrapped by the terminal and costs no count this process is
    /// keeping.
    #[must_use]
    pub fn committed(said: &str, glyphs: Glyphs) -> Row {
        Row::new()
            .then(Slot::Accent, glyphs.caret())
            .then(Slot::Plain, " ")
            .then(Slot::Plain, said)
    }

    /// The row the line is typed on, inside the frame.
    fn typed(&self, columns: usize, glyphs: Glyphs) -> Row {
        let inner = inner(columns);
        let edge = glyphs.vertical();
        let (shown, _) = self.window(inner);

        let mut line = Row::plain(shown);
        line.pad(inner);

        Row::new()
            .then(self.tone, edge)
            .then(Slot::Plain, " ")
            .then(Slot::Accent, glyphs.caret())
            .then(Slot::Plain, " ")
            .join(line)
            .then(Slot::Plain, " ")
            .then(self.tone, edge)
    }

    /// The same row with no frame around it.
    fn bare(&self, columns: usize, glyphs: Glyphs) -> Row {
        // The mark and the space after it are the last chrome there is, and a
        // terminal too narrow for even that gets nothing: a row wider than the
        // screen is one the terminal wraps itself, which leaves the cursor a
        // row below where the next frame expects it.
        if columns < BARE {
            return Row::new();
        }

        Row::new()
            .then(Slot::Accent, glyphs.caret())
            .then(Slot::Plain, " ")
            .then(Slot::Plain, self.window(inner(columns)).0)
    }

    /// The row under the box: the mode, and the keys that change it.
    ///
    /// Not padded out to the width. It is the one row of this component that is
    /// not holding an edge up, and trailing spaces on it would be bytes written
    /// every keystroke to draw nothing.
    fn status(&self, columns: usize) -> Row {
        let mut row = Row::new().then(self.tone, clip(self.mode, columns));

        // A hint that does not fit whole is not drawn at all. Half of the keys
        // to press is not half as useful as all of them — it is a fragment
        // beside the one fact this row exists to carry.
        let wanted = width::columns(self.hint);
        let left = columns.saturating_sub(row.columns());
        if wanted > 0 && wanted < left {
            row.push(Slot::Quiet, format!(" {}", self.hint));
        }

        row
    }

    /// The part of the line there is room for, and where the cursor sits in it.
    ///
    /// A line longer than the box is windowed rather than wrapped, because the
    /// box is a fixed number of rows and a wrap would need another one. The
    /// window is worked out from the cursor every time rather than remembered:
    /// a kept scroll position is a second piece of state that the line can get
    /// out of step with, and there is nothing it would buy.
    ///
    /// One column is left for the cursor itself. Without it a line that filled
    /// the box exactly would put the cursor on the border.
    fn window(&self, inner: usize) -> (&str, usize) {
        let start = self.column.saturating_sub(inner.saturating_sub(1));
        let mut gone = clip(self.said, start);

        // A wide character lying across the start of the window is skipped
        // whole rather than kept. Half of one cannot be drawn, and the half
        // that could would leave the cursor a column further along than the
        // box has room for -- which is the row, and only that row, closing a
        // column early.
        if width::columns(gone) < start {
            gone = clip(self.said, start + 1);
        }

        let rest = self.said.get(gone.len()..).unwrap_or_default();

        (
            clip(rest, inner),
            self.column.saturating_sub(width::columns(gone)),
        )
    }
}

/// How many columns of the line a terminal `columns` wide has room for.
///
/// The one number every row of this component is laid out against, and the
/// reason the frame closes where the caret says it does: what the box holds is
/// the width minus what the chrome around it takes.
fn inner(columns: usize) -> usize {
    let chrome = if columns < FRAMED_AT {
        BARE
    } else {
        FRAMED + CLOSING
    };

    columns.saturating_sub(chrome)
}

/// `text` with at most `columns` display columns of it kept.
///
/// Columns rather than characters, and cut where this crate's own walk says to:
/// a cut that split a wide character would leave the row a column short of
/// where the border was drawn.
fn clip(text: &str, columns: usize) -> &str {
    match width::cut(text, columns) {
        Some(at) => text.get(..at).unwrap_or_default(),
        None => text,
    }
}

#[cfg(test)]
mod tests;
