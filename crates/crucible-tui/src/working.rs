//! The row that says a turn is running, and for how long.
//!
//! One row, above the box and under whatever the turn has written so far. A
//! turn is the only stretch of a session where the program is busy and the
//! screen can be still for a minute at a time, so this is the row that says the
//! stillness is work rather than a hang: a mark that turns, the one word for
//! what is being done, and a clock.
//!
//! Like [`crate::Notice`] it returns a [`Row`] and draws nothing, so what it
//! says is decided with no terminal anywhere near it — including the face the
//! mark is wearing, which is read off the clock rather than counted. That is
//! what makes the whole row a function of how long the turn has run: a caller
//! that redraws twice in the same beat draws the same row twice, and one that
//! missed a beat is behind by a face rather than out of step for good.

use std::time::Duration;

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width::clip;

/// How long one face of the mark is on screen.
///
/// Four faces a second: fast enough to read as motion, slow enough that the row
/// is redrawn four times a second rather than sixty. The clock beside the mark
/// counts in whole seconds, which is a multiple of this — so [`Working::beat`]
/// changing is the whole of what makes the row different.
const BEAT: Duration = Duration::from_millis(250);

/// Seconds in a minute, and minutes in an hour.
const OVER: u64 = 60;

/// What a turn is doing, how long it has been doing it, and how to stop it.
#[derive(Debug, Clone, Copy)]
pub struct Working<'a> {
    /// The one word for what is happening at this moment.
    pub doing: &'a str,
    /// How long the turn has been running.
    pub running: Duration,
    /// The key that stops it. `None` where there is nothing left to ask for —
    /// a turn already stopping is not one to offer to stop again.
    pub stops: Option<&'a str>,
}

impl Working<'_> {
    /// Which beat of the mark's turn `running` falls in.
    ///
    /// The one number the row changes with, which is why it is public: a caller
    /// redrawing on a clock of its own asks this instead of laying the row out
    /// to see whether it moved. Everything on the row is read from `running`,
    /// and everything it is read *by* divides this — so a beat that has not
    /// changed is a row that would be drawn identically.
    #[must_use]
    pub fn beat(running: Duration) -> u64 {
        u64::try_from(running.as_millis() / BEAT.as_millis()).unwrap_or(u64::MAX)
    }

    /// The row, drawn for a terminal `columns` wide.
    ///
    /// Never wider than that: a row past the last column is one the terminal
    /// wraps itself, which puts the cursor a row below where the next frame
    /// expects it.
    #[must_use]
    pub fn row(&self, columns: usize, glyphs: Glyphs) -> Row {
        let mark = glyphs.turning(Self::beat(self.running));
        let wide = crate::width::columns(mark);

        if columns < wide {
            return Row::new();
        }

        let mut row = Row::new().then(Slot::Accent, mark);
        let doing = clip(self.doing, columns.saturating_sub(wide + 1));

        if !doing.is_empty() {
            row.push(Slot::Plain, format!(" {doing}"));
        }

        // Longest first, and the first that fits is the one drawn. What is
        // dropped is what a second reading gets back: the key is under the box
        // as well, and the clock is a number that will be there next second.
        let clock = elapsed(self.running);
        let room = columns.saturating_sub(row.columns());
        let both = self
            .stops
            .map(|stops| format!(" ({clock} {} {stops})", glyphs.dot()));

        if let Some(both) = both.filter(|said| crate::width::columns(said) <= room) {
            row.push(Slot::Quiet, both);
        } else {
            let alone = format!(" ({clock})");

            if crate::width::columns(&alone) <= room {
                row.push(Slot::Quiet, alone);
            }
        }

        row
    }
}

