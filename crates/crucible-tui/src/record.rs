//! The record: what crucible has drawn, and what of it is on screen.
//!
//! The alternate screen has no scrollback, so this process is the one keeping
//! it. What that costs is bounded on purpose — [`MOST`] lines, oldest dropped —
//! because the budget in `CONTRIBUTING.md` is a ceiling on the whole process
//! and a store that grew with the session would spend it on rows nobody is
//! looking at. What falls off the top is not lost: the session log holds every
//! message, and the line a session ends on says where that is.
//!
//! A line is held as a [`Row`] — spans carrying slots — rather than as the
//! bytes a terminal would receive, so a narrower window re-wraps rather than
//! reflows. Only the lines the viewport covers are folded and painted, and that
//! is what keeps a frame proportional to the window rather than to the session.
//!
//! Two kinds of line, because two kinds of thing arrive here. Prose *flows*:
//! the model's answer and a tool's output were written as text and a wrap is
//! the only thing deciding where a row ends, so the width they are folded at is
//! whatever the window is now. A component's rows are *set*: a table, a diff, a
//! box were laid out against a width by something that is no longer here to lay
//! them out again, so a narrower window clips them rather than pretending it
//! can fold what it did not build.
//!
//! One block is set and is laid out again anyway, because for that one the
//! reason above is not true. The opening is drawn from facts read once at
//! launch and kept for the whole session, so what laid it is still here — and
//! it is the block a reader is most often looking at when they reach for the
//! corner of the window. [`Record::opens`] takes what laid it rather than only
//! what it laid, and a resize replaces those lines with the same card drawn for
//! the window there is now.

use std::fmt;

use std::collections::VecDeque;

use crate::color::Slot;
use crate::row::Row;

/// The most lines the record keeps.
///
/// Lines rather than turns, because a turn is not a size: one that read a file
/// is thousands of lines and one that answered a question is four. At roughly
/// the width of a window this is a few megabytes and tens of screens of
/// scrolling, which is deeper than a terminal's own default and far inside the
/// peak the budget allows.
const MOST: usize = 20_000;

/// One line of the record, and whether a narrower window may re-fold it.
#[derive(Debug, Clone)]
enum Line {
    /// Text that was written as text. Folded at whatever the window is now.
    Flowed(Row),
    /// Rows a component laid out against a width. Clipped, never re-folded.
    Set(Row),
}

/// The opening, and what can draw it again.
///
/// Where its lines are rather than a promise that they are first: nothing else
/// is laid this way today, and a span that says where it is costs one word and
/// cannot be wrong about it.
struct Opening {
    /// The first of its lines, counted as [`Record::gone`] counts.
    from: usize,
    /// How many lines it laid, at the width they were laid at.
    lines: usize,
    /// What laid them, kept for as long as they are held.
    lay: Box<dyn Fn(usize) -> Vec<Row>>,
}

impl fmt::Debug for Opening {
    /// By hand because a closure has no `Debug`, and the span is the part of
    /// this a reader debugging a scroll position wants anyway.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Opening")
            .field("from", &self.from)
            .field("lines", &self.lines)
            .finish_non_exhaustive()
    }
}

/// Everything crucible has drawn into the transcript band, and where in it the
/// reader is looking.
#[derive(Debug)]
pub(crate) struct Record {
    /// Lines, oldest first. The last is the one streamed text is appended to.
    lines: VecDeque<Line>,
    /// How many display rows each line folds to at [`Self::columns`].
    ///
    /// One `u16` a line, kept beside the lines rather than worked out per
    /// frame: scrolling asks how tall the record is on every wheel event, and
    /// folding twenty thousand lines to answer it is the shape of slowness this
    /// crate exists to refuse. Emptied whole on a resize, which is the only
    /// thing that can make every answer wrong at once.
    tall: VecDeque<u16>,
    /// The width every height above was worked out at.
    columns: usize,
    /// How many display rows the record comes to, in total.
    ///
    /// The sum of [`Self::tall`], kept rather than added up: see above.
    rows: usize,
    /// How many lines have been dropped off the top.
    ///
    /// Added to an index into [`Self::lines`] it gives a number that means the
    /// same thing for as long as the session does, which is what lets [`Spot`]
    /// hold still while the record fills and spills underneath it.
    gone: usize,
    /// Where the top of the transcript band is in the record.
    top: Spot,
    /// Whether the band follows the foot of the record as it grows.
    ///
    /// The state a session starts in and returns to, because a reader who has
    /// not scrolled is watching the answer arrive. Scrolling up clears it;
    /// scrolling back to the foot sets it again.
    following: bool,
    /// The opening and what laid it, while its lines are still held.
    ///
    /// `None` once the record has spilled far enough to eat into them: the
    /// lines that are left were drawn at a width that has gone, and replacing
    /// part of a card with a whole one would draw it twice.
    opening: Option<Opening>,
    /// Whether the last line is still being written to.
    ///
    /// A line is open from the first delta that lands in it until the newline
    /// that ends it, and only an open line can change. Held rather than worked
    /// out from what the last line looks like, because a line somebody wrote
    /// nothing on is a blank line and a line nobody has written on yet is not:
    /// the two are the same row and different facts.
    open: bool,
}

