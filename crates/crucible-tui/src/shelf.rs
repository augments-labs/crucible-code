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

/// Where the track's first rung opens.
const RUNG_AT: usize = 12;

/// What the marks in front of a row of the pane of models cost: a space, the
/// mark, a space, the state, a space.
const LEADING: usize = 5;

/// The same for a row of the pane of providers, which has no state to say.
const LEADING_BY: usize = 3;

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

        let mut rows = Vec::with_capacity(room);
        rows.push(self.titled(columns));
        rows.push(Row::new());
        rows.extend(self.searched(columns, glyphs));
        rows.push(Row::new());
        rows.extend(self.paned(columns, body, glyphs));
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

    /// The name of the panel, and what is in force drawn away from it.
    fn titled(&self, columns: usize) -> Row {
        let mut row = Row::new();
        row.push(Slot::Plain, "  ");
        row.push(Slot::Strong, clip(self.title, columns.saturating_sub(4)));

        let room = columns.saturating_sub(row.columns() + 4);
        if wide(self.now) <= room {
            let at = columns - 2 - wide(self.now);
            row.pad(at);
            row.push(Slot::Quiet, self.now);
        }
        row.clipped(columns)
    }

    /// The framed line the query is typed into.
    fn searched(&self, columns: usize, glyphs: Glyphs) -> [Row; 3] {
        let mut line = Row::new();
        line.push(Slot::Accent, glyphs.vertical());
        line.push(Slot::Quiet, " Search");

        let room = columns - TYPED_AT - 1;
        line.pad(TYPED_AT);
        if self.query.is_empty() {
            line.push(Slot::Quiet, clip(self.hint, room));
        } else {
            line.push(Slot::Plain, clip(self.query, room));
        }
        line.pad(columns - 1);
        line.push(Slot::Accent, glyphs.vertical());

        [
            ruled(glyphs.top(), columns - 2, glyphs),
            line.clipped(columns),
            ruled(glyphs.bottom(), columns - 2, glyphs),
        ]
    }

    /// The bordered panes, and everything inside them.
    ///
    /// One frame either way. Wide enough and the providers have a pane of their
    /// own beside the models; below that they fold into the header of the one
    /// frame that is left, because two panes down there cost more in borders
    /// and padding than they return.
    fn paned(&self, columns: usize, body: usize, glyphs: Glyphs) -> Vec<Row> {
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
        header.push(Slot::Accent, glyphs.vertical());
        if apart {
            header.push(Slot::Quiet, clip(" Providers", SERVES));
            header.pad(SERVES + 1);
            header.push(Slot::Accent, glyphs.vertical());
            header.push(Slot::Quiet, clip(" Models", inside));
        } else {
            header = header.join(self.folded(inside, glyphs));
        }
        header.pad(columns - 1);
        header.push(Slot::Accent, glyphs.vertical());
        rows.push(header.clipped(columns));

        rows.push(joined(
            glyphs.joining(),
            glyphs.crossing(),
            apart,
            inside,
            glyphs,
        ));

        let serving = self.serving(body, apart, glyphs);
        let stocking = self.stocking(body, inside, glyphs);
        for (beside, stocked) in serving.into_iter().zip(stocking) {
            let mut row = Row::new();
            row.push(Slot::Accent, glyphs.vertical());
            if apart {
                row = row.join(beside);
                row.pad(SERVES + 1);
                row.push(Slot::Accent, glyphs.vertical());
            }
            row = row.join(stocked);
            row.pad(columns - 1);
            row.push(Slot::Accent, glyphs.vertical());
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
        let mut row = Row::new();
        for (at, provider) in self.providers.iter().enumerate() {
            if at > 0 {
                row.push(Slot::Quiet, format!(" {} ", glyphs.dot()));
            } else {
                row.push(Slot::Plain, " ");
            }
            let marked = at == self.provider;
            row.push(
                Slot::Accent,
                if marked { glyphs.caret() } else { " " }.to_owned(),
            );
            row.push(Slot::Plain, " ");
            row.push(
                if marked && self.pane == Pane::Providers {
                    Slot::Strong
                } else {
                    Slot::Quiet
                },
                provider.name,
            );
        }
        row.clipped(inside)
    }

    /// Each row of the pane of providers, padded to its width.
    fn serving(&self, body: usize, apart: bool, glyphs: Glyphs) -> Vec<Row> {
        if !apart {
            return vec![Row::new(); body];
        }
        let from = scrolled(self.provider, body, self.providers.len());

        (0..body)
            .map(|at| {
                let mut row = Row::new();
                match self.providers.get(from + at) {
                    None => {}
                    Some(provider) => {
                        let marked = from + at == self.provider;
                        row.push(Slot::Plain, " ");
                        row.push(
                            Slot::Accent,
                            if marked { glyphs.caret() } else { " " }.to_owned(),
                        );
                        row.push(Slot::Plain, " ");
                        let count = match provider.count {
                            Some(count) => count.to_string(),
                            None => glyphs.dot().to_owned(),
                        };
                        let ends = SERVES - 1;
                        let name = ends - wide(&count) - 1;
                        row.push(
                            if marked && self.pane == Pane::Providers {
                                Slot::Strong
                            } else {
                                Slot::Plain
                            },
                            clip(provider.name, name.saturating_sub(LEADING_BY)),
                        );
                        row.pad(ends - wide(&count));
                        row.push(Slot::Quiet, count);
                    }
                }
                row.pad(SERVES);
                row.clipped(SERVES)
            })
            .collect()
    }

    /// Each row of the pane of models, padded to its width.
    ///
    /// More models than rows shows the ones that fit and says how many it did
    /// not on the last of them — a count of models left, never of rows, because
    /// the row saying it is one of the rows.
    fn stocking(&self, body: usize, inside: usize, glyphs: Glyphs) -> Vec<Row> {
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
                Some(one) if at < shown => self.stocked(one, from + at, ends, glyphs),
                _ if at == shown && left > 0 => {
                    let mut row = Row::new();
                    row.pad(LEADING - 2);
                    row.push(Slot::Quiet, format!("{} {left} more", glyphs.dot()));
                    row.clipped(inside)
                }
                _ => Row::new(),
            })
            .collect()
    }

    /// One model's row, against the columns the pane settled.
    fn stocked(&self, one: &Stocked<'_>, at: usize, ends: Ends, glyphs: Glyphs) -> Row {
        let marked = at == self.model;

        let mut row = Row::new();
        row.push(Slot::Plain, " ");
        row.push(
            Slot::Accent,
            if marked { glyphs.caret() } else { " " }.to_owned(),
        );
        row.push(Slot::Plain, " ");
        row.push(
            if one.now { Slot::DoneMark } else { Slot::Plain },
            if one.now { glyphs.done() } else { " " }.to_owned(),
        );
        row.push(Slot::Plain, " ");
        row.push(
            if marked && self.pane == Pane::Models {
                Slot::Strong
            } else {
                Slot::Plain
            },
            clip(one.name, ends.name.saturating_sub(LEADING)),
        );

        for (opens, text) in [
            (ends.by, one.by),
            (ends.window, one.window),
            (ends.note, one.note),
        ] {
            let (Some(opens), false) = (opens, text.is_empty()) else {
                continue;
            };
            row.pad(opens);
            row.push(Slot::Quiet, clip(text, ends.inside - opens));
        }
        row.pad(ends.inside);
        row.clipped(ends.inside)
    }

    /// The track of rungs under both panes.
    ///
    /// Every rung keeps a column for the mark whether it has it or not, so the
    /// words do not slide sideways as the mark walks along them.
    fn tracked(&self, columns: usize, glyphs: Glyphs) -> Row {
        let mut row = Row::new();
        row.push(Slot::Plain, "  ");
        row.push(Slot::Quiet, clip("Effort", columns.saturating_sub(4)));
        row.pad(RUNG_AT);

        if self.rungs.is_empty() {
            row.push(Slot::Quiet, clip(self.norung, columns - RUNG_AT - 2));
            return row.clipped(columns);
        }

        for (at, rung) in self.rungs.iter().enumerate() {
            let marked = at == self.rung;
            if at > 0 {
                row.push(Slot::Plain, "  ");
            }
            row.push(
                Slot::Accent,
                if marked { glyphs.caret() } else { " " }.to_owned(),
            );
            row.push(Slot::Plain, " ");
            row.push(if marked { Slot::Strong } else { Slot::Quiet }, *rung);
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
fn ruled(ends: (&str, &str), inside: usize, glyphs: Glyphs) -> Row {
    Row::new()
        .then(Slot::Accent, ends.0)
        .then(Slot::Accent, glyphs.horizontal().repeat(inside))
        .then(Slot::Accent, ends.1)
}

/// The same, with the joint the panes meet at where they meet.
fn joined(ends: (&str, &str), joint: &str, apart: bool, inside: usize, glyphs: Glyphs) -> Row {
    if !apart {
        return ruled(ends, inside, glyphs);
    }
    Row::new()
        .then(Slot::Accent, ends.0)
        .then(Slot::Accent, glyphs.horizontal().repeat(SERVES))
        .then(Slot::Accent, joint)
        .then(Slot::Accent, glyphs.horizontal().repeat(inside))
        .then(Slot::Accent, ends.1)
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
