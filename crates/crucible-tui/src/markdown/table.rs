//! A table, gathered from the lines that spell it and laid out against the
//! window it has to fit.
//!
//! The one thing in an answer that cannot be drawn where it is read. Every other
//! marker decides the run standing next to it and nothing else, so the scanner
//! hands text on as it goes; a column is as wide as the widest cell anywhere
//! below it, so the first row cannot be drawn until the last one has arrived.
//! Holding the block is what this module is for, and it is the whole of why it
//! exists.
//!
//! Which makes the cap the point rather than a detail. A block held is a block
//! not on screen, so what is gathered is bounded by [`TABLE_CAP`] and by
//! nothing else: past it the lines go out exactly as the model wrote them,
//! which is what they would have done had this module never seen them.
//!
//! A table that closes is handed on by its source, and laid out from it again
//! by [`Table::of`] whenever the width it has to fit changes.
//!
//! A table is written with its outer bars or without them. With them, the
//! first bar of a line says a table may be here. Without them nothing says so
//! until the delimiter row has arrived, so the caller reads the first line as
//! the prose it may be and a table only watches a copy of it: a bare table is
//! a header with at least two cells and a delimiter row with the same number
//! of them, each line watched for no more than [`HEADER_CAP`]. The break of the
//! header and what is written of the line under it are all that is held back,
//! and they are given back to the caller to read as the prose they are whenever
//! the second line stops being a delimiter row.
//!
//! The same watch serves a heading written as a line over a row of `=` or `-`,
//! so it is kept on every paragraph line and not only on one of two cells or
//! more. The line under a title is a delimiter row, an underline, or neither;
//! a row of dashes under a line that is not a header of two cells or more is
//! therefore an underline, and under one that is, with as many cells in the
//! row, a delimiter row, since a table was there first.
//!
use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width;

use super::{HEADER_CAP, Level, TABLE_CAP};

/// What stands between one column and the next: a space, a bar, a space.
const BETWEEN: usize = 3;

/// The fewest columns a column is worth drawing in.
const NARROWEST: usize = 1;

/// Room to pad a cell out of without asking for memory to do it.
const SPACES: &str = "                                                                ";

/// Which side of its column a cell is drawn against.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Sided {
    #[default]
    Left,
    Right,
    Middle,
}

impl Sided {
    /// The side a delimiter row's `cell` asks for. The colons are the whole of
    /// the notation: one on the left, one on the right, or one at each end.
    fn of(cell: &str) -> Self {
        match (cell.starts_with(':'), cell.ends_with(':')) {
            (true, true) => Self::Middle,
            (false, true) => Self::Right,
            _ => Self::Left,
        }
    }

    /// The padding before and after a cell `wide` columns across, drawn in a
    /// column with `room` of them.
    fn around(self, wide: usize, room: usize) -> (usize, usize) {
        let spare = room.saturating_sub(wide);
        match self {
            Self::Left => (0, spare),
            Self::Right => (spare, 0),
            Self::Middle => (spare / 2, spare - spare / 2),
        }
    }
}

/// One cell, with its own markers already read.
type Cell = Vec<(Slot, String)>;

/// What a table written without outer bars makes of the next character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Sight {
    /// It belongs to the block, which goes on.
    Keep,
    /// It does not: the block is a table, and the blank line ended it.
    Over,
    /// The row it ends underlines the line above it: that line is the title of
    /// a heading of this level, and neither is a table. Nothing is held back
    /// to be read; the break that ended the row is spent.
    Under(Level),
    /// It is not a table after all, and what was held back is the caller's to
    /// read ([`Table::held`]). The character is not in it.
    Release,
    /// The block has grown past [`TABLE_CAP`] and goes out as it was written.
    Spill,
}

/// The lines of a block that opened with a bar, or that are a header and a
/// delimiter row without one, until a line that is not its own.
#[derive(Debug)]
pub(super) struct Table {
    /// Every line so far, the bars included and the break not.
    lines: Vec<String>,
    /// The line being filled.
    line: String,
    /// Whether the scan is at the start of a line, which is where a bar decides
    /// whether the block goes on.
    fresh: bool,
    /// How much has been held, so [`TABLE_CAP`] is an answer rather than a
    /// hope.
    held: usize,
    /// Whether it opened without a bar, on a line that has to prove itself a
    /// header.
    bare: bool,
    /// Whether the line above the one being read has two cells or more, which
    /// is what a delimiter row under it needs.
    tabular: bool,
}

