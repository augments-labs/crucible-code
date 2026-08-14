//! The panel a provider is chosen from: a rule, a title, and a list of two-row
//! entries with one of them marked.
//!
//! Not [`crate::Menu`] with different data, which is its next reader's first
//! question. A menu row is a name and what it does on one line in two columns,
//! and what makes that work — one row per item, a column standing after the
//! longest name, a whole row greyed once the choice has passed over it — stops
//! working when an item is two rows.
//!
//! **Rows, not a screen.** This hands back rows and draws nothing itself. It
//! never asks how tall the terminal is, never assumes its rule is the first
//! thing on it, and takes its title, its sentence and its footer from the
//! caller. That is what lets it stand under a wordmark on a first run, with the
//! room that is left, instead of being copied and changed; a string written in
//! here is that reuse taken away.
//!
//! **What clips and what folds.** A description is a label, so it is cut and
//! ended in the ellipsis. The sentence is prose, so it folds — half an
//! explanation explains nothing, and this one runs to two rows at eighty
//! columns. A display name does neither: it is cut where a column ends and
//! never given an ellipsis, because a name is what has to be typed and an
//! ellipsis inside one would be typed with it.
//!
//! **Where the colour goes.** The mark is carried on the display-name row
//! alone — chosen [`Slot::Strong`], passed over [`Slot::Plain`], and every
//! description [`Slot::Quiet`] in every state. [`crate::Menu`] greys the whole
//! passed-over row, which reads correctly when a row is one line; here the
//! description is quiet by role already, so greying the name too would flatten
//! the entry.

use std::borrow::Cow;

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width::{clip, columns as wide, fold};

/// The room kept in front of *both* rows of every entry — the mark and the
/// space after it — so the names stand in one column as the mark moves down
/// them.
const POINTING: usize = 2;

/// One thing a panel offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offered<'a> {
    /// The display name, spelled the way the vendor spells it. What is typed
    /// and configured is the lowercase name, and it never reaches this crate.
    pub name: &'a str,
    /// What it is, on the row beneath.
    pub says: &'a str,
}

/// A titled list of two-row entries, one of them marked.
#[derive(Debug, Clone, Copy)]
pub struct Panel<'a> {
    /// The few words at the top saying what is being chosen.
    pub title: &'a str,
    /// The sentence under the title, where there is one worth reading, and
    /// `None` where the choice explains itself.
    pub said: Option<&'a str>,
    /// What to offer, in the order it is listed.
    pub shown: &'a [Offered<'a>],
    /// Which entry a key would act on.
    ///
    /// Not an `Option`, unlike [`crate::Menu`]'s: this is only ever a list
    /// being chosen from. There is no read-only panel, and an index past the
    /// end simply marks nothing.
    pub chosen: usize,
    /// The one key worth naming, on the last row.
    pub footer: &'a str,
}

impl Panel<'_> {
    /// The whole panel, drawn for a terminal `columns` wide.
    ///
    /// Never wider than that anywhere: a row past the last column is one the
    /// terminal wraps itself, which leaves the cursor a row below where the
    /// next frame expects it. Height is not considered — the caller is assumed
    /// to have room.
    #[must_use]
    pub fn rows(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        // No room for the mark is no list to choose from: at that width it
        // would stand where the name's first column is, and the name is the
        // part that has to be typed.
        let front = if columns > POINTING { POINTING } else { 0 };

        let mut rows = vec![
            Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
            Row::new(),
            Row::new().then(Slot::Strong, clip(self.title, columns)),
        ];

        rows.extend(self.sentence(columns));
        rows.push(Row::new());

        for (at, one) in self.shown.iter().enumerate() {
            if at > 0 {
                rows.push(Row::new());
            }
            rows.extend(entry(columns, front, glyphs, one, at == self.chosen));
        }

        rows.push(Row::new());
        rows.push(Row::new().then(Slot::Quiet, clip(self.footer, columns)));
        rows
    }

    /// The blank row and the sentence under the title, where there is one.
    ///
    /// Folded rather than clipped, and drawn in the reader's own foreground: it
    /// is the part of the panel that is read once and then not looked at again.
    fn sentence(&self, columns: usize) -> Vec<Row> {
        let folded = self
            .said
            .map(|said| fold(said, columns))
            .unwrap_or_default();

        if folded.is_empty() {
            return Vec::new();
        }

        let mut rows = vec![Row::new()];
        rows.extend(folded.into_iter().map(Row::plain));
        rows
    }
}

