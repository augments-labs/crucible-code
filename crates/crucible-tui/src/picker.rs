//! A picker: a search line, a two-line list beside a live preview, and keys.
//!
//! Like [`Shelf`](crate::Shelf) it **fills** the room it is handed, because the
//! band it stands in has no share and takes what it asks for. Where the room
//! shortens, the preview gives its rows up before the list gives any of its
//! own — what the reader is here to do is pick, and picking needs the list.
//! Short of the rows one entry needs it answers with nothing at all, which a
//! caller reads as *there was no room to stand one*.
//!
//! It is handed strings and pre-drawn rows and knows no domain type. What
//! narrowed the query, what the preview holds, and what the metadata line says
//! are all decided before they arrive. Nothing here names a colour: every span
//! asks for a [`Slot`] and the palette settles what one is worth.
//!
//! The preview is drawn from the *end* of the rows it was handed: what a
//! reader opens a session to learn is how it finished, so the tail is what the
//! pane shows when there is more than fits. A caller scrolls it by handing a
//! shorter slice, never by telling this component a position it would have to
//! keep.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::render::Caret;
use crate::row::Row;
use crate::width::{clip, columns as wide};

/// The rows a picker spends on everything that is not a row of the split.
///
/// The three the search line's frame costs, the heading under it, its blank,
/// the blank over the keys, and the keys.
const CHROME: usize = 7;

/// The fewest body rows a picker stands in: one entry of the list.
///
/// Below it there is nothing to pick, and a picker with nothing to pick is not
/// a picker. The preview has already surrendered by this point.
const FLOOR: usize = 2;

/// What the preview's anchored foot costs: the rule, the metadata line, and
/// the line naming what Enter and Esc do.
const FOOTED: usize = 3;

/// The row the search line is on, counted from the top of what `within`
/// answered: the frame's own top.
const SEARCHING: usize = 1;

/// Where the search line opens, counted from the left of the window.
const TYPED_AT: usize = 10;

/// The first body row, counted the same way: the search frame's three rows,
/// the heading, and its blank.
const LISTED: usize = 5;

/// What the marks in front of a list row cost: a space, the mark, a space.
const LEADING: usize = 3;

/// One session offered on the picker.
///
/// Everything already in the reader's words: how old it is is a phrase rather
/// than a timestamp, and a branch nothing recorded is an empty string rather
/// than a sentinel.
#[derive(Debug, Clone, Copy)]
pub struct Kept<'a> {
    /// What it is called, in whatever spelling the reader is shown it in.
    pub title: &'a str,
    /// How long ago it began, as words.
    pub when: &'a str,
    /// The branch it was recorded on, or empty where nothing was.
    pub branch: &'a str,
}

/// What a place on the picker answers to, for a caller that has to act on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Nothing the picker lights up.
    Nothing,
    /// The line the query is typed into, or either rule of its frame.
    Search,
    /// A session, by its place in the slice the picker was handed. Both of an
    /// entry's rows answer with it — the pair is one thing to the reader.
    Session(usize),
    /// The preview pane, which is what the wheel scrolls.
    Preview,
}

/// What the pointer is resting on, in the picker's own rows.
///
/// [`Hit`] is the public reading in slice terms; this one is in pane rows,
/// before the scroll has been counted back out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Under {
    /// Nothing the picker lights up.
    Nothing,
    /// The line the query is typed into, or either rule of its frame.
    Searching,
    /// A row of the list, counted from the first row inside it.
    Listed(usize),
    /// The preview pane.
    Previewing,
}

/// The whole picker, as one borrowing view.
#[derive(Debug, Clone, Copy)]
pub struct Picker<'a> {
    /// The line under the search field: what this is, how many the query
    /// left of how many there are, and where — already joined by the caller.
    pub heading: &'a str,
    /// What has been typed into the search line.
    pub query: &'a str,
    /// The caret's place within `query` — or within `renaming`, while there is
    /// one — counted in characters.
    pub typed: usize,
    /// What the search line says when `query` is empty.
    pub hint: &'a str,
    /// The sessions the query left, in the order the list walks them.
    pub sessions: &'a [Kept<'a>],
    /// Which of them the mark is on.
    pub marked: usize,
    /// The marked title mid-rename, standing where the title was. `None` is a
    /// list that is only being walked.
    pub renaming: Option<&'a str>,
    /// The tail of the marked session, already drawn. The pane shows the end
    /// of this slice; a caller scrolls by handing a shorter one.
    pub preview: &'a [Row],
    /// The line under the preview's rule: age, count and branch, and whatever
    /// else is true of the marked session right now.
    pub preview_meta: &'a str,
    /// What Enter and Esc do, said under the metadata.
    pub takes: &'a str,
    /// What the list says where `sessions` is empty.
    pub nothing: &'a str,
    /// What the preview says where `sessions` is empty and a query did it.
    pub noview: &'a str,
    /// The keys row, and the short form for a narrow window.
    pub keys: (&'a str, &'a str),
    /// Where the pointer is resting: a row of what [`Picker::within`] answered,
    /// and a column of the window. `None` is a pointer never reported.
    pub pointer: Option<(usize, usize)>,
}

