//! A change drawn as one box: a header per file, a row per line, and a close.
//!
//! ```text
//! ╭─ src/parse.rs +3 −2
//! │   10    let mut tree = Tree::new();
//! │   11 -  for line in input.lines() {
//! │   11 +  for (at, line) in input.lines().enumerate() {
//! │  ⋯  26 lines between · fn parse  ⋯
//! ├─ README.md +1
//! │    2 +  A small parser.
//! ╰─
//! ```
//!
//! The header names the file and counts what moved; a count of nothing is
//! left off. Each line is the number a reader would find it at, the sign that
//! says which way it went, and the line. A line that moved is drawn on its
//! change's ground carried to the last column, and the words that changed in
//! it are raised on that ground; a line that did not move is on the reader's
//! own ground, coloured as its file's language when the path names one. Lines
//! the diff never held are one quiet row that counts them, and a file of more
//! than [`FOLD`] rows draws its head and tail around a row that counts the
//! rest and names the key that shows it.
//!
//! The rows are laid out once at their natural width and then clipped to the
//! room: a line cut short ends in the glyph set's ellipsis, and a header that
//! cannot hold its path and its counts shortens the path from its start, then
//! leaves the counts off, rather than cut a number in half.

use std::ops::Range;

use crucible_tui::chrome::Undrawn;
use crucible_tui::drawing::{Draws, EXPAND, Laid, Room};
use crucible_tui::syntax::Syntax;
use crucible_tui::{Glyphs, Row, Slot, columns};

use crate::change::{Change, FOLD, File, HEAD, Line, Side, TAIL};
use crate::read::{NotADiff, read};
use crate::words::{PAIRS, Raised, raised};

/// The narrowest the column of numbers is, so two changes in a transcript
/// start their lines in the same place whatever part of a file each touched.
const NUMBER: usize = 3;

/// The most lines of one change read for their language's colours. Past it a
/// line that did not move is drawn plain.
const SYNTAX_LINES: usize = 512;

/// The columns a tab stands for in a line drawn here.
const TAB: &str = "    ";

/// The fewest columns of a path a header shortens it to before it leaves its
/// counts off instead.
const FEWEST_PATH: usize = 4;

/// What draws a change, as a fence's block and as a tool's edit.
#[derive(Debug, Clone, Copy, Default)]
pub struct Changes;

impl Changes {
    /// `change` laid out for `room`.
    ///
    /// The one function a fence and a tool's edit are both drawn by.
    #[must_use]
    pub fn draw(&self, change: &Change, room: &Room) -> Laid {
        let laid = natural(change, room.glyphs);
        let widest = laid.iter().map(Drawn::columns).max().unwrap_or(0);
        let rows: Vec<Row> = laid.into_iter().map(|drawn| drawn.fitted(room)).collect();
        Laid {
            natural: (widest, rows.len()),
            rows,
            narrow: None,
        }
    }
}

impl Draws for Changes {
    fn names(&self) -> &'static [&'static str] {
        &["diff", "patch"]
    }

    fn label(&self, _: &str, _: &str) -> Option<&'static str> {
        Some("a change to files")
    }

    fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn> {
        let change = read(source).map_err(|why| match why {
            NotADiff::PastCap => Undrawn::PastCap,
            NotADiff::Unread => Undrawn::NotHere,
        })?;
        Ok(self.draw(&change, room))
    }
}

/// One row as it is laid out before a room is known.
enum Drawn {
    /// A file's header: the corner, the name, and the counts.
    Header {
        corner: String,
        name: Option<String>,
        counts: Option<Row>,
    },
    /// Any other row, with the ground it is filled to the last column in.
    Body { row: Row, ground: Option<Slot> },
}

impl Drawn {
    fn columns(&self) -> usize {
        match self {
            Self::Header {
                corner,
                name,
                counts,
            } => {
                columns(corner)
                    + name.as_deref().map_or(0, |name| 1 + columns(name))
                    + counts.as_ref().map_or(0, |counts| 1 + counts.columns())
            }
            Self::Body { row, .. } => row.columns(),
        }
    }

    fn fitted(self, room: &Room) -> Row {
        match self {
            Self::Header {
                corner,
                name,
                counts,
            } => header(&corner, name.as_deref(), counts, room),
            Self::Body { row, ground } => {
                let mut row = clipped(row, room, ground.unwrap_or(Slot::Quiet));
                if let Some(ground) = ground {
                    row.fill(ground, room.columns);
                }
                row
            }
        }
    }
}

