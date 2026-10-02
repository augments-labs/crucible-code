//! The list `/release-notes` opens: versions to walk, newest first.
//!
//! The eight newest releases and a last row that reveals the rest, in columns
//! of version, date and how many entries the release holds, with the running
//! version tagged. Enter on a release puts it into the transcript the way
//! `/release-notes <version>` does, and escape writes nothing. Enter on the
//! last row opens every release in place, the mark staying on the row that was
//! ninth, with the counts of what lies above and below the rows shown.
//!
//! What a release is, and how its entries are counted, are [`super::notes`]'s,
//! which is the one place that knows how the changelog is written; this reads
//! them and draws. It lives beside that module because the command owns
//! nothing that stands over the box, and `notes` is held to the values it
//! takes from the changelog alone.
//!
//! The rows are the panel's single-row entries, laid out here rather than by
//! [`crucible_tui::Panel`], whose entries are a name over a description: the
//! three columns line up across rows, and the date is the first thing given up
//! when the window is narrow, since the count is what a reader picks by.

use crucible_tui::{Glyphs, Key, Pressed, Renderer, Row, Slot, Terminal, clip, columns, fold};

use crate::cli::Fatal;
use crate::cli::converse::Terms;
use crate::cli::converse::region::{self, Ended, Moved};

use super::notes::{self, Release};

/// How many of the newest releases are listed before the row that reveals the
/// rest.
const LISTED: usize = 8;

/// What the list is called, over its rows.
const TITLE: &str = "Release notes";

/// What the running version is tagged with.
const TAG: &str = "this version";

/// The rows the list needs besides its releases: the rule, the blank, the
/// title, the blank under it, and the blank over the foot.
const CHROME: usize = 5;

/// The fewest rows of the list worth standing: two counts and a release, which
/// is what the opened list needs, and the same for the closed one.
const FLOOR: usize = 3;

/// Columns from the version's column to the date's, and to the count's where
/// there is no date.
const BEFORE_DATE: usize = 6;
const BEFORE_COUNT: usize = 4;

/// Columns between the date and the count.
const AFTER_DATE: usize = 4;

/// Columns between the count and the tag.
const BEFORE_TAG: usize = 3;

/// The longer of the two words a count is followed by, which both are padded
/// to so the tags stand in a column.
const ENTRIES: &str = "entries";

/// Stands the list where the box was, and puts the release taken off it into
/// the transcript. Says whether it did: `false` is a window with no room to
/// stand the list in, and what is owed is the command's other answer.
///
/// Escape writes nothing. The command's own row is already in the transcript,
/// and a row saying the list was dropped would be the only thing under it.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn run<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms) -> Result<bool, Fatal> {
    let glyphs = terms.style().glyphs();
    let mut listing = Listing::new(notes::releases(notes::CHANGELOG), notes::RUNNING);

    let ended = region::stand(
        renderer,
        |_: &Listing<'_>| terms.style(),
        &mut listing,
        |listing, columns, room| (listing.rows(columns, room, glyphs), None),
        |arrived, listing| listing.against(arrived),
    )?;

    match ended {
        Ended::Took => {
            if let Some(release) = listing.chosen() {
                let rows = notes::alone(release, renderer.transcript_columns(), glyphs);
                renderer.apart()?;
                renderer.present(&rows)?;
                renderer.commit("")?;
            }
            Ok(true)
        }
        Ended::Left => Ok(true),
        Ended::Cramped => Ok(false),
    }
}

/// What the list is showing, between one key and the next.
#[derive(Debug)]
pub(super) struct Listing<'a> {
    /// Every release, newest first.
    versions: Vec<Release<'a>>,
    /// How many entries each release holds, in the same order.
    entries: Vec<usize>,
    /// The version this binary is.
    running: &'a str,
    /// Which row a key acts on. Counted among the rows that can be walked: the
    /// reveal row is one while it is there.
    at: usize,
    /// Whether every release is listed rather than the newest few.
    all: bool,
    /// The first row in the window, in the same count as `at`.
    from: usize,
    /// The release that was taken, once one has been.
    taken: Option<usize>,
}