/// Where in the record a display row is: a line, and how far into it.
///
/// A line rather than a row number, so that a resize — which changes how many
/// rows every line folds to, and therefore what any row number meant — leaves
/// the reader looking at the line they were looking at.
/// Ordered head-first, which the derive gets right only because `line` is
/// declared before `into`: one spot is above another when its line is earlier,
/// and within a line when it is fewer rows in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Spot {
    /// The line, counted from the first of the session rather than the first
    /// still held: see [`Record::gone`].
    line: usize,
    /// How many of that line's display rows are above the band.
    into: u16,
}

impl Record {
    /// An empty record, to be drawn at `columns`.
    pub(crate) fn new(columns: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            tall: VecDeque::new(),
            columns,
            rows: 0,
            gone: 0,
            top: Spot { line: 0, into: 0 },
            following: true,
            opening: None,
            open: false,
        }
    }

    /// Append streamed text, which flows.
    ///
    /// A `\n` ends the line it is in rather than being drawn, and text with no
    /// `\n` in it continues whatever line is open — which is what makes a delta
    /// arriving mid-word land in the same line as the word it finishes.
    pub(crate) fn write(&mut self, slot: Slot, text: &str) {
        let mut rest = text;
        while let Some(at) = rest.find('\n') {
            self.extend(slot, &rest[..at]);
            self.close();
            rest = &rest[at + 1..];
        }
        if !rest.is_empty() {
            self.extend(slot, rest);
        }
    }

    /// Append rows a component laid out, which are set.
    pub(crate) fn lay(&mut self, rows: impl IntoIterator<Item = Row>) {
        for row in rows {
            self.put(Line::Set(row));
        }
    }

    /// Lay down the opening, keeping what laid it.
    ///
    /// The one block a resize draws again — see the prose at the top of this
    /// file for why this one and nothing else.
    pub(crate) fn opens(&mut self, lay: Box<dyn Fn(usize) -> Vec<Row>>) {
        self.end();
        let from = self.gone + self.lines.len();
        let laid = lay(self.columns);
        let lines = laid.len();
        self.lay(laid);
        self.opening = Some(Opening { from, lines, lay });
    }

    /// Draw the opening again for `columns`, in the lines it already holds.
    ///
    /// Before the heights are worked out again rather than after: this changes
    /// which lines there are, and [`Self::resized`] is what measures them.
    fn relay(&mut self, columns: usize) {
        let Some(opening) = self.opening.take() else {
            return;
        };

        // Part of it has fallen off the top, so what is left is no longer a
        // card — it is the bottom of one. Dropped rather than redrawn, which
        // leaves those lines standing as any other set rows do.
        if opening.from < self.gone {
            return;
        }

        let at = opening.from - self.gone;
        let laid = (opening.lay)(columns);
        let lines = laid.len();

        for _ in 0..opening.lines {
            self.lines.remove(at);
        }
        for (step, row) in laid.into_iter().enumerate() {
            self.lines.insert(at + step, Line::Set(row));
        }

        // The reader's place is a line number, and there are now a different
        // number of lines above it. One inside the card has nowhere of its own
        // to go back to, so it goes to the top of the card.
        let past = opening.from + opening.lines;
        if self.top.line >= past {
            self.top.line = (self.top.line + lines).saturating_sub(opening.lines);
        } else if self.top.line > opening.from {
            self.top.line = opening.from;
        }

        self.opening = Some(Opening { lines, ..opening });
    }

    /// Add `text` to the open line, opening one if none is.
    fn extend(&mut self, slot: Slot, text: &str) {
        if let (true, Some(Line::Flowed(row))) = (self.open, self.lines.back_mut()) {
            row.push(slot, text);
            self.remeasure();
            return;
        }

        let mut row = Row::new();
        row.push(slot, text);
        self.put(Line::Flowed(row));
        self.open = true;
    }

    /// End the open line, so the next text starts a new one.
    ///
    /// A newline with no line open is a line somebody left blank, which is the
    /// row between two paragraphs and is drawn. Which is what separates this
    /// from [`Self::end`], where nothing open means nothing to do.
    fn close(&mut self) {
        if self.open {
            self.open = false;
            return;
        }
        self.put(Line::Flowed(Row::new()));
    }

    /// Add a line, dropping the oldest if the record is full.
    fn put(&mut self, line: Line) {
        self.open = false;
        let tall = Self::measure(&line, self.columns);
        self.lines.push_back(line);
        self.tall.push_back(tall);
        self.rows += usize::from(tall);
        while self.lines.len() > MOST {
            self.lines.pop_front();
            let tall = self.tall.pop_front().unwrap_or(0);
            self.rows -= usize::from(tall);
            self.gone += 1;
        }
    }

    /// Work out the last line's height again, after text was added to it.
    fn remeasure(&mut self) {
        let Some(line) = self.lines.back() else {
            return;
        };
        let now = Self::measure(line, self.columns);
        let Some(was) = self.tall.back_mut() else {
            return;
        };
        self.rows = self.rows - usize::from(*was) + usize::from(now);
        *was = now;
    }

    /// How many display rows a line comes to at the current width.
    ///
    /// Saturating rather than wrapping: a line taller than a `u16` is one
    /// nobody can read anyway, and the alternative is arithmetic that is right
    /// until somebody pastes a megabyte.
    fn measure(line: &Line, columns: usize) -> u16 {
        match line {
            Line::Flowed(row) => u16::try_from(row.folds(columns)).unwrap_or(u16::MAX),
            Line::Set(_) => 1,
        }
    }

    /// Whether a line is still being written to.
    pub(crate) fn writing(&self) -> bool {
        self.open
    }

    /// End whatever line is open, and add nothing where none is.
    ///
    /// What a caller says before putting down something that is a line in its
    /// own right — a row a component laid out, or the end of a message.
    pub(crate) fn end(&mut self) {
        self.open = false;
    }

    /// Whether the record already ends in a blank line.
    ///
    /// What a caller asks before putting one there, so that two things that
    /// each want space around them get one row between them rather than two.
    /// A record nobody has written to is parted: there is nothing above to be
    /// parted from.
    pub(crate) fn parted(&self) -> bool {
        match self.lines.back() {
            None => true,
            Some(Line::Flowed(row)) => !self.open && row.text().trim().is_empty(),
            Some(Line::Set(row)) => row.text().trim().is_empty(),
        }
    }

    /// How many lines the session has taken, including those since dropped.
    ///
    /// Counted from the first line of the session rather than the first still
    /// held, so a number kept by a caller goes on naming the same line after
    /// the record has spilled underneath it.
    pub(crate) fn lines(&self) -> usize {
        self.gone + self.lines.len()
    }
}