/// Every row of `change`, at its natural width.
fn natural(change: &Change, glyphs: Glyphs) -> Vec<Drawn> {
    let mut drawn = Vec::new();
    let widest = numbered(change);
    let mut budget = Budget {
        pairs: PAIRS,
        syntax: SYNTAX_LINES,
    };

    for (at, file) in change.files().iter().enumerate() {
        let corner = if at == 0 {
            glyphs.top().0
        } else {
            glyphs.joining().0
        };
        drawn.push(Drawn::Header {
            corner: format!("{corner}{}", glyphs.horizontal()),
            name: named(file, glyphs),
            counts: counts(file, glyphs),
        });
        body(file, widest, glyphs, &mut budget, &mut drawn);
    }

    if !drawn.is_empty() {
        drawn.push(Drawn::Body {
            row: Row::new().then_structural(
                Slot::Quiet,
                format!("{}{}", glyphs.bottom().0, glyphs.horizontal()),
            ),
            ground: None,
        });
    }
    drawn
}

/// What one change may still spend on raising words and on colour.
struct Budget {
    pairs: usize,
    syntax: usize,
}

/// What one file's lines are drawn with: the column the numbers fill, the
/// glyph set, the reader of the file's language, and what the change may
/// still spend.
struct Lining<'a> {
    widest: usize,
    glyphs: Glyphs,
    syntax: Option<Syntax>,
    budget: &'a mut Budget,
}

/// The rows under one file's header.
fn body(file: &File, widest: usize, glyphs: Glyphs, budget: &mut Budget, drawn: &mut Vec<Drawn>) {
    let lines = file.lines();
    let shown: Vec<bool> = (0..lines.len())
        .map(|at| lines.len() <= FOLD || at < HEAD || at >= lines.len() - TAIL)
        .collect();
    let raised = paired(lines, &shown, budget);
    let mut lining = Lining {
        widest,
        glyphs,
        syntax: file.path().and_then(language).and_then(Syntax::of),
        budget,
    };

    for (at, line) in lines.iter().enumerate() {
        if lines.len() > FOLD && at == HEAD {
            drawn.push(Drawn::Body {
                row: folded(lines.len() - HEAD - TAIL, glyphs),
                ground: None,
            });
        }
        if !shown.get(at).copied().unwrap_or(false) {
            continue;
        }
        let row = match line {
            Line::Gap { lines, section } => Drawn::Body {
                row: between(*lines, section.as_deref(), glyphs),
                ground: None,
            },
            Line::Text { side, number, text } => {
                let marks = raised.get(at).and_then(Option::as_deref);
                lining.text_row(*side, *number, text, marks)
            }
        };
        drawn.push(row);
    }

    if file.is_binary() {
        drawn.push(Drawn::Body {
            row: quiet_row("binary, not drawn", glyphs),
            ground: None,
        });
    }
    if file.dropped() > 0 {
        let lines = if file.dropped() == 1 { "line" } else { "lines" };
        drawn.push(Drawn::Body {
            row: quiet_row(&format!("{} {lines} not shown", file.dropped()), glyphs),
            ground: None,
        });
    }
}

impl Lining<'_> {
    /// One line of the file: the number, the sign, and the line.
    fn text_row(
        &mut self,
        side: Side,
        number: usize,
        text: &str,
        marks: Option<&[Range<usize>]>,
    ) -> Drawn {
        let (ground, word, numbered, sign) = match side {
            Side::Kept => (Slot::Plain, Slot::Plain, Slot::Quiet, ' '),
            Side::Removed => (Slot::Removed, Slot::RemovedWord, Slot::RemovedNumber, '-'),
            Side::Added => (Slot::Added, Slot::AddedWord, Slot::AddedNumber, '+'),
        };
        let text = flattened(text);
        let widest = self.widest;

        let mut row = Row::new()
            .then_structural(Slot::Quiet, self.glyphs.vertical())
            .then(Slot::Plain, " ")
            .then(numbered, format!(" {number:>widest$} {sign}"))
            .then(Slot::Plain, " ");

        // The new side is read in order so a string or a comment opened on
        // one line is still open on the next; only the lines that did not
        // move wear its colours.
        let coloured = match (side, self.syntax.as_mut()) {
            (Side::Removed, _) | (_, None) => None,
            (_, Some(_)) if self.budget.syntax == 0 => None,
            (side, Some(reading)) => {
                self.budget.syntax -= 1;
                let mut runs = Vec::new();
                reading.read(&text, &mut |slot, said| runs.push((slot, said.to_owned())));
                (side == Side::Kept).then_some(runs)
            }
        };

        match (coloured, marks) {
            (Some(runs), _) => {
                for (slot, said) in runs {
                    row.push(slot, said);
                }
            }
            (None, Some(marks)) => {
                let mut at = 0;
                for mark in marks {
                    row.push(ground, text.get(at..mark.start).unwrap_or_default());
                    row.push(word, text.get(mark.clone()).unwrap_or_default());
                    at = mark.end;
                }
                row.push(ground, text.get(at..).unwrap_or_default());
            }
            (None, None) => row.push(ground, text),
        }

        Drawn::Body {
            row,
            ground: (side != Side::Kept).then_some(ground),
        }
    }
}

