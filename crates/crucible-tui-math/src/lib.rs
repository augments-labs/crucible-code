//! TeX math in an answer drawn as text: compact text in a line, rows of text for a block.
//!
//! [`Formulas`] reads the TeX between the markers the reader found, never
//! markdown, and draws it the way it reads on paper as nearly as a terminal's
//! characters allow: `\frac{a}{b}` stacked over a rule in a block and `a/b` in
//! a line, scripts raised where a script character exists, Greek and operators
//! as their characters, matrices and cases between their bracket pieces.
//!
//! What this crate holds to:
//!
//! - **The formula or nothing.** A formula with any part this does not know (a
//!   command, an environment, a group left open, a character that means
//!   something only to TeX) is refused whole, and the reader shows it as it
//!   was written: a drawing that is half text and half TeX reads as neither.
//!   A whole document (`\documentclass`) is such a part.
//! - **Bounded before it is read.** A formula past [`MOST_BYTES`] is refused
//!   before it is parsed; its nesting stops at [`MOST_DEPTH`], and a layout
//!   past [`MOST_ROWS`] rows or [`MOST_COLUMNS`] columns is refused rather than
//!   built further.
//! - **Hostile text refused, not drawn.** A control, bidirectional or
//!   invisible formatting character in a formula refuses it, so nothing a
//!   terminal would act on reaches a row through it, and nothing it names (a
//!   file, an address) is opened.
//! - **ASCII where it is asked for.** Under the ASCII glyph set the frame, the
//!   fraction rules, the bracket pieces and the minus sign are ASCII: a double
//!   bar is two bars and an angle bracket the less-than or greater-than sign,
//!   and a ceiling or a floor, which no ASCII character draws, refuses the
//!   formula. The letters, operators, roots and raised scripts of the formula
//!   are its content and stay.
//! - **Every row fits.** A block wider than its room is refused for that room,
//!   never cut, so a narrower window shows it as written.

mod convert;

#[cfg(test)]
mod fits;
#[cfg(test)]
mod tests;

use crucible_tui::chrome::{Frame, Label, Undrawn};
use crucible_tui::drawing::{Draws, Laid, Room, Run};
use crucible_tui::{Row, Slot, columns};

pub use convert::{MOST_BYTES, MOST_COLUMNS, MOST_DEPTH, MOST_ROWS};

use convert::Refused;

/// The columns a frame's gutter takes before each row of a formula.
const GUTTER: usize = 2;

/// What the frame of a display formula is called.
const LABEL: &str = "math";

/// TeX math drawn as text: the drawer for a display block, a math fence and an
/// inline span.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Formulas;

impl Draws for Formulas {
    fn names(&self) -> &'static [&'static str] {
        &["math", "latex", "tex", "katex"]
    }

    fn label(&self, _: &str, _: &str) -> Option<&'static str> {
        Some("a formula")
    }

    fn inline(&self, span: &str, room: &Room) -> Option<Vec<Run>> {
        let text = convert::inline(span, room.glyphs).ok()?;
        Some(vec![Run {
            slot: Slot::Emphasis,
            text,
        }])
    }

    fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn> {
        let lines = convert::display(source, room.glyphs).map_err(|refused| match refused {
            Refused::TooLong | Refused::TooLarge => Undrawn::PastCap,
            Refused::Unclean => Undrawn::NotText,
            Refused::Empty => Undrawn::NotHere,
        })?;
        let widest = lines.iter().map(|line| columns(line)).max().unwrap_or(0);
        if widest.saturating_add(GUTTER) > room.columns {
            return Err(Undrawn::TooWide);
        }
        let inner: Vec<Row> = lines
            .into_iter()
            .map(|line| Row::new().then(Slot::Plain, line))
            .collect();
        let frame = Frame {
            label: Label::new(LABEL, room.glyphs),
            refusal: None,
        };
        let rows = frame.rows(room.columns, &inner);
        let natural = (
            widest.saturating_add(GUTTER).max(columns(&frame.top())),
            rows.len(),
        );
        Ok(Laid {
            rows,
            natural,
            narrow: None,
        })
    }
}
