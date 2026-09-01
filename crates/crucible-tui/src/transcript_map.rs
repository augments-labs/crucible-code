//! The transcript map: absolute travel over what the record retains.
//!
//! It owns the fixed row at the bottom of the window, below the prompt's status.
//! At rest the row directly under the prompt status offers `transcript map` at
//! its right edge; under the pointer the exact accent becomes the compact
//! rectangle behind contrasting text. Clicked, the whole row becomes a track
//! from the oldest retained line to the live edge. A drag along the track moves
//! the transcript directly to the corresponding place, so
//! distance costs one gesture rather than enough wheel notches to cross it. The
//! wheel remains the precise control and moves the mark with the transcript.
//!
//! The map is deliberately neutral. Its labels, landmarks and untouched rail
//! use [`Slot::Quiet`]; the travelled rail and current place use
//! [`Slot::Accent`]. Both are jobs every palette already answers, so dark,
//! light, colourblind, ANSI and colourless runs get the map their own theme can
//! draw without a hue invented for this one component. Landmarks differ by
//! shape, not colour.
//!
//! Nothing here grows with the transcript. The record binary-searches cached
//! display-row ends and keeps a fixed number of prompt landmarks; this row
//! allocates only in proportion to the terminal width. It also holds no clock
//! thread. The input waits already owned by the terminal wake at the map's idle
//! deadline and put the bottom-row control back.

use std::ops::Range;
use std::time::{Duration, Instant};

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::record::{MapSpan, Record};
use crate::row::Row;
use crate::width;

/// How long after the last map gesture its row remains visible.
const REST_AFTER: Duration = Duration::from_secs(3);

/// The door into absolute transcript travel, before its opening mark.
const CONTROL: &str = "transcript map";

/// Which columns the resting door occupies.
pub(crate) fn door(columns: usize) -> Option<Range<usize>> {
    // The mark is one column in both glyph sets, and the space before it one
    // more. The range therefore means the same before the glyph set is known.
    // One space on either side is part of the control. Under the pointer those
    // cells take the ground too, making a compact rectangle rather than a
    // background that stops at the first and last letter.
    let wide = width::columns(CONTROL) + 4;
    let start = columns.checked_sub(wide)?;
    Some(start..columns)
}

/// The fixed bottom row before its map is opened.
pub(crate) fn resting(columns: usize, glyphs: Glyphs, pointed: bool) -> Row {
    let Some(door) = door(columns) else {
        return Row::new();
    };
    let mut row = Row::new();
    row.pad(door.start);
    row.push(
        if pointed { Slot::Pointed } else { Slot::Accent },
        format!(" {CONTROL} {} ", glyphs.stepping().1),
    );
    row
}

/// Labels outside the track. Kept short because every column they spend is one
/// fewer absolute destination a narrow terminal can name.
const FIRST: &str = "first ";
const NOW: &str = " now";

/// A map as it stands between input events.
#[derive(Debug, Default)]
pub(crate) struct TranscriptMap {
    /// The retained range meant when the map opened. Frozen for the short time
    /// it stands, so streamed output cannot move a destination under a pointer.
    span: Option<MapSpan>,
    /// Its laid-out row, rebuilt only when an interaction moves the mark.
    row: Option<Row>,
    /// A button went down on the track and has not come up yet.
    dragging: bool,
    /// The column that press began on, for a click whose release is reported
    /// somewhere else. A drag uses every reported column and never this one.
    pressed: Option<usize>,
    /// Whether motion arrived between that press and release. A plain click can
    /// snap to a landmark; a drag is exact.
    moved: bool,
    /// When the identity row comes back. None while closed or while held.
    rests: Option<Instant>,
    /// Whether a standing panel has the bottom band.
    ///
    /// Set for the length of a panel stood over the prompt: the prompt is
    /// covered then, and the row beside it goes blank with it, so what stands
    /// is the panel and not a door that reports on a screen the panel owns.
    /// The map itself is not closed or moved, only unpainted, and returns as
    /// it was when the panel does.
    covered: bool,
}

