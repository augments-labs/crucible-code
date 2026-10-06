//! The record: what crucible has drawn, and what of it is on screen.
//!
//! The alternate screen has no scrollback, so this process is the one keeping
//! it. What that costs is bounded on purpose — [`MOST`] retained units, oldest
//! dropped —
//! because the budget in `CONTRIBUTING.md` is a ceiling on the whole process
//! and a store that grew with the session would spend it on rows nobody is
//! looking at. What falls off the top is not lost: the session log holds every
//! message, and the line a session ends on says where that is.
//!
//! In native mode the terminal's own scrollback keeps it instead, and a line
//! written there is let go of here as it is written, numbered as if it had
//! fallen off the top: the record then holds only what has not gone out yet,
//! and remembers only whether the last line to go was blank, so the next block
//! is parted from it as it would be from a line still held.
//!
//! A line is held as a [`Row`] — spans carrying slots — rather than as the
//! bytes a terminal would receive, so a narrower window re-wraps rather than
//! reflows. Only the lines the viewport covers are folded and painted, and that
//! is what keeps a frame proportional to the window rather than to the session.
//!
//! Three kinds of line, because three kinds of thing arrive here. Prose
//! *flows*: the model's answer and a tool's output were written as text and a
//! wrap is the only thing deciding where a row ends, so the width they are
//! folded at is whatever the window is now. A component whose source is still
//! held is *responsive*: a prompt or a diff is laid out again at the new width.
//! Everything else is *set*: a table or a box laid out by something that is gone
//! is clipped rather than pretending it can be rebuilt.
//!
//! The opening is responsive for the same reason. It is drawn from facts read
//! once at launch and kept for the whole session, so what laid it is still here
//! and a resize replaces those lines with the same card drawn for the window
//! there is now, in the glyph set in force now: a reader who changed it since
//! launch is not handed the card back in the characters they turned away from.

use std::fmt;

use std::collections::VecDeque;
use std::ops::Range;

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::scroll_rail::Place;

/// The most retained units the record keeps.
///
/// An ordinary line costs one. Responsive source is charged in window-sized
/// byte units as well, because a prompt or diff is not a size merely because it
/// draws as one block. At roughly the width of a window this is a few megabytes
/// and tens of screens of scrolling, deeper than a terminal's own default and
/// far inside the peak budget.
///
/// Public as [`RECORDED`](crate::RECORDED), for a caller printing more rows at
/// once than it can be sure fit: what it prints past this is dropped from the
/// top as the rest arrives.
pub(crate) const MOST: usize = 20_000;

/// Bytes of retained responsive source charged as one ordinary record line.
///
/// A prompt or diff keeps source that fixed rows used to discard after layout.
/// Charging it in window-sized units keeps that new hold under the record's
/// existing ceiling without making every ordinary line count its allocation.
const RETAINED_ROW_BYTES: usize = 80;

/// The most prompt landmarks the record keeps for the scroll rail to mark.
///
/// More than the rows of any ordinary window, and fixed so a prompt per line
/// cannot become a second record beside the record. Past it the oldest prompts
/// are let go, so the oldest stretch of a very long session goes unmarked.
const MOST_LANDMARKS: usize = 256;

/// One line of the record, and whether a narrower window may re-fold it.
enum Line {
    /// Text that was written as text. Folded at whatever the window is now.
    Flowed(Row),
    /// One row of a component rebuilt from retained source at each width.
    Responsive {
        /// The display rows at the record's current width.
        rows: Vec<Row>,
        /// A prefix applied to each row after every responsive rebuild.
        prefix: Option<Subordinate>,
        /// How many ordinary record lines this retained source costs.
        weight: usize,
        /// What lays the whole block out at a width.
        lay: Box<dyn Fn(usize) -> Vec<Row>>,
    },
    /// Rows a component laid out against a width. Clipped, never re-folded.
    Set(Row),
}

/// A subordinate block's structural mark and text-column width.
#[derive(Debug, Clone)]
struct Subordinate {
    mark: Box<str>,
    opening: usize,
    /// Whether this prefix contributes the first row's mark.
    marks_first: bool,
    /// Whether a responsive rebuild already contributes that first-row mark.
    keeps_first: bool,
}

impl Subordinate {
    /// The mark on the first row, or its text-column indent after that.
    fn row(&self, first: &mut bool) -> Row {
        let mut row = Row::new();
        if *first && self.marks_first {
            row.push_structural(Slot::Quiet, &*self.mark);
            *first = false;
        } else {
            row.push_structural(Slot::Quiet, " ".repeat(self.opening));
        }
        row.push(Slot::Plain, " ");
        row
    }
}

/// A mark hung on what is written from a line on, before it is written.
///
/// Native mode writes a line out once, and a reply can leave the live region
/// before the command writing it ends. What it is to be hung under is known
/// before the reply starts, so each of its lines is marked as it goes out and
/// [`Record::subordinate`] marks the rest.
#[derive(Debug)]
struct Hanging {
    /// The line the reply starts at, as [`Record::lines`] numbers it.
    from: usize,
    /// The first line not marked yet.
    next: usize,
    mark: Box<str>,
    /// Whether the mark itself is still to be given, rather than the indent.
    first: bool,
}

impl Line {
    /// Its share of the record's retained-memory ceiling.
    fn weight(&self) -> usize {
        match self {
            Self::Responsive { weight, .. } => *weight,
            Self::Flowed(_) | Self::Set(_) => 1,
        }
    }
}

