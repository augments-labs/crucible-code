//! A shelf: a search line, two panes of rows beside each other, and a track of
//! rungs beneath both.
//!
//! It inverts one bargain [`Panel`](crate::Panel) makes, and only one. `Panel`
//! gives rows up as the room shortens, because it is drawn into whatever is
//! left over. This **fills** the room it is handed, padding its panes to the
//! bottom, because the band it stands in has no share and takes what it asks
//! for — the transcript is first in the order of surrender, so a component that
//! asks for everything under the head band gets it. Short of the rows its own
//! chrome needs it still answers with nothing at all, which is what a caller
//! reads as *there was no room to stand one*.
//!
//! It is handed strings and indices and knows no domain type. What narrowed the
//! query, what counted a pane, and which rungs a row may take are all decided
//! before they arrive, because this crate must never name a provider. A shelf
//! that could tell an Anthropic model from any other would be that decision,
//! sitting one layer below the only place allowed to make it.
//!
//! Nothing here names a colour. Every span asks for a [`Slot`] and the palette
//! settles what one is worth, so a theme changes what a shelf looks like
//! without changing a line of it — and no row paints a ground, so the one
//! behind the panel stays the reader's own.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::render::Caret;
use crate::row::Row;
use crate::width::{clip, columns as wide};

/// The rows a shelf spends on everything that is not a row of a pane.
///
/// The title and its blank, the three the search line's frame costs, its blank,
/// the pane's top, header and rule, its bottom and blank, the track, its blank,
/// and the keys. A room short of one more than this has no pane at all, and a
/// pane with no rows in it is a border around nothing.
const CHROME: usize = 14;

/// The width at which the pane of providers stops being a pane.
///
/// Below it the two panes cost more in borders and padding than they return, so
/// the providers fold into the header of the one border that is left.
const FOLDS_AT: usize = 59;

/// The inside of the pane of providers, in columns.
///
/// Fixed rather than a share, because what it holds is a name and a count and
/// neither grows with the window. Every column past it belongs to the pane that
/// has rows worth widening.
const SERVES: usize = 21;

/// The narrowest window a shelf is drawn in at all.
///
/// Under this the search line has no room left for what was typed into it, and
/// a search line that cannot show the query is the one row here that has to.
const NARROWEST: usize = 24;

/// The row the search line is on, counted from the top of what `within`
/// answered: the title, its blank, and the frame's own top.
const SEARCHING: usize = 3;

/// Where the search line opens, counted from the left of the window.
const TYPED_AT: usize = 10;

/// The row the panes' frame opens on, counted the same way as [`SEARCHING`]:
/// the title, its blank, the search frame's three rows, and its blank.
const PANED: usize = 6;

/// What the panes' frame costs above the first row inside it: the top, the
/// header, and the rule under it.
const HEADED: usize = 3;

/// Where the track's first rung opens.
const RUNG_AT: usize = 12;

/// What the marks in front of a row cost: a space, the mark, a space.
///
/// The same on both panes. What is in force does not spend a column here — it
/// is said at the right of the row it is true of, where the reader is already
/// looking for what is different about one row.
const LEADING: usize = 3;

/// The widths of the columns at the right of a model's row, and the gap kept
/// between each of them and whatever is drawn to its left.
const NOTE: usize = 8;
const WINDOW: usize = 4;
const SERVED_BY: usize = 12;
const GAP: usize = 2;

/// The narrowest pane of models that has room to say who serves each one.
const BY_AT: usize = 46;

/// The fewest columns a name is worth keeping in.
///
/// What decides whether a column at the right is drawn at all: one that leaves
/// the name less than this has spent the row on everything except the thing the
/// reader came to read.
const WORD: usize = 6;

/// One model on the shelf.
#[derive(Debug, Clone, Copy)]
pub struct Stocked<'a> {
    /// What it is called, in whatever spelling the reader is shown it in.
    pub name: &'a str,
    /// Who serves it. Drawn only where the shelf is unnarrowed and the pane is
    /// wide enough, because once the shelf is one provider's it is not news.
    pub by: &'a str,
    /// How much it accepts at once, already in the reader's units, or an em
    /// dash where nothing is known.
    pub window: &'a str,
    /// The quiet word at the end of the row, or empty.
    pub note: &'a str,
    /// Whether this is the one in force.
    pub now: bool,
}

