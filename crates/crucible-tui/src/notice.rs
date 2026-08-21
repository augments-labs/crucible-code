//! A ruled block that says one thing about the session rather than about the
//! work.
//!
//! Drawn between the welcome and the first prompt, which is the one place a
//! sentence can be put where it is read once and then scrolls away with
//! everything else. Two rules and a heading, because what it says is true of the
//! whole session and not of the row above it — the rules are what stop it from
//! reading as the last line of the welcome or the first line of a turn.
//!
//! Like [`crate::Welcome`] this returns [`Row`]s and draws nothing, so its
//! layout is decided without a terminal anywhere near it.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width::{clip, fold};

/// Something worth saying before the session starts.
#[derive(Debug, Clone, Copy)]
pub struct Notice<'a> {
    /// The few words that say what kind of thing this is.
    pub heading: &'a str,
    /// The sentence under it.
    pub said: &'a str,
    /// What to type or open, drawn in the accent so it can be found again in a
    /// screen of prose. Nothing is drawn in its place when there is none.
    pub named: Option<&'a str>,
}

impl Notice<'_> {
    /// The block, drawn for a terminal `columns` wide.
    ///
    /// Never wider than that anywhere: a row past the last column is one the
    /// terminal wraps itself, which puts the cursor a row below where the next
    /// frame expects it.
    #[must_use]
    pub fn rows(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let rule = || Row::new().then(Slot::Quiet, glyphs.horizontal().repeat(columns));

        let mut rows = vec![
            rule(),
            Row::new().then(Slot::Strong, clip(self.heading, columns)),
        ];
        rows.extend(self.sentence(columns));
        rows.push(rule());
        rows
    }

    /// The sentence, with whatever is to be typed picked out of it.
    ///
    /// One row where both halves fit on one, and otherwise the prose wrapped
    /// with what is named on a row of its own. A URL or a command is the half
    /// somebody copies, and one broken across two rows is one that has to be
    /// retyped by hand.
    fn sentence(&self, columns: usize) -> Vec<Row> {
        let Some(named) = self.named else {
            return prose(self.said, columns, Slot::Plain);
        };

        let named = clip(named, columns);
        let together = crate::width::columns(self.said) + 1 + crate::width::columns(named);

        if together <= columns {
            return vec![
                Row::new()
                    .then(Slot::Plain, self.said)
                    .then(Slot::Plain, " ")
                    .then(Slot::Accent, named),
            ];
        }

        let mut rows = prose(self.said, columns, Slot::Plain);
        rows.push(Row::new().then(Slot::Accent, named));
        rows
    }
}

/// A sentence as however many rows it takes.
fn prose(said: &str, columns: usize, slot: Slot) -> Vec<Row> {
    fold(said, columns)
        .into_iter()
        .map(|row| Row::new().then(slot, row))
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::color::Palette;
    use crate::dump::dump;

    use super::*;

    fn drawn(notice: &Notice<'_>, columns: usize) -> Vec<String> {
        notice
            .rows(columns, Glyphs::Unicode)
            .iter()
            .map(Row::text)
            .collect()
    }

    fn update() -> Notice<'static> {
        Notice {
            heading: "Update Available",
            said: "New version 0.1.0 is available. Download it from",
            named: Some("https://example.test/releases"),
        }
    }

    /// One row of what was drawn, named rather than indexed.
    fn at(rows: &[String], row: usize) -> &str {
        rows.get(row).map_or("<no such row>", String::as_str)
    }

    /// The notice at `columns`, against the picture checked in beside it under
    /// `name@columns`.
    ///
    /// The width is the suffix rather than something written into the name, so
    /// that a picture cannot end up checked against a drawing of some other
    /// terminal: one argument decides both what was drawn and which file it is
    /// read from.
    fn pictured(name: &str, notice: &Notice<'_>, columns: usize, glyphs: Glyphs) {
        insta::with_settings!({snapshot_suffix => columns.to_string()}, {
            insta::assert_snapshot!(name, dump(&notice.rows(columns, glyphs), columns));
        });
    }

    #[test]
    fn a_notice_is_a_heading_and_a_sentence_between_two_rules() {
        // Two widths, because the sentence is the part that moves: one that has
        // room for the prose and what is to be typed on a single row, and one
        // that does not.
        pictured("fits", &update(), 80, Glyphs::Unicode);
        pictured("wrapped", &update(), 34, Glyphs::Unicode);
    }

    #[test]
    fn a_font_without_box_drawing_rules_the_block_off_with_what_it_has() {
        // The rules are the whole of the frame here, so the set reaches all of
        // it: a block that kept one drawing character would be the one row of
        // hollow squares on that terminal.
        pictured("ascii", &update(), 80, Glyphs::Ascii);
    }

    #[test]
    fn nothing_is_ever_drawn_past_the_last_column() {
        // A row wider than the terminal is wrapped by the terminal, into a
        // second row of a band that was given room for one.
        for columns in [1, 2, 8, 20, 40, 79, 80, 200] {
            for row in drawn(&update(), columns) {
                assert!(crate::width::columns(&row) <= columns, "{columns}: {row:?}");
            }
        }
    }

    #[test]
    fn what_is_to_be_typed_stays_on_one_row_when_the_prose_no_longer_fits() {
        // A URL broken across two rows is one that has to be retyped by hand,
        // so the prose wraps and the URL takes a row of its own.
        let rows = drawn(&update(), 34);

        assert!(
            rows.iter()
                .any(|row| row == "https://example.test/releases"),
            "{rows:?}"
        );
        assert!(rows.len() > 4, "the prose should have wrapped: {rows:?}");
    }

    #[test]
    fn a_notice_with_nothing_to_type_is_the_sentence_alone() {
        let notice = Notice {
            heading: "Warning",
            said: "no model is selected",
            named: None,
        };

        assert_eq!(at(&drawn(&notice, 40), 2), "no model is selected");
    }

    #[test]
    fn the_heading_is_the_one_row_drawn_in_bold() {
        // What a notice is for is being noticed. Read back through the palette
        // rather than by inspecting slots, because the escape is what the user
        // meets.
        let palette = Palette::plain();
        let rows = update().rows(80, Glyphs::Unicode);
        let heading = rows.get(1).expect("a heading");

        assert!(!heading.paint(&palette).is_empty());
    }
}
