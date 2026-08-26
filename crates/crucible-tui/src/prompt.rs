//! The prompt component: the box a line is typed in, and the row under it.
//!
//! A border, as many rows as the line needs, a border, and a row under them
//! that is not part of the box. The border is coloured by the mode in force and
//! the row underneath says what that colour means in words, which is the
//! arrangement that keeps the colour from being the only thing that says it — a
//! terminal with no colour at all still reads the mode off the screen. The
//! remaining model window stands the same way above the box: a quiet row of its
//! own against the top right, on screen for exactly as long as the box is.
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
//! model join it without re-bordering anything. On a terminal too narrow for a
//! frame, the remaining-window fact joins this row and shortens before it can
//! overflow.
//!
//! That row has two ends, and what decides which end a fact goes to is whether
//! it is about the next key or about the next turn. The mode is the first: it
//! says what a tool call arriving now costs, and it stands where the eye
//! starts. Whose model it is, which model, and the rung it is being asked on
//! are the second, and on a framed prompt they stand at the far end — beside
//! the box every key that changes them is typed into. On a bare prompt the
//! remaining-window fact owns that far end and these session facts fit before
//! it. The directory is said once, on the welcome card, and nowhere else.
//!
//! Like [`crate::Welcome`] this returns [`Row`]s and draws nothing, so every
//! width is asserted with no terminal attached. Unlike it, the rows are live:
//! they are redrawn where they stand as the line changes, which is why the
//! component also says where the cursor goes.

use crate::color::Slot;
use crate::editor::Projection;
use crate::glyphs::Glyphs;
use crate::render::Caret;
use crate::row::Row;
use crate::width;

use std::num::NonZeroUsize;

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

/// Which row of a framed prompt the line starts on: the reading above the
/// box, then the top border, then the line.
const FRAMED_ROW: usize = 2;

/// The framed rows that are not the line: the reading above the box and the
/// top border.
const BEFORE_LINE: usize = FRAMED_ROW;

/// At least this much between what the status row says on the left and what it
/// says on the right, so that the two never read as one sentence.
const APART: usize = 2;

/// How much rule stands between the top left corner and a label inset into it.
///
/// Enough to read as a corner the label is set into rather than as a corner the
/// label is written on. The space either side of the words is counted
/// separately, so the shortest top edge that can carry one is this, the two
/// spaces, the words, and the [`AFTER`] the rule keeps on the far side.
const BEFORE: usize = 2;

/// And how much of it has to survive on the other side of that label.
///
/// One column, which is the whole of the difference between a rule with a word
/// set into it and a rule that stops at a word. A box whose top edge cannot
/// spare it is drawn with no label at all.
const AFTER: usize = 1;

/// What a framed box costs beyond the line itself: two borders and the status
/// row under them.
///
/// What [`Prompt::room`] takes off the height before halving it, so that a box
/// filling its allowance is half the window rather than half the window plus
/// three.
const CHROME: usize = 3;

/// Text and cursor state that cannot disagree about which representation is drawn.
#[derive(Debug, Clone, Copy)]
pub struct Draft<'a> {
    shaped: Shaped<'a>,
}

#[derive(Debug, Clone, Copy)]
enum Shaped<'a> {
    Plain {
        said: &'a str,
        line: usize,
        column: usize,
    },
    Projected(Projection<'a>),
}

impl<'a> Draft<'a> {
    /// Plain text with its cursor at the source byte boundary `at`.
    ///
    /// An offset past the end is capped, and one inside a UTF-8 character is
    /// moved to that character's start. The displayed line and column are then
    /// derived from the same text rather than accepted as separate facts.
    #[must_use]
    pub fn at(said: &'a str, at: usize) -> Self {
        let mut at = at.min(said.len());
        while !said.is_char_boundary(at) {
            at = at.saturating_sub(1);
        }
        let before = said.get(..at).unwrap_or_default();
        let line = before.matches('\n').count();
        let column = width::columns(before.split('\n').next_back().unwrap_or_default());

        Self {
            shaped: Shaped::Plain { said, line, column },
        }
    }