impl TranscriptMap {
    /// Whether the bottom row is currently the map.
    pub(crate) fn open(&self) -> bool {
        self.span.is_some()
    }

    /// The retained range the map was opened over.
    pub(crate) fn span(&self) -> Option<MapSpan> {
        self.span
    }

    /// What replaces the resting control while this stands.
    pub(crate) fn row(&self) -> Option<&Row> {
        self.row.as_ref()
    }

    /// Opens over `span`, already laid out as `row`.
    pub(crate) fn show(&mut self, span: MapSpan, row: Row, now: Instant) {
        self.span = Some(span);
        self.row = Some(row);
        self.dragging = false;
        self.pressed = None;
        self.moved = false;
        self.touch(now);
    }

    /// Replaces the row after its mark moved.
    pub(crate) fn replace(&mut self, row: Row) {
        self.row = Some(row);
    }

    /// Starts a pointer gesture on the track.
    pub(crate) fn press(&mut self, column: usize) {
        self.dragging = true;
        self.pressed = Some(column);
        self.moved = false;
        self.rests = None;
    }

    /// Records that the held pointer moved.
    pub(crate) fn drag(&mut self) -> bool {
        if !self.dragging {
            return false;
        }
        self.moved = true;
        true
    }

    /// Ends a held gesture and answers whether it was a drag rather than a
    /// click. Either starts the idle period from this release.
    pub(crate) fn release(&mut self, column: usize, now: Instant) -> Option<(usize, bool)> {
        if !self.dragging {
            return None;
        }
        self.dragging = false;
        let dragged = self.moved;
        let column = if dragged {
            column
        } else {
            self.pressed.unwrap_or(column)
        };
        self.pressed = None;
        self.moved = false;
        self.touch(now);
        Some((column, dragged))
    }

    /// Starts the idle period again after a wheel turn or landmark click.
    pub(crate) fn touch(&mut self, now: Instant) {
        if self.open() && !self.dragging {
            self.rests = now.checked_add(REST_AFTER);
        }
    }

    /// How long an input wait may sleep before this has to be put away.
    pub(crate) fn remaining(&self, now: Instant) -> Option<Duration> {
        self.rests.map(|rests| rests.saturating_duration_since(now))
    }

    /// Puts the resting control back if the idle period has elapsed.
    pub(crate) fn repose(&mut self, now: Instant) -> bool {
        if self.rests.is_none_or(|rests| now < rests) {
            return false;
        }
        self.close()
    }

    /// Makes the next idle check restore the identity row.
    #[cfg(test)]
    pub(crate) fn due(&mut self) {
        self.rests = Some(Instant::now());
    }

    /// Puts the resting control back now.
    pub(crate) fn close(&mut self) -> bool {
        let was = self.open();
        self.span = None;
        self.row = None;
        self.dragging = false;
        self.pressed = None;
        self.moved = false;
        self.rests = None;
        was
    }

    /// Covers or uncovers the bottom row, around a panel stood over the prompt.
    ///
    /// The change reports whether the row is painted differently afterwards, so
    /// a caller knows whether the frame is worth redrawing: covering an
    /// uncovered map is, covering a covered one is not.
    pub(crate) fn cover(&mut self, covered: bool) -> bool {
        let changed = self.covered != covered;
        self.covered = covered;
        changed
    }

    /// Whether a panel has the bottom band, so the row is painted blank.
    pub(crate) fn covered(&self) -> bool {
        self.covered
    }
}

/// Which columns of a map row are the absolute track.
pub(crate) fn track(columns: usize) -> Option<Range<usize>> {
    let first = width::columns(FIRST);
    let now = width::columns(NOW);
    let end = columns.checked_sub(now)?;
    (end > first).then_some(first..end)
}

