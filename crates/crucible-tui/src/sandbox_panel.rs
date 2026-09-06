//! The `/sandbox` chooser: two tabs, a bounded list, and the keys that move it.
//!
//! This is a rendering component only. The caller supplies the rows currently
//! available for the selected tab and decides what selecting one means. No
//! host inspection or sandbox policy belongs in the TUI crate.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::panel::Offered;
use crate::row::Row;
use crate::width::{clip, columns, fold};

const TAB_SANDBOX: &str = "Sandbox";
const TAB_DEPENDENCIES: &str = "Dependencies";
const FOOTER: &str = "esc close - left/right tabs - up/down select - enter choose";
const POINTING: usize = 2;
const CHROME: usize = 8;
const ENTRY: usize = 2;

#[derive(Clone, Copy)]
struct Visible<'a> {
    items: &'a [Offered<'a>],
    chosen: usize,
    more: usize,
}

/// The tab currently being shown by [`SandboxPanel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxTab {
    /// Sandbox choices.
    Sandbox,
    /// Dependencies available to the caller.
    Dependencies,
}

/// A reusable, pure rendering component for the `/sandbox` chooser.
#[derive(Debug, Clone, Copy)]
pub struct SandboxPanel<'a> {
    /// Which of the two tabs is selected.
    pub tab: SandboxTab,
    /// Rows for the selected tab.
    pub items: &'a [Offered<'a>],
    /// Row a key would act on. Values past the list select no row visually.
    pub chosen: usize,
    /// Optional explanation shown above the list.
    pub summary: Option<&'a str>,
}

impl SandboxPanel<'_> {
    /// Draw the complete panel at `columns` display columns.
    #[must_use]
    pub fn rows(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        self.laid(columns, glyphs, self.items)
    }

    /// Draw the panel within `room` rows, dropping summary and then list rows
    /// as space gets scarce. Empty is returned when the fixed chrome cannot fit.
    #[must_use]
    pub fn within(&self, columns: usize, room: usize, glyphs: Glyphs) -> Vec<Row> {
        let whole = self.rows(columns, glyphs);
        if whole.len() <= room {
            return whole;
        }

        let concise = Self {
            summary: None,
            ..*self
        };
        let shorter = concise.rows(columns, glyphs);
        if shorter.len() <= room {
            return shorter;
        }

        let count = room.saturating_sub(CHROME) / ENTRY;
        if count == 0 {
            return Vec::new();
        }
        let count = count.min(self.items.len());
        let last = self.items.len().saturating_sub(count);
        let from = self
            .chosen
            .saturating_sub(count.saturating_sub(1))
            .min(last);
        let visible = self.items.get(from..from + count).unwrap_or_default();
        let selected = self.chosen.saturating_sub(from);
        concise.laid_visible(
            columns,
            glyphs,
            Visible {
                items: visible,
                chosen: selected,
                more: self.items.len() - count,
            },
        )
    }

    fn laid(&self, columns: usize, glyphs: Glyphs, items: &[Offered<'_>]) -> Vec<Row> {
        self.laid_visible(
            columns,
            glyphs,
            Visible {
                items,
                chosen: self.chosen,
                more: 0,
            },
        )
    }

    fn laid_visible(&self, columns: usize, glyphs: Glyphs, visible: Visible<'_>) -> Vec<Row> {
        let Visible {
            items,
            chosen,
            more,
        } = visible;
        let mut rows = vec![
            Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
            Row::new(),
            self.tabs(columns),
            Row::new(),
        ];
        if let Some(summary) = self.summary {
            rows.extend(fold(summary, columns).into_iter().map(Row::plain));
            rows.push(Row::new());
        }
        let title = match self.tab {
            SandboxTab::Sandbox => TAB_SANDBOX,
            SandboxTab::Dependencies => TAB_DEPENDENCIES,
        };
        rows.push(Row::new().then(Slot::Strong, clip(title, columns)));
        for (at, item) in items.iter().enumerate() {
            rows.extend(item_rows(
                columns,
                glyphs,
                item,
                at == chosen && !items.is_empty(),
            ));
        }
        if more > 0 {
            let mut row = Row::new();
            row.pad(POINTING.min(columns));
            let left = columns.saturating_sub(row.columns());
            row.push(
                Slot::Quiet,
                clip(&format!("{} {more} more", glyphs.dot()), left),
            );
            rows.push(row);
        }
        rows.push(Row::new());
        let footer = if columns >= FOOTER.len() {
            FOOTER
        } else if columns >= 38 {
            "esc close - arrows move - enter choose"
        } else if columns >= 18 {
            "esc close - enter"
        } else {
            "esc close"
        };
        rows.push(Row::new().then(Slot::Quiet, clip(footer, columns)));
        rows
    }

    fn tabs(&self, width: usize) -> Row {
        let (left, right) = match self.tab {
            SandboxTab::Sandbox => (Slot::Strong, Slot::Quiet),
            SandboxTab::Dependencies => (Slot::Quiet, Slot::Strong),
        };
        let mut row = Row::new().then(left, clip(TAB_SANDBOX, width));
        if width > columns(TAB_SANDBOX) {
            row.push(Slot::Plain, " ");
            row.push(
                right,
                clip(TAB_DEPENDENCIES, width.saturating_sub(row.columns())),
            );
        }
        row
    }
}