impl Table {
    /// One opened by a bar the caller is still holding.
    pub(super) fn opening() -> Self {
        Self {
            lines: Vec::new(),
            line: String::new(),
            fresh: false,
            held: 0,
            bare: false,
            tabular: false,
        }
    }

    /// One opened by a first character that is not a bar, which the caller is
    /// still holding.
    pub(super) fn bare() -> Self {
        Self {
            bare: true,
            ..Self::opening()
        }
    }

    /// The table `source` spells, as it stood once the last of it arrived.
    pub(super) fn of(source: &str) -> Self {
        let mut lines: Vec<String> = source.split('\n').map(str::to_owned).collect();
        let line = lines.pop().unwrap_or_default();
        Self {
            lines,
            line,
            fresh: source.ends_with('\n'),
            held: source.len(),
            bare: false,
            tabular: false,
        }
    }

    /// The block as it was written, every break included.
    pub(super) fn source(&self) -> String {
        let mut source = String::with_capacity(self.held);
        for line in &self.lines {
            source.push_str(line);
            source.push('\n');
        }
        source.push_str(&self.line);
        source
    }

    /// Whether it opened without a bar.
    pub(super) fn is_bare(&self) -> bool {
        self.bare
    }

    /// Whether it is a header still waiting for the delimiter row that would
    /// make it a table, which is a line of prose until it arrives.
    pub(super) fn pending(&self) -> bool {
        self.bare && self.lines.len() < 2
    }

    /// Whether it is still reading its first line, which the caller reads as
    /// prose all the same and this only watches.
    pub(super) fn mirrors(&self) -> bool {
        self.bare && self.lines.is_empty()
    }

    /// How many lines of it are whole.
    pub(super) fn lined(&self) -> usize {
        self.lines.len()
    }

    /// What a header that did not turn out to be one held back: its break and
    /// the start of the line under it, to be read as written. `None` while the
    /// header is still arriving, which is all in the caller's hands.
    pub(super) fn held(&self) -> Option<String> {
        (!self.lines.is_empty()).then(|| format!("\n{}", self.line))
    }

    /// How many of its lines have begun, the one still arriving included.
    pub(super) fn begun(&self) -> usize {
        self.lines.len() + usize::from(!self.line.is_empty())
    }

    /// Whether it has the delimiter row that makes it a table, rather than
    /// lines somebody wrote with bars in them.
    pub(super) fn delimited(&self) -> bool {
        self.lines.get(1).is_some_and(|line| delimits(line))
    }

    /// Whether the scan stands at the start of a line.
    pub(super) fn fresh(&self) -> bool {
        self.fresh
    }

    /// Takes `character` into the block. `false` says the block has grown past
    /// [`TABLE_CAP`]: it is not going to be a table, and the caller writes it
    /// out.
    pub(super) fn takes(&mut self, character: char) -> bool {
        self.fresh = character == '\n';

        if self.fresh {
            self.lines.push(std::mem::take(&mut self.line));
        } else {
            self.line.push(character);
        }

        self.held = self.held.saturating_add(character.len_utf8());
        self.held <= TABLE_CAP
    }

    /// Takes `character` into a block that opened without a bar, and says what
    /// became of it.
    ///
    /// The first line is a title: any line of prose of no more than
    /// [`HEADER_CAP`] bytes that is not the start of a block of another kind
    /// (a marker, a fence, a rule, a number and a stop) and is not blank. The
    /// line under it is a delimiter row if the first has at least two cells and
    /// the row has as many, an underline if it is nothing but `=` or `-` and
    /// the blanks around them, and otherwise neither. Both are decided as the
    /// line ends, except that a row is refused the moment it holds a character
    /// that neither has, and past the cap. The body of a table is every line
    /// after them, up to the first blank one. The title's own characters are
    /// also the caller's to read: this only watches them, and a `Keep` for them
    /// says nothing about whether they were taken.
    pub(super) fn sees(&mut self, character: char) -> Sight {
        if self.lines.len() >= 2 {
            if self.fresh && character == '\n' {
                return Sight::Over;
            }
            return if self.takes(character) {
                Sight::Keep
            } else {
                Sight::Spill
            };
        }

        let ends = character == '\n';
        if self.lines.is_empty() {
            if ends {
                if !titles(&self.line) {
                    return Sight::Release;
                }
                self.tabular = celled(&self.line).len() >= 2;
            } else {
                if self.line.len() >= HEADER_CAP {
                    return Sight::Release;
                }
                self.line.push(character);
                let opens = opens_block(&self.line);
                self.line.pop();
                if opens {
                    return Sight::Release;
                }
            }
            self.takes(character);
            return Sight::Keep;
        }

        if ends {
            let delimited = self.tabular
                && delimits(&self.line)
                && self
                    .lines
                    .first()
                    .is_some_and(|header| celled(&self.line).len() == celled(header).len());
            if delimited {
                self.takes(character);
                return Sight::Keep;
            }
            return match underline(&self.line) {
                Some(level) => Sight::Under(level),
                None => Sight::Release,
            };
        }

        self.line.push(character);
        let kept = self.line.len() <= HEADER_CAP
            && (underlines(&self.line)
                || (self.tabular && matches!(character, '-' | ':' | '|' | ' ' | '\t')));
        self.line.pop();
        if !kept {
            return Sight::Release;
        }

        self.takes(character);
        Sight::Keep
    }