/// One provider in the pane beside the shelf.
#[derive(Debug, Clone, Copy)]
pub struct Serving<'a> {
    /// What it is called.
    pub name: &'a str,
    /// How many models the query left it. `None` is a provider the query
    /// emptied, drawn as a quiet dot rather than a zero.
    pub count: Option<usize>,
}

/// Which pane the mark is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    /// The pane of providers, and the one that is folded away where the window
    /// is too narrow to hold two.
    Providers,
    /// The pane of models, which is the one every width has.
    Models,
}

/// The whole shell, as one borrowing view.
#[derive(Debug, Clone, Copy)]
pub struct Shelf<'a> {
    /// What the panel is called.
    pub title: &'a str,
    /// What is in force, drawn right-justified against the title.
    pub now: &'a str,
    /// What has been typed into the search line.
    pub query: &'a str,
    /// The caret's place within `query`, counted in characters.
    pub typed: usize,
    /// What the search line says when `query` is empty.
    pub hint: &'a str,
    /// Every provider, in the order the pane walks them.
    pub providers: &'a [Serving<'a>],
    /// Which of them the mark is on.
    pub provider: usize,
    /// The models the query left, in the order the pane walks them.
    pub models: &'a [Stocked<'a>],
    /// How many there were before the query narrowed them, which is what the
    /// header counts against. Equal to `models.len()` where nothing is typed.
    pub held: usize,
    /// Which of them the mark is on.
    pub model: usize,
    /// The rungs the marked model's provider serves, already narrowed. Empty is
    /// a model that takes none.
    pub rungs: &'a [&'a str],
    /// Which of them the mark is on.
    pub rung: usize,
    /// What the shelf says where the query left nothing.
    pub nothing: &'a str,
    /// Which pane the mark is in, and so which one the arrows walk.
    pub pane: Pane,
    /// The keys row, and the short form for a narrow window.
    pub keys: (&'a str, &'a str),
    /// What the track says where `rungs` is empty.
    pub norung: &'a str,
    /// Where the pointer is resting: a row of what [`Shelf::within`] answered,
    /// and a column of the window.
    ///
    /// `None` is a pointer that has never been reported, which is every session
    /// on a terminal that says nothing about the mouse. Nothing here moves a
    /// mark — what the pointer is over and what the keys are on are two
    /// different things, and a reader whose hand is on the mouse is still owed
    /// the row the arrows left.
    pub pointer: Option<(usize, usize)>,
}

/// What the pointer is resting on, in the shelf's own terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Under {
    /// Nothing the shelf lights up.
    Nothing,
    /// The line the query is typed into, or either rule of its frame.
    Searching,
    /// A row of the pane of providers, counted from the first row inside it.
    Provider(usize),
    /// A row of the pane of models, counted the same way.
    Model(usize),
}

