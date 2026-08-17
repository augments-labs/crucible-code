//! The row that says a turn is running, for how long, and at what cost.
//!
//! One row, above the box and under whatever the turn has written so far. A
//! turn is the only stretch of a session where the program is busy and the
//! screen can be still for a minute at a time, so this is the row that says the
//! stillness is work rather than a hang: a mark that turns, the one word for
//! what is being done, a clock, and what has been spent reaching this point.
//!
//! Like [`crate::Notice`] it returns a [`Row`] and draws nothing, so what it
//! says is decided with no terminal anywhere near it — including the face the
//! mark is wearing, which is read off the clock rather than counted. So the row
//! is a function of its own fields and of nothing else: a caller that redraws
//! twice in the same beat, with nothing else changed, draws the same row twice,
//! and one that missed a beat is behind by a face rather than out of step for
//! good.
//!
//! A caller with a second row to write under this one aligns it with
//! [`Working::gutter`] rather than with a two it counted off the screen. The
//! mark is this file's, and so is how wide it is.

use std::time::Duration;

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width::clip;

/// How long one face of the mark is on screen.
///
/// Four faces a second: fast enough to read as motion, slow enough that the row
/// is redrawn four times a second rather than sixty. The clock beside the mark
/// counts in whole seconds, which is a multiple of this — so between one event
/// and the next, [`Working::beat`] changing is the whole of what makes the row
/// different.
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
    /// What it has spent so far, counted in tokens. `None` where nobody has
    /// said — which is not the same as zero, and is what a provider that never
    /// reports and a turn that has produced nothing look like apart.
    pub spent: Option<u64>,
    /// The key that stops it. `None` where there is nothing left to ask for —
    /// a turn already stopping is not one to offer to stop again.
    pub stops: Option<&'a str>,
}

impl Working<'_> {
    /// Which beat of the mark's turn `running` falls in.
    ///
    /// Everything the row reads off the clock is read off this: the face the
    /// mark is wearing, and the elapsed time, whose every unit divides it. It
    /// is public so that a caller redrawing on a clock of its own can ask this
    /// instead of laying the row out to see whether it moved — but it stands in
    /// for the clock alone. A caller keying a redraw on it owes the other
    /// fields beside it, and a field left out is one that reaches the screen
    /// only when something else on the row happens to change with it.
    #[must_use]
    pub fn beat(running: Duration) -> u64 {
        u64::try_from(running.as_millis() / BEAT.as_millis()).unwrap_or(u64::MAX)
    }

    /// How far the words on this row stand from the left, in columns.
    ///
    /// The mark and the space after it, measured rather than counted — and
    /// measured off one face rather than off the one being worn, because every
    /// face is a column wide and a gutter that moved with the beat would be a
    /// row sliding sideways four times a second.
    ///
    /// Published because a caller writing a second row under this one has to
    /// start it in the same column the word starts in, and the alternative is a
    /// two written down somewhere else — a copy of a fact this file owns, which
    /// goes wrong on the day a glyph set spells the mark differently.
    #[must_use]
    pub fn gutter(glyphs: Glyphs) -> usize {
        crate::width::columns(glyphs.turning(0)).saturating_add(1)
    }

    /// The row, drawn for a terminal `columns` wide.
    ///
    /// Never wider than that: a row past the last column is one the terminal
    /// wraps itself, which puts the cursor a row below where the next frame
    /// expects it.
    #[must_use]
    pub fn row(&self, columns: usize, glyphs: Glyphs) -> Row {
        let mark = glyphs.turning(Self::beat(self.running));

        if columns < crate::width::columns(mark) {
            return Row::new();
        }

        let mut row = Row::new().then(Slot::Accent, mark);
        let doing = clip(self.doing, columns.saturating_sub(Self::gutter(glyphs)));

        if !doing.is_empty() {
            row.push(Slot::Plain, format!(" {doing}"));
        }

        if let Some(said) = self.tail(columns.saturating_sub(row.columns()), glyphs) {
            row.push(Slot::Quiet, said);
        }

        row
    }

    /// What is said after the word, in `room` columns or not at all.
    ///
    /// The segments are dropped from the right, longest reading first, and what
    /// goes is what a second look gets back anyway: the key is named under the
    /// box as well, and the count is a number that will be there next second.
    fn tail(&self, room: usize, glyphs: Glyphs) -> Option<String> {
        let mut segments = vec![elapsed(self.running)];

        if let Some(spent) = self.spent {
            segments.push(format!("{} {}", glyphs.down(), counted(spent)));
        }
        if let Some(stops) = self.stops {
            segments.push(stops.to_owned());
        }

        let parting = format!(" {} ", glyphs.dot());

        while !segments.is_empty() {
            let said = format!(" ({})", segments.join(&parting));

            if crate::width::columns(&said) <= room {
                return Some(said);
            }

            segments.pop();
        }

        None
    }
}

