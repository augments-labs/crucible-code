//! What a component drew, as a picture a person can read.
//!
//! Every component here returns [`Row`]s and leaves colour to [`Row::paint`],
//! so what one drew can be read back with no terminal attached. This writes
//! that back as text: what the rows say, with every run that asked for a colour
//! wrapped in the *slot* it asked for — `<Accent>…</>`, `<Quiet>…</>`.
//!
//! The slot rather than the hue, because the slot is the part the component
//! chose. What that is worth in bytes is the palette's answer about the
//! terminal in front of it, and a picture full of `\x1b[38;2;18;137;127m` is one
//! nobody reads: the whole point of checking a drawing in beside its component
//! is that the diff shows what moved on screen.
//!
//! Tests only, and it is where a test may reach for a snapshot at all: a
//! picture is the right assertion where the thing under test *is* a picture,
//! and the wrong one for a rule. "No row is ever drawn past the last column" is
//! a property, it is tested as one, and a snapshot of it would assert today's
//! answer rather than the rule — so it would go green the day somebody accepted
//! a wrong one.

use crate::color::Slot;
use crate::row::Row;

/// What closes a run.
///
/// The slot is named where the run opens and not again where it ends. Naming it
/// twice would put more markup than art on the rows that carry the most colour,
/// and there is nothing between the two ends for a reader to lose track of.
const CLOSE: &str = "</>";

/// The rows as one block of text, each padded out to `columns`.
///
/// The padding is this module's own rather than a component's. It stands every
/// right edge in one column, so a row that outgrew the width sticks out of the
/// block instead of hiding at the end of a line — and a row of nothing is a row
/// of spaces rather than an empty line, which is what it will be on screen.
///
/// What a component pads for *itself* is not read off this: the picture shows a
/// rectangle either way. That is a rule, and it is asserted as one where the
/// component is.
pub(crate) fn dump(rows: &[Row], columns: usize) -> String {
    let mut dumped = String::new();

    for row in rows {
        for (slot, text) in row.spans() {
            match marked(slot) {
                Some(name) => {
                    dumped.push('<');
                    dumped.push_str(name);
                    dumped.push('>');
                    dumped.push_str(text);
                    dumped.push_str(CLOSE);
                }
                None => dumped.push_str(text),
            }
        }

        dumped.push_str(&" ".repeat(columns.saturating_sub(row.columns())));
        dumped.push('\n');
    }

    dumped
}

/// What a run in `slot` is wrapped in, or nothing where it is wrapped in
/// nothing.
///
/// The variant's own name, so the picture and the code that built it are read
/// with one word. [`Slot::Plain`] is the one that is never written: it is the
/// reader's own foreground, the palette writes no bytes for it, and most of
/// every component is plain — tagged, the wordmark and the padding would arrive
/// buried in markup that says only that they were left alone.
fn marked(slot: Slot) -> Option<&'static str> {
    match slot {
        Slot::Plain => None,
        Slot::Accent => Some("Accent"),
        Slot::Strong => Some("Strong"),
        Slot::Quiet => Some("Quiet"),
        Slot::AllowEdits => Some("AllowEdits"),
        Slot::FullAccess => Some("FullAccess"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_is_named_by_the_slot_it_asked_for_and_plain_is_not_named_at_all() {
        let row = Row::new()
            .then(Slot::Accent, "›")
            .then(Slot::Plain, " ")
            .then(Slot::Strong, "crucible");

        assert_eq!(dump(&[row], 10), "<Accent>›</> <Strong>crucible</>\n");
    }

    #[test]
    fn a_picture_is_as_wide_as_the_width_it_was_drawn_for() {
        // Which is what makes a trailing space somebody added visible: every
        // other row ends in the same column, so the one that does not is the
        // one sticking out of the block.
        let rows = [Row::plain("ask"), Row::new()];

        assert_eq!(dump(&rows, 6), "ask   \n      \n");
    }

    #[test]
    fn a_row_that_outgrew_the_width_is_left_over_it_rather_than_cut() {
        // A picture that cut it would hide the one defect a picture of a
        // component is worth taking.
        assert_eq!(
            dump(&[Row::plain("a much longer answer")], 4),
            "a much longer answer\n"
        );
    }

    #[test]
    fn a_wide_character_is_two_of_the_columns_a_picture_is_padded_to() {
        // Padded by display width, like everything else here: counted in
        // characters, a row of CJK would arrive with twice the padding it needs
        // and every right edge in the block would be somewhere different.
        assert_eq!(dump(&[Row::plain("日本語")], 8), "日本語  \n");
    }
}