    /// The line a row of `=` or `-` stands under, as it was written, where
    /// there is one.
    pub(super) fn title(&self) -> Option<&str> {
        self.lines.first().map(String::as_str)
    }

    /// The level of the heading the line under the title makes it, where that
    /// line is whole and is nothing but an underline: what the end of a message
    /// decides, being a break.
    pub(super) fn underlined(&self) -> Option<Level> {
        (self.lines.len() == 1).then(|| underline(&self.line))?
    }

    /// Whether what has arrived can still turn out to be a table.
    ///
    /// The delimiter row is what makes one: without it a line of bars is a line
    /// somebody wrote with bars in it. It is the second line, so this is
    /// answered once and answered early, which is what keeps the wait short for
    /// a block that was never going to be a table at all.
    pub(super) fn possible(&self) -> bool {
        self.lines.get(1).is_none_or(|line| delimits(line))
    }

    /// The block exactly as it arrived, for a caller putting it back.
    pub(super) fn spilt(&self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        for line in &self.lines {
            say(Slot::Plain, line, None);
            say(Slot::Plain, "\n", None);
        }
        if !self.line.is_empty() {
            say(Slot::Plain, &self.line, None);
        }
    }

    /// Draws the block as a table `room` columns wide, or puts it back as it was
    /// written if it cannot be drawn in that many.
    ///
    /// The header, a rule under it, and the body. No border around the outside:
    /// what a reader wants from a table is the columns, and four edges are a box
    /// drawn around something that already has a shape.
    pub(super) fn laid(
        &self,
        glyphs: Glyphs,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let rows = self.rows(glyphs);
        let (Some(header), Some(delimiter)) = (rows.first(), self.lines.get(1)) else {
            self.spilt(say);
            return;
        };

        let sides: Vec<Sided> = celled(delimiter).into_iter().map(Sided::of).collect();
        let Some(widths) = widths(&rows, room) else {
            // Narrower than one column apiece. Nothing that could be drawn here
            // would be a table, and the bars the model wrote are at least the
            // shape of one.
            self.spilt(say);
            return;
        };

        let laid = Laid {
            widths,
            sides,
            glyphs,
        };

        // The header is the row that says what the others are, so it is raised
        // whatever its own cells were written as.
        laid.row(header, Some(Slot::Bold), say);
        laid.rule(say);
        for cells in rows.iter().skip(1) {
            laid.row(cells, None, say);
        }
    }

    /// The rows that are drawn: every line but the delimiter, cut into cells and
    /// read once each.
    fn rows(&self, glyphs: Glyphs) -> Vec<Vec<Cell>> {
        self.lines
            .iter()
            .chain(std::iter::once(&self.line).filter(|line| !line.is_empty()))
            .enumerate()
            .filter(|(at, _)| *at != 1)
            .map(|(_, line)| {
                celled(line)
                    .into_iter()
                    .map(|cell| read(cell, glyphs))
                    .collect()
            })
            .collect()
    }
}

/// Whether `line` is the row of dashes that makes the block above it a table.
///
/// Every cell dashes, with a colon at either end or both, and at least one dash:
/// `| --- | ---: |` is a delimiter row and `| a | b |` is not.
fn delimits(line: &str) -> bool {
    let cells = celled(line);

    !cells.is_empty()
        && cells.iter().all(|cell| {
            cell.contains('-') && cell.chars().all(|character| matches!(character, '-' | ':'))
        })
}

