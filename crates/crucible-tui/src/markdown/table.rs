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
//! not on screen, so what is gathered is bounded by [`MOST`] and by nothing
//! else: past it the lines go out exactly as the model wrote them, which is what
//! they would have done had this module never seen them.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::width;

/// How much of a block may be held before it is written out as itself.
///
/// A table is drawn only once the last of it has arrived, so this is the reader
/// with nothing to show — enough for any table an answer sensibly contains, and
/// an end to a wall of bars that is never going to stop.
const MOST: usize = 8 * 1024;

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

/// The lines of a block that opened with a bar, until one that does not.
#[derive(Debug)]
pub(super) struct Table {
    /// Every line so far, the bars included and the break not.
    lines: Vec<String>,
    /// The line being filled.
    line: String,
    /// Whether the scan is at the start of a line, which is where a bar decides
    /// whether the block goes on.
    fresh: bool,
    /// How much has been held, so [`MOST`] is an answer rather than a hope.
    held: usize,
}

impl Table {
    /// One opened by a bar the caller is still holding.
    pub(super) fn opening() -> Self {
        Self {
            lines: Vec::new(),
            line: String::new(),
            fresh: false,
            held: 0,
        }
    }

    /// Whether the scan stands at the start of a line.
    pub(super) fn fresh(&self) -> bool {
        self.fresh
    }

    /// Takes `character` into the block. `false` says the block has grown past
    /// [`MOST`]: it is not going to be a table, and the caller writes it out.
    pub(super) fn takes(&mut self, character: char) -> bool {
        self.fresh = character == '\n';

        if self.fresh {
            self.lines.push(std::mem::take(&mut self.line));
        } else {
            self.line.push(character);
        }

        self.held = self.held.saturating_add(character.len_utf8());
        self.held <= MOST
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
        laid.row(header, Some(Slot::Strong), say);
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
    /// `worn`, where it is given, is the slot every run of the row takes
    /// whatever its own markers said. That is the header, and only the header.
    fn row(
        &self,
        cells: &[Cell],
        worn: Option<Slot>,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let empty = Cell::new();

        for (at, room) in self.widths.iter().enumerate() {
            if at > 0 {
                say(Slot::Quiet, " ", None);
                say(Slot::Quiet, self.glyphs.vertical(), None);
                say(Slot::Quiet, " ", None);
            }

            let cell = clipped(cells.get(at).unwrap_or(&empty), *room, self.glyphs);
            let side = self.sides.get(at).copied().unwrap_or_default();
            let (before, after) = side.around(wide(&cell), *room);

            // The padding is the table's rather than the cell's, so it goes out
            // quiet with the bars either side of it. Which slot a space wears
            // decides nothing on screen and everything about how a row reads
            // back in a test.
            pad(before, say);
            for (slot, text) in &cell {
                say(worn.unwrap_or(*slot), text, None);
            }
            pad(after, say);
        }

        say(Slot::Plain, "\n", None);
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
    let mut markdown = super::Markdown::new(glyphs);
    let mut runs = Cell::new();

    markdown.read(cell, 0, &mut |slot, text, _| {
        runs.push((slot, text.to_owned()));
    });
    markdown.finish(0, &mut |slot, text, _| runs.push((slot, text.to_owned())));

    runs
}

/// `cell` with at most `room` columns of it kept, and a sign where the rest was.
fn clipped(cell: &Cell, room: usize, glyphs: Glyphs) -> Cell {
    if wide(cell) <= room {
        return cell.clone();
    }

    // The ellipsis is one column in both sets, as everything laid out against a
    // column width has to be.
    let ellipsis = glyphs.ellipsis();
    let ceiling = room.saturating_sub(width::columns(ellipsis));
    let mut kept = Cell::new();
    let mut spent = 0;

    for (slot, text) in cell {
        let text = width::clip(text, ceiling.saturating_sub(spent));
        spent = spent.saturating_add(width::columns(text));
        if !text.is_empty() {
            kept.push((*slot, text.to_owned()));
        }
    }
    kept.push((Slot::Quiet, ellipsis.to_owned()));

    kept
}
