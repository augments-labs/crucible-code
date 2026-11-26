//! Code: the fences a block of it is written between, the lines read inside
//! one, and the frame its rows are drawn in.
//!
//! The reader opens a fenced block on a run of three or more backticks or
//! tildes and says it as it streams; what is here is what decides where it
//! ends ([`Fenced`]), what its info string names, how a run of backticks with
//! another backtick after it on its line is read as the prose it is, where it
//! opens a span of code, how a line that could have closed it and did not is
//! given back, and how a line of code is handed on, read in the language the
//! block names. A row of code is read back off the screen by its gutter
//! ([`framed`]), and broken where the room runs out rather than at a space.

use std::ops::Range;

use super::{CODE, Gutter, HeldMarks, Inside, MOST, Markdown, PLACEHOLDER, Quoted, SPACES};
use crate::chrome;
use crate::color::Slot;
use crate::escape::Escapes;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width;

/// What stands in front of a row of code that carries on the one above it,
/// where a line of code wrapped: a mark and a space, as wide as the gutter.
#[must_use]
pub(crate) const fn continuation(glyphs: Glyphs) -> &'static str {
    match glyphs {
        Glyphs::Unicode => "┆ ",
        Glyphs::Ascii => ": ",
    }
}

/// The set a row of code in its frame was drawn in, which its gutter says.
fn gutters(row: &Row) -> Option<(Glyphs, usize)> {
    let (slot, text) = row.spans().next()?;
    if slot != Slot::Quiet {
        return None;
    }
    [Glyphs::Unicode, Glyphs::Ascii]
        .into_iter()
        .find(|glyphs| chrome::gutter(*glyphs) == text)
        .map(|glyphs| (glyphs, text.len()))
}

/// A frame's edge, as the structure it is: drawn, never copied.
///
/// Only a row of nothing but quiet words is read as one, which is how the
/// reader says an edge and no running text is said.
fn edged(row: &Row) -> bool {
    row.spans().all(|(slot, _)| slot == Slot::Quiet) && chrome::is_edge(&row.text())
}

/// Where `code` breaks to fit `room` columns, as byte ranges that partition it.
///
/// A line of code is broken wherever the room runs out, never at a space: it
/// is cut as written and nothing is dropped at a break, because what a reader
/// copies off a wrapped line has to be the line. Whitespace that would end a
/// row begins the next one instead, so that the rows read back off the screen
/// (which has no way to tell a space from the blank after it) join into the
/// line. A row of nothing but whitespace keeps it, and may lose it on a copy.
///
/// The one thing that is dropped: a character wider than `room` itself, which
/// no row could draw (a double-width character where one column is left), is
/// left out with the marks on it, so a copy of that line lacks it.
fn cuts(code: &str, room: usize) -> Vec<Range<usize>> {
    let mut parts = Vec::new();
    let mut base = 0;

    while base < code.len() {
        let rest = code.get(base..).unwrap_or_default();
        let Some(over) = width::cut(rest, room) else {
            parts.push(base..code.len());
            break;
        };

        // A character wider than the room can be drawn nowhere in it, so it is
        // left out, with the marks that ride on it.
        if over == 0 {
            let mut left = rest.chars();
            let first = left.next().map_or(rest.len(), char::len_utf8);
            let marks: usize = left
                .take_while(|mark| width::advance(*mark) == Some(0))
                .map(char::len_utf8)
                .sum();
            base += first + marks;
            continue;
        }

        let row = rest.get(..over).unwrap_or_default();
        let kept = row.trim_end_matches(char::is_whitespace).len();
        let at = if kept == 0 { over } else { kept };
        parts.push(base..base + at);
        base += at;
    }

    parts
}