/// Whether `line`, a whole first line, can be the title of a heading: it has
/// something on it, and is not a rule.
fn titles(line: &str) -> bool {
    !line.trim().is_empty() && !rules(line)
}

/// Whether `line` is a row of three or more of one of the marks a rule is
/// written with, and nothing else: what the reader draws across the room.
fn rules(line: &str) -> bool {
    let row = line.trim_start_matches(' ');
    row.chars().next().is_some_and(|mark| {
        matches!(mark, '-' | '*' | '_') && row.len() >= 3 && row.chars().all(|c| c == mark)
    })
}

/// Whether `line`, as far as it has arrived, opens a block of another kind: a
/// heading, a bullet, a quote, a fence or a numbered item. A paragraph line is
/// none of those, and the line under it cannot underline anything.
///
/// Only the start of the line is read, and a line of prose is told at its first
/// character.
fn opens_block(line: &str) -> bool {
    let row = line.trim_start_matches(' ');
    if line.len() - row.len() > 3 {
        return true;
    }
    let mut chars = row.chars();
    match chars.next() {
        Some('#') => row.trim_start_matches('#').starts_with(' '),
        Some('-' | '+' | '*') => row.get(1..).is_some_and(|rest| rest.starts_with(' ')),
        Some('>') => true,
        Some('`') => row.starts_with("```"),
        Some('~') => row.starts_with("~~~"),
        Some('0'..='9') => {
            let rest = row.trim_start_matches(|c: char| c.is_ascii_digit());
            rest.starts_with(". ") || rest.starts_with(") ")
        }
        _ => false,
    }
}

/// The level of the heading `line` underlines the line above it into, where it
/// is whole: up to three spaces, one mark or more of `=` or of `-`, and then
/// spaces and tabs.
fn underline(line: &str) -> Option<Level> {
    if !underlines(line) {
        return None;
    }
    match line.trim_start_matches(' ').chars().next()? {
        '=' => Some(Level::One),
        '-' => Some(Level::Two),
        _ => None,
    }
}

/// Whether `line`, as far as it has arrived, can still be an underline.
///
/// Up to three spaces, then one mark repeated, then blanks that end it: a mark
/// after a blank, or a second kind of mark, is not one.
fn underlines(line: &str) -> bool {
    let row = line.trim_start_matches(' ');
    if line.len() - row.len() > 3 {
        return false;
    }
    let Some(mark) = row.chars().next() else {
        return true;
    };
    if !matches!(mark, '=' | '-') {
        return false;
    }
    let marks = row.trim_start_matches(mark);
    marks.chars().all(|c| matches!(c, ' ' | '\t'))
}

/// `line` cut into the cells its bars separate.
///
/// The bar at each end is notation rather than an empty cell, and one written
/// `\|` is a bar the model wanted inside a cell.
fn celled(line: &str) -> Vec<&str> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = line.strip_suffix('|').unwrap_or(line);

    let mut cells = Vec::new();
    let mut from = 0;
    let mut escaped = false;

    for (at, character) in line.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '|' {
            cells.push(line.get(from..at).unwrap_or_default().trim());
            from = at.saturating_add(1);
        }
    }
    cells.push(line.get(from..).unwrap_or_default().trim());

    cells
}

/// How wide each column is drawn, or `None` if `room` cannot hold the table at
/// all.
///
/// The widest cell in the column, and then, while the row is wider than the
/// window, a column off whichever is widest — so a table gives up the room it
/// has most of, and a column of one-word cells keeps what it needs.
fn widths(rows: &[Vec<Cell>], room: usize) -> Option<Vec<usize>> {
    let across = rows.iter().map(Vec::len).max().unwrap_or_default();
    if across == 0 {
        return None;
    }

    let mut widths = vec![0; across];
    for cells in rows {
        for (at, cell) in cells.iter().enumerate() {
            if let Some(width) = widths.get_mut(at) {
                *width = (*width).max(wide(cell));
            }
        }
    }

    let bars = BETWEEN.saturating_mul(across.saturating_sub(1));
    if bars.saturating_add(NARROWEST.saturating_mul(across)) > room {
        return None;
    }

    while widths.iter().sum::<usize>().saturating_add(bars) > room {
        let widest = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, width)| **width)
            .map(|(at, _)| at)?;
        let width = widths.get_mut(widest)?;
        *width = width.saturating_sub(1).max(NARROWEST);
    }

    Some(widths)
}

