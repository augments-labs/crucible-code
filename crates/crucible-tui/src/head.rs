//! The head component: the row above everything, saying where this session is.
//!
//! One row, at the top of the window, held there while everything under it
//! moves. One fact stands on it and it is not about what was said: the
//! directory this session is bound to. It is the answer to *where am I*, and it
//! is the answer a terminal window cannot give — nothing in a title bar says
//! which checkout is in it, so a second crucible open beside the first was two
//! identical screens.
//!
//! It used to be said once, on the welcome card, which is the first thing a
//! session scrolls away. Which model the next turn goes to is said under the
//! box instead, on the row beside the keys that change it — a fact said in two
//! places is a fact that will disagree with itself, and both rows are held in
//! place, so neither of them is the one that survives a scroll.
//!
//! The path is shortened rather than dropped, because a path is the one thing
//! on this screen that has a useful end — the leaf is what tells two checkouts
//! apart, and the columns before it are the same for every project somebody
//! keeps in one place. At the far end stands `transcript map` with an opening
//! arrow, the door into the absolute map over what the session has said. It
//! gives way only where the row
//! is too narrow to carry a useful piece of the path beside it.
//!
//! Like [`crate::Welcome`] this returns a [`Row`] and draws nothing, so every
//! width is asserted with no terminal attached.

use std::ops::Range;

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width;

/// The door into absolute transcript travel.
///
/// Names the picture it opens and ends in the same mark a list uses to say a
/// row opens, so it is a control before a pointer has arrived to colour it.
const TRANSCRIPT: &str = "transcript map";

/// Room between the path and the door, so neither reads as part of the other.
const APART: usize = 2;

/// Enough of a path to keep a useful ending beside the door. Below this the
/// path gets the whole row and no control is offered half-drawn.
const PATH: usize = 20;

/// What the head row says.
///
/// Every field is already spelled the way it will be drawn: this crate names no
/// domain type and settles no colour.
#[derive(Debug, Clone, Copy)]
pub struct Head<'a> {
    /// The directory this session is bound to.
    pub root: &'a str,
}

impl Head<'_> {
    /// How many rows the head band asks for.
    ///
    /// One, always. It is a constant so that the caller sharing out the window
    /// asks the component rather than knowing the answer.
    pub const ROWS: usize = 1;

    /// The row, drawn for a terminal `columns` wide.
    #[must_use]
    pub fn row(&self, columns: usize, glyphs: Glyphs) -> Row {
        self.pointed(columns, glyphs, false)
    }

    /// The same row while the pointer is or is not over its transcript door.
    ///
    /// Only the door changes job: the working directory remains quiet, and the
    /// palette decides which hue `Accent` means for the configured theme.
    #[must_use]
    pub(crate) fn pointed(&self, columns: usize, glyphs: Glyphs, over: bool) -> Row {
        let label = format!("{TRANSCRIPT} {}", glyphs.stepping().1);
        let Some(control) = Self::transcript(columns) else {
            let said = tail(self.root, columns, glyphs)
                .unwrap_or_else(|| width::clip(self.root, columns).to_owned());
            return Row::new().then(Slot::Quiet, said);
        };

        let room = control.start.saturating_sub(APART);
        let said = tail(self.root, room, glyphs)
            .unwrap_or_else(|| width::clip(self.root, room).to_owned());
        let mut row = Row::new().then(Slot::Quiet, said);
        row.pad(control.start);
        row.push(if over { Slot::Accent } else { Slot::Quiet }, label);
        row
    }

    /// The columns the transcript door occupies, where this width has room for
    /// both it and something meaningful of the path.
    pub(crate) fn transcript(columns: usize) -> Option<Range<usize>> {
        // The mark at the end is one column in both glyph sets, so this
        // range is the same whichever spelling the row draws.
        let wide = width::columns(TRANSCRIPT) + 2;
        let start = columns.checked_sub(wide)?;
        (start >= PATH + APART).then_some(start..columns)
    }
}

/// `path` shortened to `columns`, keeping the end of it.
///
/// Whole segments go, rather than characters, so what is left still reads as a
/// path rather than as a string that was cut. Both separators are recognised
/// because the caller hands over whatever its own system spells a path with.
///
/// `None` where not even the last segment fits, and then the caller clips what
/// it has: a row that says the front of a path is still better than an empty
/// one at the width where nothing else would fit either.
fn tail(path: &str, columns: usize, glyphs: Glyphs) -> Option<String> {
    if width::columns(path) <= columns {
        return Some(path.to_owned());
    }

    let mark = glyphs.ellipsis();
    let room = columns.checked_sub(width::columns(mark))?;

    let mut from = 0;
    while let Some(found) = path[from..].find(['/', '\\']) {
        let at = from + found;

        // Kept from the separator rather than from after it, so what is left
        // still opens the way a path does.
        if width::columns(&path[at..]) <= room {
            return Some(format!("{mark}{}", &path[at..]));
        }

        from = at + 1;
    }

    None
}

#[cfg(test)]
mod tests;
