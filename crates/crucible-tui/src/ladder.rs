//! The rungs of one setting, drawn as a track with a mark on it: a rule, a
//! title, what each end buys, and the rungs across.
//!
//! Not [`crate::Panel`] with different data, which is its next reader's first
//! question. A panel lists things that are unlike each other — providers,
//! models, ways to sign in — so it spends two rows on each and reads down. These
//! rungs are one thing at five settings, and the fact worth seeing is not what
//! any of them is called but where the one in force sits between the two ends.
//! A list puts five descriptions where the answer is an ordering, and costs
//! fourteen rows and a paragraph doing it — the whole of an eighty-by-twenty-four
//! window, for a question with five answers.
//!
//! **Rows, not a screen.** Its title, its ends and its footer are the caller's,
//! and it never asks how tall the terminal is — the bargain [`crate::Panel`] is
//! on, and what keeps the next setting with an ordering from copying this one.
//!
//! **One left edge.** The rule, the title, the track and the footer all open in
//! the first column, and the rungs hang under the track. A nine-row block is
//! not tall enough to need a margin of its own.
//!
//! **Where the colour goes.** The track is [`Slot::Accent`], the mark and the
//! rung under it [`Slot::Strong`], every other rung and both ends
//! [`Slot::Quiet`]. So the eye finds the mark first and the ordering second,
//! and a terminal with no colour at all still has the mark's own shape saying
//! which rung it stands over.
//!
//! The rule across the top is [`Slot::Quiet`] and not the accent a panel draws
//! its one rule in, because this component has two horizontal lines and only
//! one of them is the subject. Drawn in the same colour they compete, and the
//! wider of the two — the one that is merely a boundary — wins.
//!
//! **What it will not do.** There is no bar, no fill and no percentage. Which
//! rungs a model serves is its vendor's answer and how much thinking each buys
//! is not a number this program is ever told, so a quantity drawn here would be
//! crucible inventing one. A mark on a track says an ordering, which is the
//! most that is true.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width::{clip, columns as wide};

/// The widest a track is drawn, however wide the window is.
///
/// Past this the rungs stop reading as one row and start reading as five words
/// scattered across a desk, which is what a list already does better. A window
/// wider than this gets the ladder at this width and blank columns after it.
const WIDEST: usize = 62;

/// Blank columns between one rung and the next.
///
/// Wide enough that two rungs never read as one phrase, and the reason there is
/// track to be seen between the stops it joins.
const GAP: usize = 4;

/// One setting's rungs, in order, with one of them marked.
#[derive(Debug, Clone, Copy)]
pub struct Ladder<'a> {
    /// The few words at the top saying what is being chosen.
    pub title: &'a str,
    /// The rungs, lowest first. Their order is the ordering being drawn, so a
    /// caller that hands them in shuffled has drawn a shuffled ladder.
    pub rungs: &'a [&'a str],
    /// Which rung a key would act on. Past the end marks nothing, the same
    /// answer [`crate::Panel`] gives.
    pub chosen: usize,
    /// What the two ends buy, low first — the pair that makes the ordering mean
    /// something rather than being five words in a row.
    pub ends: (&'a str, &'a str),
    /// The keys worth naming, on the last row.
    pub footer: &'a str,
}

impl Ladder<'_> {
    /// The whole ladder, drawn for a terminal `columns` wide.
    ///
    /// Empty where the rungs do not fit across, which is the same last rung
    /// [`crate::Panel::within`] has and for the same reason: a ladder drawn at
    /// as-much-as-fits reads as the whole ladder, and the caller with no room
    /// owes the reader the rungs some other way.
    #[must_use]
    pub fn rows(&self, columns: usize, glyphs: Glyphs) -> Vec<Row> {
        let Some(laid) = Laid::new(self.rungs, columns) else {
            return Vec::new();
        };

        vec![
            Row::new().then(Slot::Quiet, glyphs.horizontal().repeat(columns)),
            Row::new(),
            Row::new().then(Slot::Strong, clip(self.title, columns)),
            Row::new(),
            self.ended(&laid),
            self.tracked(&laid, glyphs),
            self.runged(&laid),
            Row::new(),
            Row::new().then(Slot::Quiet, clip(self.footer, columns)),
        ]
    }

    /// What each end buys: the low one over the start of the track, the high one
    /// ending on its last column.
    fn ended(&self, laid: &Laid) -> Row {
        let (low, high) = self.ends;
        let (low, high) = (clip(low, laid.across), clip(high, laid.across));

        // Where the two would meet there is no room to say both, and half a
        // phrase over each end of a track says less than the rungs under it
        // already do.
        let between = laid.across.saturating_sub(wide(low) + wide(high));
        if between == 0 {
            return Row::new();
        }

        Row::new()
            .then(Slot::Quiet, low)
            .then(Slot::Quiet, " ".repeat(between))
            .then(Slot::Quiet, high)
    }

    /// The track, with the mark standing on the rung in force.
    fn tracked(&self, laid: &Laid, glyphs: Glyphs) -> Row {
        let Some(at) = laid.middles.get(self.chosen).copied() else {
            return Row::new().then(Slot::Accent, glyphs.horizontal().repeat(laid.across));
        };

        let mut row = Row::new().then(Slot::Accent, glyphs.horizontal().repeat(at));

        row.push(Slot::Strong, glyphs.mark());
        row.push(
            Slot::Accent,
            glyphs.horizontal().repeat(laid.across - at - 1),
        );

        row
    }

    /// The rungs across, each centred under its own stop on the track.
    fn runged(&self, laid: &Laid) -> Row {
        let mut row = Row::new();
        let mut at = 0;

        for (which, (rung, middle)) in self.rungs.iter().zip(&laid.middles).enumerate() {
            let opens = middle.saturating_sub(wide(rung) / 2);
            let slot = if which == self.chosen {
                Slot::Strong
            } else {
                Slot::Quiet
            };

            row.push(Slot::Quiet, " ".repeat(opens.saturating_sub(at)));
            row.push(slot, (*rung).to_owned());

            at = opens.max(at) + wide(rung);
        }

        row
    }
}

