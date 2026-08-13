//! The prompt component: the box a line is typed in, and the row under it.
//!
//! A border, as many rows as the line needs, a border, and a row under them
//! that is not part of the box. The border is coloured by the mode in force and
//! the row underneath says what that colour means in words, which is the
//! arrangement that keeps the colour from being the only thing that says it — a
//! terminal with no colour at all still reads the mode off the screen.
//!
//! The line wraps rather than scrolling sideways, because a prompt is written
//! and read at the same time and a paragraph scrolled out of sight is one
//! nobody can check before sending. The box therefore grows, and stops growing
//! at about half the window: past that the line scrolls under the top edge.
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

/// Which row of a framed prompt the line starts on.
const FRAMED_ROW: usize = 1;

/// What a framed box costs beyond the line itself: two borders and the status
/// row under them.
///
/// What [`Prompt::room`] takes off the height before halving it, so that a box
/// filling its allowance is half the window rather than half the window plus
/// three.
const CHROME: usize = 3;

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
    /// A row under the status, for something waiting on the very next key.
    ///
    /// `None` in the ordinary state, and then the component is the height it
    /// has always been. It takes a row of its own rather than sitting after the
    /// mode because the two are not the same kind of fact: the mode is true
    /// until somebody changes it, and this is true until the next keystroke.
    pub asking: Option<&'a str>,
    /// How many rows of the line the box may show at once.
    ///
    /// The box grows to what the line needs and stops here. [`Prompt::room`] is
    /// what a caller holding the window height works it out with; a caller
    /// drawing a box nobody is typing into passes 1.
    pub room: usize,
}

impl Prompt<'_> {
    /// How many rows of the line a box may show in a window this tall.
    ///
    /// About half of it. A prompt is written and read at the same time, so a
    /// paragraph being typed has to be visible as a paragraph; what it must not
    /// do is take the screen away from what it is a reply to. Past the
    /// allowance the line scrolls inside the box, which is the same bargain the
    /// window along a single row used to make in one dimension.
    #[must_use]
    pub fn room(rows: usize) -> usize {
        (rows / 2).saturating_sub(CHROME).max(1)
    }

    /// The component, drawn for a terminal `columns` wide.
    ///
    /// The box is as tall as the line needs and never taller than [`room`]. It
    /// grows on the keystroke that fills a row, which does push what is above it
    /// up the screen — the alternative is a line that scrolls sideways out of
    /// sight, and a prompt too long to see is worse than a transcript that moved.
    /// The ceiling is what keeps the growth bounded: the region is taken back by
    /// moving the cursor over it, so one taller than the screen could not be
    /// taken back at all.
    ///
    /// [`room`]: Prompt::room
    #[must_use]
    pub fn rows(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let mut rows = if columns < FRAMED_AT {
            let mut rows = self.bare(columns, glyphs);
            rows.push(self.status(columns));
            rows
        } else {
            let bar = glyphs.horizontal();
            let (open, opened) = glyphs.top();
            let (close, closed) = glyphs.bottom();
            let across = bar.repeat(columns.saturating_sub(2));

            let mut rows = vec![Row::new().then(self.tone, format!("{open}{across}{opened}"))];
            rows.extend(self.typed(columns, glyphs));
            rows.push(Row::new().then(self.tone, format!("{close}{across}{closed}")));
            rows.push(self.status(columns));
            rows
        };

        // Clipped rather than dropped when it does not fit. Unlike the keys
        // after the mode, half of this still says which key is waiting, and the
        // row is only on screen because somebody has just pressed it.
        if let Some(asking) = self.asking {
            rows.push(Row::new().then(Slot::Quiet, width::clip(asking, columns)));
        }

        rows
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
        let (first, before) = if columns < FRAMED_AT {
            (0, BARE)
        } else {
            (FRAMED_ROW, FRAMED)
        };
        let shown = self.window(inner(columns));

        Caret {
            row: first + shown.row,
            column: before + shown.column,
        }
    }

    /// Where in the line a click on the box landed, in display columns from its
    /// start.
    ///
    /// `row` and `column` are counted from the top left of what
    /// [`Prompt::rows`] returned, which is what a caller that knows where it
    /// drew the component can work out from where the mouse was. `None` for a
    /// click outside the rows the line is on — the border, the status, the list
    /// above — because the answer to those is to leave the cursor alone rather
    /// than to move it to the nearest place that is inside.
    ///
    /// A click past the end of a row lands at the end of that row, which is
    /// where the eye reads the line as ending. Every other terminal does the
    /// same thing and it is the one behaviour nobody has to be taught.
    #[must_use]
    pub fn clicked(&self, columns: usize, row: usize, column: usize) -> Option<usize> {
        let (first, before) = if columns < FRAMED_AT {
            (0, BARE)
        } else {
            (FRAMED_ROW, FRAMED)
        };

        let shown = self.window(inner(columns));
        let at = row.checked_sub(first)?;
        let line = shown.rows.get(at)?;

        // The columns the rows above the clicked one already account for, so
        // that what comes back is an offset into the whole line rather than
        // into the row it happened to land on.
        let above: usize = shown
            .rows
            .get(..at)
            .unwrap_or_default()
            .iter()
            .map(|row| width::columns(row))
            .sum();

        let into = column.saturating_sub(before).min(width::columns(line));

        Some(shown.gone + above + into)
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

    /// The rows the line is typed on, inside the frame.
    ///
    /// The mark goes on the first of them and the ones under it are indented to
    /// match, so a line that wrapped reads as one line rather than as a stack of
    /// separate ones.
    fn typed(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let inner = inner(columns);
        let edge = glyphs.vertical();

        self.window(inner)
            .rows
            .into_iter()
            .enumerate()
            .map(|(at, shown)| {
                // Clipped as well as broken, for the one row breaking cannot
                // make fit: a character wider than the whole box. Half of one
                // cannot be drawn, so none of it is, and the row still ends
                // where the border expects it.
                let mut line = Row::plain(width::clip(shown, inner));
                line.pad(inner);

                let mark = if at == 0 { glyphs.caret() } else { " " };

                Row::new()
                    .then(self.tone, edge)
                    .then(Slot::Plain, " ")
                    .then(Slot::Accent, mark)
                    .then(Slot::Plain, " ")
                    .join(line)
                    .then(Slot::Plain, " ")
                    .then(self.tone, edge)
            })
            .collect()
    }

    /// The same rows with no frame around them.
    fn bare(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        // The mark and the space after it are the last chrome there is, and a
        // terminal too narrow for even that gets nothing: a row wider than the
        // screen is one the terminal wraps itself, which leaves the cursor a
        // row below where the next frame expects it.
        if columns < BARE {
            return vec![Row::new()];
        }

        self.window(inner(columns))
            .rows
            .into_iter()
            .enumerate()
            .map(|(at, shown)| {
                let mark = if at == 0 { glyphs.caret() } else { " " };

                Row::new()
                    .then(Slot::Accent, mark)
                    .then(Slot::Plain, " ")
                    .then(Slot::Plain, width::clip(shown, inner(columns)))
            })
            .collect()
    }

    /// The row under the box: the mode, and the keys that change it.
    ///
    /// Not padded out to the width. It is the one row of this component that is
    /// not holding an edge up, and trailing spaces on it would be bytes written
    /// every keystroke to draw nothing.
    fn status(&self, columns: usize) -> Row {
        let mut row = Row::new().then(self.tone, width::clip(self.mode, columns));

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

    /// The rows of the line the box has room for, and where the cursor sits
    /// among them.
    ///
    /// The whole line is broken into rows the width of the box and a window of
    /// [`room`] of them is kept — the one the cursor is on, and the ones above
    /// it. Worked out from the cursor every time rather than remembered: a kept
    /// scroll position is a second piece of state the line can get out of step
    /// with, and there is nothing it would buy.
    ///
    /// A line that exactly fills its last row is followed by an empty one, so
    /// the cursor after the last character has somewhere to stand that is not
    /// the border.
    ///
    /// [`room`]: Prompt::room
    fn window(&self, inner: usize) -> Window<'_> {
        let broken = broken(self.said, inner);
        let (row, column) = place(&broken, self.column);

        // The cursor's row is the last one shown, so a line being written grows
        // the box downwards and a line longer than the allowance scrolls under
        // it. Moving back up the line brings the rows above into view for the
        // same reason.
        let room = self.room.max(1);
        let first = row.saturating_sub(room - 1);
        let gone: usize = broken
            .get(..first)
            .unwrap_or_default()
            .iter()
            .map(|row| width::columns(row))
            .sum();

        Window {
            rows: broken.get(first..).unwrap_or_default().to_vec(),
            row: row - first,
            column,
            gone,
        }
    }
}

