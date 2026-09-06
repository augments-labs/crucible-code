//! The screen a key is typed into: the login panels' header, a breadcrumb, one
//! sentence, a labelled frame with a mark and dots inside, and a footer.
//!
//! Not [`crate::Prompt`] with the readings blanked. A prompt is a turn, and its
//! chrome — the window left, the transcript hint, the model the next turn goes
//! to — is facts about turns. A key box is a question with one field, so it is
//! laid out the way the two panels before it are, and shares their rule, title
//! and footer rather than the prompt's status row.
//!
//! **What this knows.** How many characters are held, and nothing about them.
//! The box draws one hidden mark per character and puts the caret after the
//! last, which is all a box standing in for a secret is allowed to know; the
//! secret itself never reaches this crate.
//!
//! **What clips and what folds.** The sentence is prose and folds. The
//! breadcrumb, the title and the label are labels and are cut at the width;
//! the footer shortens its action before sacrificing the cancel hint. The
//! label leaves the border altogether when it does not fit
//! beside the rule on either side of it, because a border with half a label
//! on it reads as a broken frame. The dots stop at the frame's right edge, and
//! so does the caret: the frame is the one thing on the screen that must not
//! give way.
//!
//! Height is [`KeyPanel::within`]'s subject. [`KeyPanel::rows`] draws the
//! whole screen and assumes the caller has room for it.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::prompt::Prompt;
use crate::render::Caret;
use crate::row::Row;
use crate::width::{clip, columns as wide, fold};

/// The words under the box while nothing is held, either side of the dot.
pub const EMPTY_FOOTER: (&str, &str) = ("paste or type your API key", "esc to cancel");

/// The words under the box once something is, either side of the dot.
pub const HELD_FOOTER: (&str, &str) = ("enter to save", "esc to cancel");

/// The few words at the top, the same on every screen of the walk.
const TITLE: &str = "Log in";

/// What follows the provider on the breadcrumb: the row taken on the first
/// panel, in its own words.
const ROUTE: &str = "provide your own API key";

/// The one sentence, read once: where the key goes and that it is not echoed.
const SAID: &str =
    "Paste or type the key. It goes to crucible's protected store and is never shown.";

/// What follows the provider on the label over the frame.
const LABELLED: &str = "API key";

/// The rule the label is held off the corner by, and the spaces holding the
/// label off the rule on either side of it.
const BEFORE: usize = 2;
const APART: usize = 2;
const AFTER: usize = 1;

/// What the frame spends on each row: an edge either side, and inside the
/// left one a space, the mark and a space.
const EDGES: usize = 2;
const POINTING: usize = 3;

/// The third screen of the walk: a key box for one provider.
#[derive(Debug, Clone, Copy)]
pub struct KeyPanel<'a> {
    /// The provider, spelled the way the vendor spells it.
    pub provider: &'a str,
    /// How many characters the box holds.
    pub held: usize,
}

/// The parts that give way, in the order they do: explanation goes before
/// the way here, the way here before the rule, the rule before the key that
/// leaves, and the title last — the frame and its label say what this is on
/// their own.
const SENTENCE: usize = 0;
const BREADCRUMB: usize = 1;
const RULE: usize = 2;
const FOOTER: usize = 3;
const TITLE_ROW: usize = 4;

/// One rung of the ladder: how many of the parts that give way have.
///
/// The frame is on every rung, and the rung under the last of these is
/// nothing at all.
#[derive(Clone, Copy)]
struct Rung(usize);

impl Rung {
    /// Every rung, tallest first.
    const LADDER: [Self; 6] = [Self(0), Self(1), Self(2), Self(3), Self(4), Self(5)];

    /// Whether this rung still draws `part`.
    const fn keeps(self, part: usize) -> bool {
        part >= self.0
    }
}