/// One entry: its display name on a row, what it is on the row beneath.
fn entry(
    columns: usize,
    front: usize,
    glyphs: Glyphs,
    one: &Offered<'_>,
    marked: bool,
) -> [Row; 2] {
    let room = columns - front;

    let pointed = marked && front > 0;
    let mut name = Row::new().then(Slot::Accent, if pointed { glyphs.caret() } else { "" });
    name.pad(front);
    let slot = if marked { Slot::Strong } else { Slot::Plain };
    name.push(slot, clip(one.name, room));

    let mut says = Row::new();
    says.pad(front);
    says.push(Slot::Quiet, shortened(one.says, room, glyphs));

    [name, says]
}

/// `text` in at most `room` columns, ending in the ellipsis where it did not
/// fit.
///
/// For prose about a name rather than for the name itself: half a description
/// with nothing to say it was cut reads as the whole of it. Where the room is
/// too narrow for even the ellipsis the text is simply cut, since a row that is
/// nothing but a mark saying something was dropped has dropped everything. One
/// string rather than two spans, because two spans of one slot are two escape
/// sequences handed to the terminal for one run of colour.
fn shortened(text: &str, room: usize, glyphs: Glyphs) -> Cow<'_, str> {
    if wide(text) <= room {
        return Cow::Borrowed(text);
    }

    let mark = glyphs.ellipsis();
    let Some(kept) = room.checked_sub(wide(mark)).filter(|kept| *kept > 0) else {
        return Cow::Borrowed(clip(text, room));
    };

    Cow::Owned(format!("{}{mark}", clip(text, kept)))
}

#[cfg(test)]
mod tests {
    use crate::color::Palette;
    use crate::dump::dump;

    use super::*;

    /// The sentence the login panel opens with, which is prose and runs to two
    /// rows at eighty columns.
    const SAID: &str = concat!(
        "Select a provider to use crucible as part of your subscription plan, ",
        "or billed based on API usage through your Console account."
    );

    /// What `/login` offers, in the order it is listed.
    ///
    /// Alphabetical, because no other order is defensible: recency would move
    /// the list under the reader, and any ranking of vendors is one this
    /// project would have to argue for.
    ///
    /// A description says what the key buys, since that is the part a reader is
    /// choosing between. It never says which plans a subscription covers — a
    /// plan is scoped to its vendor's own software, and a row of this panel
    /// claiming otherwise is the sentence somebody gets banned over.
    fn offered() -> [Offered<'static>; 3] {
        [
            Offered {
                name: "Anthropic",
                says: "Console API key, billed by usage",
            },
            Offered {
                name: "MoonshotAI",
                says: "Kimi Code plan, or a Platform key",
            },
            Offered {
                name: "OpenAI",
                says: "Console API key, billed by usage",
            },
        ]
    }