/// Draws `record` over the frozen `span` at this width.
pub(crate) fn row(record: &Record, span: MapSpan, columns: usize, glyphs: Glyphs) -> Row {
    let Some(track) = track(columns) else {
        return Row::new().then(Slot::Quiet, width::clip("transcript", columns));
    };

    let cells = track.len();
    let at = record.map_position(span, cells);
    let landmarks = record.map_landmarks(span, cells);
    let rail = glyphs.horizontal();
    let landmark = glyphs.landmark();

    let mut row = Row::new().then(Slot::Quiet, FIRST);
    let mut travelled = String::new();
    for marked in landmarks.iter().take(at) {
        if *marked {
            row.push(Slot::Accent, std::mem::take(&mut travelled));
            row.push(Slot::Quiet, landmark);
        } else {
            travelled.push_str(rail);
        }
    }
    row.push(Slot::Accent, travelled);
    row.push(Slot::Accent, glyphs.filled());

    let mut ahead = String::new();
    for marked in landmarks.iter().skip(at + 1) {
        ahead.push_str(if *marked { landmark } else { rail });
    }
    row.push(Slot::Quiet, ahead);
    row.push(Slot::Quiet, NOW);
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Record;

    #[test]
    fn the_resting_control_stands_at_the_bottom_rows_right_edge() {
        let row = resting(40, Glyphs::Unicode, false);
        let door = door(40).expect("room for the control");

        assert_eq!(row.columns(), 40);
        assert!(row.text().ends_with(" transcript map → "));
        assert_eq!(door.len(), width::columns(" transcript map → "));
        assert_eq!(row.kinds().last(), Some(Slot::Accent));
    }

    #[test]
    fn the_pointed_control_uses_the_accent_as_its_ground() {
        let row = resting(40, Glyphs::Unicode, true);

        assert_eq!(row.kinds().last(), Some(Slot::Pointed));
        assert!(
            resting(40, Glyphs::Ascii, false)
                .text()
                .ends_with(" transcript map > ")
        );
    }

    #[test]
    fn the_map_uses_the_palette_jobs_and_no_hue_of_its_own() {
        let mut record = Record::new(40);
        for line in 0..20 {
            if line % 5 == 0 {
                record.landmark();
            }
            record.write(Slot::Plain, &format!("line {line}\n"), None);
        }
        let span = record.map_span(4).expect("the record to scroll");
        let row = row(&record, span, 40, Glyphs::Unicode);
        let kinds: Vec<Slot> = row.kinds().collect();

        assert!(kinds.contains(&Slot::Accent));
        assert!(kinds.contains(&Slot::Quiet));
        assert!(
            kinds
                .iter()
                .all(|slot| matches!(slot, Slot::Accent | Slot::Quiet))
        );
    }

    #[test]
    fn the_map_returns_to_rest_after_three_idle_seconds() {
        let now = Instant::now();
        let mut record = Record::new(40);
        for line in 0..10 {
            record.write(Slot::Plain, &format!("line {line}\n"), None);
        }
        let span = record.map_span(4).expect("the record to scroll");
        let mut map = TranscriptMap::default();
        map.show(span, row(&record, span, 40, Glyphs::Unicode), now);

        let almost = (now + REST_AFTER)
            .checked_sub(Duration::from_millis(1))
            .expect("one millisecond inside the deadline");
        assert!(!map.repose(almost));
        assert!(map.open());
        assert!(map.repose(now + REST_AFTER));
        assert!(!map.open());
    }

    #[test]
    fn a_drag_does_not_go_idle_while_the_button_is_held() {
        let now = Instant::now();
        let mut record = Record::new(40);
        for line in 0..10 {
            record.write(Slot::Plain, &format!("line {line}\n"), None);
        }
        let span = record.map_span(4).expect("the record to scroll");
        let mut map = TranscriptMap::default();
        map.show(span, row(&record, span, 40, Glyphs::Unicode), now);
        map.press(7);

        assert_eq!(map.remaining(now + REST_AFTER * 2), None);
        assert_eq!(map.release(11, now + REST_AFTER * 2), Some((7, false)));
        assert_eq!(map.remaining(now + REST_AFTER * 2), Some(REST_AFTER));
    }
}
