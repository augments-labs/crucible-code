//! Every release on one rail, from the first to the one running.
//!
//! The older releases a row each, hollow-marked: the version, the day and how
//! many entries of each kind it held. Then the newest few in full, each under
//! a filled mark, their words hanging off the rail. The running version is the
//! last of them, and no rail runs under it: its words stand indented, and the
//! rail ends where the program the reader is using is.

use std::fmt::Write as _;

use crate::color::Slot;
use crate::forge::Forge;
use crate::glyphs::Glyphs;
use crate::markdown::Markdown;
use crate::row::Row;
use crate::width::{columns as wide, cut, folds};

/// A release told in one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Brief<'a> {
    /// Its number: `0.38.0`.
    pub version: &'a str,
    /// The day it was cut.
    pub date: &'a str,
    /// How many entries of each kind it held, in the order they are said:
    /// `("added", 8)`, `("changed", 2)`.
    pub counted: &'a [(&'a str, usize)],
}

/// A release told in full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Told<'a> {
    /// The release as one row, for when there is no room for the whole of it.
    pub brief: Brief<'a>,
    /// Its words, in markdown: paragraphs, then a `### ` heading over each
    /// kind of entry.
    pub text: &'a str,
}

/// Releases on a rail.
#[derive(Debug, Clone, Copy)]
pub struct Timeline<'a> {
    /// The releases told in a row each, oldest first.
    pub older: &'a [Brief<'a>],
    /// The releases told in full, oldest first. The last is the newest.
    pub told: &'a [Told<'a>],
    /// The version that is running, marked where it is the newest.
    pub running: Option<&'a str>,
    /// The repository a bare number in the words is counted against.
    pub forge: Option<&'a Forge>,
    /// The last words of the closing row, or `None` for no closing row.
    pub closing: Option<&'a str>,
    /// The most rows the whole may come to.
    ///
    /// Past it, what is left out goes in this order: the oldest one-row
    /// releases, then the words of the oldest told in full, each told in a row
    /// instead, then the end of the newest. The closing row says what went.
    ///
    /// The releases told in full, each cut to a row, the rail, the newest's
    /// first row and the closing row are the least the whole can be told in,
    /// and are always kept: a `most` below them gives them, and so more rows
    /// than `most`.
    pub most: usize,
}

/// Columns the words of a release hang in from the rail: the rail and two.
const HANG: usize = 3;

/// The number of releases told in full, as the closing row says it.
const COUNTED: [&str; 10] = [
    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
];

impl Timeline<'_> {
    /// The rows at `columns`.
    #[must_use]
    pub fn rows(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let columns = columns.max(1);
        let pad = self
            .older
            .iter()
            .chain(self.told.iter().map(|told| &told.brief))
            .map(|brief| wide(brief.version))
            .max()
            .unwrap_or(0);

        let newest = self.told.len().saturating_sub(1);
        let drawn = Drawn {
            older: self
                .older
                .iter()
                .map(|brief| summed(brief, pad, columns, glyphs))
                .collect(),
            briefly: self
                .told
                .iter()
                .map(|told| summed(&told.brief, pad, columns, glyphs))
                .collect(),
            full: self
                .told
                .iter()
                .enumerate()
                .map(|(at, told)| self.full(told, at == newest, columns, glyphs))
                .collect(),
        };

        // The fewest left out that fits: older rows first, then the oldest
        // told in full, down to the newest alone.
        let mut left = Left::default();
        loop {
            let laid = self.lay(&drawn, left, columns, glyphs);
            if laid.len() <= self.most {
                return laid;
            }
            if left.older < drawn.older.len() {
                left.older += 1;
            } else if left.briefly < newest {
                left.briefly += 1;
            } else {
                left.cut = true;
                let laid = self.lay(&drawn, left, columns, glyphs);
                return laid;
            }
        }
    }

    /// The whole with `left` left out.
    fn lay(&self, drawn: &Drawn, left: Left, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let mut rows: Vec<Row> = drawn
            .older
            .iter()
            .skip(left.older)
            .flatten()
            .cloned()
            .collect();
        rows.extend(drawn.briefly.iter().take(left.briefly).flatten().cloned());

        let closing = self.closing(left, columns, glyphs);
        let told = drawn.full.len();
        for (at, words) in drawn.full.iter().enumerate().skip(left.briefly) {
            if !rows.is_empty() {
                rows.push(rail(glyphs));
            }
            if left.cut && at + 1 == told {
                // The newest alone passes the most: its head and as much of
                // it as there is room for, and the closing row saying so.
                let room = self.most.saturating_sub(rows.len() + closing.len()).max(1);
                rows.extend(words.iter().take(room).cloned());
            } else {
                rows.extend(words.iter().cloned());
            }
        }

        rows.extend(closing);
        rows
    }

    /// A release told in full: its head, and its words hanging off the rail,
    /// or with no rail under the newest.
    fn full(&self, told: &Told<'_>, newest: bool, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let brief = told.brief;
        let mut head = Row::new()
            .then(Slot::Accent, format!("{} ", glyphs.told()))
            .then(Slot::Strong, brief.version)
            .then(Slot::Quiet, format!(" {} {}", glyphs.dot(), brief.date));
        if newest && self.running == Some(brief.version) {
            head.push(Slot::Quiet, format!(" {} this version", glyphs.dot()));
        }

        let mut rows = hung(&Spans::of(&head), columns, 2);
        let indent = if columns > HANG { HANG } else { 0 };
        let room = columns.saturating_sub(indent).max(1);
        let hang = |row: Row| {
            if indent == 0 {
                return row;
            }
            let lead = if newest {
                Row::new().then(Slot::Plain, " ".repeat(indent))
            } else {
                Row::new().then(Slot::Quiet, format!("{}  ", glyphs.vertical()))
            };
            if row.is_empty() {
                return if newest { Row::new() } else { rail(glyphs) };
            }
            lead.join(row)
        };

        rows.push(hang(Row::new()));
        rows.extend(
            words(told.text, room, glyphs, self.forge)
                .into_iter()
                .map(hang),
        );
        if !newest {
            rows.push(hang(Row::new()));
        }
        rows
    }

    /// The closing row, after a blank one, where there is one.
    fn closing(&self, left: Left, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let Some(hint) = self.closing else {
            return Vec::new();
        };

        let dot = glyphs.dot();
        let releases = self.older.len() + self.told.len();
        let told = self.told.len();
        let mut said = format!("{releases} releases");
        if told > 0 {
            let many = COUNTED
                .get(told - 1)
                .map_or_else(|| told.to_string(), |word| (*word).to_owned());
            let _ = write!(said, " {dot} {many} newest in full");
        }

        let mut gone = Vec::new();
        if left.older > 0 {
            gone.push(format!("{} of the older rows left out", left.older));
        }
        if left.briefly > 0 {
            gone.push(format!("{} told in a row each", left.briefly));
        }
        if left.cut
            && let Some(newest) = self.told.last()
        {
            gone.push(format!("the end of {} left out", newest.brief.version));
        }
        if !gone.is_empty() {
            let _ = write!(said, " {dot} incomplete at this width: {}", gone.join(", "));
        }
        let _ = write!(said, " {dot} {hint}");

        let row = Row::new()
            .then(Slot::Quiet, format!("  {} ", glyphs.hangs()))
            .then(Slot::Plain, said);
        let mut rows = vec![Row::new()];
        rows.extend(hung(&Spans::of(&row), columns, 4));
        rows
    }
}