    /// The login panel over `shown`, with the mark on `chosen`.
    fn login<'a>(shown: &'a [Offered<'a>], chosen: usize) -> Panel<'a> {
        Panel {
            title: "Log in",
            said: Some(SAID),
            shown,
            chosen,
            footer: "esc to cancel",
        }
    }

    /// What the panel says, row by row.
    fn art(panel: &Panel<'_>, columns: usize, glyphs: Glyphs) -> Vec<String> {
        panel.rows(columns, glyphs).iter().map(Row::text).collect()
    }

    /// Which slots one row opened, in order.
    fn slots(row: &Row) -> Vec<Slot> {
        row.spans().map(|(slot, _)| slot).collect()
    }

    /// The panel at `columns`, against the picture checked in beside it under
    /// `name@columns`.
    ///
    /// The width is the suffix rather than something written into the name, so
    /// a picture cannot end up read against a drawing of some other terminal:
    /// one argument decides both what was drawn and which file it is read from.
    fn pictured(name: &str, panel: &Panel<'_>, columns: usize, glyphs: Glyphs) {
        insta::with_settings!({snapshot_suffix => columns.to_string()}, {
            insta::assert_snapshot!(name, dump(&panel.rows(columns, glyphs), columns));
        });
    }

    #[test]
    fn a_panel_is_a_rule_a_title_and_entries_between_blank_rows() {
        let shown = offered();
        let rows = art(&login(&shown, 0), 80, Glyphs::Unicode);

        assert_eq!(rows.first().map(String::as_str), Some(&"─".repeat(80)[..]));
        assert_eq!(rows.get(2).map(String::as_str), Some("Log in"));
        assert_eq!(rows.get(7).map(|row| row.trim_end()), Some("› Anthropic"));
        assert_eq!(rows.get(10).map(|row| row.trim_end()), Some("  MoonshotAI"));
        assert_eq!(rows.get(13).map(|row| row.trim_end()), Some("  OpenAI"));

        // The blanks that part the panel's parts.
        let parting = |at: &usize| rows.get(*at).is_some_and(|row| row.trim().is_empty());
        assert!([1, 3, 6, 9, 12, 15].iter().all(parting), "{rows:?}");
        assert_eq!(rows.last().map(String::as_str), Some("esc to cancel"));
    }

    #[test]
    fn a_second_row_says_what_the_first_one_is() {
        // The mark moves and the names do not move with it, which is what the
        // room in front of every row of every entry is for.
        let shown = offered();
        let first = art(&login(&shown, 0), 80, Glyphs::Unicode);
        let second = art(&login(&shown, 1), 80, Glyphs::Unicode);

        assert_eq!(first.get(7).map(|row| row.trim_end()), Some("› Anthropic"));
        assert_eq!(second.get(7).map(|row| row.trim_end()), Some("  Anthropic"));
        assert_eq!(
            first.get(10).map(|row| row.trim_end()),
            Some("  MoonshotAI")
        );
        assert_eq!(
            second.get(10).map(|row| row.trim_end()),
            Some("› MoonshotAI")
        );
    }

    #[test]
    fn a_window_too_narrow_for_a_description_ends_it_in_an_ellipsis() {
        let shown = offered();
        let rows = art(&login(&shown, 0), 34, Glyphs::Unicode);
        // Found under its name rather than at a row number, because how far
        // down it sits is how far the title's own sentence folded at this
        // width — which is not what this test is about.
        let named = rows
            .iter()
            .position(|row| row.trim_end() == "  MoonshotAI")
            .map_or(usize::MAX, |at| at + 1);
        let says = rows.get(named).map_or("<no such row>", String::as_str);

        assert!(says.ends_with('…'), "{says:?}");
        assert!(wide(says) <= 34, "{says:?}");
        assert!(says.starts_with("  Kimi Code plan,"), "{says:?}");
    }

    #[test]
    fn a_display_name_is_cut_where_a_column_ends_and_never_given_an_ellipsis() {
        // A name is what has to be typed, so an ellipsis inside one would be
        // typed with it. Cut at a column boundary instead, which for a wide
        // glyph is not a character boundary.
        let shown = [Offered {
            name: "日本語プロバイダ",
            says: "",
        }];
        let panel = Panel {
            said: None,
            ..login(&shown, 0)
        };

        let rows = art(&panel, 8, Glyphs::Unicode);
        let name = rows.get(4).map_or("<no such row>", String::as_str);

        assert_eq!(name, "› 日本語", "{rows:?}");
        assert_eq!(wide(name), 8, "{name:?}");
        assert!(!name.contains('…'), "{name:?}");
    }

    #[test]
    fn the_descriptions_are_quiet_in_every_state() {
        // Quiet by role, chosen or not.
        let shown = offered();
        let rows = login(&shown, 0).rows(80, Glyphs::Unicode);

        for at in [8, 11, 14] {
            let says = rows.get(at).expect("a description row");
            assert_eq!(slots(says), [Slot::Plain, Slot::Quiet], "{says:?}");
        }
    }

    #[test]
    fn a_passed_over_name_is_plain_rather_than_quiet() {
        // Habit writes Quiet here, because that is what a menu does to a row
        // the choice has passed over. Greying the name as well as the
        // description would flatten the entry and lose the difference between
        // the name and what it is.
        let shown = offered();
        let rows = login(&shown, 0).rows(80, Glyphs::Unicode);
        let passed = rows.get(10).expect("the name of a passed-over entry");

        assert!(slots(passed).contains(&Slot::Plain), "{passed:?}");
        assert!(!slots(passed).contains(&Slot::Quiet), "{passed:?}");

        // And no hue for it reaches the terminal either, on one that has them all.
        let hues = Palette::resolve(true, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        });
        let painted = passed.paint(hues);
        assert!(!painted.contains(hues.open(Slot::Quiet)), "{painted:?}");
    }

