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
//! typed. Outside the frame it is a row like any other, which is what let the
//! model join it without re-bordering anything.
//!
//! That row now has two ends, and what decides which end a fact goes to is
//! whether it is about this program or about the next turn. The mode is the
//! first: it says what a tool call arriving now costs, and it stands where the
//! eye starts. Whose model it is, which model, and the rung it is being asked
//! on are the second, and they stand at the far end. It is also the row those
//! facts live on at all — they used to be on the welcome card, which is
//! scrollback by the time `/login`, `/model` or `/effort` changes them, and
//! this process draws inline and can never go back over what it has committed.
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

/// At least this much between what the status row says on the left and what it
/// says on the right, so that the two never read as one sentence.
const APART: usize = 2;

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
    /// Which model the next turn goes to, against the end of the status row
    /// away from the mode. Empty where there is none to say, and then nothing
    /// is drawn there.
    pub model: &'a str,
    /// The vendor that model is asked of, drawn before it. Empty where nothing
    /// has chosen one, and then nothing is drawn in its place.
    pub provider: &'a str,
    /// How hard that model is being asked to think, after it. `None` where no
    /// rung is in force — the vendor's own default is not this program's to
    /// name, and a rung drawn here that was never sent is the one thing a
    /// status row must never be.
    pub effort: Option<&'a str>,
    /// A row under the status, for something waiting on the very next key.
    ///
    /// `None` in the ordinary state, and then the component is the height it
    /// has always been. It takes a row of its own rather than sitting after the
    /// mode because the two are not the same kind of fact: the mode is true
    /// until somebody changes it, and this is true until the next keystroke.
    pub asking: Option<&'a str>,
    /// How many commands are still running behind this box.
    ///
    /// The number rather than the words, because the words are this component's:
    /// [`crate::Working`] spells its own segments for the same reason, and a
    /// caller that spelled this one would be a second place the sentence lives.
    ///
    /// Drawn in the accent, which is the one thing on this row that can be acted
    /// on — every other segment is a fact, and this is a fact with a door behind
    /// it. The three colours a mode is ever drawn in are the quiet one and the two
    /// a permission mode owns, so the accent here is unmistakable in every mode.
    ///
    /// `None`, or zero, is the row as it was before any of this existed.
    pub running: Option<usize>,
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
            rows.push(self.status(columns, glyphs));
            rows
        } else {
            let bar = glyphs.horizontal();
            let (open, opened) = glyphs.top();
            let (close, closed) = glyphs.bottom();
            let across = bar.repeat(columns.saturating_sub(2));

            let mut rows = vec![Row::new().then(self.tone, format!("{open}{across}{opened}"))];
            rows.extend(self.typed(columns, glyphs));
            rows.push(Row::new().then(self.tone, format!("{close}{across}{closed}")));
            rows.push(self.status(columns, glyphs));
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

    /// Whether `row` of this component is the row naming what is still running,
    /// with something there to name.
    ///
    /// Here rather than in the caller for the reason [`Prompt::clicked`] is here:
    /// how tall the box came out at this width is this component's arithmetic, and
    /// a caller that worked out which row the status ended up on would be a second
    /// copy of it — wrong the first time either of them changed.
    ///
    /// `false` with nothing running, because then the row names no door and a
    /// click on it is a click on the mode and the model, which are facts rather
    /// than offers.
    #[must_use]
    pub fn counting(&self, columns: usize, row: usize) -> bool {
        if self.running.is_none_or(|running| running == 0) {
            return false;
        }

        let framed = columns >= FRAMED_AT;
        let first = if framed { FRAMED_ROW } else { 0 };
        let typed = self.window(inner(columns)).rows.len();
        let border = usize::from(framed);

        row == first.saturating_add(typed).saturating_add(border)
    }

    /// The line as it is left in scrollback once it has been typed.
    ///
    /// The caret again, so the record reads the way the box did, and the rows
    /// under it indented to match — a line that wrapped reads as one line
    /// rather than as a stack of separate ones, which is the arrangement
    /// [`Prompt::typed`] already uses while it is being written.
    ///
    /// Wrapped here rather than left to the terminal. The renderer counts the
    /// rows it drew so that it can move back over them, and `present` does not
    /// wrap; a row handed over wider than the window is one the terminal breaks
    /// itself, leaving that count short by however many rows it took.
    ///
    /// At a space rather than at the column, unlike the box. What an input box
    /// owes the person typing into it is that the character just typed stays
    /// where it was put, which breaking at a word would move; a line nobody is
    /// typing into any more owes only that it reads well.
    ///
    /// A window with no room for the line at all still gets the mark. There is
    /// nothing true to draw of the line there, but a record with no mark in it
    /// is one that does not say a prompt was ever asked.
    ///
    /// The row takes a ground, which almost nothing here does. It is allowed to
    /// because the ground is not one this crate chose: it is the reader's own,
    /// blended one step by the palette, so the words on it stay their own
    /// foreground and stay exactly as legible as they were. Where the terminal
    /// never said what its background is, the slot resolves to nothing and this
    /// is the row it always was.
    #[must_use]
    pub fn committed(said: &str, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let mark = glyphs.caret();
        let under = width::columns(mark) + 1;
        let folded = width::fold(said, columns.saturating_sub(under));

        if folded.is_empty() {
            return vec![Row::new().then(Slot::PromptMark, mark)];
        }

        folded
            .into_iter()
            .enumerate()
            .map(|(at, line)| {
                let mut row = match at {
                    0 => Row::new()
                        .then(Slot::PromptMark, mark)
                        .then(Slot::Prompt, " ")
                        .then(Slot::Prompt, line),
                    _ => Row::new()
                        .then(Slot::Prompt, " ".repeat(under))
                        .then(Slot::Prompt, line),
                };

                // Out to the last column, in the ground rather than in the
                // reader's own: a ground that stops where the text stops has a
                // ragged right edge with theirs showing through it, and a
                // wrapped line would show that on every row but the longest.
                row.fill(Slot::Prompt, columns);
                row
            })
            .collect()
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

    /// The row under the box: the mode and the keys that change it, and at the
    /// other end the model the next turn goes to.
    ///
    /// Two ends rather than one sentence, because the two facts are about
    /// different things — what a tool call arriving now costs, and what the
    /// next turn is asked of — and run together they read as one. It is also
    /// what keeps the mode starting in the same column every frame: a model
    /// changing length moves nothing on the left of the row.
    ///
    /// Padded only as far as the model, and no further. This is the one row of
    /// the component not holding an edge up, so anything after the last thing
    /// it says is bytes written every keystroke to draw nothing.
    fn status(&self, columns: usize, glyphs: Glyphs) -> Row {
        let mut row = Row::new().then(self.tone, width::clip(self.mode, columns));

        let said = self.asked(glyphs);
        let wide = width::columns(&said);

        // Whole or not at all, and only with a gap after the mode. Half a model
        // name still says which model, but it says it crowded against the one
        // fact this row must never be read wrong.
        let at = (wide > 0 && row.columns() + APART + wide <= columns).then(|| columns - wide);

        // What is left for what stands between the mode and the model: up to the
        // gap before the model, or to the width where there is no model.
        let room = at.map_or(columns, |at| at.saturating_sub(APART));

        // What is running is measured before the keys and drawn after them, which
        // is the whole of the order things give way in here. The keys are
        // documentation and a second look gets them back; this is the only way to
        // find a process somebody started, so it is the last thing to go before
        // the mode itself. Both are whole or not at all, for the reason the model
        // is: half of a count is a number, and a number that is not the count is
        // worse than nothing.
        let counted = self.running.filter(|running| *running > 0).map(|running| {
            let plural = if running == 1 { "" } else { "s" };

            format!("{running} command{plural}")
        });

        let parting = format!(" {} ", glyphs.dot());
        let needed = counted.as_deref().map_or(0, |counted| {
            width::columns(&parting).saturating_add(width::columns(counted))
        });

        let wanted = width::columns(self.hint);
        if wanted > 0 && wanted < room.saturating_sub(row.columns()).saturating_sub(needed) {
            row.push(Slot::Quiet, format!(" {}", self.hint));
        }

        // The mark parting it from the mode stays quiet with the rest of the row.
        // Only the words naming a door are lit.
        if let Some(counted) = counted.filter(|_| needed <= room.saturating_sub(row.columns())) {
            row.push(Slot::Quiet, parting);
            row.push(Slot::Accent, counted);
        }

        if let Some(at) = at {
            row.pad(at);
            row.push(Slot::Quiet, said);
        }

        row
    }

    /// Whose model it is, which model, and the rung it is being asked on, as one
    /// string.
    ///
    /// Joined here rather than by the caller so that the dot comes out of the
    /// set in force, and so that a session with nothing chosen says nothing at
    /// all rather than naming a vendor over an empty name. The vendor is joined
    /// the way [`crate::Welcome`] joins it and the way `--model` takes it back,
    /// so the fact reads the same wherever it is said.
    fn asked(&self, glyphs: Glyphs) -> String {
        if self.model.is_empty() {
            return String::new();
        }

        let named = if self.provider.is_empty() {
            self.model.to_owned()
        } else {
            format!("{}/{}", self.provider, self.model)
        };

        match self.effort {
            Some(effort) => format!("{named} {} {effort}", glyphs.dot()),
            None => named,
        }
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