impl Picker<'_> {
    /// The width at which the preview stops being a pane.
    ///
    /// Below it the split leaves neither side room for a sentence, so the
    /// preview folds away and the list takes every column.
    pub const FOLDS_AT: usize = 70;

    /// The narrowest window a picker is drawn in at all.
    ///
    /// Under this the search line has no room left for what was typed into it,
    /// and a search line that cannot show the query is the one row here that
    /// has to.
    pub const NARROWEST: usize = 24;

    /// Every row of the picker, filled to `room`.
    ///
    /// Nothing at all where the window is too narrow or the room too short for
    /// one entry — the caller's answer to that is its own.
    #[must_use]
    pub fn within(&self, columns: usize, room: usize, glyphs: Glyphs) -> Vec<Row> {
        if columns < Self::NARROWEST || room < CHROME + FLOOR {
            return Vec::new();
        }
        let body = room - CHROME;

        // Worked out once and handed down: the frame's colour and the band
        // under a pair are one answer about where the pointer is, and asking
        // twice is how two rows of one picture come to disagree.
        let under = self.under(columns, body);

        let mut rows = Vec::with_capacity(room);
        rows.extend(self.searched(columns, glyphs, under));
        let mut heading = Row::new();
        heading.push(Slot::Plain, " ");
        heading.push(Slot::Quiet, clip(self.heading, columns.saturating_sub(2)));
        rows.push(heading.clipped(columns));
        rows.push(Row::new());
        rows.extend(self.split(columns, body, glyphs, under));
        rows.push(Row::new());
        rows.push(self.keyed(columns));
        rows
    }

    /// The framed line the query is typed into.
    ///
    /// The one frame here that changes colour, and for the one reason a reader
    /// would want it to: a field is a thing to put a pointer in, and the accent
    /// under the pointer says this is one. The split's divider stays quiet — it
    /// parts the picture rather than offers anything.
    fn searched(&self, columns: usize, glyphs: Glyphs, under: Under) -> [Row; 3] {
        let frame = if under == Under::Searching {
            Slot::Accent
        } else {
            Slot::Quiet
        };

        let mut line = Row::new();
        line.push(frame, glyphs.vertical());
        line.push(Slot::Quiet, " Search");

        let room = columns - TYPED_AT - 1;
        line.pad(TYPED_AT);
        if self.query.is_empty() {
            // Two columns on: the one the cursor is standing in, and one to
            // part it from the words. A hint drawn under the cursor is a line
            // that looks like it already has something typed into it.
            line.pad(TYPED_AT + 2);
            line.push(Slot::Quiet, clip(self.hint, room - 2));
        } else {
            line.push(Slot::Plain, clip(self.query, room));
        }
        line.pad(columns - 1);
        line.push(frame, glyphs.vertical());

        [
            ruled(glyphs.top(), columns - 2, glyphs, frame),
            line.clipped(columns),
            ruled(glyphs.bottom(), columns - 2, glyphs, frame),
        ]
    }

    /// The list beside the preview, or the list alone where the window folded
    /// the preview away.
    ///
    /// An empty list splits only while a query stands: the query is the reason
    /// it emptied, so both sides say so. A workspace that never recorded a
    /// session has no split to draw at all.
    fn split(&self, columns: usize, body: usize, glyphs: Glyphs, under: Under) -> Vec<Row> {
        let apart = self.apart(columns);
        let list = if apart { 2 * columns / 5 } else { columns };

        let entries = self.listed(list, body, glyphs, under);
        if !apart {
            return entries;
        }

        let inside = columns - list - 2;
        let tailed = self.previewed(inside, body, glyphs);
        entries
            .into_iter()
            .zip(tailed)
            .map(|(entry, shown)| {
                let mut row = entry;
                row.pad(list);
                row.push(Slot::Quiet, glyphs.vertical());
                row.push(Slot::Plain, " ");
                row.join(shown).clipped(columns)
            })
            .collect()
    }

    /// Each row of the list, two to a session, padded to `body` rows.
    ///
    /// The band under the pointer covers both rows of a pair — the pair is one
    /// thing to the reader, and half a band would say it is two.
    fn listed(&self, list: usize, body: usize, glyphs: Glyphs, under: Under) -> Vec<Row> {
        if self.sessions.is_empty() {
            return said_instead(clip(self.nothing, list.saturating_sub(2)), body);
        }
        let pairs = body / 2;
        let from = scrolled(self.marked, pairs, self.sessions.len());
        let word = list.saturating_sub(LEADING + 1);

        (0..body)
            .map(|at| {
                let on = matches!(under, Under::Listed(over) if over / 2 == at / 2)
                    && self.sessions.get(from + at / 2).is_some();
                match self.sessions.get(from + at / 2) {
                    Some(one) if at % 2 == 0 => {
                        let marked = from + at / 2 == self.marked;
                        let title = if marked {
                            self.renaming.unwrap_or(one.title)
                        } else {
                            one.title
                        };
                        let mut row = Row::new();
                        row.push(lit(on, Slot::Plain), " ");
                        row.push(
                            lit(on, Slot::Accent),
                            if marked { glyphs.caret() } else { " " }.to_owned(),
                        );
                        row.push(lit(on, Slot::Plain), " ");
                        row.push(
                            lit(on, if marked { Slot::Strong } else { Slot::Plain }),
                            clip(title, word),
                        );
                        row.fill(lit(on, Slot::Plain), list);
                        row
                    }
                    Some(one) => {
                        let mut row = Row::new();
                        row.fill(lit(on, Slot::Plain), LEADING);
                        let aged = if one.branch.is_empty() {
                            one.when.to_owned()
                        } else {
                            format!("{} {} {}", one.when, glyphs.dot(), one.branch)
                        };
                        row.push(lit(on, Slot::Quiet), clip(&aged, word).to_owned());
                        row.fill(lit(on, Slot::Plain), list);
                        row
                    }
                    None => Row::new(),
                }
            })
            .collect()
    }

    /// Each row of the preview pane: the tail of what was handed, and the
    /// anchored foot — the rule, the metadata, and what Enter and Esc do.
    ///
    /// With no session marked there is no tail and nothing for Enter to take,
    /// so the pane says so once and the foot stays undrawn.
    fn previewed(&self, inside: usize, body: usize, glyphs: Glyphs) -> Vec<Row> {
        if self.sessions.is_empty() {
            return said_instead(clip(self.noview, inside), body);
        }
        let area = body.saturating_sub(FOOTED);
        let shown = self.preview.len().min(area);
        let from = self.preview.len() - shown;

        (0..body)
            .map(|at| {
                if at < shown {
                    self.preview
                        .get(from + at)
                        .cloned()
                        .unwrap_or_default()
                        .clipped(inside)
                } else if at + FOOTED == body {
                    Row::new().then(Slot::Quiet, glyphs.horizontal().repeat(inside))
                } else if at + FOOTED == body + 1 {
                    Row::new().then(Slot::Quiet, clip(self.preview_meta, inside))
                } else if at + FOOTED == body + 2 {
                    Row::new().then(Slot::Quiet, clip(self.takes, inside))
                } else {
                    Row::new()
                }
            })
            .collect()
    }

    /// What the keys do, in the longest form the window has room for.
    fn keyed(&self, columns: usize) -> Row {
        let room = columns - 2;
        let said = if wide(self.keys.0) <= room {
            self.keys.0
        } else {
            self.keys.1
        };

        let mut row = Row::new();
        row.push(Slot::Plain, " ");
        row.push(Slot::Quiet, clip(said, room));
        row.clipped(columns)
    }

    /// What the pointer is resting on, at the size the picker is drawn at.
    ///
    /// The same reading the drawing is made from, so what this answers and what
    /// the reader sees lit are one fact. Counted into the slice the picker was
    /// handed, not into the rows it drew — a scrolled list is showing its sixth
    /// session on its first row, and the caller is owed the one on screen.
    #[must_use]
    pub fn resting(&self, columns: usize, room: usize) -> Hit {
        if columns < Self::NARROWEST || room < CHROME + FLOOR {
            return Hit::Nothing;
        }
        let body = room - CHROME;

        match self.under(columns, body) {
            Under::Nothing => Hit::Nothing,
            Under::Searching => Hit::Search,
            Under::Previewing => Hit::Preview,
            Under::Listed(at) => {
                let pairs = body / 2;
                let from = scrolled(self.marked, pairs, self.sessions.len());
                if at / 2 < pairs && from + at / 2 < self.sessions.len() {
                    Hit::Session(from + at / 2)
                } else {
                    Hit::Nothing
                }
            }
        }
    }

    /// Where the terminal cursor belongs, inside the rows [`Picker::within`]
    /// answered: the search line, or the title being renamed while one is.
    ///
    /// The glyph set is taken and not read — the frame is the same one column
    /// wide in both sets — so a caller cannot draw with one set and place the
    /// cursor against another.
    #[must_use]
    pub fn caret(&self, columns: usize, room: usize, _glyphs: Glyphs) -> Caret {
        let last = columns.saturating_sub(2);

        if let Some(renaming) = self.renaming {
            let pairs = room.saturating_sub(CHROME) / 2;
            let from = scrolled(self.marked, pairs, self.sessions.len());
            let before: String = renaming.chars().take(self.typed).collect();
            return Caret {
                row: LISTED + 2 * self.marked.saturating_sub(from),
                column: (LEADING + wide(&before)).min(last),
            };
        }

        let before: String = self.query.chars().take(self.typed).collect();
        Caret {
            row: SEARCHING,
            column: (TYPED_AT + wide(&before)).min(last),
        }
    }

    /// Whether the preview stands beside the list at this width.
    ///
    /// An empty list splits only while a query stands — the query is why it
    /// emptied — and a workspace that never recorded a session has no split.
    fn apart(&self, columns: usize) -> bool {
        columns >= Self::FOLDS_AT && (!self.sessions.is_empty() || !self.query.is_empty())
    }

    /// What the pointer is resting on, in pane rows.
    fn under(&self, columns: usize, body: usize) -> Under {
        let Some((row, column)) = self.pointer else {
            return Under::Nothing;
        };

        if (SEARCHING - 1..=SEARCHING + 1).contains(&row) {
            return Under::Searching;
        }

        let Some(at) = row.checked_sub(LISTED).filter(|at| *at < body) else {
            return Under::Nothing;
        };

        let apart = self.apart(columns);
        let list = if apart { 2 * columns / 5 } else { columns };
        if column < list {
            return Under::Listed(at);
        }
        if apart && (list + 1..columns).contains(&column) {
            return Under::Previewing;
        }

        Under::Nothing
    }
}

