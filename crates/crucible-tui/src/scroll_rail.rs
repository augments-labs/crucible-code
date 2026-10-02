//! The scroll rail: one column on the transcript's right edge that says where
//! the band is in what the record retains, and where each prompt was.
//!
//! The rail is the transcript band's height and stands for the whole retained
//! record, scaled. The thumb is the share of it on screen, never shorter than
//! a row, so it sits at the top for the oldest line held and at the bottom
//! while the band follows the foot. A prompt is a mark on the row its first
//! line falls at in the whole record, scaled the same way, so several prompts
//! that scale to one row are one mark. A mark that falls on the thumb is drawn
//! as thumb: the two share the cell and the thumb wins, so a press there takes
//! the thumb, and a prompt under it is reached by moving the thumb off it.
//! The one exception is the current prompt — the one a press on its mark last
//! landed on while that prompt still starts in the band and no prompt has been
//! sent since, else the latest that starts at or above the band's last row —
//! whose mark is always drawn grown, on the thumb too, so a reader can see
//! which prompt they are reading under. When the record fits the band there is
//! nowhere else to be, and the rail is blank.
//!
//! The rail spends no hue of its own. The thumb is [`Slot::Accent`] and the
//! track and its marks are [`Slot::Quiet`], two jobs every palette already
//! answers, so every theme and a colourless run draw the rail their own way.
//! Marks differ from the track by shape, not colour, and take the colour of
//! what they stand on: the current prompt's grown mark is quiet on the track
//! and accent on the thumb. A pointer on the rail lights the track and its
//! marks in the accent too, and grows the mark it is on, so a reader can see
//! which prompt a press there lands on; a mark the thumb covers stays covered
//! unless it is the current prompt's.
//!
//! Every cell is structural: the rail is the band's furniture, not the
//! transcript's words, so a selection dragged across it highlights none of it
//! and a copy takes none of it.
//!
//! Nothing here grows with the session. The rail is laid out from three
//! numbers and the prompt landmarks the record keeps a fixed number of, into a
//! row per band row.

use std::ops::Range;

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;

/// The narrowest window the rail is drawn in.
///
/// The width below which the prompt box already gives up its frame: under it,
/// chrome takes a share of what there is to read that the window cannot spare.
/// The two are one judgement about what a narrow window can spare, so they are
/// one number.
pub(crate) const NARROWEST: usize = crate::prompt::FRAMED_AT;

/// Whether a window `columns` wide has a column to spare for the rail.
pub(crate) fn spared(columns: usize) -> bool {
    columns >= NARROWEST
}

/// Where a band stands in what the record retains, in display rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Place {
    /// Every display row the record still holds.
    pub(crate) total: usize,
    /// How many of those are above the band's first row.
    pub(crate) top: usize,
    /// How tall the band is, and so the rail.
    pub(crate) height: usize,
}

/// The rail as laid out for one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScrollRail {
    /// Every display row the record held when the rail was laid out.
    total: usize,
    /// How tall the band is, and so the rail.
    height: usize,
    /// The rows the thumb covers; `None` while the record fits the band.
    thumb: Option<Range<usize>>,
    /// Which rail rows carry a prompt, one entry a row.
    marks: Vec<bool>,
    /// The rail row of the current prompt's mark, where there is one.
    current: Option<usize>,
}

impl ScrollRail {
    /// Lays the rail out over `place`, with a mark for each prompt starting
    /// `prompts` display rows into the record, and the one starting `landed`
    /// rows in current while it starts in the band, where a press on its mark
    /// landed on one.
    pub(crate) fn new(
        place: Place,
        prompts: impl IntoIterator<Item = usize>,
        landed: Option<usize>,
    ) -> Self {
        let Place { total, height, .. } = place;
        let mut marks = vec![false; height];
        // A band of no rows has no rail to put a thumb on, and a record that
        // fits the band has nowhere else for one to be.
        if height == 0 || total <= height {
            return Self {
                total,
                height,
                thumb: None,
                marks,
                current: None,
            };
        }

        // The band's share of the record, scaled to the rail: the rows its
        // first and last displayed rows fall on, scaled as a mark is, and
        // every row between. So the thumb covers exactly the rail rows the
        // band's rows scale to — none that only the row after the band does —
        // which puts it on the first row at the head and on the last at the
        // foot, and makes it a row at least.
        let top = place.top.min(total - height);
        let start = scaled(top, total, height);
        let end = scaled(top + height - 1, total, height) + 1;

        // The current prompt: the one landed on while it starts in the band,
        // else the latest that starts at or above the band's last row.
        let band = top..top + height;
        let mut landed_in_band = None;
        let mut latest = None;
        for prompt in prompts {
            if let Some(mark) = marks.get_mut(scaled(prompt, total, height)) {
                *mark = true;
            }
            if landed == Some(prompt) && band.contains(&prompt) {
                landed_in_band = Some(prompt);
            }
            if prompt < band.end {
                latest = latest.max(Some(prompt));
            }
        }
        let current = landed_in_band
            .or(latest)
            .map(|prompt| scaled(prompt, total, height));

        Self {
            total,
            height,
            thumb: Some(start..end),
            marks,
            current,
        }
    }