impl<'a> Listing<'a> {
    /// The list of `releases`, oldest first as the changelog gives them.
    pub(super) fn new(mut releases: Vec<Release<'a>>, running: &'a str) -> Self {
        releases.reverse();
        let entries = releases
            .iter()
            .map(|release| release.groups().iter().map(|(_, count)| count).sum())
            .collect();
        Self {
            versions: releases,
            entries,
            running,
            at: 0,
            all: false,
            from: 0,
            taken: None,
        }
    }

    /// The release that was taken, once one has been.
    pub(super) fn chosen(&self) -> Option<&Release<'a>> {
        self.taken.and_then(|at| self.versions.get(at))
    }

    /// What one key does to it.
    ///
    /// Every key is named rather than caught by a rest arm, for the reason every
    /// other standing component names its own.
    // An event token is handed over, not lent: the handler takes the one thing
    // the reader produced, and a reference would say the caller kept a say in it.
    #[allow(clippy::needless_pass_by_value)]
    pub(super) fn against(&mut self, arrived: Pressed) -> Moved {
        match arrived {
            // The wheel walks it as the arrows do, `stand` repeating a notch as
            // many times as the session's wheel speed says.
            Pressed::Up | Pressed::Scrolled { back: true } => {
                let back = self.at.checked_sub(1);
                region::step(&mut self.at, back)
            }
            Pressed::Down | Pressed::Scrolled { back: false } => {
                let on = (self.at + 1 < self.walkable()).then_some(self.at + 1);
                region::step(&mut self.at, on)
            }
            Pressed::Key(Key::Enter) if self.reveals(self.at) => {
                self.all = true;
                // Two rows of those before it, as the window opens, so the
                // reader sees where they came from; the layout puts the window
                // back inside the list if that is past an end.
                self.from = self.at.saturating_sub(2);
                Moved::Redraw
            }
            Pressed::Key(Key::Enter) => {
                self.taken = Some(self.at);
                Moved::Took
            }
            Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
            Pressed::Resized => Moved::Redraw,
            Pressed::Key(_)
            | Pressed::Pasted(_)
            | Pressed::Cycle
            | Pressed::Tab
            | Pressed::Explain
            | Pressed::Expand
            | Pressed::Background
            | Pressed::Plan
            | Pressed::Queue
            | Pressed::Copy
            | Pressed::PasteImage
            | Pressed::Rename
            | Pressed::All
            | Pressed::Clicked { .. }
            | Pressed::Dragged { .. }
            | Pressed::Hovered { .. }
            | Pressed::Released { .. }
            | Pressed::Ignored => Moved::Still,
        }
    }

    /// The rows to draw in a window of `columns` by `room`, and none where the
    /// list does not fit in it.
    ///
    /// The window the releases are shown through scrolls to keep the row a key
    /// would act on in view, which is why this takes the list by mutable
    /// reference: how many rows there are room for is found out here and not by
    /// the keyboard.
    pub(super) fn rows(&mut self, columns: usize, room: usize, glyphs: Glyphs) -> Vec<Row> {
        let (up, down) = glyphs.walking();
        let dot = glyphs.dot();
        let foot = format!("{up}{down} to walk {dot} enter opens it {dot} esc to close");
        let foot = fold(&foot, columns);

        let height = room.saturating_sub(CHROME + foot.len()).min(if self.all {
            LISTED + 1
        } else {
            self.walkable()
        });
        if height < FLOOR {
            return Vec::new();
        }

        // The opened list keeps a row for each count, so the releases between
        // them stay where they are as the window moves and the counts come and
        // go.
        let slots = if self.all { height - 2 } else { height };
        self.from = scrolled(self.from, self.at, slots, self.walkable());
        let until = (self.from + slots).min(self.walkable());

        let dated = columns >= self.dated_from();
        let mut rows = vec![
            Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
            Row::new(),
            Row::new().then(Slot::Strong, clip(TITLE, columns)),
            Row::new(),
        ];
        if self.all {
            rows.push(counted(self.from, up, "newer", columns));
        }
        for index in self.from..until {
            rows.push(self.row(index, dated, glyphs).clipped(columns));
        }
        if self.all {
            rows.push(counted(
                self.walkable().saturating_sub(until),
                down,
                "older",
                columns,
            ));
        }
        rows.push(Row::new());
        rows.extend(
            foot.into_iter()
                .map(|line| Row::new().then(Slot::Quiet, line)),
        );
        rows
    }

    /// How many rows can be walked: the releases shown, and the row that
    /// reveals the rest while it is there.
    fn walkable(&self) -> usize {
        if self.all {
            self.versions.len()
        } else {
            self.versions.len().min(LISTED) + usize::from(self.versions.len() > LISTED)
        }
    }

    /// Whether row `at` is the one that reveals the rest.
    fn reveals(&self, at: usize) -> bool {
        !self.all && self.versions.len() > LISTED && at == LISTED
    }

    /// One row of the list.
    fn row(&self, at: usize, dated: bool, glyphs: Glyphs) -> Row {
        let marked = at == self.at;
        let mut row = Row::new()
            .then(Slot::Accent, if marked { glyphs.caret() } else { " " })
            .then(Slot::Plain, " ");
        let slot = if marked { Slot::Strong } else { Slot::Plain };

        if self.reveals(at) {
            let (_, down) = glyphs.walking();
            row.push(slot, format!("all {} releases", self.versions.len()));
            row.push(Slot::Quiet, format!(" {down}"));
            return row;
        }

        let (Some(release), Some(entries)) = (self.versions.get(at), self.entries.get(at)) else {
            return row;
        };
        row.push(slot, release.version);

        let widest = self.widest(|release| columns(release.version));
        let before = if dated { BEFORE_DATE } else { BEFORE_COUNT };
        let mut rest = " ".repeat(widest + before - columns(release.version));
        if dated {
            rest.push_str(release.date);
            rest.push_str(
                &" ".repeat(self.widest(|release| columns(release.date)) - columns(release.date)),
            );
            rest.push_str(&" ".repeat(AFTER_DATE));
        }
        let noun = if *entries == 1 { "entry" } else { ENTRIES };
        let count = format!(
            "{entries:>number$} {noun:<width$}",
            number = self.number(),
            width = ENTRIES.len()
        );
        rest.push_str(&count);
        if release.version == self.running {
            rest.push_str(&" ".repeat(BEFORE_TAG));
            rest.push_str(TAG);
        }
        row.push(Slot::Quiet, rest.trim_end());
        row
    }

    /// The widest of what `measure` says of every release.
    fn widest(&self, measure: impl Fn(&Release<'_>) -> usize) -> usize {
        self.versions.iter().map(measure).max().unwrap_or_default()
    }

    /// The columns the count's number is right-aligned in: two, as the first
    /// column that a release with a dozen entries needs, and more for a
    /// changelog that has grown past them.
    fn number(&self) -> usize {
        let most = self.entries.iter().copied().max().unwrap_or_default();
        most.to_string().len().max(2)
    }

    /// The narrowest window the date is drawn in: the row with its date and the
    /// tag, as wide as any row is.
    fn dated_from(&self) -> usize {
        2 + self.widest(|release| columns(release.version))
            + BEFORE_DATE
            + self.widest(|release| columns(release.date))
            + AFTER_DATE
            + self.number()
            + 1
            + ENTRIES.len()
            + BEFORE_TAG
            + TAG.len()
    }
}

/// Where a window of `slots` rows begins so that row `at` is in it, moving as
/// little as it can from `from`, and never past the last row.
fn scrolled(from: usize, at: usize, slots: usize, total: usize) -> usize {
    let from = if at < from {
        at
    } else if at >= from + slots {
        at + 1 - slots
    } else {
        from
    };
    from.min(total.saturating_sub(slots))
}

/// The quiet row saying how many releases lie out of the window one way, and a
/// blank row where there are none.
fn counted(count: usize, arrow: &str, word: &str, columns: usize) -> Row {
    if count == 0 {
        return Row::new();
    }
    Row::new().then(
        Slot::Quiet,
        clip(&format!("  {arrow} {count} {word}"), columns),
    )
}

#[cfg(test)]
mod tests;
