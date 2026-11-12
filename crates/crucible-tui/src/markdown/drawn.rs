//! A closed fence handed to the drawer registered for its name.
//!
//! The reader holds a fence and hands it on whole; what lays the block out
//! asks here first whether a drawer in the [`Drawers`] it was given takes the
//! fence's name, and draws that drawer's rows in place of the fence as it was
//! written.
//!
//! Invariants:
//!
//! - An empty registry asks nothing, so every block is laid out exactly as it
//!   is with no drawer at all.
//! - Only a fence the answer closed is handed to a drawer. One the answer
//!   ended inside is shown as written.
//! - The drawer is found by the first word of the fence's info string, the
//!   word the reader holds the fence by, and is handed the lines between the
//!   two fence lines and neither fence line, with the room the block is laid
//!   out in. It is asked again at every room.
//! - A drawer that says the fence is not its to draw leaves it as written,
//!   with nothing said of it. One that says it is past what it holds leaves
//!   it as written in the frame, and with the words, the reader gives a fence
//!   past its own hold.
//! - A drawer's rows are laid out as it made them, but for a key they offer
//!   where none is read. [`Offers`] says which place the rows are going to:
//!   on a screen the offer stands, and for a page printed once, a run whose
//!   output is a file and a preview of a session, the drawer that drew the
//!   rows takes its offers out of them through [`Draws::unoffered`]. Rows no
//!   drawer drew are never handed to one.
//! - Told to a page printed once a drawer's rows are their runs with no
//!   address: a row gives its text and its slots, and not where a run links.

use super::{Block, Kind, Links, Markdown};
use crate::chrome::Undrawn;
use crate::color::Slot;
use crate::drawing::{Drawers, Draws, Room};
use crate::glyphs::Glyphs;
use crate::row::Row;

/// What asking the drawers about a block came to.
pub(super) enum Asked {
    /// A drawer took the fence and laid it out as these rows.
    Drawn(Vec<Row>),
    /// No drawer drew it: it is shown as written, under this refusal where a
    /// drawer gave one worth saying.
    Written(Option<Undrawn>),
}

/// What becomes of a key a drawer's rows offer, where the rows are going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Offers {
    /// They stand: the rows are for a screen whose keys are read.
    Stand,
    /// The drawer takes them out: no key is read where the rows are going.
    Withdrawn,
}

/// `block` as the drawer in `drawers` that takes its name lays it out in
/// `room`, its `offers` standing or taken out by it, or that it is to be
/// shown as written.
pub(super) fn asked(block: &Block, room: Room, drawers: &Drawers, offers: Offers) -> Asked {
    let Some((drawer, body)) = taken(block, drawers) else {
        return Asked::Written(None);
    };
    match drawer.block(body, &room) {
        Ok(mut laid) => {
            match offers {
                Offers::Stand => {}
                Offers::Withdrawn => drawer.unoffered(&mut laid.rows),
            }
            Asked::Drawn(laid.rows)
        }
        Err(Undrawn::NotHere) => Asked::Written(None),
        Err(refusal @ (Undrawn::PastCap | Undrawn::NotText | Undrawn::TooWide)) => {
            Asked::Written(Some(refusal))
        }
    }
}

/// `block` laid out as [`super::lay`] lays it, for where no key is read: a
/// run whose output is a file, and the rows such a run keeps, which are what
/// a preview of a session shows. A drawer takes its offers of a key out.
#[must_use]
pub(crate) fn lay_unoffered(
    block: &Block,
    room: usize,
    glyphs: Glyphs,
    links: &Links,
    drawers: &Drawers,
) -> Vec<Row> {
    let room = Room {
        columns: room,
        glyphs,
    };
    super::laid(block, room, links, drawers, Offers::Withdrawn)
}

/// The drawer that takes `block`, and the lines between its two fence lines.
///
/// `None` for anything but a fence the answer closed: the source of one is
/// its opening line, its lines, and the line that closed it, each ended.
fn taken<'a>(block: &'a Block, drawers: &'a Drawers) -> Option<(&'a dyn Draws, &'a str)> {
    if !block.closed || drawers.is_empty() {
        return None;
    }
    match block.kind {
        Kind::DisplayMath | Kind::Mermaid | Kind::Diff => {}
        Kind::Table | Kind::Rule | Kind::Heading(_) | Kind::Image => return None,
    }
    let (opening, rest) = block.source.split_once('\n')?;
    // Display math between dollars is held too, and is no fence.
    let mark = opening
        .chars()
        .next()
        .filter(|mark| matches!(mark, '`' | '~'))?;
    let info = opening.trim_start_matches(mark);
    let drawer = drawers.for_name(info.split_whitespace().next()?)?;
    let lines = rest.strip_suffix('\n')?;
    let body = match lines.rfind('\n') {
        Some(end) => rest.get(..=end)?,
        None => "",
    };
    Some((drawer, body))
}

/// `rows` as the runs a page printed once is told, a line's end as a break in
/// the text.
pub(super) fn told(rows: &[Row], put: &mut dyn FnMut(Slot, &str, Option<&str>)) {
    for (at, row) in rows.iter().enumerate() {
        if at > 0 {
            put(Slot::Plain, "\n", None);
        }
        for (slot, text) in row.spans() {
            put(slot, text, None);
        }
    }
}

impl Markdown {
    /// This reader, drawing `refusal` on the frame of the fence it opens with.
    pub(super) fn refusing(mut self, refusal: Option<Undrawn>) -> Self {
        self.refusal = refusal;
        self
    }
}