impl Shelf<'_> {
    /// Every row of the shell, filled to `room`.
    ///
    /// Nothing at all where `room` is short of what the chrome needs — the
    /// caller's answer to that is its own listing.
    #[must_use]
    pub fn within(&self, columns: usize, room: usize, glyphs: Glyphs) -> Vec<Row> {
        if columns < NARROWEST || room <= CHROME {
            return Vec::new();
        }
        let body = room - CHROME;

        // Worked out once and handed down, because two of the four things it
        // decides are on different rows of the same picture: a frame drawn in
        // the accent and a band drawn under a name are one answer about where
        // the pointer is, and asking twice is how the two come to disagree.
        let under = self.under(columns, body);

        let mut rows = Vec::with_capacity(room);
        rows.push(self.titled(columns));
        rows.push(Row::new());
        rows.extend(self.searched(columns, glyphs, under));
        rows.push(Row::new());
        rows.extend(self.paned(columns, body, glyphs, under));
        rows.push(Row::new());
        rows.push(self.tracked(columns, glyphs));
        rows.push(Row::new());
        rows.push(self.keyed(columns));
        rows
    }

    /// Where the terminal cursor belongs, inside the rows [`Shelf::within`]
    /// answered.
    ///
    /// The glyph set is taken and not read: the search line's frame is the same
    /// one column wide in both sets, so where the caret lands is a fact about
    /// the query alone. Taking it anyway is what keeps the two calls one
    /// signature, so a caller cannot draw with one set and place the cursor
    /// against another.
    #[must_use]
    pub fn caret(&self, columns: usize, _glyphs: Glyphs) -> Caret {
        let before: String = self.query.chars().take(self.typed).collect();
        let last = columns.saturating_sub(2);

        Caret {
            row: SEARCHING,
            column: (TYPED_AT + wide(&before)).min(last),
        }
    }

    /// What the pointer is resting on, given the size the shelf is drawn at.
    ///
    /// Worked out from the layout rather than remembered from it, which is what
    /// keeps it true when the window changes under a pointer that has not
    /// moved: the row it is on means whatever this size puts there.
    ///
    /// A folded shelf has no pane of providers to rest on — its providers are
    /// words on one header row, and a band drawn across that row would light
    /// all of them to say one.
    fn under(&self, columns: usize, body: usize) -> Under {
        let Some((row, column)) = self.pointer else {
            return Under::Nothing;
        };

        if (SEARCHING - 1..=SEARCHING + 1).contains(&row) {
            return Under::Searching;
        }

        let Some(at) = row.checked_sub(PANED + HEADED).filter(|at| *at < body) else {
            return Under::Nothing;
        };

        let apart = columns >= FOLDS_AT;
        if apart && (1..=SERVES).contains(&column) {
            return Under::Provider(at);
        }

        let opens = if apart { SERVES + 2 } else { 1 };
        if (opens..columns.saturating_sub(1)).contains(&column) {
            return Under::Model(at);
        }

        Under::Nothing
    }

    /// The name of the panel, and what is in force drawn away from it.
    fn titled(&self, columns: usize) -> Row {
        let mut row = Row::new();
        row.push(Slot::Strong, clip(self.title, columns));

        let room = columns.saturating_sub(row.columns() + GAP);
        if wide(self.now) <= room {
            row.pad(columns - wide(self.now));
            row.push(Slot::Quiet, self.now);
        }
        row.clipped(columns)
    }

    /// The framed line the query is typed into.
    ///
    /// The one frame on the shelf that changes colour, and it changes for the
    /// one reason a reader would want it to: a field is a thing to put a
    /// pointer in, and the accent under the pointer says which one this is. The
    /// panes' own frames stay quiet at every moment -- they divide the picture
    /// rather than offer anything, and a border that lit up would be answering
    /// a question nobody asked of it.
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
            line.pad(TYPED_AT + GAP);
            line.push(Slot::Quiet, clip(self.hint, room - GAP));
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

    /// The bordered panes, and everything inside them.
    ///
    /// One frame either way. Wide enough and the providers have a pane of their
    /// own beside the models; below that they fold into the header of the one
    /// frame that is left, because two panes down there cost more in borders
    /// and padding than they return.
    fn paned(&self, columns: usize, body: usize, glyphs: Glyphs, under: Under) -> Vec<Row> {
        let apart = columns >= FOLDS_AT;
        let inside = if apart {
            columns - SERVES - 3
        } else {
            columns - 2
        };

        let mut rows = Vec::with_capacity(body + 4);
        rows.push(joined(
            glyphs.top(),
            glyphs.dividing().0,
            apart,
            inside,
            glyphs,
        ));

        let mut header = Row::new();
        header.push(Slot::Quiet, glyphs.vertical());
        if apart {
            header.push(Slot::Quiet, clip("  Providers", SERVES));
            header.pad(SERVES + 1);
            header.push(Slot::Quiet, glyphs.vertical());
            header.push(Slot::Quiet, clip("  Models", inside));

            // How many of how many, against the right edge of the pane it
            // counts: a number on its own says how many are here, and what
            // somebody who has just typed wants is how many are not.
            let counted = format!("{} of {}", self.models.len(), self.held);
            if header.columns() + GAP + wide(&counted) <= columns - GAP {
                header.pad(columns - GAP - wide(&counted));
                header.push(Slot::Quiet, counted);
            }
        } else {
            header = header.join(self.folded(inside, glyphs));
        }
        header.pad(columns - 1);
        header.push(Slot::Quiet, glyphs.vertical());
        rows.push(header.clipped(columns));

        rows.push(joined(
            glyphs.joining(),
            glyphs.crossing(),
            apart,
            inside,
            glyphs,
        ));

        let serving = self.serving(body, apart, glyphs, under);
        let stocking = self.stocking(body, inside, glyphs, under);
        for (beside, stocked) in serving.into_iter().zip(stocking) {
            let mut row = Row::new();
            row.push(Slot::Quiet, glyphs.vertical());
            if apart {
                row = row.join(beside);
                row.pad(SERVES + 1);
                row.push(Slot::Quiet, glyphs.vertical());
            }
            row = row.join(stocked);
            row.pad(columns - 1);
            row.push(Slot::Quiet, glyphs.vertical());
            rows.push(row.clipped(columns));
        }

        rows.push(joined(
            glyphs.bottom(),
            glyphs.dividing().1,
            apart,
            inside,
            glyphs,
        ));
        rows
    }

    /// The providers, on the one header row a folded shelf has left for them.
    fn folded(&self, inside: usize, glyphs: Glyphs) -> Row {
        let (opens, closes) = glyphs.bracketing();

        let mut row = Row::new();
        row.push(Slot::Plain, "  ");
        for (at, provider) in self.providers.iter().enumerate() {
            if at > 0 {
                row.push(Slot::Quiet, format!(" {} ", glyphs.dot()));
            }
            let marked = at == self.provider;
            if marked {
                row.push(Slot::Accent, opens);
            }
            row.push(
                if marked && self.pane == Pane::Providers {
                    Slot::Strong
                } else {
                    Slot::Quiet
                },
                provider.name,
            );
            if marked {
                row.push(Slot::Accent, closes);
            }
        }
        row.clipped(inside)
    }

    /// Each row of the pane of providers, padded to its width.
    fn serving(&self, body: usize, apart: bool, glyphs: Glyphs, under: Under) -> Vec<Row> {
        if !apart {
            return vec![Row::new(); body];
        }
        let from = scrolled(self.provider, body, self.providers.len());

        (0..body)
            .map(|at| {
                let mut row = Row::new();
                // Nothing is lit under the last provider. A pane is padded to
                // the bottom of the window and the rows doing that padding are
                // the frame's inside rather than anything to point at.
                let on = under == Under::Provider(at) && self.providers.len() > from + at;
                match self.providers.get(from + at) {
                    None => {}
                    Some(provider) => {
                        let marked = from + at == self.provider;
                        row.push(lit(on, Slot::Plain), " ");
                        row.push(
                            lit(on, Slot::Accent),
                            if marked { glyphs.caret() } else { " " }.to_owned(),
                        );
                        row.push(lit(on, Slot::Plain), " ");
                        let count = match provider.count {
                            Some(count) => count.to_string(),
                            None => glyphs.dot().to_owned(),
                        };
                        let ends = SERVES - 1;
                        let name = ends - wide(&count) - 1;
                        row.push(
                            lit(
                                on,
                                if marked && self.pane == Pane::Providers {
                                    Slot::Strong
                                } else {
                                    Slot::Plain
                                },
                            ),
                            clip(provider.name, name.saturating_sub(LEADING)),
                        );
                        row.fill(lit(on, Slot::Plain), ends - wide(&count));
                        row.push(lit(on, Slot::Quiet), count);
                    }
                }
                row.fill(lit(on, Slot::Plain), SERVES);
                row.clipped(SERVES)
            })
            .collect()
    }

    /// Each row of the pane of models, padded to its width.
    ///
    /// More models than rows shows the ones that fit and says how many it did
    /// not on the last of them — a count of models left, never of rows, because
    /// the row saying it is one of the rows.
    fn stocking(&self, body: usize, inside: usize, glyphs: Glyphs, under: Under) -> Vec<Row> {
        if self.models.is_empty() {
            let mut said = Row::new();
            said.pad(LEADING);
            said.push(Slot::Quiet, clip(self.nothing, inside - LEADING));
            let mut rows = vec![said.clipped(inside)];
            rows.resize(body, Row::new());
            return rows;
        }

        let (from, shown, left) = if self.models.len() <= body {
            (0, self.models.len(), 0)
        } else {
            let seen = body - 1;
            let from = scrolled(self.model, seen, self.models.len());
            let left = self.models.len() - from - seen;
            if left == 0 {
                (self.models.len() - body, body, 0)
            } else {
                (from, seen, left)
            }
        };
        let ends = Ends::across(inside, self.models.iter().any(|one| !one.by.is_empty()));

        (0..body)
            .map(|at| match self.models.get(from + at) {
                Some(one) if at < shown => self.stocked(
                    one,
                    Marks {
                        marked: from + at == self.model,
                        lit: under == Under::Model(at),
                    },
                    ends,
                    glyphs,
                ),
                _ if at == shown && left > 0 => {
                    let mut row = Row::new();
                    row.pad(LEADING);
                    row.push(Slot::Quiet, format!("{} {left} more", glyphs.dot()));
                    row.clipped(inside)
                }
                _ => Row::new(),
            })
            .collect()
    }

    /// One model's row, against the columns the pane settled.
    fn stocked(&self, one: &Stocked<'_>, marks: Marks, ends: Ends, glyphs: Glyphs) -> Row {
        let Marks { marked, lit: on } = marks;

        let mut row = Row::new();
        row.push(lit(on, Slot::Plain), " ");
        row.push(
            lit(on, Slot::Accent),
            if marked { glyphs.caret() } else { " " }.to_owned(),
        );
        row.push(lit(on, Slot::Plain), " ");
        row.push(
            lit(
                on,
                if marked && self.pane == Pane::Models {
                    Slot::Strong
                } else {
                    Slot::Plain
                },
            ),
            clip(one.name, ends.name.saturating_sub(LEADING)),
        );

        // Left to right, in the order the columns sit: who serves it reads as a
        // word and starts where the others on the pane start.
        if let (Some(opens), false) = (ends.by, one.by.is_empty()) {
            row.fill(lit(on, Slot::Plain), opens);
            row.push(lit(on, Slot::Quiet), clip(one.by, SERVED_BY));
        }

        // Right-justified, because what it is worth comparing against is the
        // number on the row above and the row below, and two numbers of
        // different lengths only line up at one end.
        if let (Some(opens), false) = (ends.window, one.window.is_empty()) {
            let said = clip(one.window, WINDOW);
            row.fill(lit(on, Slot::Plain), opens + WINDOW - wide(said));
            row.push(lit(on, Slot::Quiet), said);
        }

        // What is in force wins the last column outright. Both belong to this
        // row alone, and the one that is news is the one saying the session is
        // already asking this — a note about a rung is still true tomorrow.
        if let Some(opens) = ends.note {
            row.fill(lit(on, Slot::Plain), opens);
            if one.now {
                row.push(
                    lit(on, Slot::DoneMark),
                    format!("{} now", glyphs.stepping().0),
                );
            } else {
                row.push(lit(on, Slot::Quiet), clip(one.note, ends.inside - opens));
            }
        }
        row.fill(lit(on, Slot::Plain), ends.inside);
        row.clipped(ends.inside)
    }

    /// The track of rungs under both panes.
    ///
    /// A ladder read left to right, low at one end and max at the other, with a
    /// side of the mark closed around the rung in force.
    fn tracked(&self, columns: usize, glyphs: Glyphs) -> Row {
        let mut row = Row::new();
        row.push(Slot::Plain, "  ");
        row.push(Slot::Quiet, clip("Effort", columns.saturating_sub(4)));

        if self.rungs.is_empty() {
            row.pad(RUNG_AT - 1);
            row.push(Slot::Quiet, clip(self.norung, columns - RUNG_AT));
            return row.clipped(columns);
        }

        // Every rung keeps a column of air each side of its word, and the
        // marked one spends those two columns on the sides of the mark and
        // takes another two for the air the mark pushed out. Widening the one
        // word under the mark is the picture: the rung in force is the one the
        // row has made room for, which a reader sees before reading anything.
        let (opens, closes) = glyphs.bracketing();
        row.pad(RUNG_AT - 1);

        for (at, rung) in self.rungs.iter().enumerate() {
            let marked = at == self.rung;
            if at > 0 {
                row.push(Slot::Plain, " ");
            }
            if marked {
                row.push(Slot::Accent, opens);
            }
            row.push(Slot::Plain, " ");
            row.push(if marked { Slot::Strong } else { Slot::Quiet }, *rung);
            row.push(Slot::Plain, " ");
            if marked {
                row.push(Slot::Accent, closes);
            }
        }
        row.clipped(columns)
    }

    /// What the keys do, in the longest form the window has room for.
    fn keyed(&self, columns: usize) -> Row {
        let room = columns - 4;
        let said = if wide(self.keys.0) <= room {
            self.keys.0
        } else {
            self.keys.1
        };

        let mut row = Row::new();
        row.push(Slot::Plain, "  ");
        row.push(Slot::Quiet, clip(said, room));
        row.clipped(columns)
    }
}