/// Every release drawn each way it can be told: the older a row each, the
/// newest a row each, and the newest in full.
struct Drawn {
    older: Vec<Vec<Row>>,
    briefly: Vec<Vec<Row>>,
    full: Vec<Vec<Row>>,
}

/// How much of the whole is left out.
#[derive(Debug, Clone, Copy, Default)]
struct Left {
    /// The oldest one-row releases, gone.
    older: usize,
    /// The oldest told in full, told in a row instead.
    briefly: usize,
    /// Whether the end of the newest is cut.
    cut: bool,
}

/// A row of the rail and nothing else.
fn rail(glyphs: Glyphs) -> Row {
    Row::new().then(Slot::Quiet, glyphs.vertical())
}

/// A release in one row: its mark, its number, its day and its counts.
///
/// One row whatever the width, because a row each is what the older releases
/// are: a window too narrow for all of it has the end cut, and says so with
/// an ellipsis.
fn summed(brief: &Brief<'_>, pad: usize, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let mut row = Row::new()
        .then(Slot::Quiet, format!("{} ", glyphs.summed()))
        .then(
            Slot::Plain,
            format!(
                "{}{}  ",
                brief.version,
                " ".repeat(pad.saturating_sub(wide(brief.version)))
            ),
        )
        .then(Slot::Quiet, brief.date);
    if !brief.counted.is_empty() {
        let dot = format!(" {} ", glyphs.dot());
        let counted = brief
            .counted
            .iter()
            .map(|(kind, count)| format!("{count} {kind}"))
            .collect::<Vec<_>>()
            .join(&dot);
        row.push(Slot::Quiet, format!("  {counted}"));
    }

    if row.columns() <= columns {
        return vec![row];
    }
    let ellipsis = glyphs.ellipsis();
    match columns.checked_sub(wide(ellipsis)) {
        Some(room) if room > 0 => {
            let mut row = row.clipped(room);
            row.push(Slot::Quiet, ellipsis);
            vec![row]
        }
        _ => vec![row.clipped(columns)],
    }
}

/// A release's words as rows `room` wide: paragraphs and entries read as an
/// answer is, and each `### ` heading a quiet row of its own.
fn words(text: &str, room: usize, glyphs: Glyphs, forge: Option<&Forge>) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut segment = String::new();

    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("### ") {
            read(&segment, room, glyphs, forge, &mut rows);
            segment.clear();
            if !rows.is_empty() {
                rows.push(Row::new());
            }
            rows.extend(hung(
                &Spans::of(&Row::new().then(Slot::Quiet, heading.trim())),
                room,
                0,
            ));
            continue;
        }
        segment.push_str(line);
        segment.push('\n');
    }
    read(&segment, room, glyphs, forge, &mut rows);
    rows
}

