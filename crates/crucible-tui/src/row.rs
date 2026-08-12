//! A row as slots rather than as bytes.
//!
//! What a component builds. A span is a run of text that is all one [`Slot`], so
//! a row can be measured before it is coloured and coloured without being
//! measured again: display width is a property of the text alone, and the
//! escape sequences arrive at the last moment, from the palette, on the way to
//! the terminal.
//!
//! That separation is also what makes a component testable with no terminal
//! attached. [`Row::text`] is what the row says and [`Row::paint`] is what a
//! terminal is sent — the art is asserted in one place and the colour in
//! another, and neither test has to read past the other's noise.
//!
//! It is the structure a renderer that is not a terminal would walk too. A row
//! of spans carrying a slot says what it means rather than how it looks, so the
//! part that would have to be rewritten to draw the same component somewhere
//! else is [`Row::paint`] and nothing above it.

use crate::color::{Palette, Slot};
use crate::width;

/// A run of text that is all one slot.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Span {
    /// What it says.
    text: String,
    /// The job the colour does, if the palette writes one.
    slot: Slot,
}

/// One row of a component: its spans, left to right.
///
/// Built once and then read. Nothing here is on the render path — a component
/// is drawn into scrollback and never redrawn — so a row owns its text rather
/// than borrowing what it was assembled from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row(Vec<Span>);

impl Row {
    /// An empty row.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A row that is `text`, in the reader's own foreground.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self::new().then(Slot::Plain, text)
    }

    /// The row with `text` appended in `slot`.
    #[must_use]
    pub fn then(mut self, slot: Slot, text: impl Into<String>) -> Self {
        self.push(slot, text);
        self
    }

    /// Appends `text` in `slot`.
    ///
    /// An empty span is dropped rather than kept. It would draw nothing and
    /// measure nothing, and painted it would be an opening sequence and a reset
    /// around no text at all.
    pub fn push(&mut self, slot: Slot, text: impl Into<String>) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        self.0.push(Span { text, slot });
    }

    /// The row with `other`'s spans appended.
    ///
    /// How a component puts a row it built inside a frame it built separately:
    /// the two were measured apart and stay measured, because appending spans
    /// cannot change what any of them says or costs.
    #[must_use]
    pub fn join(mut self, other: Self) -> Self {
        self.0.extend(other.0);
        self
    }

    /// Appends spaces until the row is `columns` wide.
    ///
    /// Does nothing to a row that is already at least that wide: padding is how
    /// a column reaches the divider on its right, and a row that has outgrown
    /// its column is one the caller has to shorten rather than one this can fix.
    pub fn pad(&mut self, columns: usize) {
        let short = columns.saturating_sub(self.columns());
        self.push(Slot::Plain, " ".repeat(short));
    }

    /// How many display columns the row costs.
    #[must_use]
    pub fn columns(&self) -> usize {
        self.0.iter().map(|span| width::columns(&span.text)).sum()
    }

    /// Whether the row says nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// What the row says, with no colour in it.
    ///
    /// What a redirected run is written, and what a test asserting on the art
    /// reads.
    #[must_use]
    pub fn text(&self) -> String {
        let mut said = String::with_capacity(self.bytes());
        for span in &self.0 {
            said.push_str(&span.text);
        }
        said
    }

    /// The row as a terminal is sent it.
    ///
    /// A slot the palette has no colour for — [`Slot::Plain`] always, and every
    /// slot once colour is off — writes its text and nothing around it, so a
    /// run that is not coloured costs no bytes rather than costing an empty
    /// pair of sequences.
    #[must_use]
    pub fn paint(&self, palette: Palette) -> String {
        let mut painted = String::new();
        self.paint_into(palette, &mut painted);
        painted
    }

    /// The same, appended to a buffer the caller keeps.
    ///
    /// What a component with a dozen rows to draw uses, so the whole of it
    /// costs the one allocation that buffer already grew to.
    pub fn paint_into(&self, palette: Palette, painted: &mut String) {
        // Room for the sequences as well, so a row of a dozen spans does not
        // grow the buffer twice. Over-reserving by a few bytes is cheaper than
        // either.
        painted.reserve(if palette.writes_color() {
            self.bytes() + self.0.len() * PAINTED
        } else {
            self.bytes()
        });

        for span in &self.0 {
            let open = palette.open(span.slot);
            painted.push_str(open);
            painted.push_str(&span.text);
            if !open.is_empty() {
                painted.push_str(palette.close());
            }
        }
    }

    /// How many bytes the text of this row is.
    fn bytes(&self) -> usize {
        self.0.iter().map(|span| span.text.len()).sum()
    }
}