    /// The rows the thumb covers, or `None` where the record fits the band.
    pub(crate) fn thumb(&self) -> Option<Range<usize>> {
        self.thumb.clone()
    }

    /// Whether rail row `row` carries a prompt's mark.
    pub(crate) fn marked(&self, row: usize) -> bool {
        self.marks.get(row).copied().unwrap_or(false)
    }

    /// How many display rows into the record the band's top goes for the
    /// thumb to start on rail row `start`.
    ///
    /// The inverse of the scaling above, rounded up so that the thumb laid out
    /// from the answer starts on exactly `start`. A thumb asked to start below
    /// where it stands at the foot, or to reach the rail's last row, is at the
    /// foot, and the band follows it again.
    pub(crate) fn top_for(&self, start: usize) -> usize {
        let (total, height) = (self.total, self.height);
        let foot = total.saturating_sub(height);
        let length = self.thumb.as_ref().map_or(height, Range::len);
        if height == 0 || start + length >= height {
            return foot;
        }
        start.saturating_mul(total).div_ceil(height).min(foot)
    }

    /// The first prompt, of `prompts`, whose mark is on rail row `row`.
    ///
    /// `None` where the row carries no mark, and where the record fits and
    /// the rail draws none.
    pub(crate) fn prompt_at(
        &self,
        row: usize,
        prompts: impl IntoIterator<Item = usize>,
    ) -> Option<usize> {
        self.thumb.as_ref()?;
        prompts
            .into_iter()
            .find(|prompt| scaled(*prompt, self.total, self.height) == row)
    }

    /// The rail as cells, one row a band row, for a window `columns` wide,
    /// with the pointer on rail row `pointer` where it is on the rail at all.
    ///
    /// No rows at all where the window cannot spare the column. Each row is
    /// one column, structural, and blank where the record fits, pointer or
    /// none. The current prompt's mark is grown wherever it falls, in the
    /// accent on the thumb or under a pointer and quiet on a track at rest.
    pub(crate) fn rows(&self, columns: usize, glyphs: Glyphs, pointer: Option<usize>) -> Vec<Row> {
        if !spared(columns) {
            return Vec::new();
        }
        let track = if pointer.is_some() {
            Slot::Accent
        } else {
            Slot::Quiet
        };
        (0..self.height)
            .map(|at| {
                let (slot, cell) = match &self.thumb {
                    None => (Slot::Plain, " "),
                    Some(thumb) if self.current == Some(at) => {
                        let slot = if thumb.contains(&at) {
                            Slot::Accent
                        } else {
                            track
                        };
                        (slot, glyphs.grown())
                    }
                    Some(thumb) if thumb.contains(&at) => (Slot::Accent, glyphs.thumb()),
                    Some(_) if self.marked(at) && pointer == Some(at) => (track, glyphs.grown()),
                    Some(_) if self.marked(at) => (track, glyphs.bullet()),
                    Some(_) => (track, glyphs.vertical()),
                };
                let mut row = Row::new();
                row.push_structural(slot, cell);
                row
            })
            .collect()
    }
}

