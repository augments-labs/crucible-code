//! A bar: one row the width it is given, split between parts by their size.
//!
//! Each part is drawn in its own tone, solid where it is spent and shaded where
//! it is free, so the bar still reads used against free with colour off. A
//! part with any size at all gets a cell however small its share, since a
//! category that is there and draws nothing reads as one that is not; an empty
//! part gets none. Where there are more parts with a size than columns to give
//! them, the smallest go without.
//!
//! Nothing here knows what the parts stand for. `/context` hands in the
//! categories of a request; the labels, numbers and order are the caller's.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;

/// How a part of a bar is inked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    /// Spent: every cell inked whole.
    Solid,
    /// Free: every cell shaded.
    Shaded,
}

impl Fill {
    /// The cell this fill is drawn with.
    #[must_use]
    pub fn cell(self, glyphs: Glyphs) -> &'static str {
        match self {
            Self::Solid => glyphs.solid(),
            Self::Shaded => glyphs.shaded(),
        }
    }
}

/// One part of a bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part {
    /// The tone it is drawn in.
    pub slot: Slot,
    /// Whether it is spent or free.
    pub fill: Fill,
    /// How large it is, in whatever unit every part of the bar shares.
    pub size: u64,
}

/// A bar of `parts`, in the order they are given.
#[derive(Debug, Clone, Copy)]
pub struct Bar<'a> {
    /// What the bar is split between.
    pub parts: &'a [Part],
}

impl Bar<'_> {
    /// The bar across `columns`, or nothing where no part has a size.
    #[must_use]
    pub fn row(&self, columns: usize, glyphs: Glyphs) -> Row {
        let cells = self.cells(columns);
        self.parts
            .iter()
            .zip(cells)
            .filter(|(_, cells)| *cells > 0)
            .fold(Row::new(), |row, (part, cells)| {
                row.then(part.slot, part.fill.cell(glyphs).repeat(cells))
            })
    }

    /// How many of `columns` each part is drawn across, in the parts' order.
    ///
    /// Each share is rounded down, lifted to one where it rounded to none,
    /// and then evened out to `columns` exactly: a column short goes to the
    /// part the rounding cost most, a column over comes back from the part
    /// drawn widest.
    fn cells(&self, columns: usize) -> Vec<usize> {
        let mut cells = vec![0; self.parts.len()];
        let live = self.parts.iter().filter(|part| part.size > 0).count();
        if live == 0 || columns == 0 {
            return cells;
        }

        if live > columns {
            // A cell apiece for the largest that fit, earlier first on a tie.
            let mut order: Vec<usize> = (0..self.parts.len()).collect();
            order.sort_by_key(|&at| {
                std::cmp::Reverse(self.parts.get(at).map_or(0, |part| part.size))
            });
            for at in order.into_iter().take(columns) {
                if let Some(cell) = cells.get_mut(at) {
                    *cell = 1;
                }
            }
            return cells;
        }

        let total: u128 = self.parts.iter().map(|part| u128::from(part.size)).sum();
        let wide = columns as u128;
        let share = |size: u64| u128::from(size) * wide;
        for (cell, part) in cells.iter_mut().zip(self.parts) {
            if part.size > 0 {
                let floor = usize::try_from(share(part.size) / total).unwrap_or(columns);
                *cell = floor.max(1);
            }
        }

        let mut drawn: usize = cells.iter().sum();
        while drawn > columns {
            let widest = cells
                .iter_mut()
                .filter(|cell| **cell > 1)
                .max_by_key(|cell| **cell);
            let Some(widest) = widest else { break };
            *widest -= 1;
            drawn -= 1;
        }

        let mut owed: Vec<usize> = (0..self.parts.len())
            .filter(|&at| self.parts.get(at).is_some_and(|part| part.size > 0))
            .collect();
        owed.sort_by_key(|&at| {
            std::cmp::Reverse(
                self.parts
                    .get(at)
                    .map_or(0, |part| share(part.size) % total),
            )
        });
        for at in owed.into_iter().cycle().take(columns.saturating_sub(drawn)) {
            if let Some(cell) = cells.get_mut(at) {
                *cell += 1;
            }
        }
        cells
    }
}

#[cfg(test)]
mod tests;