    #[test]
    fn the_chosen_name_is_drawn_strong() {
        // The twin of the test above, and the half of that rule a picture was
        // the only gate on: the entry the mark points at is emphasised, and it
        // is the name that carries it rather than the whole entry.
        let shown = offered();
        let rows = login(&shown, 0).rows(80, Glyphs::Unicode);
        let chosen = rows.get(7).expect("the name of the chosen entry");

        assert!(slots(chosen).contains(&Slot::Strong), "{chosen:?}");
    }

    #[test]
    fn a_font_without_the_mark_still_says_which_entry_it_is_on() {
        // The set changes what the mark is drawn with and nothing about the
        // column it stands in, so the names below it do not move either.
        let shown = offered();
        let rows = art(&login(&shown, 1), 80, Glyphs::Ascii);

        assert_eq!(rows.first().map(String::as_str), Some(&"-".repeat(80)[..]));
        assert_eq!(rows.get(7).map(|row| row.trim_end()), Some("  Anthropic"));
        assert_eq!(rows.get(10).map(|row| row.trim_end()), Some("> MoonshotAI"));
    }

    #[test]
    fn nothing_is_ever_drawn_past_the_last_column() {
        // A row wider than the terminal is one the terminal wraps itself, and
        // the frame after it then rewinds over a row this process did not
        // write.
        let shown = offered();

        for columns in 1..=80 {
            for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
                for chosen in 0..shown.len() {
                    let rows = login(&shown, chosen).rows(columns, glyphs);

                    assert!(
                        rows.iter().all(|row| row.columns() <= columns),
                        "at {columns} with {glyphs:?} on {chosen}: {rows:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_sentence_folds_because_a_clipped_explanation_explains_nothing() {
        // The one row of the panel that wraps. A description is a label and is
        // cut; this is prose, and it is two rows at eighty columns before any
        // terminal has narrowed.
        let shown = offered();
        let rows = art(&login(&shown, 0), 80, Glyphs::Unicode);
        let folded = rows.get(4..6).expect("the sentence");

        assert_eq!(folded.join(" "), SAID);
        assert!(
            folded.iter().all(|row| !row.contains('…')),
            "nothing was cut: {folded:?}"
        );
    }

    #[test]
    fn a_panel_is_only_the_rows_it_returns() {
        // Nothing above the rule, and every word in it the caller's. This is
        // what lets the panel stand under a wordmark on a first run with its
        // own title and its own last row, rather than being copied and changed
        // — and it is the test that fails when somebody writes one of those
        // strings in here.
        let shown = offered();
        let panel = Panel {
            title: "Welcome",
            said: None,
            shown: &shown,
            chosen: 0,
            footer: "press enter to continue",
        };

        let rows = art(&panel, 40, Glyphs::Unicode);

        assert_eq!(rows.first().map(String::as_str), Some(&"─".repeat(40)[..]));
        assert_eq!(rows.get(2).map(String::as_str), Some("Welcome"));
        assert_eq!(
            rows.last().map(String::as_str),
            Some("press enter to continue")
        );
    }

    // The drawings, whole. Every test above holds one rule and would go green
    // on a panel nobody would ship; these are the assertion that what is drawn
    // is the thing that was designed, and the diff on one of them is what
    // moved on screen.

    #[test]
    fn the_login_panel_is_drawn_the_way_it_was_designed() {
        let shown = offered();

        pictured("login", &login(&shown, 0), 80, Glyphs::Unicode);
    }

    #[test]
    fn the_mark_moves_and_nothing_else_on_the_panel_does() {
        let shown = offered();

        pictured("second", &login(&shown, 1), 80, Glyphs::Unicode);
    }

    #[test]
    fn a_narrow_window_folds_the_sentence_and_ends_a_description() {
        // Thirty-four columns: half of the narrowest terminal anyone opens, and
        // the width at which the two ways this panel loses text both show.
        let shown = offered();

        pictured("narrow", &login(&shown, 0), 34, Glyphs::Unicode);
    }

    #[test]
    fn a_font_without_the_glyphs_draws_the_same_panel() {
        let shown = offered();

        pictured("ascii", &login(&shown, 1), 80, Glyphs::Ascii);
    }
}
