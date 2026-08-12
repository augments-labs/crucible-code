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

use crate::color::Slot;
use crate::row::Row;
use crate::width;

/// What stands between the longest name and the words beside it.
const GAP: usize = 3;

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
}

impl Menu<'_> {
    /// One row per command, laid out for a terminal `columns` wide.
    ///
    /// The second column starts after the longest name *shown*, so a filtered
    /// list is as tight as what is left in it. It is a different list rather
    /// than the same one moving: every row of it changed on the keystroke that
    /// filtered it.
    #[must_use]
    pub fn rows(&self, columns: usize) -> Vec<Row> {
        let widest = self
            .shown
            .iter()
            .map(|one| width::columns(one.name))
            .max()
            .unwrap_or_default();

        let at = widest + GAP;
        let left = columns.saturating_sub(at);

        self.shown
            .iter()
            .map(|one| one.row(columns, at, left))
            .collect()
    }
}

impl Listed<'_> {
    /// The row for one command: its name, then what it does.
    ///
    /// A terminal too narrow for the second column gets the first alone. Half a
    /// sentence about what a command does is worth less than the name of it,
    /// and the name is what has to be typed.
    fn row(&self, columns: usize, at: usize, left: usize) -> Row {
        let mut row = Row::new().then(Slot::Strong, width::clip(self.name, columns));

        if left > 0 && !self.says.is_empty() {
            row.pad(at);
            row.push(Slot::Plain, width::clip(self.says, left));
        }

        row
    }
}

#[cfg(test)]
mod tests {
    use crate::color::Palette;

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

    /// What a list of them says, row by row.
    fn art(shown: &[Listed<'_>], columns: usize) -> Vec<String> {
        Menu { shown }.rows(columns).iter().map(Row::text).collect()
    }

    /// A palette that writes every hue it has.
    fn colourful() -> Palette {
        Palette::resolve(true, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        })
    }

    #[test]
    fn every_command_is_a_row_of_its_name_and_what_it_does() {
        assert_eq!(
            art(&every(), 60),
            [
                "/help     what these are",
                "/model    which model answers",
                "/resume   pick up an earlier session here",
            ]
        );
    }

    #[test]
    fn the_second_column_starts_after_the_longest_name_in_the_list() {
        // Filtered to two short names, the words move left with them. The list
        // that was on screen before this one was a different list; what would
        // read as drift is a column standing where a name that is no longer
        // shown put it.
        let shown = every().into_iter().take(2).collect::<Vec<_>>();

        assert_eq!(
            art(&shown, 60),
            ["/help    what these are", "/model   which model answers"]
        );
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

        assert_eq!(
            art(&shown, 60),
            [
                "/mode   ask mode on",
                "        ask · allowEdits · fullAccess"
            ]
        );
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
            let rows = Menu { shown: &every() }.rows(columns);

            assert!(
                rows.iter().all(|row| row.columns() <= columns),
                "at {columns}: {rows:?}"
            );
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
        assert!(Menu { shown: &[] }.rows(60).is_empty());
    }

    #[test]
    fn the_name_is_the_part_that_is_coloured() {
        // What is typed reads as what is typed, and the words after it are the
        // reader's own foreground: a list is one colour and a quiet one.
        let painted = Menu { shown: &every() }
            .rows(60)
            .first()
            .map(|row| row.paint(colourful()))
            .expect("a row the component drew");

        assert!(
            painted.starts_with(colourful().open(Slot::Strong)),
            "{painted:?}"
        );
        assert!(painted.ends_with("what these are"), "{painted:?}");
    }
}