/// The raised words of each shown line that has a partner: in each run of
/// removed lines followed by added ones, the first removed with the first
/// added, the second with the second.
fn paired(lines: &[Line], shown: &[bool], budget: &mut Budget) -> Vec<Option<Vec<Range<usize>>>> {
    let mut raised_at = vec![None; lines.len()];
    let side = |at: usize| match lines.get(at) {
        Some(Line::Text { side, .. }) => Some(*side),
        _ => None,
    };
    let text = |at: usize| match lines.get(at) {
        Some(Line::Text { text, .. }) => flattened(text),
        _ => String::new(),
    };

    let mut at = 0;
    while at < lines.len() {
        if side(at) != Some(Side::Removed) {
            at += 1;
            continue;
        }
        let removed = at;
        while side(at) == Some(Side::Removed) {
            at += 1;
        }
        let added = at;
        while side(at) == Some(Side::Added) {
            at += 1;
        }
        for (old, new) in (removed..added).zip(added..at) {
            let both = shown.get(old).copied().unwrap_or(false)
                && shown.get(new).copied().unwrap_or(false);
            if !both || budget.pairs == 0 {
                continue;
            }
            budget.pairs -= 1;
            if let Some(Raised {
                old: olds,
                new: news,
            }) = raised(&text(old), &text(new))
            {
                if let Some(slot) = raised_at.get_mut(old) {
                    *slot = Some(olds);
                }
                if let Some(slot) = raised_at.get_mut(new) {
                    *slot = Some(news);
                }
            }
        }
    }
    raised_at
}

/// The widest number any line of `change` carries, in columns, never under
/// [`NUMBER`].
fn numbered(change: &Change) -> usize {
    change
        .files()
        .iter()
        .flat_map(File::lines)
        .filter_map(|line| match line {
            Line::Text { number, .. } => Some(*number),
            Line::Gap { .. } => None,
        })
        .max()
        .map_or(NUMBER, |widest| widest.to_string().len().max(NUMBER))
}

/// What a header calls the file: its path, or where it was and where it is
/// for a rename.
fn named(file: &File, glyphs: Glyphs) -> Option<String> {
    let arrow = match glyphs {
        Glyphs::Unicode => "→",
        Glyphs::Ascii => "->",
    };
    match (file.from(), file.path()) {
        (Some(from), Some(path)) if from != path => Some(format!("{from} {arrow} {path}")),
        (_, Some(path)) => Some(path.to_owned()),
        (Some(from), None) => Some(from.to_owned()),
        (None, None) => None,
    }
}

/// The counts a header carries: what was added, then what was removed, each
/// only where it is more than nothing.
fn counts(file: &File, glyphs: Glyphs) -> Option<Row> {
    let minus = match glyphs {
        Glyphs::Unicode => "−",
        Glyphs::Ascii => "-",
    };
    let mut row = Row::new();
    if file.added() > 0 {
        row.push(Slot::AddedNumber, format!("+{}", file.added()));
    }
    if file.removed() > 0 {
        if !row.is_empty() {
            row.push(Slot::Plain, " ");
        }
        row.push(Slot::RemovedNumber, format!("{minus}{}", file.removed()));
    }
    (!row.is_empty()).then_some(row)
}

/// The language a path's lines are coloured as: its extension, or its name
/// where it has none.
fn language(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => Some(extension),
        _ => (!name.is_empty()).then_some(name),
    }
}