/// What the box is showing of the line, and where the cursor is in it.
struct Window<'a> {
    /// The rows on screen, top first.
    rows: Vec<&'a str>,
    /// Which of them the cursor is on.
    row: usize,
    /// How many columns into that row it sits.
    column: usize,
    /// How many columns of the line scrolled off above the first row shown.
    gone: usize,
}

/// The line broken into rows no wider than the box.
///
/// Broken at the column rather than at a space, which is what an input box owes
/// the person typing into it: a word moving to the next row as it is written
/// would move the cursor with it, and the character just typed would not be
/// where it was put.
fn broken(said: &str, inner: usize) -> Vec<&str> {
    if inner == 0 {
        return vec![""];
    }

    let mut rows = Vec::new();
    let mut rest = said;

    // `cut` answers with nothing where the rest fits, which is what ends this.
    while let Some(at) = width::cut(rest, inner) {
        // A character wider than the whole row takes no bytes off the front and
        // this would not end. It is drawn a column over instead, which is the
        // one row a box this narrow cannot hold either way.
        let at = if at == 0 { step(rest) } else { at };

        rows.push(rest.get(..at).unwrap_or_default());
        rest = rest.get(at..).unwrap_or_default();
    }

    rows.push(rest);

    // A line that exactly fills its last row is followed by an empty one, so
    // the cursor after the last character stands at the start of a row rather
    // than on the padding beside the border — which is where the next character
    // is going to appear anyway.
    if width::columns(rest) == inner {
        rows.push("");
    }

    rows
}

/// The offset one character in.
fn step(text: &str) -> usize {
    text.char_indices()
        .nth(1)
        .map_or(text.len(), |(offset, _)| offset)
}

/// Which row `column` display columns into the line falls on, and where in it.
///
/// A cursor at the very end of a full row belongs at the start of the next one,
/// which is where the character about to be typed will appear.
fn place(rows: &[&str], column: usize) -> (usize, usize) {
    let mut before = 0;

    for (at, row) in rows.iter().enumerate() {
        let wide = width::columns(row);

        if column < before + wide || at + 1 == rows.len() {
            return (at, column - before.min(column));
        }

        before += wide;
    }

    (0, column)
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

#[cfg(test)]
mod tests;