/// Where a row of code in its frame breaks at `columns`: the set it was drawn
/// in, the bytes its gutter takes, and the ranges of its code after that.
///
/// `None` for a row that is not one, and for a window with no room for code
/// beside the gutter.
fn broken(row: &Row, columns: usize) -> Option<(Glyphs, usize, Vec<Range<usize>>)> {
    let (glyphs, lead) = gutters(row)?;
    let room = columns
        .checked_sub(chrome::GUTTER)
        .filter(|room| *room > 0)?;
    let code = row.between(lead, usize::MAX).text();
    let mut parts = cuts(&code, room);
    if parts.is_empty() {
        parts.push(0..0);
    }
    Some((glyphs, lead, parts))
}

/// The display rows a row of code in its frame comes to at `columns`, where
/// `row` is one.
///
/// The gutter is the frame's, and structural, so a drag takes the code and not
/// the bar. A line that wraps carries on under a mark of its own where the
/// gutter stood, also structural, and breaks wherever the room runs out. What
/// the code says comes back whole and in order, which is what a copy of the
/// wrapped line is made from. The frame's two edges are structural too, and
/// are clipped to the window as the frame is, never folded, so that every row
/// of one is known as an edge and a drag over a block takes its code alone.
#[must_use]
pub(crate) fn framed(row: &Row, columns: usize) -> Option<Vec<Row>> {
    if edged(row) {
        let edge = Row::new().then_structural(Slot::Quiet, row.text());
        return Some(vec![edge.clipped(columns)]);
    }
    let (glyphs, lead, parts) = broken(row, columns)?;
    let code = row.between(lead, usize::MAX);

    Some(
        parts
            .into_iter()
            .enumerate()
            .map(|(at, part)| {
                let front = if at == 0 {
                    chrome::gutter(glyphs)
                } else {
                    continuation(glyphs)
                };
                Row::new()
                    .then_structural(Slot::Quiet, front)
                    .join(code.between(part.start, part.end))
            })
            .collect(),
    )
}

/// How many display rows [`framed`] gives `row`, without building them.
#[must_use]
pub(crate) fn framed_height(row: &Row, columns: usize) -> Option<usize> {
    if edged(row) {
        return Some(1);
    }
    broken(row, columns).map(|(_, _, parts)| parts.len())
}

/// What a fence's info string names, as its frame's label is made from it.
///
/// Cleaned first and split after: an escape sequence has spaces in it that
/// are not the end of the name, and a control character in the middle of a
/// word is one word with the control taken out of it. Cleaned here, once, and
/// not by whatever draws the label, because this is where the text from a
/// model is read -- what is dropped is what a terminal would act on or a
/// reader would be misled by, and the placeholder an image is placed with
/// along with the marks that give it a row and a column.
pub(super) fn fence_name(info: &str) -> String {
    let mut escapes = Escapes::default();
    let mut placed = false;
    let cleaned: String = info
        .chars()
        .filter(|character| {
            if escapes.holds(*character) {
                return false;
            }
            if *character == PLACEHOLDER {
                placed = true;
                return false;
            }
            let advance = width::advance(*character);
            let mark = placed && advance == Some(0);
            placed = placed && mark;
            advance.is_some() && !mark
        })
        .collect();
    cleaned
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The run of marks a block was fenced with, which says what closes it, and
/// the indentation of the item it was written under.
///
/// A line of the same mark, at least as many of them, indented three spaces
/// at most and followed by nothing but spaces and tabs: so a block fenced with
/// four backticks holds one fenced with three, and a row of backticks inside a
/// block fenced with tildes is a row of backticks. Under an item, every line
/// is read past as many of the item's columns as it has spaces for, and the
/// three are counted from there.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Fenced {
    pub(super) mark: char,
    pub(super) count: usize,
    /// The columns the words of the item the block opened under start at.
    pub(super) base: usize,
    /// The spaces in front of the run that opened the block, which are the
    /// line's again where the rest of it says it opens none.
    pub(super) indent: usize,
}