/// How long a turn has been running, in the units somebody reads it in.
///
/// Seconds while there are only seconds, and the larger unit as soon as there
/// is one to say. Hours drop the seconds outright: a turn measured in them is
/// one nobody is timing to the second, and a third pair would be the segment
/// that pushed the key off a narrow window.
fn elapsed(running: Duration) -> String {
    let seconds = running.as_secs();
    let (minutes, seconds) = (seconds / OVER, seconds % OVER);
    let (hours, minutes) = (minutes / OVER, minutes % OVER);

    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A turn that has been running for `seconds`, with the key to stop it.
    fn after(seconds: u64) -> Working<'static> {
        Working {
            doing: "thinking",
            running: Duration::from_secs(seconds),
            stops: Some("esc to interrupt"),
        }
    }

    /// What that row says on a terminal `wide` columns across.
    fn drawn(working: &Working<'_>, wide: usize) -> String {
        working.row(wide, Glyphs::Unicode).text()
    }

    #[test]
    fn a_running_turn_is_a_mark_a_word_the_clock_and_the_key_that_stops_it() {
        // The whole row in one assertion, because the order is the point: the
        // mark says something is happening, the word says what, and the rest is
        // said quietly after it, for whoever is deciding whether to wait.
        assert_eq!(
            drawn(&after(176), 80),
            "✳ thinking (2m 56s · esc to interrupt)"
        );
    }

    #[test]
    fn the_mark_turns_and_nothing_else_on_the_row_moves_with_it() {
        // The defect this rules out is the one that makes an animated row
        // unreadable: a face a column wider than the last shifts every word
        // after it sideways four times a second. Four faces, no two alike, one
        // width between them, and the same sentence beside every one.
        let turned: Vec<Row> = (0..4)
            .map(|beat| {
                Working {
                    running: BEAT * beat,
                    ..after(0)
                }
                .row(80, Glyphs::Unicode)
            })
            .collect();

        let mut faces = Vec::new();

        for row in &turned {
            let drawn = row.text();
            let mut letters = drawn.chars();
            let face = letters.next().expect("a mark at the front of the row");

            assert_eq!(letters.as_str(), " thinking (0s · esc to interrupt)");
            assert_eq!(Some(row.columns()), turned.first().map(Row::columns));

            faces.push(face);
        }

        for (at, face) in faces.iter().enumerate() {
            for (other, theirs) in faces.iter().enumerate() {
                assert_eq!(face == theirs, at == other, "{face} against {theirs}");
            }
        }
    }

    #[test]
    fn a_turn_is_timed_in_the_units_somebody_reads_it_in() {
        // Seconds while there are only seconds, and the larger unit as soon as
        // there is one to say. The smaller is padded once it is not the first,
        // so the row keeps its width across a whole minute rather than losing a
        // column at nine seconds past every one of them.
        for (seconds, said) in [
            (0, "(0s "),
            (21, "(21s "),
            (59, "(59s "),
            (60, "(1m 00s "),
            (176, "(2m 56s "),
            (3600, "(1h 00m "),
            (3840, "(1h 04m "),
        ] {
            let row = drawn(&after(seconds), 80);
            assert!(row.contains(said), "{seconds} seconds read as {row}");
        }
    }

    #[test]
    fn a_narrow_window_drops_the_key_before_the_clock_and_the_clock_before_the_word() {
        // In that order, because that is the order they stop being worth the
        // room. The word is what the row is for, the clock is what it is
        // watched by, and the key is named a second time under the box — so it
        // is the one thing here that a narrow window takes nothing away by
        // dropping.
        assert_eq!(drawn(&after(21), 40), "✳ thinking (21s · esc to interrupt)");
        assert_eq!(drawn(&after(21), 34), "✳ thinking (21s)");
        assert_eq!(drawn(&after(21), 15), "✳ thinking");
        assert_eq!(drawn(&after(21), 6), "✳ thin");
    }

    #[test]
    fn a_turn_that_is_already_stopping_is_not_offered_a_key_to_stop_it() {
        let stopping = Working {
            doing: "interrupting",
            stops: None,
            ..after(64)
        };

        assert_eq!(drawn(&stopping, 80), "✳ interrupting (1m 04s)");
    }

    #[test]
    fn nothing_is_ever_drawn_past_the_last_column() {
        for wide in [0, 1, 2, 8, 20, 40, 79, 80, 200] {
            for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
                let row = after(3840).row(wide, glyphs);

                assert!(row.columns() <= wide, "{wide}: {:?}", row.text());
            }
        }
    }

    #[test]
    fn the_beat_is_what_says_whether_the_row_would_be_drawn_differently() {
        // What the loop above redraws on. Twice in one beat is the same row
        // twice, so the number standing in for it has to hold still for exactly
        // as long as the row does — and has to move at the second boundary,
        // where the clock changes whether the mark would have or not.
        assert_eq!(Working::beat(Duration::ZERO), 0);
        assert_eq!(Working::beat(Duration::from_millis(249)), 0);
        assert_eq!(Working::beat(Duration::from_millis(250)), 1);
        assert_eq!(Working::beat(Duration::from_millis(999)), 3);
        assert_eq!(Working::beat(Duration::from_secs(1)), 4);
    }
}