/// What is true of one row of a pane beyond what is written on it.
#[derive(Debug, Clone, Copy)]
struct Marks {
    /// Whether the keyboard's mark is on it.
    marked: bool,
    /// Whether the pointer is resting on it.
    lit: bool,
}

/// The slot a span takes, given whether the pointer is resting on its row.
///
/// One slot for a whole row rather than a slot per span, because the ground is
/// the point of it: a ground that changed halfway along would be two rectangles
/// where the reader is being shown one row. What that costs is the difference
/// between a name and the count beside it, which is not a difference anybody is
/// reading while their pointer is on the row -- and the row the arrows are on
/// still carries its mark, so the two never have to be told apart by colour.
const fn lit(on: bool, slot: Slot) -> Slot {
    if on { Slot::Pointed } else { slot }
}

/// Where each column at the right of a model's row opens.
///
/// Settled once for the pane rather than once for every row on it, because a
/// column that moved with the name would put the whole block a place out on the
/// row carrying the longest one — and measured from the right edge inwards, in
/// the order a column is worth giving up: who serves it first, then how much it
/// accepts, and the note last, because the note is the only one of the three
/// that ever says something about *this* row alone.
#[derive(Debug, Clone, Copy)]
struct Ends {
    /// The pane's inside, which every offset here is measured against.
    inside: usize,
    by: Option<usize>,
    window: Option<usize>,
    note: Option<usize>,
    /// Where the name has to stop.
    name: usize,
}