/// The viewport: which of the record the transcript band is showing.
impl Record {
    /// Draw the band, `rows` tall, as the display rows to put in it.
    ///
    /// Only the lines the band covers are folded, which is what keeps a frame
    /// proportional to the window rather than to the session. Fewer rows than
    /// asked for means the record does not fill the band yet, and the rest of
    /// the band is under what there is: a session that has just started reads
    /// from the top of the window down, as a terminal's own would.
    pub(crate) fn view(&self, rows: usize) -> Vec<Row> {
        let from = if self.following {
            self.foot(rows)
        } else {
            self.top
        };
        let mut out = Vec::with_capacity(rows);
        let mut into = usize::from(from.into);
        for line in self.lines.iter().skip(from.line.saturating_sub(self.gone)) {
            for row in self.fold(line).into_iter().skip(into) {
                out.push(row);
                if out.len() == rows {
                    return out;
                }
            }
            into = 0;
        }
        out
    }

    /// Move the band `by` display rows, positive downwards, and say whether
    /// anything moved.
    ///
    /// Clamped at both ends rather than wrapping or refusing: a wheel spun hard
    /// at the foot of a session should stop there, not travel and come back.
    /// Which line is `into` rows down a band `rows` tall, if any is.
    ///
    /// What answers a click: the band draws lines and the terminal reports a
    /// row, and this is the one place both are known. Counted off [`Self::tall`]
    /// rather than by folding, because where a row lands does not need the
    /// words in it.
    pub(crate) fn at(&self, into: usize, rows: usize) -> Option<usize> {
        let from = if self.following {
            self.foot(rows)
        } else {
            self.top
        };

        let mut left = into;
        let mut skip = usize::from(from.into);
        for (at, tall) in self.tall.iter().enumerate().skip(from.line - self.gone) {
            let shown = usize::from(*tall).saturating_sub(skip);
            skip = 0;
            if left < shown {
                return Some(self.gone + at);
            }
            left -= shown;
        }

        None
    }

