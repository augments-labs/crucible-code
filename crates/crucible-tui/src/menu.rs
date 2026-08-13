//! The command list: what a `/` opens above the box, and what `/help` answers.
//!
//! One component for both, because they are one list. The live one is filtered
//! to what has been typed and the answered one is not, and a reader who has
//! seen either recognises the other — a name, a gap, and what the command does.
//!
//! It opens *above* the prompt rather than below it or inside it. Below would
//! push the row that says which mode is in force down the screen, or cover it;
//! inside would change the shape of a box whose three rows are fixed on
//! purpose. Above, the box and the row under it stay exactly where they were
//! and the list arrives over them.
//!
//! A row is a command and what it does, and nothing else. Which key moves,
//! runs or closes is documented once, where somebody looks it up, rather than
//! reprinted beside every list that has ever been on screen.
//!
//! A list somebody is choosing from says which row they are on, and says it
//! twice: the row is marked as well as coloured. Colour alone would be the one
//! thing on screen that a terminal without it could not report, and the row a
//! key is about to act on is the last thing to leave to a hue.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width;

/// What stands between the longest name and the words beside it.
const GAP: usize = 3;

/// The room kept in front of every row of a list being chosen from: the mark,
/// and the space after it.
///
/// Kept on every row rather than only on the marked one, so the names stand in
/// one column as the mark moves down them.
const POINTING: usize = 2;

/// One command, as a list of them draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listed<'a> {
    /// What is typed to run it, the `/` and all. Empty on a row that carries on
    /// from the one above it.
    pub name: &'a str,
    /// What it does, in the few words a row has room for.
    pub says: &'a str,
}

/// A list of commands, laid out in two columns.
#[derive(Debug, Clone, Copy)]
pub struct Menu<'a> {
    /// What to draw, in the order it is listed.
    pub shown: &'a [Listed<'a>],
    /// Which row a key would act on, where this is a list being chosen from.
    ///
    /// `None` where it is a list being read instead — `/help`'s answer, which
    /// is over as soon as it is drawn. Then no room is kept in front of it and
    /// every row is drawn alike, because there is no row to tell apart.
    pub chosen: Option<usize>,
}

/// How one row of a list is drawn.
///
/// Three of these, and which one a row gets is what says what the list is: a
/// list being read, a row of one being chosen from, and the row that is chosen.
#[derive(Debug, Clone, Copy)]
struct Drawn {
    /// What stands in the room kept in front of the row. Empty on every row
    /// but the chosen one.
    mark: &'static str,
    /// How much room that is, on this row and on every other.
    front: usize,
    /// The command's own name.
    name: Slot,
    /// What it does, beside it.
    says: Slot,
}

impl Drawn {
    /// A list being read rather than chosen from.
    const READ: Self = Self {
        mark: "",
        front: 0,
        name: Slot::Strong,
        says: Slot::Plain,
    };

    /// A row of one being chosen from that the choice has passed over.
    const fn passed(front: usize) -> Self {
        Self {
            mark: "",
            front,
            name: Slot::Quiet,
            says: Slot::Quiet,
        }
    }

    /// The row it is on.
    const fn chosen(mark: &'static str, front: usize) -> Self {
        Self {
            mark,
            front,
            name: Slot::Strong,
            says: Slot::Plain,
        }
    }
}

impl Menu<'_> {
    /// One row per command, laid out for a terminal `columns` wide.
    ///
    /// The second column starts after the longest name *shown*, so a filtered
    /// list is as tight as what is left in it. It is a different list rather
    /// than the same one moving: every row of it changed on the keystroke that
    /// filtered it.
    #[must_use]
    pub fn rows(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let front = if self.chosen.is_some() && columns > POINTING {
            POINTING
        } else {
            0
        };

        let widest = self
            .shown
            .iter()
            .map(|one| width::columns(one.name))
            .max()
            .unwrap_or_default();

        let at = front + widest + GAP;
        let left = columns.saturating_sub(at);

        self.shown
            .iter()
            .enumerate()
            .map(|(row, one)| one.row(columns, at, left, self.drawn(row, front, glyphs)))
            .collect()
    }

    /// How the row at `row` is drawn, where the list keeps `front` columns in
    /// front of every row.
    ///
    /// No room for the mark is no list to choose from. At that width the mark
    /// would stand where the name's first column is, and the name is the part
    /// that has to be typed.
    fn drawn(&self, row: usize, front: usize, glyphs: Glyphs) -> Drawn {
        let Some(chosen) = self.chosen.filter(|_| front > 0) else {
            return Drawn::READ;
        };

        if chosen == row {
            Drawn::chosen(glyphs.caret(), front)
        } else {
            Drawn::passed(front)
        }
    }
}