impl Ends {
    /// The columns a pane of `inside` has room for, given whether any row on it
    /// has a provider to name.
    fn across(inside: usize, by: bool) -> Self {
        let mut edge = inside;
        let mut ends = Self {
            inside,
            by: None,
            window: None,
            note: None,
            name: inside,
        };

        if inside >= LEADING + WORD + GAP + NOTE {
            ends.note = Some(edge - NOTE);
            edge -= NOTE + GAP;
        }
        if inside >= LEADING + WORD + 2 * GAP + NOTE + WINDOW {
            ends.window = Some(edge - WINDOW);
            edge -= WINDOW + GAP;
        }
        if by && inside >= BY_AT {
            ends.by = Some(edge - SERVED_BY);
            edge -= SERVED_BY + GAP;
        }

        ends.name = edge;
        ends
    }
}

/// A rule from one edge to the other, with no joint in the middle of it.
fn ruled(ends: (&str, &str), inside: usize, glyphs: Glyphs, slot: Slot) -> Row {
    Row::new()
        .then(slot, ends.0)
        .then(slot, glyphs.horizontal().repeat(inside))
        .then(slot, ends.1)
}

/// The same, with the joint the panes meet at where they meet.
///
/// Quiet at every moment, which is the whole of what the panes' frame is for:
/// it says where one pane stops and the other starts, and it has nothing else
/// to say at any point in the reading. The accent is spent on the two things
/// that do -- the mark walking a pane, and the field a pointer is resting in.
fn joined(ends: (&str, &str), joint: &str, apart: bool, inside: usize, glyphs: Glyphs) -> Row {
    if !apart {
        return ruled(ends, inside, glyphs, Slot::Quiet);
    }
    Row::new()
        .then(Slot::Quiet, ends.0)
        .then(Slot::Quiet, glyphs.horizontal().repeat(SERVES))
        .then(Slot::Quiet, joint)
        .then(Slot::Quiet, glyphs.horizontal().repeat(inside))
        .then(Slot::Quiet, ends.1)
}

/// The first row on screen, given where the mark is.
///
/// The mark is kept on screen at every rung of the scroll: a list scrolled to
/// its top would leave the keys about to act on something the reader cannot
/// see.
fn scrolled(mark: usize, seen: usize, all: usize) -> usize {
    if all <= seen || seen == 0 {
        return 0;
    }
    mark.saturating_sub(seen - 1).min(all - seen)
}

#[cfg(test)]
mod tests;