/// A table with its columns decided, ready to draw itself.
struct Laid {
    /// How wide each column is drawn.
    widths: Vec<usize>,
    /// Which side of its column each column's cells are drawn against.
    sides: Vec<Sided>,
    /// The set the rule and the bars come out of.
    glyphs: Glyphs,
}

impl Laid {
    /// Draws one row of cells.
    ///
    /// On as many lines as its tallest cell needs: a cell wider than its column
    /// is wrapped inside the column rather than cut, and the cells beside it
    /// are padded down to meet it, so the bars run unbroken and nothing a cell
    /// said is lost to the width of the window.
    ///
    /// `worn`, where it is given, is the slot every run of the row takes
    /// whatever its own markers said. That is the header, and only the header.
    fn row(
        &self,
        cells: &[Cell],
        worn: Option<Slot>,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let empty = Cell::new();
        let wrapped: Vec<Vec<Cell>> = self
            .widths
            .iter()
            .enumerate()
            .map(|(at, room)| folded(cells.get(at).unwrap_or(&empty), *room))
            .collect();
        let tall = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);

        for line in 0..tall {
            for (at, room) in self.widths.iter().enumerate() {
                if at > 0 {
                    say(Slot::Quiet, " ", None);
                    say(Slot::Quiet, self.glyphs.vertical(), None);
                    say(Slot::Quiet, " ", None);
                }

                let cell = wrapped
                    .get(at)
                    .and_then(|parts| parts.get(line))
                    .unwrap_or(&empty);
                let side = self.sides.get(at).copied().unwrap_or_default();
                let (before, after) = side.around(wide(cell), *room);

                // The padding is the table's rather than the cell's, so it goes
                // out quiet with the bars either side of it. Which slot a space
                // wears decides nothing on screen and everything about how a
                // row reads back in a test.
                pad(before, say);
                for (slot, text) in cell {
                    say(worn.unwrap_or(*slot), text, None);
                }
                pad(after, say);
            }

            say(Slot::Plain, "\n", None);
        }
    }

    /// Draws the rule that separates the header from the body.
    fn rule(&self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        for (at, room) in self.widths.iter().enumerate() {
            if at > 0 {
                say(Slot::Quiet, self.glyphs.horizontal(), None);
                say(Slot::Quiet, self.glyphs.crossing(), None);
                say(Slot::Quiet, self.glyphs.horizontal(), None);
            }
            for _ in 0..*room {
                say(Slot::Quiet, self.glyphs.horizontal(), None);
            }
        }

        say(Slot::Plain, "\n", None);
    }
}

/// Writes `columns` columns of nothing.
fn pad(columns: usize, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
    let mut left = columns;

    while left > 0 {
        let now = left.min(SPACES.len());
        say(Slot::Quiet, SPACES.get(..now).unwrap_or(SPACES), None);
        left = left.saturating_sub(now);
    }
}

/// The columns a cell takes once drawn.
fn wide(cell: &Cell) -> usize {
    cell.iter().map(|(_, text)| width::columns(text)).sum()
}

/// A cell's own markers, read the way the rest of an answer's are.
///
/// A whole scanner for one cell, because a cell is markdown and this is what
/// reads markdown. It holds nothing across the call — a fence needs two lines
/// and a cell is one — so the reader is made here, used once and dropped.
fn read(cell: &str, glyphs: Glyphs) -> Cell {
    let markdown = super::Markdown::new(glyphs);
    let mut runs = Cell::new();

    // As written: a cell holds no block, and one reader holding another's
    // block back would leave the cell waiting on itself.
    let mut markdown = markdown.as_written();
    let mut keep = |said: super::Said<'_>| {
        if let super::Said::Run { slot, text, .. } = said {
            runs.push((slot, text.to_owned()));
        }
    };
    markdown.read(cell, 0, &mut keep);
    markdown.finish(0, &mut keep);

    runs
}

/// `cell` in as many lines as `room` columns hold it in, each keeping the slots
/// it was written in.
///
/// Through [`Row`], because a row already knows how to fold a run of slots at
/// the breaks plain text would take and cut a span in two without losing which
/// slot either half was in; a table has no reason to work that out a second
/// way. A cell that fits is one line, and an empty cell is one empty line.
fn folded(cell: &Cell, room: usize) -> Vec<Cell> {
    let mut row = Row::new();
    for (slot, text) in cell {
        row.push(*slot, text.as_str());
    }

    row.fold(room)
        .iter()
        .map(|part| {
            part.spans()
                .map(|(slot, text)| (slot, text.to_owned()))
                .collect()
        })
        .collect()
}
