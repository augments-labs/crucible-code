//! The frame the rows go inside.
//!
//! One left edge and one right edge: every row a card emits starts at column
//! zero and ends at the last column the terminal has. Nothing is indented to
//! look tidy, because an indent is a second margin, and two margins on one
//! screen read as a mistake rather than as a choice.

use crate::color::Slot;
use crate::row::Row;
use crate::width;

use crate::glyphs::Glyphs;

/// The name the frame carries, which is also the program's.
const NAME: &str = "crucible";

/// What the top row spends on something other than the fill: the corner and the
/// column of edge before the name, the three spaces around the name and the
/// version, and the corner at the end.
const AROUND: usize = 6;

/// What a row of a single column spends on the frame: an edge and a space on
/// each side.
pub(super) const STACKED: usize = 4;

/// What a row of two columns spends on the frame: three edges, and a space on
/// each side of each of the two columns.
pub(super) const PAIRED: usize = 7;

/// The top edge, carrying the name and the version.
///
/// The name is loud and the version is quiet, because one of them is what you
/// are looking at and the other is what you check when something is wrong.
pub(super) fn top(version: &str, columns: usize, glyphs: Glyphs) -> Row {
    let (left, right) = glyphs.top();
    let bar = glyphs.horizontal();
    let fill = columns.saturating_sub(AROUND + width::columns(NAME) + width::columns(version));

    Row::new()
        .then(Slot::Accent, format!("{left}{bar} "))
        .then(Slot::Strong, NAME)
        .then(Slot::Plain, " ")
        .then(Slot::Quiet, version)
        .then(Slot::Accent, format!(" {}{right}", bar.repeat(fill)))
}

/// The bottom edge, which carries nothing.
pub(super) fn bottom(columns: usize, glyphs: Glyphs) -> Row {
    let (left, right) = glyphs.bottom();
    let bar = glyphs.horizontal().repeat(columns.saturating_sub(2));

    Row::new().then(Slot::Accent, format!("{left}{bar}{right}"))
}

/// One row of a card holding a single column.
pub(super) fn framed(row: Row, inner: usize, glyphs: Glyphs) -> Row {
    edge(glyphs)
        .then(Slot::Plain, " ")
        .join(padded(row, inner))
        .then(Slot::Plain, " ")
        .join(edge(glyphs))
}

/// One row of a card holding two, with the edge that parts them.
pub(super) fn pair(left: Row, right: Row, identity: usize, aside: usize, glyphs: Glyphs) -> Row {
    edge(glyphs)
        .then(Slot::Plain, " ")
        .join(padded(left, identity))
        .then(Slot::Plain, " ")
        .join(edge(glyphs))
        .then(Slot::Plain, " ")
        .join(padded(right, aside))
        .then(Slot::Plain, " ")
        .join(edge(glyphs))
}

/// One column of edge running down.
fn edge(glyphs: Glyphs) -> Row {
    Row::new().then(Slot::Accent, glyphs.vertical())
}

/// `row`, out to the full width of the column it sits in, so that the edge on
/// its right lands where every other row put one.
fn padded(mut row: Row, columns: usize) -> Row {
    row.pad(columns);
    row
}