/// The rail row display row `row` of a record `total` rows long falls on, on a
/// rail `height` rows tall.
fn scaled(row: usize, total: usize, height: usize) -> usize {
    row.saturating_mul(height).checked_div(total).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rail ten rows tall over a record of `total` rows, `top` rows down.
    fn laid(total: usize, top: usize, prompts: &[usize]) -> ScrollRail {
        ScrollRail::new(
            Place {
                total,
                top,
                height: 10,
            },
            prompts.iter().copied(),
            None,
        )
    }

    /// What each rail row says, as one string a row.
    fn said(rail: &ScrollRail, glyphs: Glyphs) -> Vec<String> {
        rail.rows(80, glyphs, None).iter().map(Row::text).collect()
    }

    #[test]
    fn the_rail_thumb_at_the_top_starts_on_the_first_row_and_is_the_share_on_screen() {
        // Ten rows of a hundred on screen: a tenth of the rail.
        assert_eq!(laid(100, 0, &[]).thumb(), Some(0..1));
        // Ten of forty: a quarter.
        assert_eq!(laid(40, 0, &[]).thumb(), Some(0..3));
    }

    #[test]
    fn the_rail_thumb_in_the_middle_stands_where_the_band_is() {
        assert_eq!(laid(40, 15, &[]).thumb(), Some(3..7));
        assert_eq!(laid(100, 45, &[]).thumb(), Some(4..6));
    }

    #[test]
    fn the_rail_thumb_at_the_end_reaches_the_last_row() {
        assert_eq!(laid(40, 30, &[]).thumb(), Some(7..10));
        assert_eq!(laid(100, 90, &[]).thumb(), Some(9..10));
    }

    #[test]
    fn the_rail_thumb_is_never_shorter_than_a_row() {
        // Ten rows of a million: a hundred-thousandth of the rail, drawn as one.
        for top in [0, 500_000, 999_990] {
            let thumb = laid(1_000_000, top, &[]).thumb().expect("a thumb");
            assert_eq!(thumb.len(), 1, "{top}: {thumb:?}");
        }
        assert_eq!(laid(1_000_000, 999_990, &[]).thumb(), Some(9..10));
    }

    #[test]
    fn a_rail_over_a_record_that_fits_is_blank() {
        for total in [0, 4, 10] {
            let rail = laid(total, 0, &[0, 3]);
            assert_eq!(rail.thumb(), None);
            assert_eq!(said(&rail, Glyphs::Unicode), vec![" "; 10], "{total}");
        }
    }

    #[test]
    fn a_rail_marks_each_prompt_where_it_falls_in_the_whole_record() {
        // A hundred rows on ten: a prompt at row 35 is on rail row 3, and two
        // prompts on one rail row are one mark. The latest, above the band,
        // is the current prompt, and grown.
        let rail = laid(100, 90, &[0, 35, 38, 72]);
        let rows = said(&rail, Glyphs::Unicode);

        assert_eq!(rows, vec!["•", "│", "│", "•", "│", "│", "│", "●", "│", "┃"],);
    }

    #[test]
    fn a_rail_mark_on_the_thumb_is_drawn_as_thumb_unless_it_is_the_current_prompt() {
        // Ten rows of forty from the top: the thumb is rail rows 0 to 2, over
        // the prompts at rows 0 and 4. The later of the two is current.
        let rail = laid(40, 0, &[0, 4, 30]);
        let rows = said(&rail, Glyphs::Unicode);

        assert!(rail.marked(0));
        assert_eq!(
            rows.iter().take(3).map(String::as_str).collect::<Vec<_>>(),
            ["┃", "●", "┃"]
        );
        assert_eq!(rows.get(7).map(String::as_str), Some("•"));
    }

    #[test]
    fn the_rail_draws_its_thumb_in_the_accent_and_the_rest_quiet() {
        let rail = laid(100, 90, &[35]);
        let rows = rail.rows(80, Glyphs::Ascii, None);
        let kinds: Vec<Option<Slot>> = rows.iter().map(|row| row.kinds().last()).collect();
        let text: Vec<String> = rows.iter().map(Row::text).collect();

        assert_eq!(text, vec!["|", "|", "|", "*", "|", "|", "|", "|", "|", "#"],);
        assert_eq!(kinds.last(), Some(&Some(Slot::Accent)));
        assert!(kinds.iter().take(9).all(|slot| *slot == Some(Slot::Quiet)));
    }

    #[test]
    fn every_rail_cell_is_structural_and_one_column() {
        for rail in [laid(100, 45, &[12, 60]), laid(4, 0, &[0])] {
            let rows = rail.rows(80, Glyphs::Unicode, None);
            assert_eq!(rows.len(), 10);
            for row in rows {
                assert_eq!(row.columns(), 1);
                assert_eq!(row.structural(), vec![0..1], "{:?}", row.text());
            }
        }
    }

    #[test]
    fn the_rail_is_not_drawn_at_the_narrowest_width_that_cannot_spare_it() {
        let rail = laid(100, 45, &[12]);

        assert!(rail.rows(NARROWEST - 1, Glyphs::Unicode, None).is_empty());
        assert_eq!(rail.rows(NARROWEST, Glyphs::Unicode, None).len(), 10);
        assert!(!spared(NARROWEST - 1));
        assert!(spared(NARROWEST));
    }

    #[test]
    fn the_rail_seeks_the_top_that_puts_the_thumb_on_the_row_asked_for() {
        // Every row, and back: what the rail shows after a seek is the row the
        // pointer asked for, or the foot's where the thumb cannot start lower.
        for total in [11, 40, 100, 1_000, 123_457] {
            let first = laid(total, 0, &[]);
            let lowest = laid(total, total - 10, &[]).thumb().expect("a thumb");
            for start in 0..10 {
                let top = first.top_for(start);
                assert!(top <= total - 10, "{total}: {start} -> {top}");
                let after = laid(total, top, &[]).thumb().expect("a thumb");
                assert_eq!(
                    after.start,
                    start.min(lowest.start),
                    "{total}: {start} -> {top}"
                );
            }
            assert_eq!(first.top_for(10), total - 10);
        }
    }

    #[test]
    fn a_rail_mark_names_the_first_prompt_on_its_row() {
        let prompts = [0, 35, 38, 72];
        let rail = laid(100, 90, &prompts);

        assert_eq!(rail.prompt_at(3, prompts), Some(35));
        assert_eq!(rail.prompt_at(7, prompts), Some(72));
        assert_eq!(rail.prompt_at(5, prompts), None);
    }
}