    /// An editor projection, whose displayed text and source cursor share one owner.
    #[must_use]
    pub fn projected(projection: Projection<'a>) -> Self {
        Self {
            shaped: Shaped::Projected(projection),
        }
    }

    fn text(self) -> &'a str {
        match self.shaped {
            Shaped::Plain { said, .. } => said,
            Shaped::Projected(projection) => projection.text(),
        }
    }

    fn position(self) -> (usize, usize) {
        match self.shaped {
            Shaped::Plain { line, column, .. } => (line, column),
            Shaped::Projected(projection) => (projection.line(), projection.column()),
        }
    }

    fn source_position(self, position: (usize, usize)) -> (usize, usize) {
        match self.shaped {
            Shaped::Plain { .. } => position,
            Shaped::Projected(projection) => projection.source_position(position.0, position.1),
        }
    }
}

/// A usable-window reading, unknown or bounded to a percentage.
#[derive(Debug, Clone, Copy, Default)]
pub struct Remaining(Option<u8>);

impl Remaining {
    /// A reading supplied by the runner, capping defensive external input.
    #[must_use]
    pub fn new(left: Option<u8>) -> Self {
        Self(left.map(|left| left.min(100)))
    }

    fn get(self) -> Option<u8> {
        self.0
    }
}

/// Where in the retained prompts the line standing in the box came from.
///
/// A place and a count together, because neither is worth drawing without the
/// other: `87` says nothing about how far back it is, and a walk that could
/// report one without the other is one that has lost track of what it is
/// walking.
///
/// Nothing by default, which is the state the box is in for the whole of a
/// session nobody reaches back through.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Recalled(Option<(usize, usize)>);

impl Recalled {
    /// The `at`th prompt back, of `of` a walk may ever reach.
    ///
    /// Counted from the newest, so the first press back is `1` and the number
    /// rises as the walk goes on. The pair reads as a position in a journey the
    /// reader is making rather than as an address in a file they cannot see: on
    /// the first press it says how far they have come and how far they may go,
    /// and both halves keep meaning that for the whole of the walk.
    ///
    /// `of` is the window and not how much of it is filled, so it is the same
    /// number on the first day as on the hundredth.
    ///
    /// A place outside the prompts it counts is nothing, the way an unknown
    /// window reading is: the two numbers come off one walk, so a pair that
    /// disagrees is a caller that has miscounted, and a border drawn from it
    /// would say the line came from a prompt that is not there.
    #[must_use]
    pub fn new(at: usize, of: usize) -> Self {
        Self((at > 0 && at <= of).then_some((at, of)))
    }

    /// What the top border says, where there is anything to say.
    fn said(self) -> Option<String> {
        let (at, of) = self.0?;
        Some(format!("history {at}/{of}"))
    }
}

/// A nonzero command count and whether its one visible control is pointed.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommandCount {
    running: Option<NonZeroUsize>,
    pointed: bool,
}

impl CommandCount {
    /// A count from the process registry and its current pointer state.
    #[must_use]
    pub fn new(running: usize, pointed: bool) -> Self {
        let running = NonZeroUsize::new(running);
        Self {
            running,
            pointed: pointed && running.is_some(),
        }
    }

    fn resting(self) -> Self {
        Self {
            pointed: false,
            ..self
        }
    }

    fn pointed(self) -> Self {
        Self {
            pointed: self.running.is_some(),
            ..self
        }
    }
}