/// The slot a span takes, given whether the pointer is resting on its row.
///
/// One slot for the whole pair rather than one per span, because the ground is
/// the point: a ground that changed halfway along would be two rectangles where
/// the reader is being shown one thing — and the row the mark is on still
/// carries its caret, so the two never have to be told apart by colour.
const fn lit(on: bool, slot: Slot) -> Slot {
    if on { Slot::Pointed } else { slot }
}

/// A pane with nothing to show: one quiet sentence, then blank to `body`.
fn said_instead(sentence: &str, body: usize) -> Vec<Row> {
    (0..body)
        .map(|at| {
            if at == 0 {
                Row::new()
                    .then(Slot::Plain, " ")
                    .then(Slot::Quiet, sentence)
            } else {
                Row::new()
            }
        })
        .collect()
}

/// A rule from one edge to the other, with no joint in the middle of it.
fn ruled(ends: (&str, &str), inside: usize, glyphs: Glyphs, slot: Slot) -> Row {
    Row::new()
        .then(slot, ends.0)
        .then(slot, glyphs.horizontal().repeat(inside))
        .then(slot, ends.1)
}

/// The first entry on screen, given where the mark is.
///
/// The mark is kept on screen at every rung of the scroll: a list scrolled to
/// its top would leave Enter about to resume something the reader cannot see.
fn scrolled(mark: usize, seen: usize, all: usize) -> usize {
    if all <= seen || seen == 0 {
        return 0;
    }
    mark.saturating_sub(seen - 1).min(all - seen)
}

#[cfg(test)]
mod tests;
