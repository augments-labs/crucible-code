//! A panel's row of tabs: every name in the order the arrows walk them, the
//! one open in the strong slot and the rest quiet.
//!
//! `/sandbox` and `/settings` both draw theirs here. A row with no marks is
//! cut at the width like any other row. A row whose open tab wears marks,
//! one on each side, keeps the open tab longest: the other names first lose
//! the padding that holds them still as the marks move, then go, and the
//! open tab alone is what is cut at the narrowest width.

use crate::color::Slot;
use crate::row::Row;
use crate::width::columns as columns_of;

/// The gap between a heading and the first name, and, when only the open tab
/// is left beside it, the narrower one between them.
const AFTER_HEADING: &str = "   ";
const BESIDE_HEADING: &str = " ";

/// A row of tabs, drawn at a width.
#[derive(Debug, Clone, Copy)]
pub struct TabRow<'a> {
    /// The panel's name, drawn strong before the tabs when it has one.
    pub heading: Option<&'a str>,
    /// Every tab's name, in the order the arrows walk them.
    pub names: &'a [&'a str],
    /// Which name is open. One past the list opens none.
    pub open: usize,
    /// The two sides drawn around the open name, when it wears them.
    pub marks: Option<(&'a str, &'a str)>,
}

impl TabRow<'_> {
    /// The row at `columns` display columns.
    #[must_use]
    pub fn row(&self, columns: usize) -> Row {
        let whole = self.drawn(true);
        if self.marks.is_none() {
            return whole.clipped(columns);
        }
        let open = Row::new().then(Slot::Strong, self.worn(self.open_name()));
        let mut beside = Row::new();
        if let Some(heading) = self.heading {
            beside.push(Slot::Strong, heading);
            beside.push(Slot::Plain, BESIDE_HEADING);
        }
        beside.push(Slot::Strong, self.worn(self.open_name()));
        [whole, self.drawn(false), beside]
            .into_iter()
            .find(|row| row.columns() <= columns)
            .unwrap_or_else(|| open.clipped(columns))
    }

    /// Every name, the closed ones padded as wide as the marks when `padded`.
    fn drawn(&self, padded: bool) -> Row {
        let pad = self.marks.map_or_else(String::new, |(left, right)| {
            " ".repeat(columns_of(left).max(columns_of(right)) + 1)
        });
        let mut row = Row::new();
        if let Some(heading) = self.heading {
            row.push(Slot::Strong, heading);
            row.push(Slot::Plain, AFTER_HEADING);
        }
        for (at, name) in self.names.iter().enumerate() {
            if at > 0 {
                row.push(Slot::Plain, " ");
            }
            if at == self.open {
                row.push(Slot::Strong, self.worn(name));
            } else if padded {
                row.push(Slot::Quiet, format!("{pad}{name}{pad}"));
            } else {
                row.push(Slot::Quiet, *name);
            }
        }
        row
    }

    fn open_name(&self) -> &str {
        self.names.get(self.open).copied().unwrap_or_default()
    }

    /// `name` with the marks on either side, or bare when there are none.
    fn worn(&self, name: &str) -> String {
        self.marks.map_or_else(
            || name.to_owned(),
            |(left, right)| format!("{left} {name} {right}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 3] = ["Status", "Config", "Usage"];

    fn marked(open: usize) -> TabRow<'static> {
        TabRow {
            heading: Some("Settings"),
            names: &NAMES,
            open,
            marks: Some(("<", ">")),
        }
    }

    #[test]
    fn a_marked_tab_row_pads_the_closed_names_as_wide_as_the_marks_where_it_fits() {
        assert_eq!(
            marked(1).row(80).text(),
            "Settings     Status   < Config >   Usage  "
        );
    }

    #[test]
    fn a_marked_tab_row_drops_the_padding_then_the_closed_names_then_the_heading() {
        assert_eq!(
            marked(1).row(34).text(),
            "Settings   Status < Config > Usage"
        );
        assert_eq!(marked(1).row(33).text(), "Settings < Config >");
        assert_eq!(marked(1).row(18).text(), "< Config >");
        assert_eq!(marked(1).row(4).text(), "< Co");
    }

    #[test]
    fn an_unmarked_tab_row_is_cut_at_the_width() {
        let row = TabRow {
            heading: None,
            names: &NAMES,
            open: 2,
            marks: None,
        };
        assert_eq!(row.row(80).text(), "Status Config Usage");
        assert_eq!(row.row(9).text(), "Status Co");
    }
}