/// What the prompt says, and where the cursor is in it.
///
/// Every field is already spelled the way it will be drawn. The mode is a
/// sentence rather than an enum and the tone is a slot rather than a hue,
/// because this crate names no domain type and settles no colour.
#[derive(Debug, Clone, Copy)]
pub struct Prompt<'a> {
    /// The line being typed and its cursor, from one consistent representation.
    pub draft: Draft<'a>,
    /// How much of the model window remains, or that it is not known.
    pub left: Remaining,
    /// Where in the retained prompts the line in the box came from, while the
    /// arrows are what put it there. Nothing while the line is somebody's own,
    /// and then the top border is the rule it has always been.
    pub history: Recalled,
    /// What the status row says the mode in force is.
    pub mode: &'a str,
    /// The colour that mode's own sentence is drawn in. Not the border's: see
    /// [`Prompt::BORDER`].
    pub tone: Slot,
    /// What is said quietly after it — the keys that change the mode. Nothing
    /// is drawn in its place when there is none.
    pub hint: &'a str,
    /// Which model the next turn goes to. On a framed prompt it stands against
    /// the end of the status row away from the mode; on a bare prompt it fits
    /// before the remaining-window reading. Empty where there is none to say,
    /// and then nothing is drawn there.
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
    /// At rest its words use the accent; while pointed the same words and
    /// geometry use the pointed slot. Zero is the row as it was before the
    /// control existed and cannot carry a pointed state.
    pub commands: CommandCount,
    /// How many rows of the line the box may show at once.
    ///
    /// The box grows to what the line needs and stops here. [`Prompt::room`] is
    /// what a caller holding the window height works it out with; a caller
    /// drawing a box nobody is typing into passes 1.
    pub room: usize,
}