    /// Move the band `by` display rows, and say whether it moved.
    ///
    /// Negative is towards the head of the session. A band that was following
    /// the foot starts from where the foot put it, so the first turn of a wheel
    /// moves from what the reader is looking at rather than from a spot left
    /// over from the last time anybody scrolled.
    pub(crate) fn scroll(&mut self, by: i32, rows: usize) -> bool {
        let foot = self.foot(rows);
        let was = if self.following { foot } else { self.top };
        let far = usize::try_from(by.unsigned_abs()).unwrap_or(usize::MAX);
        let now = match by {
            0 => return false,
            up if up < 0 => self.back(was, far),
            _ => self.on(was, far, foot),
        };
        self.following = now == foot;
        self.top = now;
        now != was
    }

    /// Whether the band is showing the foot of the record.
    ///
    /// Nothing outside this file asks: what the renderer does about a viewport
    /// that has been scrolled away from the foot is put it back, and that is
    /// [`Self::follow`]. Here to be asserted about, since the flag is what
    /// decides whether arriving text moves somebody who is reading back.
    #[cfg(test)]
    pub(crate) fn following(&self) -> bool {
        self.following
    }

    /// Follow the foot of the record again.
    pub(crate) fn follow(&mut self) {
        self.following = true;
    }

    /// Lay the record out for a window of a different width.
    ///
    /// Every height is wrong at once, so every height is worked out again —
    /// and the spot keeps its line and loses its offset into it, because the
    /// row that was third of five in a line is not the third of two.
    pub(crate) fn resized(&mut self, columns: usize) {
        if columns == self.columns {
            return;
        }
        self.columns = columns;
        self.relay(columns);
        self.rows = 0;
        self.tall.clear();
        for line in &self.lines {
            let tall = Self::measure(line, columns);
            self.tall.push_back(tall);
            self.rows += usize::from(tall);
        }
        self.top.into = 0;
    }

    /// The spot that puts the last display row at the foot of a band `rows`
    /// tall — walking back from the end, so the cost is the band's rather than
    /// the record's.
    fn foot(&self, rows: usize) -> Spot {
        let mut left = rows;
        for (back, &tall) in self.tall.iter().enumerate().rev() {
            let tall = usize::from(tall);
            if tall >= left {
                return Spot {
                    line: self.gone + back,
                    into: u16::try_from(tall - left).unwrap_or(u16::MAX),
                };
            }
            left -= tall;
        }
        Spot {
            line: self.gone,
            into: 0,
        }
    }

    /// `spot` moved `rows` display rows towards the head of the record.
    fn back(&self, spot: Spot, rows: usize) -> Spot {
        let mut left = rows;
        let mut at = spot.line.saturating_sub(self.gone);
        let mut into = usize::from(spot.into);
        loop {
            if into >= left {
                return Spot {
                    line: self.gone + at,
                    into: u16::try_from(into - left).unwrap_or(u16::MAX),
                };
            }
            left -= into;
            let Some(next) = at.checked_sub(1) else {
                return Spot {
                    line: self.gone,
                    into: 0,
                };
            };
            at = next;
            into = usize::from(self.tall.get(at).copied().unwrap_or(1));
        }
    }

    /// `spot` moved `rows` display rows towards the foot, never past `foot`.
    fn on(&self, spot: Spot, rows: usize, foot: Spot) -> Spot {
        let mut left = rows;
        let mut at = spot.line.saturating_sub(self.gone);
        let mut into = usize::from(spot.into);
        loop {
            let tall = usize::from(self.tall.get(at).copied().unwrap_or(1));
            let rest = tall.saturating_sub(into);
            if rest > left {
                let now = Spot {
                    line: self.gone + at,
                    into: u16::try_from(into + left).unwrap_or(u16::MAX),
                };
                return now.min(foot);
            }
            left -= rest;
            into = 0;
            at += 1;
            if at >= self.lines.len() {
                return foot;
            }
        }
    }