impl Fenced {
    /// Whether `line`, with or without its break, closes the block.
    pub(super) fn closes(self, line: &str) -> bool {
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.get(self.margin(line)..).unwrap_or_default();
        let marks = line.trim_start_matches(' ');
        let rest = marks.trim_start_matches(self.mark);
        line.len().saturating_sub(marks.len()) < CODE
            && marks.len().saturating_sub(rest.len()) >= self.count
            && rest.trim_start_matches([' ', '\t']).is_empty()
    }

    /// How many of the spaces `line` opens with are the item's.
    pub(super) fn margin(self, line: &str) -> usize {
        let spaces = line
            .len()
            .saturating_sub(line.trim_start_matches(' ').len());
        spaces.min(self.base)
    }

    /// Takes the item's indentation off every line of `source`, where it was
    /// written under one; in place, since what is handed on is the block.
    pub(super) fn unindents(self, source: &mut String) {
        if self.base == 0 {
            return;
        }
        // The item's spaces taken off the line so far, or none once the line
        // has said something else.
        let mut taken = Some(0);
        source.retain(|character| {
            if character == '\n' {
                taken = Some(0);
                return true;
            }
            match taken {
                Some(spaces) if character == ' ' && spaces < self.base => {
                    taken = Some(spaces + 1);
                    false
                }
                _ => {
                    taken = None;
                    true
                }
            }
        });
    }
}

impl Markdown {
    /// Reads `character` into the rest of a fence's own line. Answers whether
    /// it was kept rather than said.
    ///
    /// What is left of an opening fence's line names the language it is
    /// written in, or nothing at all. Either way it is not the answer, so it
    /// goes the way the fence went — but it is kept, because it is what
    /// decides how the block is read. What is left of a closing one is spaces
    /// and tabs, kept to be given back if the line turns out to be code; any
    /// other character shows that it is, and is code itself.
    ///
    /// What follows a run of backticks may hold no backtick, so one there
    /// says the run opened no block: see [`Markdown::unopens`]. Past [`MOST`]
    /// bytes of what follows, the bound on what is kept, an opening line is
    /// read as the fence it opened as, and a closing one as a line of code,
    /// its kept blanks given back so that none is lost; a run with more
    /// blanks than that after it closes nothing.
    pub(super) fn keeps_fence(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        if self.inside == Inside::Closing && !matches!(character, ' ' | '\t') {
            self.unfence(say);
            self.previous = character;
            return false;
        }
        if self.inside == Inside::Opening
            && self.fenced.mark == '`'
            && character == '`'
            && self.language.len() < MOST
        {
            self.unopens(character, room, say);
            return true;
        }
        if self.language.len() < MOST {
            self.language.push(character);
        }
        if self.inside == Inside::Closing && self.language.len() >= MOST {
            self.unfence(say);
            self.previous = character;
        }
        true
    }

    /// Reads the line a run of backticks opened a block on as the prose it
    /// is, at `character`, a backtick after the run: nothing of the line has
    /// been said, so it is said now as it would have been. The spaces in
    /// front of the run are the line's again, the run opens a span of code as
    /// one written in prose would, and what followed it, with `character`, is
    /// read again into that span.
    ///
    /// Decided at the first backtick, which nothing later on the line can
    /// undo; a line with none is a fence only once it ends, before which
    /// nothing of it is drawn.
    fn unopens(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let fenced = std::mem::take(&mut self.fenced);
        self.fence = None;
        self.inside = Inside::Prose;
        self.indent = fenced.indent;
        self.ticked = Some(super::ticks::Span::opened(fenced.count));
        let mut rest = std::mem::take(&mut self.language);
        rest.push(character);
        self.again(&rest, room, say);
    }