impl Prompt<'_> {
    /// The narrowest window this box is drawn framed in.
    ///
    /// Published because a frame drawn above the box is read against it: one
    /// that kept a border where this one had given it up would be a border
    /// around nothing, standing over a caret with no box under it.
    pub const FRAMED_AT: usize = FRAMED_AT;

    /// The colour the box is framed in, whatever else is true of the session.
    ///
    /// One colour rather than the permission mode's, though the mode's sentence
    /// under the box still carries it. The border is the largest thing on the
    /// screen and it is up in every frame, so colouring it by mode paints a
    /// quarter of the window in a hue that means *be careful* and leaves it
    /// there — which is a warning that has stopped being one by the second
    /// prompt. The sentence saying which mode is in force is one row, is read
    /// when it changes, and is where the ramp belongs.
    ///
    /// Published so that a frame drawn above the box is drawn in the same
    /// colour: two borders in two colours read as two kinds of thing.
    pub const BORDER: Slot = Slot::Quiet;

    /// What a framed row spends on its border and the space inside it, both
    /// sides together — so the line itself gets `columns - CHROME`.
    ///
    /// Published for the same reason and against the same defect: a frame that
    /// worked its own chrome out is one that ends a column short of this one
    /// the first time either changes, and a border that does not line up with
    /// the border beneath it is the first thing an eye finds.
    pub const CHROME: usize = FRAMED + CLOSING;

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
        self.laid_out(columns, glyphs).0
    }

    /// Resting rows and the one actionable status row in its pointed state.
    ///
    /// The component is laid out once. When the command count survives the
    /// width, only that status row is composed a second time with its pointed
    /// palette slot; the prompt text, borders, and wrapping are not rebuilt.
    #[must_use]
    pub fn rows_with_pointed(
        &self,
        columns: usize,
        glyphs: Glyphs,
    ) -> (Vec<Row>, Option<(usize, Row)>) {
        let mut resting = *self;
        resting.commands = resting.commands.resting();
        let (rows, status) = resting.laid_out(columns, glyphs);
        let pointed = status.map(|at| {
            let mut pointed = resting;
            pointed.commands = pointed.commands.pointed();
            (at, pointed.status(columns, glyphs))
        });

        (rows, pointed)
    }

    /// Resting rows and the relative row of the command control, where drawn.
    fn laid_out(&self, columns: usize, glyphs: Glyphs) -> (Vec<Row>, Option<usize>) {
        let mut rows = if columns < FRAMED_AT {
            self.bare(columns, glyphs)
        } else {
            let bar = glyphs.horizontal();
            let (close, closed) = glyphs.bottom();
            let across = bar.repeat(columns.saturating_sub(2));

            // The reading stands on a row of its own, quiet and against the
            // right edge: not part of the box, and on screen for exactly as
            // long as the box is. The spaces reaching it to the edge are
            // written on its left rather than its right — like the status
            // row, it holds no edge up, so trailing spaces would be bytes
            // written every keystroke to draw nothing.
            let reading = Row::new().then(Slot::Quiet, format!("{:>columns$}", self.reading()));

            let mut rows = vec![
                reading,
                Row::new().then(Self::BORDER, self.opening(columns, glyphs)),
            ];
            rows.extend(self.typed(columns, glyphs));
            rows.push(Row::new().then(Self::BORDER, format!("{close}{across}{closed}")));
            rows
        };

        let status = rows.len();
        let (row, counted) = self.status_counted(columns, glyphs);
        rows.push(row);

        // Clipped rather than dropped when it does not fit. Unlike the keys
        // after the mode, half of this still says which key is waiting, and the
        // row is only on screen because somebody has just pressed it.
        if let Some(asking) = self.asking {
            rows.push(Row::new().then(Slot::Quiet, width::clip(asking, columns)));
        }

        (rows, counted.then_some(status))
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
            (BEFORE_LINE, FRAMED)
        };
        let shown = self.shown(inner(columns));

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
    ///
    /// What comes back is the line and the column within it, rather than a
    /// column into the whole text: a newline makes the second of those
    /// ambiguous, and the editor places its cursor by the first.
    #[must_use]
    pub fn clicked(&self, columns: usize, row: usize, column: usize) -> Option<(usize, usize)> {
        let (first, before) = if columns < FRAMED_AT {
            (0, BARE)
        } else {
            (BEFORE_LINE, FRAMED)
        };

        let shown = self.shown(inner(columns));
        let at = row.checked_sub(first)?;
        let hit = shown.rows.get(at)?;

        // The columns the wrapped rows of the same line above the clicked one
        // account for, so that the column comes back into the line rather than
        // into the row it happened to land on. Walked backwards, because only
        // the rows directly above this one can be its own line: a newline below
        // them starts another.
        let above: usize = shown
            .rows
            .get(..at)
            .unwrap_or_default()
            .iter()
            .rev()
            .take_while(|row| row.line == hit.line)
            .map(|row| width::columns(row.text))
            .sum();

        let into = column.saturating_sub(before).min(width::columns(hit.text));

        let position = (hit.line, above + into);
        Some(self.draft.source_position(position))
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
        if self.commands.running.is_none() {
            return false;
        }

        let framed = columns >= FRAMED_AT;
        let first = if framed { BEFORE_LINE } else { 0 };
        let typed = self.shown(inner(columns)).rows.len();
        let border = usize::from(framed);

        if row != first.saturating_add(typed).saturating_add(border) {
            return false;
        }

        self.status_counted(columns, Glyphs::Ascii).1
    }

    /// The line as it is left in the transcript once it has been typed.
    ///
    /// The caret again, so the record reads the way the box did, and the rows
    /// under it indented to match — a line that wrapped reads as one line
    /// rather than as a stack of separate ones, which is the arrangement
    /// `Prompt::typed` already uses while it is being written.
    ///
    /// Wrapped here rather than left to the terminal. The renderer counts the
    /// rows it drew so that it can move back over them, and `present` does not
    /// wrap; a row handed over wider than the window is one the terminal breaks
    /// itself, leaving that count short by however many rows it took.
    ///
    /// At a space rather than inside a word, the same as the live box. Source
    /// whitespace is retained in the rows so the transcript still says exactly
    /// what was asked.
    ///
    /// `banded` is whether the palette this will be painted with writes a
    /// ground for the row. Where it does not, the row stops where the words do:
    /// padding it out would be trailing spaces that draw nothing and follow the
    /// text into every copy taken out of the transcript.
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
    pub fn committed(said: &str, columns: usize, glyphs: Glyphs, banded: bool) -> Vec<Row> {
        let mark = glyphs.caret();
        let under = width::columns(mark) + 1;
        let room = columns.saturating_sub(under);
        // The same source-preserving word wrap as the live box. A record says
        // what was asked, including indentation and repeated whitespace.
        let mut folded = if room == 0 {
            Vec::new()
        } else {
            broken(said, room)
        };
        if folded.len() > 1 && folded.last().is_some_and(|line| line.text.is_empty()) {
            folded.pop();
        }

        if folded.is_empty() {
            // The one row a window this narrow can hold, and it carries the
            // ground like every other where there is one to carry.
            let mut row = Row::new().then(Slot::PromptMark, mark);
            if banded {
                row.fill(Slot::Prompt, columns);
            }
            return vec![row];
        }

        folded
            .into_iter()
            .enumerate()
            .map(|(at, line)| {
                // Clipped as well as broken, for the one row breaking cannot
                // make fit: a glyph wider than the whole row takes no bytes off
                // the front, so it comes back whole and would put the row past
                // the last column. The terminal would wrap it and the count of
                // what was drawn would be short by one.
                let line = width::clip(line.text, room);

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
                //
                // Only where there is a ground to carry. A palette that writes
                // nothing for the slot would pad the row with spaces that draw
                // nothing at all — trailing whitespace in the record, and on
                // every copy taken out of it.
                if banded {
                    row.fill(Slot::Prompt, columns);
                }
                row
            })
            .collect()
    }

    /// The rows the files sent with a line are named on, under it.
    ///
    /// One row each, indented past the mark so the block reads as one thing
    /// somebody sent rather than as a stack of separate lines. Each is handed
    /// over already decided: what the file is called, and what it is. Choosing
    /// those two words is the caller's, because they come off a domain this
    /// crate is not told about.
    ///
    /// No picture is drawn. What is on screen is this process's to account for,
    /// and putting an image on it is a separate question nobody has answered.
    #[must_use]
    pub fn attached(
        files: &[(&str, &str)],
        columns: usize,
        glyphs: Glyphs,
        banded: bool,
    ) -> Vec<Row> {
        let indent = width::columns(glyphs.caret()) + 1;
        let dot = glyphs.dot();
        // The dot and the space after it. Everything past them is the file.
        let opening = width::columns(dot) + 1;
        let room = columns.saturating_sub(indent + opening);

        // A window with no room for any of the name draws none of these. The
        // row would be an indent and a mark saying a file is there without
        // saying which, and the line above has already degraded to its own mark
        // by this width — a reader at four columns is not reading either.
        if room == 0 {
            return Vec::new();
        }

        files
            .iter()
            .map(|(name, what)| {
                // Clipped rather than broken: a path is one thing to read, and
                // a second row of it under the first reads as a second file.
                let said = format!("{name} {} {what}", glyphs.dash());
                let mut row = Row::new()
                    .then(Slot::Prompt, " ".repeat(indent))
                    .then(Slot::PromptMark, dot)
                    .then(Slot::Prompt, " ")
                    .then(Slot::Prompt, width::clip(&said, room).to_owned());

                // The same ground the line above took, for the same reason it
                // reaches the last column there: the file went with the words,
                // and a row that dropped the band would cut the block in two.
                if banded {
                    row.fill(Slot::Prompt, columns);
                }
                row
            })
            .collect()
    }

    /// The rule across the top of the box, with the place in the history set
    /// into it where there is one and where it fits.
    ///
    /// Set into the top edge rather than given a row, because it is a fact
    /// about the line inside the box and the rows either side are spoken for:
    /// the window reading stands above and the mode stands below. A border is
    /// the one part of a container everything drawn on it is read as belonging
    /// to, which is exactly what is wanted here and exactly what made the row
    /// under the box wrong for it.
    ///
    /// Whole or not at all, the same bargain the model on the status row makes:
    /// half of `87/100` is a number, and a number that is not the place is
    /// worse than saying nothing. What never gives way is the rule's own width
    /// — the border under the box is laid out against the same one, and a top
    /// edge a column short of it is the first thing an eye finds.
    fn opening(&self, columns: usize, glyphs: Glyphs) -> String {
        // The two spaces that hold the words off the rule either side of them.
        const APART: usize = 2;

        let bar = glyphs.horizontal();
        let (open, opened) = glyphs.top();
        let inner = columns.saturating_sub(2);
        let said = self
            .history
            .said()
            .filter(|said| BEFORE + APART + width::columns(said) + AFTER <= inner);

        let Some(said) = said else {
            return format!("{open}{}{opened}", bar.repeat(inner));
        };

        let before = bar.repeat(BEFORE);
        let after = bar.repeat(inner - BEFORE - APART - width::columns(&said));

        format!("{open}{before} {said} {after}{opened}")
    }

    /// The rows the line is typed on, inside the frame.
    ///
    /// The mark goes on the first of them and the ones under it are indented to
    /// match, so a line that wrapped reads as one line rather than as a stack of
    /// separate ones.
    fn typed(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let inner = inner(columns);
        let edge = glyphs.vertical();

        self.shown(inner)
            .rows
            .into_iter()
            .enumerate()
            .map(|(at, shown)| {
                // Clipped as well as broken, for the one row breaking cannot
                // make fit: a character wider than the whole box. Half of one
                // cannot be drawn, so none of it is, and the row still ends
                // where the border expects it.
                let mut line = Row::plain(width::clip(shown.text, inner));
                line.pad(inner);

                let mark = if at == 0 { glyphs.caret() } else { " " };

                Row::new()
                    .then(Self::BORDER, edge)
                    .then(Slot::Plain, " ")
                    .then(Slot::Accent, mark)
                    .then(Slot::Plain, " ")
                    .join(line)
                    .then(Slot::Plain, " ")
                    .then(Self::BORDER, edge)
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

        self.shown(inner(columns))
            .rows
            .into_iter()
            .enumerate()
            .map(|(at, shown)| {
                let mark = if at == 0 { glyphs.caret() } else { " " };

                Row::new()
                    .then(Slot::Accent, mark)
                    .then(Slot::Plain, " ")
                    .then(Slot::Plain, width::clip(shown.text, inner(columns)))
            })
            .collect()
    }

    /// The row under the box: the mode and the keys that change it. A framed
    /// row puts the model the next turn goes to at the other end; a bare row
    /// gives that end to the remaining-window reading and fits the model before
    /// it.
    ///
    /// Two ends rather than one sentence, because the two facts are about
    /// different things — what a tool call arriving now costs, and what the
    /// next turn is asked of — and run together they read as one. It is also
    /// what keeps the mode starting in the same column every frame: a model
    /// changing length moves nothing on the left of the row.
    ///
    /// The model is read here rather than on the row at the top because this is
    /// the row a reader is already looking at: it is against the box they are
    /// typing into, and every key that changes the model is typed there. Both
    /// rows are held in place, so neither is the one that survives a scroll.
    ///
    /// Padded only as far as the last fact: the model when framed, the
    /// remaining-window reading when bare. This is the one row of the component
    /// not holding an edge up, so anything after that is bytes written every
    /// keystroke to draw nothing.
    fn status(&self, columns: usize, glyphs: Glyphs) -> Row {
        self.status_counted(columns, glyphs).0
    }

    /// The status row and whether its command control survived the width.
    fn status_counted(&self, columns: usize, glyphs: Glyphs) -> (Row, bool) {
        if columns >= FRAMED_AT {
            return self.session_status(columns, glyphs);
        }

        let reading = self.bare_reading(columns);
        let wide = width::columns(&reading);
        let gap = APART.min(columns.saturating_sub(wide));
        let room = columns.saturating_sub(wide + gap);
        let (mut row, counted) = self.session_status(room, glyphs);
        row.pad(room);
        row.push(Slot::Quiet, " ".repeat(gap));
        row.push(Slot::Quiet, reading);
        (row, counted)
    }

    /// The session facts that occupy the status row before its window reading.
    fn session_status(&self, columns: usize, glyphs: Glyphs) -> (Row, bool) {
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
        let counted = self.commands.running.map(|running| {
            let running = running.get();
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
        let counted = counted.filter(|_| needed <= room.saturating_sub(row.columns()));
        let drew_counted = counted.is_some();
        if let Some(counted) = counted {
            row.push(Slot::Quiet, parting);
            let tone = if self.commands.pointed {
                Slot::Pointed
            } else {
                Slot::Accent
            };
            row.push(tone, counted);
        }

        if let Some(at) = at {
            row.pad(at);
            row.push(Slot::Quiet, said);
        }

        (row, drew_counted)
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

    /// The usable-window fact in its full spelling.
    fn reading(&self) -> String {
        self.left.get().map_or_else(
            || "window unknown".to_owned(),
            |left| format!("{left}% window left"),
        )
    }

    /// The longest usable-window spelling a bare row can hold.
    fn bare_reading(&self, columns: usize) -> String {
        let full = self.reading();
        if width::columns(&full) <= columns {
            return full;
        }

        let compact = self.left.get().map(|left| format!("{left}%"));
        if compact
            .as_deref()
            .is_some_and(|compact| width::columns(compact) <= columns)
        {
            return compact.unwrap_or_default();
        }

        width::clip("?", columns).to_owned()
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
    fn shown(&self, inner: usize) -> Shown<'_> {
        let said = self.draft.text();
        let (line, column) = self.draft.position();
        let broken = broken(said, inner);
        let (row, column) = place(&broken, line, column);

        // The cursor's row is the last one shown, so a line being written grows
        // the box downwards and a line longer than the allowance scrolls under
        // it. Moving back up the line brings the rows above into view for the
        // same reason.
        let room = self.room.max(1);
        let first = row.saturating_sub(room - 1);

        Shown {
            rows: broken.get(first..).unwrap_or_default().to_vec(),
            row: row - first,
            column,
        }
    }
}

/// What the box is showing of the line, and where the cursor is in it.
struct Shown<'a> {
    /// The rows on screen, top first.
    rows: Vec<RowInLine<'a>>,
    /// Which of them the cursor is on.
    row: usize,
    /// How many columns into that row it sits.
    column: usize,
}

/// One display row: the text, and which logical line it is part of.
///
/// The line number is what lets a click and a cursor be placed on the right
/// line rather than at a column into the whole text, which a newline makes
/// ambiguous: the fiftieth column of a three-line prompt is not one place.
#[derive(Debug, Clone, Copy)]
struct RowInLine<'a> {
    text: &'a str,
    line: usize,
}

/// The text broken into display rows no wider than the box.
///
/// First into lines at each newline, then each line into rows at the column —
/// the order matters, because a newline is a break the reader put there and a
/// wrap is one the box did. A wrapped row continues the mark's indent; a new
/// line does too, so the two read the same and only the cursor's arithmetic
/// tells them apart.
///
/// Broken at a word boundary where one exists, while retaining every source
/// byte. Only a word or glyph sequence wider than the row is hard-broken.
fn broken(said: &str, inner: usize) -> Vec<RowInLine<'_>> {
    if inner == 0 {
        return vec![RowInLine { text: "", line: 0 }];
    }

    let mut rows = Vec::new();

    for (line, text) in said.split('\n').enumerate() {
        for range in width::wraps(text, inner) {
            rows.push(RowInLine {
                text: text.get(range).unwrap_or_default(),
                line,
            });
        }

        if text.is_empty() {
            rows.push(RowInLine { text, line });
        }

        // A line that exactly fills its last row is followed by an empty one, so
        // the cursor after the last character stands at the start of a row rather
        // than on the padding beside the border — which is where the next character
        // is going to appear anyway.
        if rows
            .last()
            .is_some_and(|row| row.line == line && width::columns(row.text) == inner)
        {
            rows.push(RowInLine { text: "", line });
        }
    }

    rows
}

/// Which display row the cursor's (`line`, `column`) falls on, and where in it.
///
/// A cursor at the very end of a full row belongs at the start of the next one,
/// which is where the character about to be typed will appear. The line is found
/// first, then the column within its rows: a column into the whole text would be
/// one place only on a prompt with no newlines.
fn place(rows: &[RowInLine<'_>], line: usize, column: usize) -> (usize, usize) {
    let mut before = 0;
    let mut last = 0;

    for (at, row) in rows.iter().enumerate() {
        if row.line == line {
            last = at;
            let wide = width::columns(row.text);

            if column < before + wide {
                return (at, column - before.min(column));
            }

            before += wide;
        }
    }

    // Past the line's last row is the end of it, which is where the eye reads a
    // line as ending. A line the text does not have is the last one shown.
    let wide = rows.get(last).map_or(0, |row| width::columns(row.text));
    (last, column.min(wide))
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
