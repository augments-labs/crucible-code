//! The head component: the row above everything, saying what this session is.
//!
//! One row, at the top of the window, held there while everything under it
//! moves. Two facts stand on it and neither of them is about what was said:
//! which model the next turn goes to, and the directory this session is bound
//! to. They are the answers to *what am I talking to* and *where am I*, and a
//! reader who has scrolled a long way back is the reader most likely to want
//! them.
//!
//! The directory is the reason the row exists. It used to be said once, on the
//! welcome card, which is the first thing a session scrolls away — and a
//! terminal window says nothing about which checkout is in it, so a second
//! crucible open beside the first was two identical screens. The model came up
//! here to meet it: it was already permanent, at the far end of the status row,
//! and a fact said in two places is a fact that will disagree with itself.
//!
//! Two ends rather than one sentence, the way the status row under the box is
//! laid out and for the same reason: a path is as long as somebody's home
//! directory happens to make it, and joined to the model it would move the
//! model along the row every time the reader changed directory.
//!
//! Which of them gives way is the other way round from every other row here.
//! The model goes whole or not at all, as it does under the box; the path is
//! shortened rather than dropped, because a path is the one thing on this
//! screen that has a useful end — the leaf is what tells two checkouts apart,
//! and the columns before it are the same for every project somebody keeps in
//! one place.
//!
//! Like [`crate::Welcome`] this returns a [`Row`] and draws nothing, so every
//! width is asserted with no terminal attached.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width;

/// At least this much between the model and the path, so the two never read as
/// one sentence.
const APART: usize = 2;

/// What the head row says.
///
/// Every field is already spelled the way it will be drawn: this crate names no
/// domain type and settles no colour.
#[derive(Debug, Clone, Copy)]
pub struct Head<'a> {
    /// The vendor the model is asked of, drawn before it. Empty where nothing
    /// has chosen one, and then nothing is drawn in its place.
    pub provider: &'a str,
    /// Which model the next turn goes to. Empty where there is none to say, and
    /// then the path has the row to itself.
    pub model: &'a str,
    /// How hard that model is being asked to think, after it. `None` where no
    /// rung is in force — the vendor's own default is not this program's to
    /// name, and a rung drawn here that was never sent is worse than no rung at
    /// all.
    pub effort: Option<&'a str>,
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
        let named = self.asked(glyphs);
        let wide = width::columns(&named);

        // The model is whole or not at all, as it is under the box — and only
        // where what is left of the row still holds the end of the path. A path
        // cut in the middle of a segment says less than the model that
        // displaced it, so where those are the choice the model is what goes.
        let place = (wide > 0 && wide + APART < columns)
            .then(|| tail(self.root, columns - wide - APART, glyphs))
            .flatten();

        let Some(place) = place else {
            let alone = tail(self.root, columns, glyphs)
                .unwrap_or_else(|| width::clip(self.root, columns).to_owned());

            return Row::new().then(Slot::Quiet, alone);
        };

        let mut row = Row::new().then(Slot::Quiet, place);
        row.pad(columns - wide);
        row.push(Slot::Quiet, named);

        row
    }

    /// Whose model it is, which model, and the rung it is being asked on, as
    /// one string.
    ///
    /// Joined here rather than by the caller so that the dot comes out of the
    /// set in force, and so that a session with nothing chosen says nothing at
    /// all rather than naming a vendor over an empty name. The vendor is joined
    /// the way [`crate::Welcome`] joins it and the way `--model` takes it back,
    /// so the fact reads the same wherever it is said.
    fn asked(&self, glyphs: Glyphs) -> String {
        if self.model.is_empty() {
            return String::new();
        }

        let named = if self.provider.is_empty() {
            self.model.to_owned()
        } else {
            format!("{}/{}", self.provider, self.model)
        };

        match self.effort {
            Some(effort) => format!("{named} {} {effort}", glyphs.dot()),
            None => named,
        }
    }
}

/// `path` shortened to `columns`, keeping the end of it.
///
/// Whole segments go, rather than characters, so what is left still reads as a
/// path rather than as a string that was cut. Both separators are recognised
/// because the caller hands over whatever its own system spells a path with.
///
/// `None` where not even the last segment fits, which is the answer that costs
/// the model its place on the row: it says the end of the path cannot be kept
/// in the columns offered, and the caller is the one holding more of them.
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