/// Where each rung stands, worked out once for the three rows that need it.
struct Laid {
    /// The column each rung's middle sits on.
    middles: Vec<usize>,
    /// How wide the track runs.
    across: usize,
}

impl Laid {
    /// Lays `rungs` out for a terminal `columns` wide, or `None` where the row
    /// they need is wider than that.
    ///
    /// Every rung is given the room of the widest one, so the stops are evenly
    /// spaced whatever the words are — an ordering drawn at uneven intervals is
    /// one whose spacing says something the caller did not mean.
    ///
    /// The track takes the width it is given, up to [`WIDEST`]. A track that
    /// stopped at its narrowest would leave the mark travelling a third of a
    /// wide window, which reads as a ladder that has been cut off rather than
    /// one with room to move in.
    fn new(rungs: &[&str], columns: usize) -> Option<Self> {
        let each = rungs.iter().copied().map(wide).max()?;
        let least = rungs.len() * each + rungs.len().saturating_sub(1) * GAP;
        if least > columns {
            return None;
        }

        let across = columns.min(WIDEST).max(least);

        // The first and last stops are the middles of the rungs standing on the
        // ends, so the track starts and finishes under a word rather than in
        // the blank beside one. What is shared out is the room between them.
        let between = across - each;
        let highest = rungs.len().saturating_sub(1);

        Some(Self {
            middles: (0..rungs.len())
                .map(|at| each / 2 + (at * between).checked_div(highest).unwrap_or(0))
                .collect(),
            across,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::dump::dump;

    use super::*;

    /// crucible's own rungs, which are what this component was drawn for.
    const RUNGS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

    /// The narrowest window they fit across: five `medium`s and four gaps.
    const TIGHT: usize = 5 * 6 + 4 * GAP;

    /// The effort ladder, with the mark on `chosen`.
    fn effort(chosen: usize) -> Ladder<'static> {
        Ladder {
            title: "Effort · claude-sonnet-5",
            rungs: &RUNGS,
            chosen,
            ends: ("Faster", "Smarter"),
            footer: "←/→ to adjust · enter to confirm · esc to cancel",
        }
    }

    /// What the ladder says, row by row.
    fn art(ladder: &Ladder<'_>, columns: usize, glyphs: Glyphs) -> Vec<String> {
        ladder.rows(columns, glyphs).iter().map(Row::text).collect()
    }

    /// One of those rows, or a sentence saying there was none.
    fn said(rows: &[String], at: usize) -> &str {
        rows.get(at).map_or("<no such row>", String::as_str)
    }

    /// Where the one strong run of `row` opens and closes, in columns.
    ///
    /// Read off the slots rather than by searching for a word: `high` is inside
    /// `xhigh`, so a test that went looking would find the wrong one.
    fn strong(row: &Row) -> Option<(usize, usize)> {
        let mut at = 0;

        for (slot, text) in row.spans() {
            if slot == Slot::Strong {
                return Some((at, at + wide(text)));
            }

            at += wide(text);
        }

        None
    }

    #[test]
    fn a_ladder_is_a_rule_a_title_the_ends_the_track_and_the_rungs() {
        let rows = art(&effort(0), 80, Glyphs::Unicode);

        assert_eq!(rows.len(), 9, "{rows:?}");
        assert_eq!(said(&rows, 0), "─".repeat(80));
        assert_eq!(said(&rows, 2), "Effort · claude-sonnet-5");
        assert!(said(&rows, 4).starts_with("Faster"), "{rows:?}");
        assert!(said(&rows, 4).ends_with("Smarter"), "{rows:?}");
        assert_eq!(said(&rows, 6).split_whitespace().collect::<Vec<_>>(), RUNGS);
        assert_eq!(
            said(&rows, 8),
            "←/→ to adjust · enter to confirm · esc to cancel"
        );

        // The blanks that part the ladder's parts.
        let parting = |at: &usize| said(&rows, *at).trim().is_empty();
        assert!([1, 3, 7].iter().all(parting), "{rows:?}");
    }

    #[test]
    fn the_mark_stands_over_the_rung_in_force_wherever_that_is() {
        // The whole of what this component says: which rung is in force, and
        // where it sits between the ends. A mark that drifted off its word
        // would name a different rung than the one the next key acts on.
        for chosen in 0..RUNGS.len() {
            let rows = effort(chosen).rows(80, Glyphs::Unicode);
            let (mark, _) = rows.get(5).and_then(strong).expect("a marked track");
            let (opens, closes) = rows.get(6).and_then(strong).expect("a marked rung");

            // Its middle, not merely somewhere inside it: a rung drawn from the
            // stop rather than centred on it still has the mark over a letter of
            // the right word, with every word on the row half a word adrift.
            assert_eq!(
                mark,
                opens + (closes - opens) / 2,
                "rung {chosen} runs {opens}..{closes} and the mark is at {mark}"
            );
        }
    }

    #[test]
    fn the_rung_in_force_is_the_only_one_the_colour_picks_out() {
        // Four quiet rungs and one strong: a second strong run would leave the
        // colour saying one thing and the mark another.
        let rows = effort(3).rows(80, Glyphs::Unicode);
        let strong = rows.get(6).map_or(0, |row| {
            row.spans()
                .filter(|(slot, _)| *slot == Slot::Strong)
                .count()
        });

        assert_eq!(strong, 1, "{rows:?}");
    }

    #[test]
    fn the_two_ends_are_measured_against_the_track_and_not_the_window() {
        // What makes five words an ordering rather than a row of words.
        let rows = effort(0).rows(TIGHT, Glyphs::Unicode);

        assert_eq!(rows.get(4).map(Row::columns), Some(TIGHT), "{rows:?}");
        assert_eq!(rows.get(5).map(Row::columns), Some(TIGHT));
    }

    #[test]
    fn ends_that_would_meet_in_the_middle_are_not_drawn_at_all() {
        // Half a phrase over each end of a short track says less than the rungs
        // under it already do.
        let rungs = ["a", "b"];
        let ladder = Ladder {
            rungs: &rungs,
            ends: ("altogether", "unabbreviated"),
            ..effort(0)
        };

        assert_eq!(said(&art(&ladder, 6, Glyphs::Unicode), 4), "");
    }

    #[test]
    fn a_window_with_no_room_across_is_drawn_as_nothing() {
        // One column short is a ladder the caller has to say some other way,
        // and it says so by handing back nothing rather than four of the five.
        assert!(effort(0).rows(TIGHT - 1, Glyphs::Unicode).is_empty());
        assert_eq!(effort(0).rows(TIGHT, Glyphs::Unicode).len(), 9);
    }

    #[test]
    fn no_row_is_ever_drawn_past_the_last_column_of_the_window() {
        // The rule the whole crate is on: a row wider than the terminal is a row
        // the terminal wraps, and a wrapped row is a frame that rewinds over the
        // wrong number of lines.
        // Both glyph sets, because what keeps the mark inside the track is that
        // it is one column wide whichever character it is drawn with.
        for columns in TIGHT..=200 {
            for (chosen, glyphs) in (0..RUNGS.len()).zip([Glyphs::Unicode, Glyphs::Ascii].repeat(3))
            {
                let rows = effort(chosen).rows(columns, glyphs);

                assert_eq!(rows.len(), 9, "{columns} columns");
                assert!(
                    rows.iter().all(|row| row.columns() <= columns),
                    "{columns} columns, rung {chosen}: {rows:?}"
                );
            }
        }
    }

    #[test]
    fn a_terminal_without_the_marks_gets_a_track_it_can_draw() {
        let rows = art(&effort(2), 80, Glyphs::Ascii);
        let track = said(&rows, 5);

        assert!(track.starts_with('-'), "{track:?}");
        assert!(track.contains('^'), "{track:?}");
        assert!(!track.contains('─') && !track.contains('▲'), "{track:?}");
    }

    #[test]
    fn the_ladder_at_eighty_columns() {
        // The one snapshot: where the colour falls is what a picture says
        // better than an assertion. The rules above are the rules.
        insta::assert_snapshot!(dump(&effort(2).rows(80, Glyphs::Unicode), 80));
    }
}