fn item_rows(columns: usize, glyphs: Glyphs, item: &Offered<'_>, selected: bool) -> [Row; 2] {
    let front = if columns > POINTING { POINTING } else { 0 };
    let mut row = Row::new();
    row.push(
        Slot::Accent,
        if selected && front > 0 {
            glyphs.caret()
        } else {
            ""
        },
    );
    row.pad(front);
    row.push(
        if selected { Slot::Strong } else { Slot::Plain },
        clip(item.name, columns.saturating_sub(front)),
    );
    let mut description = Row::new();
    description.pad(front);
    description.push(Slot::Quiet, clip(item.says, columns.saturating_sub(front)));
    [row, description]
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEMS: [Offered<'static>; 3] = [
        Offered {
            name: "automatic",
            says: "use the configured sandbox",
        },
        Offered {
            name: "workspace",
            says: "keep commands in the workspace",
        },
        Offered {
            name: "restricted",
            says: "allow only restricted access",
        },
    ];

    fn panel(
        tab: SandboxTab,
        chosen: usize,
        summary: Option<&'static str>,
    ) -> SandboxPanel<'static> {
        SandboxPanel {
            tab,
            items: &ITEMS,
            chosen,
            summary,
        }
    }

    #[test]
    fn selected_tab_and_row_are_visible() {
        let rows = panel(
            SandboxTab::Dependencies,
            1,
            Some("Choose how commands may access the workspace."),
        )
        .rows(80, Glyphs::Unicode);
        let text: Vec<_> = rows.iter().map(Row::text).collect();
        assert!(
            text.iter()
                .any(|row| row.contains("Sandbox") && row.contains("Dependencies"))
        );
        assert!(
            text.iter()
                .any(|row| row.contains("›") && row.contains("workspace")),
            "{text:?}"
        );
        assert!(
            text.iter()
                .any(|row| row.contains("up/down select") && row.contains("esc close"))
        );
    }

    #[test]
    fn a_small_panel_keeps_the_close_hint_and_active_tab() {
        for width in [20, 30, 45] {
            let rows = panel(SandboxTab::Dependencies, 1, Some("Disabled")).within(
                width,
                12,
                Glyphs::Ascii,
            );
            assert!(rows.iter().any(|row| row.text().contains("esc close")));
            assert!(rows.iter().any(|row| row.text().contains("Dependencies")));
        }
    }

    #[test]
    fn empty_items_and_invalid_selection_are_safe() {
        let panel = SandboxPanel {
            tab: SandboxTab::Sandbox,
            items: &[],
            chosen: usize::MAX,
            summary: None,
        };
        for columns in 0..40 {
            for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
                assert!(
                    panel
                        .rows(columns, glyphs)
                        .iter()
                        .all(|row| row.columns() <= columns)
                );
            }
        }
    }

    #[test]
    fn within_is_bounded_and_keeps_selected_row_in_view() {
        let panel = panel(
            SandboxTab::Sandbox,
            2,
            Some("A deliberately long summary that folds."),
        );
        for columns in 1..=80 {
            for room in 0..=20 {
                let rows = panel.within(columns, room, Glyphs::Ascii);
                assert!(rows.len() <= room);
                assert!(rows.iter().all(|row| row.columns() <= columns));
            }
        }
    }
}