    /// Answers what is left of `rest`, the end of a delta, to say.
    ///
    /// A line of a fence that has said nothing but spaces when the delta ends,
    /// past a quote's mark where it has one, keeps them: whether they stand in
    /// front of the line that closes it is for the rest of the line to say, so
    /// they are counted rather than said.
    pub(super) fn counts_margin<'a>(&mut self, rest: &'a str) -> &'a str {
        if self.inside == Inside::Fence && !self.started && rest.bytes().all(|byte| byte == b' ') {
            self.indent += rest.len();
            return "";
        }
        rest
    }

    /// Reads the quote's mark that opens a line of a block fenced inside the
    /// quote. Answers whether `next`, the character after the marks, was taken
    /// with it.
    ///
    /// The mark and one space after it are the quote's, and the bar is drawn
    /// in their place, after the spaces in front of the mark; the line of code
    /// starts after them, so a fence there may close the block. One quote is
    /// read and no more: a second mark is code.
    pub(super) fn requotes(
        &mut self,
        held: HeldMarks,
        next: char,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        self.line.quoted = self.row();
        let mut indent = std::mem::take(&mut self.indent);
        while indent > 0 {
            let spaces = indent.min(SPACES.len());
            say(Slot::Quiet, SPACES.get(..spaces).unwrap_or_default(), None);
            indent -= spaces;
        }
        self.bar(say);
        if held.count > 1 {
            let code = HeldMarks {
                count: held.count - 1,
                opened: false,
                ..held
            };
            self.unmark(code, say);
            return false;
        }
        // Nothing of the line of code has been read yet.
        self.started = false;
        next == ' '
    }

    /// Ends a block fenced inside a quote, and the quote with it, at the first
    /// line that is no row of the quote: the block's bottom edge, on a row of
    /// the quote, before anything of the line.
    pub(super) fn unquotes(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.line.quoted = self.row();
        self.bar(say);
        say(Slot::Quiet, chrome::bottom(self.glyphs), None);
        say(Slot::Quiet, "\n", None);
        self.line.quoted = Quoted::Not;
        self.quoting = Quoted::Not;
        self.inside = Inside::Prose;
        self.syntax = None;
    }

    /// What a row of the quote a block is fenced inside is part of: an alert
    /// wears its own bar, and any other quote's bar is drawn already.
    fn row(&self) -> Quoted {
        match self.quoting {
            Quoted::Alert => Quoted::Alert,
            Quoted::Not | Quoted::Owed | Quoted::Plain => Quoted::Plain,
        }
    }

    /// Gives back a line that could have closed the fenced block and did not,
    /// because something other than a space or a tab followed its marks, or
    /// more of them than are kept: its spaces, its marks and what stood after
    /// them are a line of code.
    pub(super) fn unfence(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.inside = Inside::Fence;
        if let Some((opened, count)) = self.fence.take() {
            let held = HeldMarks {
                mark: self.fenced.mark,
                count,
                opened,
            };
            self.unmark(held, say);
        }
        let after = std::mem::take(&mut self.language);
        self.say(&after, say);
    }

    /// Hands on the gutter of the line of code being read, once.
    ///
    /// One run, so that whatever folds the row afterwards can tell it from
    /// the rest by what it says: see [`framed`].
    pub(super) fn gutter(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if matches!(self.inside, Inside::Fence | Inside::Indented)
            && self.line.gutter == Gutter::Owed
        {
            self.line.gutter = Gutter::Put;
            say(Slot::Quiet, chrome::gutter(self.glyphs), None);
        }
    }

    /// Hands on the line of code collected so far, read.
    ///
    /// Every byte comes back exactly once and in order, which is the property
    /// the whole thing rests on — a byte dropped is code that quietly changed
    /// meaning on screen, and a byte doubled is a row wider than it measured.
    pub(super) fn read_code(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if self.code.is_empty() {
            return;
        }

        let mut code = std::mem::take(&mut self.code);
        // The reader wants the break as part of the line: a great many rules in
        // a syntax definition are written against the end of one.
        code.push('\n');

        match self.syntax.as_mut() {
            Some(syntax) => syntax.read(&code, &mut |slot, text| {
                // The break is the caller's to write, once, below.
                let text = text.strip_suffix('\n').unwrap_or(text);
                if !text.is_empty() {
                    say(slot, text, None);
                }
            }),
            None => say(Slot::Quiet, code.trim_end_matches('\n'), None),
        }
    }
}