/// What a turn has spent, in the units somebody reads it in.
///
/// Exact while the number is small enough to read exactly, and one decimal of
/// the larger unit after that. The decimal is what keeps the count moving:
/// whole thousands hold still for a thousand tokens at a time, and a number
/// that has stopped, on the row whose whole job is to say work has not, is the
/// row saying the opposite of what it is there for.
///
/// A tenth of nothing is not written. `1k` is the whole of what 1000 has to
/// say, and the `.0` in `1.0k` is a column spent on a digit that is there to
/// hold a place rather than to be read.
fn counted(spent: u64) -> String {
    for (unit, over) in [('M', 1_000_000), ('k', 1_000)] {
        if spent >= over {
            // Integer throughout: the tenth is taken off the remainder rather
            // than rounded off a float, which is one decimal digit that cannot
            // be a rounding of a number nobody counted that way.
            let (whole, tenth) = (spent / over, (spent % over) * 10 / over);

            return match tenth {
                0 => format!("{whole}{unit}"),
                _ => format!("{whole}.{tenth}{unit}"),
            };
        }
    }

    spent.to_string()
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
            spent: None,
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
    fn what_a_turn_has_spent_is_said_between_the_clock_and_the_key() {
        // Between them because that is where it is read: the clock says how
        // long this has been going on, the count says how much of it there has
        // been, and the key is what somebody does about the answer.
        let spending = Working {
            spent: Some(12_800),
            ..after(176)
        };

        assert_eq!(
            drawn(&spending, 80),
            "✳ thinking (2m 56s · ↓ 12.8k · esc to interrupt)"
        );
    }

    #[test]
    fn a_turn_nobody_has_told_what_it_spent_leaves_the_segment_out() {
        // Rather than drawing a zero. A provider that never reports and a turn
        // that has produced nothing are different facts, and only one of them
        // is worth a number on the screen — and the first is what every turn
        // looks like for its first second.
        assert_eq!(
            drawn(&after(176), 80),
            "✳ thinking (2m 56s · esc to interrupt)"
        );
    }

    #[test]
    fn what_a_turn_has_spent_is_counted_in_the_units_somebody_reads_it_in() {
        // Exact while the number is small enough to read exactly, and one
        // decimal of the larger unit after that. The decimal is what keeps the
        // count moving: whole thousands hold still for a thousand tokens at a
        // time, which on the row that says work is happening reads as work
        // having stopped.
        //
        // And a tenth of nothing is not written: 1000 is `1k`, because the
        // digit in `1.0k` holds a place rather than saying anything.
        for (spent, said) in [
            (0, "↓ 0 "),
            (320, "↓ 320 "),
            (999, "↓ 999 "),
            (1_000, "↓ 1k "),
            (1_161, "↓ 1.1k "),
            (1_420, "↓ 1.4k "),
            (12_800, "↓ 12.8k "),
            (128_400, "↓ 128.4k "),
            (1_000_000, "↓ 1M "),
            (1_250_000, "↓ 1.2M "),
        ] {
            let row = drawn(
                &Working {
                    spent: Some(spent),
                    ..after(21)
                },
                80,
            );

            assert!(row.contains(said), "{spent} read as {row}");
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
    fn a_narrow_window_drops_the_key_then_the_count_then_the_clock_then_clips_the_word() {
        // In that order, because that is the order they stop being worth the
        // room. The word is what the row is for, the clock is what it is
        // watched by, the count is a number that will be there next second, and
        // the key is named a second time under the box — so it is the one thing
        // here that a narrow window takes nothing away by dropping.
        let spending = Working {
            spent: Some(12_800),
            ..after(21)
        };

        assert_eq!(
            drawn(&spending, 45),
            "✳ thinking (21s · ↓ 12.8k · esc to interrupt)"
        );
        assert_eq!(drawn(&spending, 44), "✳ thinking (21s · ↓ 12.8k)");
        assert_eq!(drawn(&spending, 25), "✳ thinking (21s)");
        assert_eq!(drawn(&spending, 15), "✳ thinking");
        assert_eq!(drawn(&spending, 6), "✳ thin");
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
    fn the_gutter_is_the_column_the_word_on_the_row_starts_in() {
        // What a second row written under this one is aligned by. Asserted
        // against the row itself rather than against a number, because the
        // number is the thing that would go stale: a mark spelt differently in
        // some later glyph set moves the word, and a gutter that did not move
        // with it would indent the row under it to nowhere.
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let said = after(21).row(80, glyphs).text();
            let head = clip(&said, Working::gutter(glyphs));

            assert!(head.ends_with(' '), "{glyphs:?}: {said}");
            assert!(
                said.strip_prefix(head)
                    .is_some_and(|word| word.starts_with("thinking")),
                "{glyphs:?}: {said}"
            );
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