/// Reads one stretch of markdown into rows `room` wide, after `rows`.
///
/// A stretch that follows a heading starts on the row under it; one that
/// follows words starts after a blank row, as a paragraph does.
fn read(segment: &str, room: usize, glyphs: Glyphs, forge: Option<&Forge>, rows: &mut Vec<Row>) {
    let segment = segment.trim_matches('\n');
    if segment.trim().is_empty() {
        return;
    }

    let mut lines: Vec<Spans> = vec![Spans::default()];
    let mut say = |slot: Slot, text: &str, link: Option<&str>| {
        let mut parts = text.split('\n');
        if let (Some(first), Some(line)) = (parts.next(), lines.last_mut()) {
            line.push(slot, first, link);
        }
        for part in parts {
            let mut line = Spans::default();
            line.push(slot, part, link);
            lines.push(line);
        }
    };
    let mut markdown = Markdown::new(glyphs).counting(forge.cloned());
    markdown.read(segment, room, &mut say);
    markdown.finish(room, &mut say);

    while lines.last().is_some_and(Spans::is_empty) {
        lines.pop();
    }
    let first = lines.iter().position(|line| !line.is_empty()).unwrap_or(0);

    let bullet = glyphs.bullet();
    for line in lines.iter().skip(first) {
        let text = line.text();
        let spaces = text.len() - text.trim_start_matches(' ').len();
        let marked = text.get(spaces..).is_some_and(|rest| {
            rest.starts_with(bullet)
                && rest
                    .get(bullet.len()..)
                    .is_some_and(|after| after.starts_with(' '))
        });
        let hang = if marked {
            spaces + wide(bullet) + 1
        } else {
            spaces
        };
        rows.extend(hung(line, room, hang));
    }
}

/// `line` folded to `columns`, every row after the first standing `hang`
/// columns in, under the words rather than the mark they follow.
///
/// A line that folds to nothing, because it is blank or every character in it
/// is wider than a row, is one row of what fits: nothing, or the mark alone.
fn hung(line: &Spans, columns: usize, hang: usize) -> Vec<Row> {
    let text = line.text();
    if hang == 0 || hang >= columns {
        let rows: Vec<Row> = folds(&text, columns)
            .into_iter()
            .map(|part| line.between(part.start, part.end))
            .collect();
        return if rows.is_empty() {
            vec![Row::new()]
        } else {
            rows
        };
    }

    // Where the first `hang` columns end, which is not `hang` bytes in: a
    // bullet or a diamond is one column and three bytes.
    let mark = cut(&text, hang).unwrap_or(text.len());
    let words = text.get(mark..).unwrap_or_default();
    let parts = folds(words, columns - hang);
    if parts.is_empty() {
        return vec![line.between(0, mark)];
    }
    parts
        .into_iter()
        .enumerate()
        .map(|(at, part)| {
            let row = line.between(mark + part.start, mark + part.end);
            if at == 0 {
                line.between(0, mark).join(row)
            } else {
                Row::new().then(Slot::Plain, " ".repeat(hang)).join(row)
            }
        })
        .collect()
}

/// A line of runs, each with its slot and the address it links to.
///
/// Kept apart from a [`Row`] until it is folded, because folding under a mark
/// cuts the line at byte offsets a row does not hand out.
#[derive(Debug, Default)]
struct Spans(Vec<(Slot, String, Option<Box<str>>)>);

impl Spans {
    /// The runs `row` is made of.
    fn of(row: &Row) -> Self {
        Self(
            row.spans()
                .map(|(slot, text)| (slot, text.to_owned(), None))
                .collect(),
        )
    }

    fn push(&mut self, slot: Slot, text: &str, link: Option<&str>) {
        if !text.is_empty() {
            self.0.push((slot, text.to_owned(), link.map(Into::into)));
        }
    }

    fn is_empty(&self) -> bool {
        self.0.iter().all(|(_, text, _)| text.is_empty())
    }

    fn text(&self) -> String {
        self.0.iter().map(|(_, text, _)| text.as_str()).collect()
    }

    /// The bytes of [`Spans::text`] between `from` and `to`, as a row.
    fn between(&self, from: usize, to: usize) -> Row {
        let mut row = Row::new();
        let mut at = 0;
        for (slot, text, link) in &self.0 {
            let (start, end) = (at, at + text.len());
            at = end;
            let (from, to) = (from.max(start), to.min(end));
            let Some(part) = text.get(from.saturating_sub(start)..to.saturating_sub(start)) else {
                continue;
            };
            if from >= to {
                continue;
            }
            match link {
                Some(link) => row.push_linked(*slot, part, link.clone()),
                None => row.push(*slot, part),
            }
        }
        row
    }
}

#[cfg(test)]
mod tests;