/// The header, fitted to the room.
fn header(corner: &str, name: Option<&str>, counts: Option<Row>, room: &Room) -> Row {
    let width = room.columns;
    let mut row = Row::new().then_structural(Slot::Quiet, corner);
    let spent = columns(corner);
    let name_columns = name.map_or(0, columns);
    let counts_columns = counts.as_ref().map_or(0, Row::columns);
    let full = spent
        + name.map_or(0, |_| 1 + name_columns)
        + counts.as_ref().map_or(0, |_| 1 + counts_columns);

    let (name, counts) = if full <= width {
        (name.map(str::to_owned), counts)
    } else {
        let beside = width
            .saturating_sub(spent + 1)
            .saturating_sub(1 + counts_columns);
        let fewest = columns(room.glyphs.ellipsis()) + FEWEST_PATH;
        match name {
            Some(name) if counts.is_some() && beside >= fewest => {
                (Some(shortened(name, beside, room.glyphs)), counts)
            }
            Some(name) => (
                Some(shortened(
                    name,
                    width.saturating_sub(spent + 1),
                    room.glyphs,
                )),
                None,
            ),
            None => (None, counts.filter(|_| spent + 1 + counts_columns <= width)),
        }
    };

    if let Some(name) = name.filter(|name| !name.is_empty()) {
        row.push(Slot::Plain, " ");
        row.push(Slot::Path, name);
    }
    if let Some(counts) = counts {
        row = row.then(Slot::Plain, " ").join(counts);
    }
    row.clipped(width)
}

/// `name` in at most `room` columns, its start given up for an ellipsis
/// where it does not fit: the end of a path is the part that says which file.
fn shortened(name: &str, room: usize, glyphs: Glyphs) -> String {
    if columns(name) <= room {
        return name.to_owned();
    }
    let more = glyphs.ellipsis();
    let Some(left) = room.checked_sub(columns(more)) else {
        return String::new();
    };
    if left == 0 {
        return String::new();
    }
    let mut kept = 0;
    let mut from = name.len();
    let mut buffer = [0; 4];
    for (at, character) in name.char_indices().rev() {
        let wide = columns(character.encode_utf8(&mut buffer));
        if kept + wide > left {
            break;
        }
        kept += wide;
        from = at;
    }
    format!("{more}{}", name.get(from..).unwrap_or_default())
}

/// `row` cut to the room, ending in the ellipsis in `slot` where it was cut
/// and the room has space for one.
fn clipped(row: Row, room: &Room, slot: Slot) -> Row {
    if row.columns() <= room.columns {
        return row;
    }
    let more = room.glyphs.ellipsis();
    match room.columns.checked_sub(columns(more)) {
        Some(left) if left > 0 => {
            let mut cut = row.clipped(left);
            cut.push(slot, more);
            cut
        }
        _ => row.clipped(room.columns),
    }
}

/// A quiet row inside the box saying `what`, between two marks that say it
/// stands for lines.
fn quiet_row(what: &str, glyphs: Glyphs) -> Row {
    let mark = elided(glyphs);
    Row::new()
        .then_structural(Slot::Quiet, glyphs.vertical())
        .then(Slot::Quiet, format!("  {mark}  {what}  {mark}"))
}

/// The row that counts the lines between two hunks, with the section they
/// sit in where the diff named one.
fn between(lines: usize, section: Option<&str>, glyphs: Glyphs) -> Row {
    let noun = if lines == 1 { "line" } else { "lines" };
    let section = section.map_or_else(String::new, |section| format!(" · {}", flattened(section)));
    quiet_row(&format!("{lines} {noun} between{section}"), glyphs)
}

/// The row that stands for the lines a fold hides, naming the key that shows
/// them.
fn folded(lines: usize, glyphs: Glyphs) -> Row {
    let mark = elided(glyphs);
    let noun = if lines == 1 { "line" } else { "lines" };
    Row::new()
        .then_structural(Slot::Quiet, glyphs.vertical())
        .then(Slot::Quiet, format!("  {mark}  {lines} {noun} folded · "))
        .then(Slot::Cut, EXPAND)
        .then(Slot::Quiet, format!("  {mark}"))
}

/// The mark that stands for lines not drawn.
const fn elided(glyphs: Glyphs) -> &'static str {
    match glyphs {
        Glyphs::Unicode => "⋯",
        Glyphs::Ascii => "...",
    }
}

/// A line as it is drawn: its end trimmed, a tab as [`TAB`], and any other
/// control character a space.
fn flattened(text: &str) -> String {
    let mut line = String::with_capacity(text.len());
    for character in text.trim_end().chars() {
        match character {
            '\t' => line.push_str(TAB),
            control if control.is_control() => line.push(' '),
            character => line.push(character),
        }
    }
    line
}