    /// A line as the display rows it comes to at the current width.
    fn fold(&self, line: &Line) -> Vec<Row> {
        match line {
            Line::Flowed(row) => row.fold(self.columns),
            Line::Set(row) => vec![row.clipped(self.columns)],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record holding `lines` numbered lines, each one flowed.
    fn filled(columns: usize, lines: usize) -> Record {
        let mut record = Record::new(columns);
        for line in 0..lines {
            record.write(Slot::Plain, &format!("{line}\n"));
        }
        record
    }

    /// What the view says, as plain text.
    fn said(record: &Record, rows: usize) -> Vec<String> {
        record.view(rows).iter().map(Row::text).collect()
    }

    /// A source that lays one row per column of the window, so what width it
    /// was called at can be read straight off what it laid.
    fn ruler() -> Box<dyn Fn(usize) -> Vec<Row>> {
        Box::new(|columns| {
            (0..columns)
                .map(|row| Row::new().then(Slot::Plain, format!("{row}")))
                .collect()
        })
    }

    #[test]
    fn the_opening_is_laid_out_again_when_the_window_changes() {
        let mut record = Record::new(8);
        record.opens(ruler());
        record.write(Slot::Plain, "after\n");

        record.resized(5);

        let laid: Vec<String> = record.lines.iter().map(measured).collect();
        assert_eq!(laid, ["0", "1", "2", "3", "4", "after"]);
    }

    #[test]
    fn a_card_that_changes_height_leaves_the_reader_on_the_line_they_were_on() {
        let mut record = Record::new(8);
        record.opens(ruler());
        for line in 0..6 {
            record.write(Slot::Plain, &format!("said {line}\n"));
        }

        // Above the foot, so the reader's place is a spot rather than a
        // promise to follow — and below the card, which is the half of this a
        // shorter card moves.
        record.scroll(-2, 4);
        let reading = said(&record, 4);
        assert_eq!(reading.first().map(String::as_str), Some("said 0"));

        // Two lines shorter than it was, so a place counted from the top of the
        // record means two lines further down than it did. Wide enough that
        // what is under the card still folds to one row apiece, because that
        // is the other half of a resize and is not what this is about.
        record.resized(6);

        assert_eq!(said(&record, 4), reading);
    }

    #[test]
    fn a_reader_inside_a_card_that_was_laid_out_again_is_left_at_the_top_of_it() {
        let mut record = Record::new(8);
        record.opens(ruler());
        for line in 0..6 {
            record.write(Slot::Plain, &format!("said {line}\n"));
        }

        // Four rows into the card, which is a row the shorter card does not
        // have: eight rows became five. There is no line to be left on, so the
        // reader is left on the thing they were reading rather than on a
        // number that used to be inside it.
        record.scroll(-6, 4);
        assert_eq!(said(&record, 1), ["4"]);

        record.resized(5);

        assert_eq!(said(&record, 1), ["0"]);
    }

    /// The text of one line, whichever kind it is.
    fn measured(line: &Line) -> String {
        match line {
            Line::Flowed(row) | Line::Set(row) => row.text(),
        }
    }

    /// What the record is tall, worked out the slow way.
    fn counted(record: &Record) -> usize {
        record
            .lines
            .iter()
            .map(|line| record.fold(line).len())
            .sum()
    }

    #[test]
    fn what_the_record_says_it_is_tall_is_what_its_lines_come_to() {
        let mut record = Record::new(10);
        record.write(Slot::Plain, "a word that will not fit in ten\n");
        record.write(Slot::Plain, "short\n");
        record.lay([set("a set row that is far wider than ten columns")]);
        record.write(Slot::Accent, "and more");

        assert_eq!(record.rows, counted(&record));
        let tall: usize = record.tall.iter().map(|&t| usize::from(t)).sum();
        assert_eq!(record.rows, tall);
    }

    #[test]
    fn a_line_that_grows_past_the_width_grows_the_record_with_it() {
        let mut record = Record::new(10);
        record.write(Slot::Plain, "one");
        assert_eq!(record.rows, 1);

        // The same line, now too long for one row: the height kept beside it
        // has to be worked out again, or the record is a row short for the
        // rest of the session.
        record.write(Slot::Plain, " two three four five");

        assert!(record.rows > 1);
        assert_eq!(record.rows, counted(&record));
    }

    #[test]
    fn a_delta_that_stops_mid_word_lands_in_the_line_the_word_is_in() {
        let mut record = Record::new(40);
        record.write(Slot::Plain, "hel");
        record.write(Slot::Plain, "lo wor");
        record.write(Slot::Plain, "ld");

        assert_eq!(said(&record, 4), ["hello world"]);
    }

    #[test]
    fn a_newline_ends_the_line_it_is_in_and_is_never_drawn() {
        let mut record = Record::new(40);
        record.write(Slot::Plain, "one\ntwo\n");

        // Two, not three. The trailing newline ends the line it is in and
        // opens nothing: on a screen this process owns, the row a cursor sits
        // on belongs to the box, so a blank row here would be one the reader
        // is given for nothing.
        assert_eq!(said(&record, 8), ["one", "two"]);
    }

    #[test]
    fn a_band_shows_the_foot_while_nobody_has_scrolled() {
        let record = filled(40, 10);

        assert!(record.following());
        assert_eq!(said(&record, 3), ["7", "8", "9"]);
    }

    #[test]
    fn a_band_taller_than_the_record_is_given_what_there_is() {
        let record = filled(40, 2);

        assert_eq!(said(&record, 40).len(), 2);
    }

    #[test]
    fn scrolling_up_stops_at_the_head_and_says_when_it_did_not_move() {
        let mut record = filled(40, 10);

        assert!(record.scroll(-100, 3));
        assert!(!record.scroll(-1, 3));
        assert_eq!(said(&record, 3), ["0", "1", "2"]);
    }

    #[test]
    fn scrolling_back_to_the_foot_follows_again() {
        let mut record = filled(40, 10);

        assert!(record.scroll(-4, 3));
        assert!(!record.following());
        assert!(record.scroll(100, 3));
        assert!(record.following());
        assert_eq!(said(&record, 3), ["7", "8", "9"]);
    }

    #[test]
    fn a_set_row_is_clipped_and_never_folded() {
        let mut record = Record::new(6);
        record.lay([set("far wider than six")]);

        assert_eq!(record.rows, 1);
        assert_eq!(said(&record, 4), ["far wi"]);
    }

    #[test]
    fn a_resize_keeps_the_line_the_reader_was_on_and_drops_the_row() {
        let mut record = Record::new(10);
        for line in 0..3 {
            record.write(
                Slot::Plain,
                &format!("line {line} with several words in it\n"),
            );
        }
        record.scroll(-6, 2);
        let was = record.top.line;

        // Partway into a line, which is the case a resize invalidates: the row
        // that was fourth of five in a line is not the fourth of two.
        assert!(record.top.into > 0);

        record.resized(18);

        assert_eq!(record.top.line, was);
        assert_eq!(record.top.into, 0);
        // Wide enough to change every height and narrow enough that they are
        // still not one, which is what makes the new heights observable.
        assert!(record.rows > record.lines.len());
        assert_eq!(record.rows, counted(&record));
    }

    #[test]
    fn the_oldest_lines_are_dropped_and_counted() {
        let record = filled(40, MOST + 100);

        assert_eq!(record.lines.len(), MOST);
        assert_eq!(record.gone, record.lines.len() + record.gone - MOST);
        assert!(record.gone >= 100);
        assert_eq!(record.rows, counted(&record));
        assert_eq!(
            said(&record, 2),
            [format!("{}", MOST + 98), format!("{}", MOST + 99)]
        );
    }

    #[test]
    fn a_reader_looking_at_a_line_that_spills_is_left_at_the_head() {
        let mut record = filled(40, 10);
        record.scroll(-100, 3);
        assert_eq!(record.top.line, 0);

        for line in 0..MOST + 100 {
            record.write(Slot::Plain, &format!("more {line}\n"));
        }

        // The line they were on is gone. The head of what is left is the
        // closest thing to where they were looking, and it is not a panic.
        assert!(record.top.line < record.gone);
        assert_eq!(said(&record, 1), [format!("more {}", record.gone - 10)]);
    }

    /// A row of `text`, laid out rather than flowed.
    fn set(text: &str) -> Row {
        let mut row = Row::new();
        row.push(Slot::Plain, text);
        row
    }
}