impl KeyPanel<'_> {
    /// The whole screen, drawn for a terminal `columns` wide.
    ///
    /// Never wider than that anywhere: a row past the last column is one the
    /// terminal wraps itself, which leaves the cursor a row below where the
    /// next frame expects it. Height is not considered here — see
    /// [`KeyPanel::within`] for a caller that has a window to fit inside.
    #[must_use]
    pub fn rows(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        self.laid(columns, glyphs, Rung(0)).0
    }

    /// The screen as it fits in `room` rows, and where the caret goes in it.
    ///
    /// Rung by rung: the whole screen, then the same without its sentence,
    /// its breadcrumb, its rule, its footer and its title in that order, and
    /// then nothing at all. The frame is what every rung keeps, so a caller
    /// with room for three rows still gets a box to type into — and one with
    /// less gets nothing and no caret, rather than a frame with no inside.
    #[must_use]
    pub fn within(&self, columns: usize, room: usize, glyphs: Glyphs) -> (Vec<Row>, Option<Caret>) {
        Rung::LADDER
            .iter()
            .map(|rung| self.laid(columns, glyphs, *rung))
            .find(|(rows, _)| rows.len() <= room)
            .unwrap_or_default()
    }

    /// The screen as `rung` draws it, and the caret inside its frame.
    fn laid(&self, columns: usize, glyphs: Glyphs, rung: Rung) -> (Vec<Row>, Option<Caret>) {
        let mut rows = Vec::new();

        if rung.keeps(RULE) {
            rows.push(Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)));
            rows.push(Row::new());
        }

        if rung.keeps(TITLE_ROW) {
            rows.push(Row::new().then(Slot::Strong, clip(TITLE, columns)));
            rows.push(Row::new());
        }

        if rung.keeps(BREADCRUMB) {
            rows.push(self.breadcrumb(columns, glyphs));
            rows.push(Row::new());
        }

        if rung.keeps(SENTENCE) {
            rows.extend(fold(SAID, columns).into_iter().map(Row::plain));
            rows.push(Row::new());
        }

        let caret = Caret {
            row: rows.len() + 1,
            column: self.column(columns),
        };
        rows.push(self.opening(columns, glyphs));
        rows.push(self.boxed(columns, glyphs));
        rows.push(Self::closing(columns, glyphs));

        if rung.keeps(FOOTER) {
            rows.push(Row::new().then(Slot::Quiet, clip(&self.footer(columns, glyphs), columns)));
        }

        (rows, Some(caret))
    }

    /// The way here: the mark, the provider and the row taken on the first
    /// panel, so the screen says what it is a step of.
    fn breadcrumb(&self, columns: usize, glyphs: Glyphs) -> Row {
        let mark = glyphs.caret();
        let Some(room) = columns.checked_sub(wide(mark) + 1) else {
            return Row::new().then(Slot::Accent, clip(mark, columns));
        };

        let said = format!("{} {} {ROUTE}", self.provider, glyphs.dash());
        Row::new()
            .then(Slot::Accent, mark)
            .then(Slot::Plain, format!(" {}", clip(&said, room)))
    }

    /// The top border, with the label on it where the label fits.
    fn opening(&self, columns: usize, glyphs: Glyphs) -> Row {
        let bar = glyphs.horizontal();
        let (open, opened) = glyphs.top();
        let inner = columns.saturating_sub(EDGES);

        let label = format!("{} {LABELLED}", self.provider);
        let drawn = if BEFORE + APART + wide(&label) + AFTER <= inner {
            let after = bar.repeat(inner - BEFORE - APART - wide(&label));
            format!("{open}{} {label} {after}{opened}", bar.repeat(BEFORE))
        } else {
            format!("{open}{}{opened}", bar.repeat(inner))
        };

        Row::new().then(Prompt::BORDER, clip(&drawn, columns).to_owned())
    }

    /// The row inside the frame: the mark, one hidden mark per character held
    /// as far as the frame has room for, and the right edge.
    fn boxed(&self, columns: usize, glyphs: Glyphs) -> Row {
        let edge = glyphs.vertical();
        let Some(room) = columns.checked_sub(EDGES + POINTING) else {
            return Row::new().then(Prompt::BORDER, clip(edge, columns));
        };

        let mut row = Row::new()
            .then(Prompt::BORDER, edge)
            .then(Slot::Plain, " ")
            .then(Slot::Accent, glyphs.caret())
            .then(Slot::Plain, " ")
            .then(Slot::Plain, glyphs.hidden().repeat(self.held.min(room)));
        row.pad(columns - 1);
        row.push(Prompt::BORDER, edge);
        row
    }

    /// The bottom border.
    fn closing(columns: usize, glyphs: Glyphs) -> Row {
        let (close, closed) = glyphs.bottom();
        let drawn = format!(
            "{close}{}{closed}",
            glyphs.horizontal().repeat(columns.saturating_sub(EDGES))
        );

        Row::new().then(Prompt::BORDER, clip(&drawn, columns).to_owned())
    }

    /// Where the caret stands on the row inside the frame: after the last dot,
    /// and on the right edge once the dots reach it.
    fn column(&self, columns: usize) -> usize {
        let room = columns.saturating_sub(EDGES + POINTING);
        (1 + POINTING + self.held.min(room)).min(columns.saturating_sub(1))
    }

    /// The row under the box: what Enter does now, and what escape does.
    fn footer(&self, columns: usize, glyphs: Glyphs) -> String {
        let (does, leaves) = if self.held == 0 {
            EMPTY_FOOTER
        } else {
            HELD_FOOTER
        };

        let full = format!("{does} {} {leaves}", glyphs.dot());
        if wide(&full) <= columns {
            return full;
        }
        let action = if self.held == 0 {
            "paste key"
        } else {
            "enter save"
        };
        let short = format!("{action} {} {leaves}", glyphs.dot());
        if wide(&short) <= columns {
            short
        } else {
            leaves.into()
        }
    }
}

#[cfg(test)]
mod tests;