impl Listed<'_> {
    /// The row for one command: its name, then what it does.
    ///
    /// A terminal too narrow for the second column gets the first alone. Half a
    /// sentence about what a command does is worth less than the name of it,
    /// and the name is what has to be typed.
    fn row(&self, columns: usize, at: usize, left: usize, drawn: Drawn) -> Row {
        let mut row = Row::new().then(Slot::Accent, drawn.mark);
        row.pad(drawn.front);
        row.push(
            drawn.name,
            width::clip(self.name, columns.saturating_sub(drawn.front)),
        );

        if left > 0 && !self.says.is_empty() {
            row.pad(at);
            row.push(drawn.says, width::clip(self.says, left));
        }

        row
    }
}

#[cfg(test)]
mod tests {
    use crate::color::Palette;
    use crate::dump::dump;

    use super::*;

    /// The list as `/help` answers with it.
    fn every() -> [Listed<'static>; 3] {
        [
            Listed {
                name: "/help",
                says: "what these are",
            },
            Listed {
                name: "/model",
                says: "which model answers",
            },
            Listed {
                name: "/resume",
                says: "pick up an earlier session here",
            },
        ]
    }

    /// What a list of them says, row by row, as `/help` answers with it.
    fn art(shown: &[Listed<'_>], columns: usize) -> Vec<String> {
        drawn(shown, columns, None)
    }

    /// The same for a list somebody is choosing from.
    fn drawn(shown: &[Listed<'_>], columns: usize, chosen: Option<usize>) -> Vec<String> {
        Menu { shown, chosen }
            .rows(columns, Glyphs::Unicode)
            .iter()
            .map(Row::text)
            .collect()
    }

    /// The list at `columns`, against the picture checked in beside it under
    /// `name@columns`.
    ///
    /// The width is the suffix rather than something written into the name, so
    /// that a picture cannot end up checked against a drawing of some other
    /// terminal: one argument decides both what was drawn and which file it is
    /// read from.
    fn pictured(
        name: &str,
        shown: &[Listed<'_>],
        chosen: Option<usize>,
        columns: usize,
        glyphs: Glyphs,
    ) {
        let menu = Menu { shown, chosen };

        insta::with_settings!({snapshot_suffix => columns.to_string()}, {
            insta::assert_snapshot!(name, dump(&menu.rows(columns, glyphs), columns));
        });
    }

    /// A palette that writes every hue it has.
    fn colourful() -> Palette {
        Palette::resolve(true, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        })
    }

    #[test]
    fn every_command_is_a_row_of_its_name_and_what_it_does() {
        pictured("every", &every(), None, 60, Glyphs::Unicode);
    }

    #[test]
    fn the_second_column_starts_after_the_longest_name_in_the_list() {
        // Filtered to two short names, the words move left with them. The list
        // that was on screen before this one was a different list; what would
        // read as drift is a column standing where a name that is no longer
        // shown put it.
        let shown = every().into_iter().take(2).collect::<Vec<_>>();

        pictured("filtered", &shown, None, 60, Glyphs::Unicode);
    }

    #[test]
    fn a_row_that_carries_on_from_the_one_above_it_has_no_name() {
        let shown = [
            Listed {
                name: "/mode",
                says: "ask mode on",
            },
            Listed {
                name: "",
                says: "ask · allowEdits · fullAccess",
            },
        ];

        pictured("carried_on", &shown, None, 60, Glyphs::Unicode);
    }

    #[test]
    fn a_window_too_narrow_for_the_words_keeps_the_name_whole() {
        // The name is the part that has to be typed, so it is the part that
        // survives. What is dropped is dropped whole: the second column is
        // either there or it is not.
        assert_eq!(art(&every(), 10), ["/help", "/model", "/resume"]);
    }

    #[test]
    fn what_does_not_fit_is_cut_rather_than_wrapped() {
        // Every row of a live region is one row. A row that wrapped would leave
        // the cursor one row below where the next frame expects it, and the
        // frame after that would erase the wrong lines.
        for columns in 1..=60 {
            for chosen in [None, Some(0), Some(2)] {
                let rows = Menu {
                    shown: &every(),
                    chosen,
                }
                .rows(columns, Glyphs::Unicode);

                assert!(
                    rows.iter().all(|row| row.columns() <= columns),
                    "at {columns} with {chosen:?}: {rows:?}"
                );
            }
        }

        assert_eq!(
            art(&every(), 20).last().map(String::as_str),
            Some("/resume   pick up an")
        );
    }

    #[test]
    fn a_name_wider_than_the_window_is_cut_where_a_column_ends() {
        // Nothing lists a name this long, and a row a column over the width is
        // the one thing a component may never hand back.
        let shown = [Listed {
            name: "日本語",
            says: "",
        }];

        assert_eq!(art(&shown, 4), ["日本"]);
    }

    #[test]
    fn a_list_with_nothing_in_it_draws_nothing() {
        assert!(
            Menu {
                shown: &[],
                chosen: None,
            }
            .rows(60, Glyphs::Unicode)
            .is_empty()
        );
    }

    #[test]
    fn the_name_is_the_part_that_is_coloured() {
        // What is typed reads as what is typed, and the words after it are the
        // reader's own foreground: a list is one colour and a quiet one.
        let painted = Menu {
            shown: &every(),
            chosen: None,
        }
        .rows(60, Glyphs::Unicode)
        .first()
        .map(|row| row.paint(colourful()))
        .expect("a row the component drew");

        assert!(
            painted.starts_with(colourful().open(Slot::Strong)),
            "{painted:?}"
        );
        assert!(painted.ends_with("what these are"), "{painted:?}");
    }

    #[test]
    fn the_row_being_chosen_is_marked_and_the_rest_stand_in_the_same_column() {
        // The mark moves down the list and the names do not move with it, which
        // is what the room in front of every row is for. The picture carries
        // the other half of it: the chosen row is the loud one and the rows the
        // choice has passed over are quiet, so the row a key is about to act on
        // is said twice.
        pictured("chosen", &every(), Some(1), 60, Glyphs::Unicode);
    }

    #[test]
    fn a_list_nobody_is_choosing_from_keeps_no_room_for_a_mark() {
        // `/help`'s answer is over as soon as it is drawn. Two columns of
        // nothing in front of it would be an offer that is not being made.
        assert_eq!(drawn(&every(), 60, None), art(&every(), 60));

        let first = art(&every(), 60);
        let first = first.first().expect("the row `/help` is on");

        assert!(first.starts_with("/help"), "{first:?}");
    }

    #[test]
    fn a_font_without_the_mark_still_says_which_row_it_is_on() {
        // The set changes what the mark is drawn with and nothing about the
        // column it stands in, so the names below it do not move either.
        pictured("chosen_ascii", &every(), Some(0), 60, Glyphs::Ascii);
    }

    #[test]
    fn the_rows_the_choice_has_passed_over_are_the_quiet_ones() {
        // The second thing that says which row it is on, for a reader who has
        // colour. The mark is the first, and it is the one that survives a
        // terminal that has none.
        let painted: Vec<String> = Menu {
            shown: &every(),
            chosen: Some(0),
        }
        .rows(60, Glyphs::Unicode)
        .iter()
        .map(|row| row.paint(colourful()))
        .collect();

        let chosen = painted.first().expect("the row it is on");
        let passed = painted.get(1).expect("a row it has passed over");

        assert!(
            chosen.contains(colourful().open(Slot::Strong)),
            "{chosen:?}"
        );
        assert!(
            !passed.contains(colourful().open(Slot::Strong)),
            "{passed:?}"
        );
        assert!(passed.contains(colourful().open(Slot::Quiet)), "{passed:?}");
    }
}