/// Bytes to reserve per coloured span: the longest opening sequence the palette
/// writes is `\x1b[38;2;255;255;255m`, and every one of them is closed.
const PAINTED: usize = 24;

#[cfg(test)]
mod tests {
    use super::*;

    /// A palette that writes every hue it has, without an environment to say so.
    fn colourful() -> Palette {
        Palette::resolve(true, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        })
    }

    #[test]
    fn a_row_measures_what_it_says_and_not_what_it_is_painted_as() {
        // The property the whole separation rests on: colour is bytes, not
        // columns, so a row padded to a width stays that width once it is
        // coloured. A row that grew when it was painted is a row the terminal
        // wraps somewhere this process did not predict.
        let row = Row::new()
            .then(Slot::Accent, "crucible")
            .then(Slot::Plain, " ")
            .then(Slot::Quiet, "v0.0.8");

        assert_eq!(row.columns(), "crucible v0.0.8".len());
        assert_eq!(width::columns(&row.paint(colourful())), row.columns());
        assert_eq!(width::columns(&row.paint(Palette::plain())), row.columns());
    }

    #[test]
    fn a_row_says_the_same_thing_however_it_is_painted() {
        let row = Row::new()
            .then(Slot::Strong, "Tips")
            .then(Slot::Plain, ": ")
            .then(Slot::Accent, "/");

        assert_eq!(row.text(), "Tips: /");
        assert_eq!(row.paint(Palette::plain()), "Tips: /");
        assert!(row.paint(colourful()).contains("Tips"));
    }

    #[test]
    fn a_run_the_palette_has_no_colour_for_costs_no_bytes() {
        // Plain is the reader's own foreground at every rung, so a row of it
        // is the same bytes coloured or not — which is what keeps the wordmark
        // and the padding from paying for a palette they do not use.
        let row = Row::plain("   ~/code   ");

        assert_eq!(row.paint(colourful()), row.text());
    }

    #[test]
    fn a_row_is_padded_to_a_width_it_has_not_already_passed() {
        let mut row = Row::plain("ask");
        row.pad(10);
        assert_eq!(row.text(), "ask       ");
        assert_eq!(row.columns(), 10);

        // Already past it: shortening is the caller's decision, and silently
        // taking columns off here would corrupt a border rather than fix one.
        let mut row = Row::plain("a much longer answer");
        row.pad(4);
        assert_eq!(row.text(), "a much longer answer");
    }

    #[test]
    fn a_wide_character_is_two_of_the_columns_a_row_is_padded_to() {
        let mut row = Row::plain("日本語");
        row.pad(10);

        assert_eq!(row.columns(), 10);
        assert_eq!(row.text(), "日本語    ");
    }

    #[test]
    fn a_span_with_nothing_in_it_is_not_a_span() {
        let row = Row::new()
            .then(Slot::Accent, "")
            .then(Slot::Plain, "there")
            .then(Slot::Quiet, "");

        assert_eq!(
            row.paint(colourful()),
            colourful().open(Slot::Plain).to_owned() + "there"
        );
        assert!(!row.is_empty());
        assert!(Row::new().is_empty());
    }

    #[test]
    fn a_coloured_span_is_closed_where_it_ends() {
        // An attribute left open runs on into whatever is drawn next, which for
        // the last row of a component is the reader's own transcript.
        let painted = Row::plain("root: ")
            .then(Slot::Accent, "~/code")
            .paint(colourful());

        assert!(painted.ends_with(colourful().close()), "{painted:?}");
    }
}