impl fmt::Debug for Line {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Flowed(row) => out.debug_tuple("Flowed").field(row).finish(),
            Self::Responsive { rows, weight, .. } => out
                .debug_struct("Responsive")
                .field("rows", &rows.len())
                .field("weight", weight)
                .finish_non_exhaustive(),
            Self::Set(row) => out.debug_tuple("Set").field(row).finish(),
        }
    }
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
    /// What laid them, kept for as long as they are held, and handed the
    /// width and the glyph set each time it lays them.
    lay: Box<dyn Fn(usize, Glyphs) -> Vec<Row>>,
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
    /// Logical lines, oldest first. A responsive one may draw several rows.
    /// The last flowed line is the one streamed text is appended to.
    lines: VecDeque<Line>,
    /// How many display rows each line folds to at [`Self::columns`].
    ///
    /// One `u16` a line, kept beside the lines rather than worked out per
    /// frame: scrolling asks how tall the record is on every wheel event, and
    /// folding twenty thousand lines to answer it is the shape of slowness this
    /// crate exists to refuse. Emptied whole on a resize, which is the only
    /// thing that can make every answer wrong at once.
    tall: VecDeque<u16>,
    /// The cumulative display-row end of each line at [`Self::columns`].
    ///
    /// Absolute within the current epoch, which a resize or emptying the
    /// record starts afresh: dropping a line advances [`Self::before`] rather
    /// than subtracting from every end, so a seek from the scroll rail can
    /// binary-search this list without work proportional to the record on
    /// either append or spill. An end kept past its epoch places the next
    /// session's lines at the old one's rows, and the rail with them.
    ends: VecDeque<usize>,
    /// Display rows before the first retained line in the current width epoch.
    before: usize,
    /// The width every height above was worked out at.
    columns: usize,
    /// Retained-memory units across the logical lines currently held.
    weight: usize,
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
    /// Prompt boundaries still retained, oldest first, in stable line numbers.
    ///
    /// Fixed independently of the record: the scroll rail needs enough places
    /// to mark across a long transcript, not one allocation for every prompt
    /// in it.
    landmarks: VecDeque<usize>,
    /// The prompt a press on its rail mark last landed on, as one of
    /// [`Self::landmarks`].
    ///
    /// Held here rather than by whoever pressed, because a resize that lays
    /// the opening out again renumbers every landmark under it, and this has
    /// to move with them or it names another prompt. A prompt that has left
    /// the record names nothing, and a prompt sent after it ends the landing:
    /// the prompt being answered is the one read under.
    landed: Option<usize>,
    /// Whether the last line is still being written to.
    ///
    /// A line is open from the first delta that lands in it until the newline
    /// that ends it, and only an open line can change. Held rather than worked
    /// out from what the last line looks like, because a line somebody wrote
    /// nothing on is a blank line and a line nobody has written on yet is not:
    /// the two are the same row and different facts.
    open: bool,
    /// Whether what was let go of last ended in a blank line, for
    /// [`Self::parted`] to answer once nothing is held.
    ///
    /// In native mode the record empties every time the session waits for a
    /// key, but the transcript has not: it is in the terminal, ending in
    /// whatever went out last. A record that answered from what it holds alone
    /// would take every block after that for the first of the session, and
    /// part none of them from the one above.
    parted_before: bool,
    /// The line, by its number counted from the first of the session, that
    /// parts what is above it from what follows although it is not blank.
    ///
    /// A divider: a block that follows it asks for a blank row and is given
    /// none, because the divider is already the space between them.
    parting: Option<usize>,
    /// The mark a reply still being written is hung under, from
    /// [`Self::hangs`] until [`Self::subordinate`] or [`Self::unhangs`].
    hanging: Option<Hanging>,
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
            ends: VecDeque::new(),
            before: 0,
            columns,
            weight: 0,
            rows: 0,
            gone: 0,
            top: Spot { line: 0, into: 0 },
            following: true,
            opening: None,
            landmarks: VecDeque::new(),
            landed: None,
            parted_before: true,
            parting: None,
            open: false,
            hanging: None,
        }
    }

    /// Append streamed text, which flows.
    ///
    /// A `\n` ends the line it is in rather than being drawn, and text with no
    /// `\n` in it continues whatever line is open — which is what makes a delta
    /// arriving mid-word land in the same line as the word it finishes.
    /// `link` is where the run goes when it is clicked, and `None` is the run
    /// that goes nowhere — which is nearly all of them. Carried alongside
    /// rather than written into the text, so a run that wraps stays one address
    /// on both of its rows and a row that is measured is measured in the
    /// columns a reader can see.
    pub(crate) fn write(&mut self, slot: Slot, text: &str, link: Option<&str>) {
        let mut rest = text;
        while let Some(at) = rest.find('\n') {
            self.extend(slot, &rest[..at], link);
            self.close();
            rest = &rest[at + 1..];
        }
        if !rest.is_empty() {
            self.extend(slot, rest, link);
        }
    }

    /// Append rows a component laid out, which are set.
    pub(crate) fn lay(&mut self, rows: impl IntoIterator<Item = Row>) {
        for row in rows {
            self.put(Line::Set(row));
        }
    }

    /// Lay down one block whose width-independent source remains available.
    ///
    /// One logical record line however many display rows the component needs.
    /// Its stable line number therefore survives a resize that changes its
    /// height, and only a viewport covering it clones the rows already laid out.
    pub(crate) fn responsive(&mut self, retained: usize, lay: Box<dyn Fn(usize) -> Vec<Row>>) {
        self.end();
        let rows = responsive_rows(lay(self.columns));
        if !rows.is_empty() {
            let weight = rows.len().max(retained.div_ceil(RETAINED_ROW_BYTES)).max(1);
            self.put(Line::Responsive {
                rows,
                prefix: None,
                weight,
                lay,
            });
        }
    }

    /// Lay down the opening in `glyphs`, keeping what laid it.
    ///
    /// The one block a resize draws again — see the prose at the top of this
    /// file for why this one and nothing else.
    pub(crate) fn opens(&mut self, glyphs: Glyphs, lay: Box<dyn Fn(usize, Glyphs) -> Vec<Row>>) {
        self.end();
        let from = self.gone + self.lines.len();
        let laid = lay(self.columns, glyphs);
        let lines = laid.len();
        self.lay(laid);
        self.opening = Some(Opening { from, lines, lay });
    }

    /// Draw the opening again for `columns` in `glyphs`, in the lines it
    /// already holds.
    ///
    /// Before the heights are worked out again rather than after: this changes
    /// which lines there are, and [`Self::resized`] is what measures them.
    fn relay(&mut self, columns: usize, glyphs: Glyphs) {
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
        let laid = (opening.lay)(columns, glyphs);
        let lines = laid.len();

        for _ in 0..opening.lines {
            self.lines.remove(at);
            self.weight = self.weight.saturating_sub(1);
        }
        for (step, row) in laid.into_iter().enumerate() {
            self.lines.insert(at + step, Line::Set(row));
            self.weight = self.weight.saturating_add(1);
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
        for landmark in self.landmarks.iter_mut().chain(self.landed.as_mut()) {
            if *landmark >= past {
                *landmark = (*landmark + lines).saturating_sub(opening.lines);
            } else if *landmark > opening.from {
                *landmark = opening.from;
            }
        }

        self.opening = Some(Opening { lines, ..opening });
    }

    /// Add `text` to the open line, opening one if none is.
    fn extend(&mut self, slot: Slot, text: &str, link: Option<&str>) {
        if let (true, Some(Line::Flowed(row))) = (self.open, self.lines.back_mut()) {
            Self::push_run(row, slot, text, link);
            self.remeasure();
            return;
        }

        let mut row = Row::new();
        Self::push_run(&mut row, slot, text, link);
        self.put(Line::Flowed(row));
        self.open = true;
    }

    /// Appends one run, with or without somewhere for it to go.
    fn push_run(row: &mut Row, slot: Slot, text: &str, link: Option<&str>) {
        match link {
            Some(link) => row.push_linked(slot, text, link),
            None => row.push(slot, text),
        }
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
        self.weight = self.weight.saturating_add(line.weight());
        self.lines.push_back(line);
        self.tall.push_back(tall);
        let after = self.ends.back().copied().unwrap_or(self.before) + usize::from(tall);
        self.ends.push_back(after);
        self.rows += usize::from(tall);
        self.spill();
    }

    /// Drops oldest logical lines until the record is back under its ceiling.
    fn spill(&mut self) {
        while self.weight > MOST && self.lines.len() > 1 {
            self.drop_oldest();
        }
        self.forget_landmarks();
    }

    /// Drops the oldest line held, counting it as gone.
    fn drop_oldest(&mut self) {
        if let Some(line) = self.lines.pop_front() {
            self.weight = self.weight.saturating_sub(line.weight());
            self.parted_before = Self::blank(&line) || self.parting == Some(self.gone);
        }
        let tall = self.tall.pop_front().unwrap_or(0);
        self.before = self.ends.pop_front().unwrap_or(self.before);
        self.rows -= usize::from(tall);
        self.gone += 1;
    }

    /// Lets go of the prompt landmarks whose lines have gone.
    fn forget_landmarks(&mut self) {
        while self.landmarks.front().is_some_and(|line| *line < self.gone) {
            self.landmarks.pop_front();
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
        if let Some(after) = self.ends.back_mut() {
            *after = after.saturating_sub(usize::from(*was)) + usize::from(now);
        }
        *was = now;
    }

    /// Work out every retained line's height again after rows were rewritten.
    fn remeasure_all(&mut self) {
        self.rows = 0;
        self.tall.clear();
        self.ends.clear();
        let mut after = self.before;
        for line in &self.lines {
            let tall = Self::measure(line, self.columns);
            self.tall.push_back(tall);
            self.rows += usize::from(tall);
            after += usize::from(tall);
            self.ends.push_back(after);
        }
        self.top.into = 0;
    }

    /// How many display rows a line comes to at the current width.
    ///
    /// Saturating rather than wrapping: a line taller than a `u16` is one
    /// nobody can read anyway, and the alternative is arithmetic that is right
    /// until somebody pastes a megabyte.
    fn measure(line: &Line, columns: usize) -> u16 {
        match line {
            Line::Flowed(row) => u16::try_from(row.folds(columns)).unwrap_or(u16::MAX),
            Line::Responsive { rows, .. } => u16::try_from(rows.len()).unwrap_or(u16::MAX),
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
    /// parted from. One holding nothing because it let go of everything it had
    /// ends in whatever it let go of last, which is still above. A line marked
    /// by [`Self::parts`] parts as a blank one does.
    pub(crate) fn parted(&self) -> bool {
        match self.lines.back() {
            None => self.parted_before,
            Some(Line::Flowed(_)) if self.open => false,
            Some(line) => Self::blank(line) || self.parting == self.lines().checked_sub(1),
        }
    }

    /// Marks the last line as one that parts what follows it from what is
    /// above, as a blank line would.
    pub(crate) fn parts(&mut self) {
        if !self.lines.is_empty() {
            self.parting = Some(self.lines() - 1);
        }
    }

    /// Whether the last line is one [`Self::parts`] marked, with nothing open
    /// after it.
    ///
    /// Asked even once that line has gone out and been let go of: the count
    /// of lines goes on including it, so it is still the last.
    pub(crate) fn divided(&self) -> bool {
        !self.open && self.parting.is_some() && self.parting == self.lines().checked_sub(1)
    }

    /// Whether a finished line is a row of nothing.
    fn blank(line: &Line) -> bool {
        match line {
            Line::Flowed(row) | Line::Set(row) => row.text().trim().is_empty(),
            Line::Responsive { rows, .. } => {
                rows.last().is_none_or(|row| row.text().trim().is_empty())
            }
        }
    }

    /// Drop every line, leaving the record empty and the numbering where it was.
    ///
    /// What a session picked up asks for: the transcript it replaces is the
    /// whole of the band, so the lines that were there go rather than being
    /// pushed up out of sight. Nothing else in this process is asked to forget
    /// with it — the numbering carries on past the lines it dropped and one
    /// reset boundary, so a number some other part of the program is holding
    /// still names the line it named, and names nothing once that line has gone.
    ///
    /// The opening goes too, and cannot come back: what a resize redraws is a
    /// card whose lines are still held, and these are not.
    pub(crate) fn empties(&mut self) {
        // Advance past a reset boundary as well as every removed line, so a
        // stable boundary taken before the reset cannot alias the first line of
        // the replacement conversation.
        self.gone += self.lines.len() + 1;
        self.lines.clear();
        self.tall.clear();
        self.ends.clear();
        self.before = 0;
        self.weight = 0;
        self.rows = 0;
        self.top = Spot {
            line: self.gone,
            into: 0,
        };
        self.following = true;
        self.opening = None;
        self.open = false;
        self.parted_before = true;
    }

    /// Drop every line as [`Self::empties`] does, where what was said is still
    /// above what replaces it.
    ///
    /// What emptying means in the terminal's own buffer, which keeps what went
    /// out: the next block is parted from the last row written there, not from
    /// nothing, so whether that row was blank is kept.
    pub(crate) fn empties_under(&mut self) {
        let parted = self.parted();
        self.empties();
        self.parted_before = parted;
    }

    /// How many lines the session has taken, including those since dropped.
    ///
    /// Counted from the first line of the session rather than the first still
    /// held, so a number kept by a caller goes on naming the same line after
    /// the record has spilled underneath it.
    pub(crate) fn lines(&self) -> usize {
        self.gone + self.lines.len()
    }

    /// Hangs the output written since `from` under one structural mark.
    ///
    /// The first retained display row receives the mark. Every row after it is
    /// indented to the same text column, making a multiline answer one block
    /// rather than a list of unrelated results. If the beginning spilled or the
    /// command replaced the transcript, nothing is changed: the retained rows no
    /// longer identify one complete answer block.
    ///
    /// Where [`Self::hangs`] was told of `from` before the output began, the
    /// lines it has already marked are left as they are and the rest are
    /// marked after them.
    pub(crate) fn subordinate(&mut self, from: usize, mark: &str) {
        self.end();
        let (next, mut first) = self
            .hanging
            .take()
            .filter(|hanging| hanging.from == from)
            .map_or((from, true), |hanging| (hanging.next, hanging.first));
        if next < self.gone || next >= self.lines() || mark.is_empty() {
            return;
        }
        self.mark(next..self.lines(), mark, &mut first);
    }

    /// Hangs what is written from here on under `mark`, before it is written.
    ///
    /// For a reply that may leave the record before it ends: each line of it
    /// is marked as [`Self::hangs_through`] lets it go, and
    /// [`Self::subordinate`], given the line this started at, marks the rest.
    /// Emptying the record forgets it.
    pub(crate) fn hangs(&mut self, mark: &str) {
        let from = self.lines();
        self.hanging = (!mark.is_empty()).then(|| Hanging {
            from,
            next: from,
            mark: mark.into(),
            first: true,
        });
    }

    /// Forgets the mark [`Self::hangs`] hung, marking nothing more under it.
    pub(crate) fn unhangs(&mut self) {
        self.hanging = None;
    }

    /// Marks the lines of a hung reply before `through`, which are about to
    /// be let go of.
    ///
    /// Nothing where no reply is hung, and the mark is forgotten where lines
    /// of the reply have already gone unmarked, as [`Self::subordinate`]
    /// forgets a block whose beginning spilled.
    pub(crate) fn hangs_through(&mut self, through: usize) {
        let Some(mut hanging) = self.hanging.take() else {
            return;
        };
        if hanging.next < self.gone {
            return;
        }
        let through = through.min(self.lines());
        if hanging.next < through {
            let mark = std::mem::take(&mut hanging.mark);
            self.mark(hanging.next..through, &mark, &mut hanging.first);
            hanging.mark = mark;
            hanging.next = through;
        }
        self.hanging = Some(hanging);
    }

    /// Puts `mark` on the first row of `lines` still owed it while `first`,
    /// and its indent on every other row; lines of the opening are skipped.
    fn mark(&mut self, lines: Range<usize>, mark: &str, first: &mut bool) {
        let opening = self
            .opening
            .as_ref()
            .map(|opening| opening.from..opening.from + opening.lines);
        let subordinate = Subordinate {
            mark: mark.into(),
            opening: crate::width::columns(mark),
            marks_first: true,
            keeps_first: false,
        };
        let gone = self.gone;

        for (at, line) in self
            .lines
            .iter_mut()
            .enumerate()
            .skip(lines.start - gone)
            .take(lines.len())
        {
            if opening
                .as_ref()
                .is_some_and(|opening| opening.contains(&(gone + at)))
            {
                continue;
            }
            match line {
                Line::Flowed(row) | Line::Set(row) => {
                    if *first && row.starts_structural() {
                        *first = false;
                    } else {
                        row.prepend(subordinate.row(first));
                    }
                }
                Line::Responsive { rows, prefix, .. } => {
                    if *first && rows.first().is_some_and(Row::starts_structural) {
                        *first = false;
                        let mut retained = subordinate.clone();
                        retained.marks_first = false;
                        retained.keeps_first = true;
                        for row in rows.iter_mut().skip(1) {
                            row.prepend(subordinate.row(first));
                        }
                        *prefix = Some(retained);
                        continue;
                    }
                    let mut retained = subordinate.clone();
                    retained.marks_first = *first && !rows.is_empty();
                    retained.keeps_first = false;
                    for row in rows {
                        row.prepend(subordinate.row(first));
                    }
                    *prefix = Some(retained);
                }
            }
        }

        self.remeasure_all();
    }

    /// Edits the rows line `at` was written as, now and at every width it is
    /// laid out at again.
    ///
    /// For a row that has to say less than it did when it was written: an
    /// offer whose result has gone. The line keeps its place and its share of
    /// the record; only its rows change. A line no longer held, or not written
    /// yet, is left alone, and so is every other line.
    pub(crate) fn amend(&mut self, at: usize, edit: fn(&mut [Row])) {
        let Some(line) = at
            .checked_sub(self.gone)
            .and_then(|held| self.lines.get_mut(held))
        else {
            return;
        };

        match line {
            Line::Flowed(row) | Line::Set(row) => edit(std::slice::from_mut(row)),
            Line::Responsive { rows, lay, .. } => {
                edit(rows);
                // Kept with what lays the block out, or the next width would
                // lay it out as it was written.
                let before = std::mem::replace(lay, Box::new(|_| Vec::new()));
                *lay = Box::new(move |columns| {
                    let mut laid = before(columns);
                    edit(&mut laid);
                    laid
                });
            }
        }

        self.remeasure_all();
    }

    /// Marks the next line laid down as the start of a prompt.
    ///
    /// Called after the blank that parts blocks and before the prompt's own
    /// rows. Repeated calls at the same boundary make one landmark: replay and
    /// a live prompt use the same door, and neither owes a double mark.
    pub(crate) fn landmark(&mut self) {
        let line = self.lines();
        if self.landmarks.back() == Some(&line) {
            return;
        }
        self.landmarks.push_back(line);
        self.landed = None;
        while self.landmarks.len() > MOST_LANDMARKS {
            self.landmarks.pop_front();
        }
    }
}

/// What native mode hands over to the terminal's own scrollback.
///
/// There the record holds only what has not gone out yet: a line written into
/// the reader's buffer is the terminal's to keep, and holding it here as well
/// would be a second copy of the session that nothing draws again.
impl Record {
    /// The first line still held, numbered as [`Self::lines`] numbers them.
    pub(crate) fn first(&self) -> usize {
        self.gone
    }

    /// One past the last line that can no longer change: every line held but
    /// the one still being written to.
    pub(crate) fn finished(&self) -> usize {
        self.lines() - usize::from(self.open)
    }

    /// How many display rows the held lines from `line` on come to.
    pub(crate) fn rows_from(&self, line: usize) -> usize {
        self.tall
            .iter()
            .skip(line.saturating_sub(self.gone))
            .map(|tall| usize::from(*tall))
            .sum()
    }

    /// Line `line` as the display rows it comes to now; none where it is not
    /// held.
    pub(crate) fn folded(&self, line: usize) -> Vec<Row> {
        line.checked_sub(self.gone)
            .and_then(|held| self.lines.get(held))
            .map(|held| self.fold(held))
            .unwrap_or_default()
    }

    /// Lets go of every line before `through`, keeping the numbering.
    ///
    /// The same as a spill, which is what it is: a line that has gone keeps its
    /// number, so a caller holding one names nothing rather than another line,
    /// and an opening that has partly gone is no longer laid out again.
    pub(crate) fn lets_go(&mut self, through: usize) {
        while self.gone < through && !self.lines.is_empty() {
            self.drop_oldest();
        }
        self.forget_landmarks();
        if self.top.line < self.gone {
            self.top = Spot {
                line: self.gone,
                into: 0,
            };
        }
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
        // Saturating because the line the reader was on can have spilled since
        // they stopped there, leaving `from` above `gone` — the state the
        // scroll tests pin as intended. Every sibling walking this state does
        // the same.
        for (at, tall) in self
            .tall
            .iter()
            .enumerate()
            .skip(from.line.saturating_sub(self.gone))
        {
            let shown = usize::from(*tall).saturating_sub(skip);
            skip = 0;
            if left < shown {
                return Some(self.gone + at);
            }
            left -= shown;
        }

        None
    }

    /// Which rows of a band `rows` tall are showing the line numbered `at`.
    ///
    /// The other half of [`Record::at`], and worked out the same way: that one
    /// says which line a row is showing, this one says every row showing a
    /// line. Empty for a line the band is not showing.
    ///
    /// What the pair is for is a cut result long enough to wrap. It is one
    /// result however many rows it folded to, so a pointer anywhere on it is
    /// pointing at the whole of it -- and at none of what is above or below.
    pub(crate) fn covering(&self, at: usize, rows: usize) -> Range<usize> {
        let from = if self.following {
            self.foot(rows)
        } else {
            self.top
        };

        let mut start = 0;
        let mut skip = usize::from(from.into);
        for (line, tall) in self
            .tall
            .iter()
            .enumerate()
            .skip(from.line.saturating_sub(self.gone))
        {
            let shown = usize::from(*tall).saturating_sub(skip);
            skip = 0;
            if self.gone + line == at {
                return start..(start + shown).min(rows);
            }
            start += shown;
            if start >= rows {
                break;
            }
        }

        start.min(rows)..start.min(rows)
    }

    /// Whether the line numbered `at` carries a span in `slot`.
    ///
    /// Numbered as [`Record::at`] hands them back, which is what makes the two
    /// a pair: one says which line a window row is showing and this says what
    /// kind of line it is. Asked of the line rather than of the display row it
    /// folded to, so a cut result long enough to wrap is one cut result on
    /// every row of itself.
    ///
    /// `false` for a line the record has since dropped, which is the honest
    /// answer: what is no longer held is not on screen either.
    pub(crate) fn wears(&self, at: usize, slot: Slot) -> bool {
        let Some(line) = at.checked_sub(self.gone).and_then(|at| self.lines.get(at)) else {
            return false;
        };

        match line {
            Line::Flowed(row) | Line::Set(row) => row.kinds().any(|kind| kind == slot),
            Line::Responsive { rows, .. } => {
                rows.iter().any(|row| row.kinds().any(|kind| kind == slot))
            }
        }
    }

    /// The first line of the run of cut lines `at` is in, or `at` itself where
    /// it is not in one.
    ///
    /// A result written down over several lines is one result, and the offer to
    /// open it was made on the first of them. Nothing else is written down in
    /// the middle of a result, so the run of lines wearing the cut slot around
    /// `at` is that result and no other — the same reading the pointer's light
    /// makes, so what lights under a click is what the click opens.
    pub(crate) fn heads(&self, at: usize) -> usize {
        if !self.wears(at, Slot::Cut) {
            return at;
        }

        let mut first = at;
        while first > self.gone && self.wears(first - 1, Slot::Cut) {
            first -= 1;
        }
        first
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

    /// The absolute display row shown on the first row of a band `rows` tall.
    ///
    /// What a selection's ends are named in, so that scrolling the band moves
    /// the highlight with the words rather than leaving it over the rows.
    pub(crate) fn top_row(&self, rows: usize) -> usize {
        let from = if self.following {
            self.foot(rows)
        } else {
            self.top
        };
        self.row_of(from)
    }

    /// The last absolute display row the record has, if it has any.
    pub(crate) fn last_row(&self) -> Option<usize> {
        self.ends
            .back()
            .and_then(|end| end.checked_sub(1))
            .filter(|last| *last >= self.before)
    }

    /// The display row at absolute position `row`, if the record still holds
    /// it.
    ///
    /// Folded on demand, for the rows a selection reached that the band is
    /// not showing: a drag that scrolled past them still took them.
    pub(crate) fn row_at(&self, row: usize) -> Option<Row> {
        if row < self.before || self.last_row().is_none_or(|last| row > last) {
            return None;
        }
        let spot = self.spot_at(row);
        let line = self.lines.get(spot.line.checked_sub(self.gone)?)?;
        self.fold(line).into_iter().nth(usize::from(spot.into))
    }

    /// Where a band `rows` tall stands in what the record retains, in display
    /// rows: what the scroll rail is laid out from.
    ///
    /// The total is kept, and the top is one lookup in the cumulative ends
    /// beside the line heights — or, while the band follows the foot, a walk
    /// back over the lines the band shows — so a rail drawn on every frame
    /// costs the band's height at most, never the record's or the session's.
    pub(crate) fn place(&self, rows: usize) -> Place {
        Place {
            total: self.rows,
            top: self.top_row(rows).saturating_sub(self.before),
            height: rows,
        }
    }

    /// Where each prompt still retained starts, in display rows into what is
    /// retained, oldest first.
    ///
    /// The walk is capped by [`MOST_LANDMARKS`], and a prompt whose line has
    /// spilled off the head leaves with it.
    pub(crate) fn prompts(&self) -> impl Iterator<Item = usize> + '_ {
        self.landmarked().map(|(_, row)| row)
    }

    /// Each prompt still retained, oldest first, as its stable line number
    /// beside where it starts, in display rows into what is retained.
    ///
    /// What holds a prompt across scrolling and spills, where its row moves
    /// and its line number does not. A resize that lays the opening out again
    /// renumbers every line under it, landmarks and [`Self::landed`] with
    /// them, so only a number read back from here since the last resize is
    /// one. Capped as [`Self::prompts`] is.
    fn landmarked(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.landmarks.iter().filter_map(|line| {
            let row = self.start_of(*line)?;
            Some((*line, row.saturating_sub(self.before)))
        })
    }

    /// Lands on the prompt that starts at display row `row` of what is
    /// retained, so the scroll rail holds it as current; a row no prompt
    /// starts on lands on nothing.
    pub(crate) fn lands(&mut self, row: usize) {
        let landed = self
            .landmarked()
            .find(|(_, start)| *start == row)
            .map(|(line, _)| line);
        self.landed = landed;
    }

    /// Where the prompt last landed on starts, in display rows into what is
    /// retained, while it is still retained.
    pub(crate) fn landed(&self) -> Option<usize> {
        let line = self.landed?;
        self.landmarked()
            .find(|(landmark, _)| *landmark == line)
            .map(|(_, row)| row)
    }

    /// Moves the band's top to display row `row` of what is retained, and
    /// says whether it moved.
    ///
    /// Never past the foot: a row the band cannot start on and still be full
    /// is the foot, and the band follows the record again from there, as the
    /// wheel leaves it when it reaches the end. A binary search through the
    /// same cumulative ends [`Self::place`] reads, plus a walk bounded by the
    /// band's height to find the foot.
    pub(crate) fn seek(&mut self, row: usize, rows: usize) -> bool {
        let foot = self.foot(rows);
        let now = self.spot_at(self.before.saturating_add(row)).min(foot);
        let was = if self.following { foot } else { self.top };
        self.top = now;
        self.following = now == foot;
        now != was
    }

    /// The display-row position of `spot` in this width epoch.
    fn row_of(&self, spot: Spot) -> usize {
        let at = spot.line.saturating_sub(self.gone);
        let before = at
            .checked_sub(1)
            .and_then(|line| self.ends.get(line).copied())
            .unwrap_or(self.before);
        before + usize::from(spot.into)
    }

    /// The spot at absolute display-row position `row`.
    fn spot_at(&self, row: usize) -> Spot {
        let row = row.clamp(
            self.before,
            self.ends.back().copied().unwrap_or(self.before),
        );
        let mut low = 0;
        let mut high = self.ends.len();
        while low < high {
            let middle = low + (high - low) / 2;
            if self.ends.get(middle).is_some_and(|end| *end <= row) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let at = low.min(self.tall.len().saturating_sub(1));
        let before = at
            .checked_sub(1)
            .and_then(|line| self.ends.get(line).copied())
            .unwrap_or(self.before);
        let into = row.saturating_sub(before);
        Spot {
            line: self.gone + at,
            into: u16::try_from(into).unwrap_or(u16::MAX),
        }
    }

    /// The display-row position where stable `line` starts.
    fn start_of(&self, line: usize) -> Option<usize> {
        let at = line.checked_sub(self.gone)?;
        if at >= self.lines.len() {
            return None;
        }
        Some(
            at.checked_sub(1)
                .and_then(|line| self.ends.get(line).copied())
                .unwrap_or(self.before),
        )
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

    /// Lay the record out for a window of a different width, with the opening
    /// drawn again in `glyphs`.
    ///
    /// Every height is wrong at once, so every height is worked out again —
    /// and the spot keeps its line and loses its offset into it, because the
    /// row that was third of five in a line is not the third of two.
    pub(crate) fn resized(&mut self, columns: usize, glyphs: Glyphs) {
        if columns == self.columns {
            return;
        }
        self.columns = columns;
        self.relay(columns, glyphs);
        self.relay_responsive(columns);
        self.rows = 0;
        self.before = 0;
        self.tall.clear();
        self.ends.clear();
        for line in &self.lines {
            let tall = Self::measure(line, columns);
            self.tall.push_back(tall);
            self.rows += usize::from(tall);
            self.ends.push_back(self.rows);
        }
        self.spill();
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

    /// Rebuilds responsive blocks once for the new width.
    fn relay_responsive(&mut self, columns: usize) {
        for line in &mut self.lines {
            if let Line::Responsive {
                rows, prefix, lay, ..
            } = line
            {
                *rows = responsive_rows(lay(columns));
                if let Some(prefix) = prefix {
                    let mut first = prefix.marks_first;
                    for (at, row) in rows.iter_mut().enumerate() {
                        if !(prefix.keeps_first && at == 0) {
                            row.prepend(prefix.row(&mut first));
                        }
                    }
                }
            }
        }
    }

    /// A line as the display rows it comes to at the current width.
    fn fold(&self, line: &Line) -> Vec<Row> {
        match line {
            Line::Flowed(row) => row.fold(self.columns),
            Line::Responsive { rows, .. } => rows.clone(),
            Line::Set(row) => vec![row.clipped(self.columns)],
        }
    }
}

/// Caps one responsive layout to the record's existing display-row ceiling.
fn responsive_rows(mut rows: Vec<Row>) -> Vec<Row> {
    rows.truncate(MOST);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record holding `lines` numbered lines, each one flowed.
    fn filled(columns: usize, lines: usize) -> Record {
        let mut record = Record::new(columns);
        for line in 0..lines {
            record.write(Slot::Plain, &format!("{line}\n"), None);
        }
        record
    }

    /// What the view says, as plain text.
    fn said(record: &Record, rows: usize) -> Vec<String> {
        record.view(rows).iter().map(Row::text).collect()
    }

    /// A source that lays one row per column of the window, so what width it
    /// was called at can be read straight off what it laid.
    fn ruler() -> Box<dyn Fn(usize, Glyphs) -> Vec<Row>> {
        Box::new(|columns, _| {
            (0..columns)
                .map(|row| Row::new().then(Slot::Plain, format!("{row}")))
                .collect()
        })
    }

    #[test]
    fn the_rows_covering_a_line_are_exactly_the_rows_showing_it() {
        // The pair this makes with [`Record::at`]: one says which line a row is
        // showing and the other every row showing that line, so a pointer on
        // any row of a wrapped line is pointing at the whole of it.
        let mut record = Record::new(8);
        record.write(Slot::Plain, "one\n", None);
        record.write(Slot::Plain, "a line that takes several rows\n", None);
        record.write(Slot::Plain, "last\n", None);

        let rows = 12;
        for row in 0..rows {
            let Some(line) = record.at(row, rows) else {
                continue;
            };
            assert!(
                record.covering(line, rows).contains(&row),
                "row {row} shows line {line} and is not among the rows covering it"
            );
        }

        // And something here really did fold, or the walk above would hold just
        // as well for a record that never wrapped a line in its life.
        let wrapped = record.at(1, rows).expect("a second row");
        assert!(record.covering(wrapped, rows).len() > 1);
    }

    #[test]
    fn a_line_the_band_is_not_showing_is_covered_by_no_row_of_it() {
        let record = filled(8, 40);

        assert!(record.covering(0, 5).is_empty());
    }

    #[test]
    fn a_responsive_block_changes_height_without_renumbering_later_lines() {
        let mut record = Record::new(8);
        record.responsive(
            32,
            Box::new(|columns| {
                let rows = if columns < 6 { 3 } else { 1 };
                (0..rows)
                    .map(|row| Row::plain(format!("responsive {row}")))
                    .collect()
            }),
        );
        record.landmark();
        record.write(Slot::Plain, "after\n", None);
        let after = record.landmarks.front().copied().expect("a landmark");
        assert_eq!(after, 1, "one responsive block is one logical line");

        record.resized(5, Glyphs::Unicode);

        assert_eq!(record.landmarks.front().copied(), Some(after));
        assert_eq!(record.start_of(after), Some(3));
        assert_eq!(record.lines(), 2);
    }

    #[test]
    fn responsive_source_pays_into_the_same_bounded_record() {
        let mut record = Record::new(40);
        record.responsive(
            RETAINED_ROW_BYTES * (MOST - 1),
            Box::new(|_| vec![Row::plain("large retained source")]),
        );
        record.write(Slot::Plain, "newest\n", None);

        assert_eq!(record.weight, MOST);
        assert_eq!(record.lines.len(), 2);

        record.write(Slot::Plain, "one more\n", None);

        assert_eq!(record.weight, 2);
        assert_eq!(record.lines.len(), 2);
        assert_eq!(said(&record, 2), ["newest", "one more"]);
    }

    #[test]
    fn a_record_that_was_emptied_has_nothing_left_of_what_it_held() {
        let mut record = Record::new(8);
        record.opens(Glyphs::Unicode, ruler());
        record.write(Slot::Plain, "said\n", None);

        record.empties();

        assert!(said(&record, 8).is_empty());

        // The card goes with the lines and does not come back: what a resize
        // lays out again is an opening whose lines are still held, and these
        // are not.
        record.resized(5, Glyphs::Unicode);
        assert!(said(&record, 8).is_empty());
    }

    #[test]
    fn a_subordinate_block_gets_one_mark_and_aligned_continuations() {
        let mut record = Record::new(80);
        let from = record.lines();
        record.write(Slot::Plain, "first\nsecond\n", None);

        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["⎿ first", "  second"]);
        let one = || std::iter::once(0..1).collect::<Vec<_>>();
        assert_eq!(
            record
                .view(8)
                .iter()
                .map(Row::structural)
                .collect::<Vec<_>>(),
            [one(), one()]
        );
    }

    #[test]
    fn a_result_already_starting_with_structural_art_gets_no_second_mark() {
        let mut record = Record::new(80);
        let from = record.lines();
        record.lay([Row::new()
            .then_structural(Slot::Trouble, "⎿")
            .then(Slot::Trouble, " failed")]);

        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["⎿ failed"]);
    }

    #[test]
    fn a_premarked_responsive_result_aligns_later_rows_after_resize() {
        let mut record = Record::new(8);
        let from = record.lines();
        record.responsive(
            8,
            Box::new(|columns| {
                vec![
                    Row::new()
                        .then_structural(Slot::Trouble, "⎿")
                        .then(Slot::Trouble, format!(" failed at {columns}")),
                    Row::new().then(Slot::Trouble, "details"),
                ]
            }),
        );

        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["⎿ failed at 8", "  details"]);
        record.resized(12, Glyphs::Unicode);
        assert_eq!(said(&record, 8), ["⎿ failed at 12", "  details"]);
    }

    #[test]
    fn literal_art_at_the_start_of_a_result_still_gets_the_structural_mark() {
        let mut record = Record::new(80);
        let from = record.lines();
        record.write(Slot::Plain, "⎿ literal\n", None);

        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["⎿ ⎿ literal"]);
    }

    #[test]
    fn a_subordinate_block_that_starts_after_old_rows_leaves_them_alone() {
        let mut record = Record::new(80);
        record.write(Slot::Plain, "before\n", None);
        let from = record.lines();
        record.write(Slot::Plain, "answer\n", None);

        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["before", "⎿ answer"]);
    }

    #[test]
    fn a_subordinate_block_keeps_one_mark_after_a_responsive_resize() {
        let mut record = Record::new(8);
        let from = record.lines();
        record.responsive(
            8,
            Box::new(|columns| vec![Row::plain(format!("at {columns}"))]),
        );
        record.write(Slot::Plain, "after\n", None);
        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["⎿ at 8", "  after"]);
        record.resized(12, Glyphs::Unicode);
        assert_eq!(said(&record, 8), ["⎿ at 12", "  after"]);
    }

    #[test]
    fn an_empty_command_answer_adds_no_mark() {
        let mut record = Record::new(80);
        let from = record.lines();

        record.subordinate(from, "⎿");

        assert!(said(&record, 8).is_empty());
    }

    #[test]
    fn a_command_that_emptied_the_record_does_not_mark_the_replacement() {
        let mut record = Record::new(80);
        let from = record.lines();
        record.empties();
        record.write(Slot::Plain, "new opening\n", None);

        record.subordinate(from, "⎿");

        assert_eq!(said(&record, 8), ["new opening"]);
    }

    #[test]
    fn the_numbering_carries_on_past_the_lines_a_record_dropped() {
        let mut record = filled(8, 6);
        let numbered = record.lines();

        record.empties();
        assert_eq!(record.lines(), numbered + 1);

        // So a number some other part of the program is holding names the line
        // it named, and names nothing once that line has gone — rather than
        // quietly naming whatever was written in its place. The extra number is
        // the reset boundary itself.
        record.write(Slot::Plain, "after\n", None);
        assert_eq!(record.lines(), numbered + 2);
        assert_eq!(said(&record, 8), ["after"]);
    }

    #[test]
    fn the_opening_is_laid_out_again_when_the_window_changes() {
        let mut record = Record::new(8);
        record.opens(Glyphs::Unicode, ruler());
        record.write(Slot::Plain, "after\n", None);

        record.resized(5, Glyphs::Unicode);

        let laid: Vec<String> = record.lines.iter().map(measured).collect();
        assert_eq!(laid, ["0", "1", "2", "3", "4", "after"]);
    }

    #[test]
    fn a_card_that_changes_height_leaves_the_reader_on_the_line_they_were_on() {
        let mut record = Record::new(8);
        record.opens(Glyphs::Unicode, ruler());
        for line in 0..6 {
            record.write(Slot::Plain, &format!("said {line}\n"), None);
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
        record.resized(6, Glyphs::Unicode);

        assert_eq!(said(&record, 4), reading);
    }

    #[test]
    fn a_card_that_changes_height_keeps_prompt_landmarks_on_their_prompts() {
        let mut record = Record::new(8);
        record.opens(Glyphs::Unicode, ruler());
        record.landmark();
        record.write(Slot::Plain, "the prompt\n", None);
        let before = record.landmarks.front().copied().expect("a landmark");

        record.resized(5, Glyphs::Unicode);

        let after = record.landmarks.front().copied().expect("the landmark");
        assert_eq!(after, before - 3);
        assert_eq!(record.start_of(after), Some(5));
    }

    #[test]
    fn a_reader_inside_a_card_that_was_laid_out_again_is_left_at_the_top_of_it() {
        let mut record = Record::new(8);
        record.opens(Glyphs::Unicode, ruler());
        for line in 0..6 {
            record.write(Slot::Plain, &format!("said {line}\n"), None);
        }

        // Four rows into the card, which is a row the shorter card does not
        // have: eight rows became five. There is no line to be left on, so the
        // reader is left on the thing they were reading rather than on a
        // number that used to be inside it.
        record.scroll(-6, 4);
        assert_eq!(said(&record, 1), ["4"]);

        record.resized(5, Glyphs::Unicode);

        assert_eq!(said(&record, 1), ["0"]);
    }

    /// The text of one line, whichever kind it is.
    fn measured(line: &Line) -> String {
        match line {
            Line::Flowed(row) | Line::Set(row) => row.text(),
            Line::Responsive { rows, .. } => rows.iter().map(Row::text).collect(),
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
        record.write(Slot::Plain, "a word that will not fit in ten\n", None);
        record.write(Slot::Plain, "short\n", None);
        record.lay([set("a set row that is far wider than ten columns")]);
        record.write(Slot::Accent, "and more", None);

        assert_eq!(record.rows, counted(&record));
        let tall: usize = record.tall.iter().map(|&t| usize::from(t)).sum();
        assert_eq!(record.rows, tall);
    }

    #[test]
    fn a_line_that_grows_past_the_width_grows_the_record_with_it() {
        let mut record = Record::new(10);
        record.write(Slot::Plain, "one", None);
        assert_eq!(record.rows, 1);

        // The same line, now too long for one row: the height kept beside it
        // has to be worked out again, or the record is a row short for the
        // rest of the session.
        record.write(Slot::Plain, " two three four five", None);

        assert!(record.rows > 1);
        assert_eq!(record.rows, counted(&record));
    }

    #[test]
    fn a_delta_that_stops_mid_word_lands_in_the_line_the_word_is_in() {
        let mut record = Record::new(40);
        record.write(Slot::Plain, "hel", None);
        record.write(Slot::Plain, "lo wor", None);
        record.write(Slot::Plain, "ld", None);

        assert_eq!(said(&record, 4), ["hello world"]);
    }

    #[test]
    fn a_newline_ends_the_line_it_is_in_and_is_never_drawn() {
        let mut record = Record::new(40);
        record.write(Slot::Plain, "one\ntwo\n", None);

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

    /// A record ten columns wide whose first line folds to several rows.
    fn folded() -> Record {
        let mut record = Record::new(10);
        record.write(
            Slot::Plain,
            "one two three four five six seven eight nine ten\n",
            None,
        );
        record.landmark();
        record.write(Slot::Plain, "short\n", None);
        record.write(Slot::Plain, "another short\n", None);
        record
    }

    #[test]
    fn the_rail_place_is_measured_in_folded_display_rows() {
        // A line-count measure would say three lines and a prompt on the
        // second; what the rail stands for is the rows those lines fold to.
        let record = folded();
        let rows = record.rows;
        assert!(rows > 3, "{rows}");

        assert_eq!(
            record.place(2),
            Place {
                total: rows,
                top: rows - 2,
                height: 2,
            }
        );
        assert_eq!(record.prompts().collect::<Vec<_>>(), [rows - 3]);
    }

    #[test]
    fn a_rail_seek_lands_on_the_display_row_asked_for_and_follows_at_the_foot() {
        let mut record = folded();
        let rows = record.rows;

        // Into the long first line, which a line-count seek could not reach.
        assert!(record.seek(1, 1));
        assert!(!record.following());
        assert_eq!(record.place(1).top, 1);
        assert_eq!(said(&record, 1), ["three four"]);

        // Past the foot is the foot, and the band follows it again.
        assert!(record.seek(rows + 5, 1));
        assert!(record.following());
        assert_eq!(record.place(1).top, rows - 1);
        assert!(!record.seek(rows - 1, 1));
    }

    #[test]
    fn rail_prompts_leave_with_the_lines_they_started() {
        let mut record = Record::new(40);
        for line in 0..MOST + 10 {
            if line % 5 == 0 {
                record.landmark();
            }
            record.write(Slot::Plain, &format!("line {line}\n"), None);
        }
        let prompts: Vec<usize> = record.prompts().collect();

        assert!(!prompts.is_empty());
        assert!(prompts.iter().all(|row| *row < record.rows));
        assert_eq!(prompts.last().copied(), Some(record.rows - 5));
    }

    #[test]
    fn prompt_landmarks_are_bounded_and_old_ones_leave_with_the_record() {
        let mut record = Record::new(40);
        for line in 0..MOST + MOST_LANDMARKS * 2 {
            record.landmark();
            record.write(Slot::Plain, &format!("line {line}\n"), None);
        }

        assert_eq!(record.landmarks.len(), MOST_LANDMARKS);
        assert!(record.landmarks.iter().all(|line| *line >= record.gone));
    }

    #[test]
    fn cumulative_row_ends_stay_aligned_as_lines_grow_and_spill() {
        let mut record = Record::new(10);
        record.write(Slot::Plain, "one", None);
        record.write(Slot::Plain, " two three four five", None);
        for line in 0..MOST + 20 {
            record.write(Slot::Plain, &format!("line {line}\n"), None);
        }

        assert_eq!(record.ends.len(), record.lines.len());
        assert_eq!(
            record.ends.back().copied().unwrap_or(record.before) - record.before,
            record.rows
        );
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
                None,
            );
        }
        record.scroll(-6, 2);
        let was = record.top.line;

        // Partway into a line, which is the case a resize invalidates: the row
        // that was fourth of five in a line is not the fourth of two.
        assert!(record.top.into > 0);

        record.resized(18, Glyphs::Unicode);

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
            record.write(Slot::Plain, &format!("more {line}\n"), None);
        }

        // The line they were on is gone. The head of what is left is the
        // closest thing to where they were looking, and it is not a panic.
        assert!(record.top.line < record.gone);
        assert_eq!(said(&record, 1), [format!("more {}", record.gone - 10)]);
    }

    #[test]
    fn a_pointer_resting_on_a_band_whose_line_has_spilled_still_answers() {
        // The defect this catches: `at` subtracted `gone` from the top line
        // unchecked, while every sibling walking the same state saturates. The
        // state where the top line has spilled is the one the test above pins
        // as intended, and a pointer resting on the band reaches `at` every
        // frame — a panic in debug, an empty answer in release.
        let mut record = filled(40, 10);
        record.scroll(-100, 3);

        for line in 0..MOST + 100 {
            record.write(Slot::Plain, &format!("more {line}\n"), None);
        }
        assert!(record.top.line < record.gone);

        assert_eq!(record.at(0, 3), Some(record.gone));
    }

    /// A row of `text`, laid out rather than flowed.
    fn set(text: &str) -> Row {
        let mut row = Row::new();
        row.push(Slot::Plain, text);
        row
    }
}
