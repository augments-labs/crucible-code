//! How a window's rows are shared out.
//!
//! Four bands, always in this order down the screen: the transcript is
//! everything that has been said and the only band that scrolls; the turn says
//! what is happening while one runs and holds whatever else stands over the box
//! between turns; the prompt is the box, where the reader is typing, and grows
//! upwards as they do; the foot holds a blank spacer and the transcript map
//! below the prompt's own status. Bands do not overlap and together they are
//! the window, which is what lets a frame place a row absolutely and never
//! wonder what else is there.
//!
//! Their sizes are not a layout so much as an order of surrender. Two of them
//! want a fixed number of rows, one wants as many as it has, and one takes what
//! is left — and on a window too small for that, something has to go. What goes
//! is stated here once, from the least missed to the most: the transcript
//! shrinks to nothing first, then the turn, then the foot, and the prompt is
//! last because a reader who cannot see what they are typing has no way to fix
//! anything else. A window of one row is a prompt.
//!
//! Nothing here reads the terminal. It is given a number of rows and answers
//! with where each band starts and ends, so every question about the layout has
//! one answer and a test can ask it at any size.

use std::ops::Range;

/// The most of the window the prompt may take before the transcript stops
/// giving way to it.
///
/// Half, so that a long prompt can be read while what it is answering still
/// is. Past that the box scrolls internally, which is the box's business and
/// not this module's.
///
/// It bounds the box and nothing else. A list or a plan standing over the box
/// is the turn band's, which has no share and takes what it asks for — this
/// number is a rule about how much screen a prompt being written may take from
/// what it is a reply to, and a list is neither.
const SHARE: usize = 2;

/// How many rows each band that asks for a number would like.
///
/// The transcript is not here. It is what is left, which is the whole of what
/// makes it the band that scrolls: every other band is as tall as what stands
/// in it, and the one that cannot be is the one holding a session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Wants {
    /// What a running turn is showing, and anything else standing over the box.
    pub(crate) turn: usize,
    /// How tall the box has grown.
    pub(crate) prompt: usize,
    /// One transcript-map row below the prompt, or none.
    pub(crate) foot: usize,
}

/// Where each band is, in screen rows.
///
/// Empty ranges are meaningful and are not an error: a session between turns
/// has no turn band, and a window three rows tall has no transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bands {
    /// Everything that has been said. The only band that scrolls.
    pub(crate) transcript: Range<usize>,
    /// What a running turn is doing, and anything else standing over the box.
    pub(crate) turn: Range<usize>,
    /// The box being typed into.
    pub(crate) prompt: Range<usize>,
    /// The transcript-map row below the prompt.
    pub(crate) foot: Range<usize>,
}

impl Bands {
    /// Share `rows` out between the bands that asked for a number.
    pub(crate) fn share(rows: usize, wants: Wants) -> Self {
        // Bottom up, because the two that answer to a reader's hands — the
        // prompt and the status under it — are the two at the bottom, and
        // taking their rows first is what makes the order of surrender above
        // true rather than merely intended.
        //
        // A band nobody put anything in takes no rows at all. Only the prompt
        // is held to a row it did not ask for, and only where the window has
        // one: a box drawn as nothing is a session with no way back into it.
        let prompt = if rows == 0 || wants.prompt == 0 {
            0
        } else {
            wants.prompt.max(1).min((rows / SHARE).max(1))
        };
        let mut left = rows - prompt;

        let foot = wants.foot.min(left);
        left -= foot;

        let turn = wants.turn.min(left);
        let transcript = left - turn;

        // Laid out top-first, each band starting where the one above it ended,
        // so the four ranges are the window exactly once — no gap a frame would
        // leave stale and no overlap two bands would fight over.
        let transcript = 0..transcript;
        let turn = transcript.end..transcript.end + turn;
        let prompt = turn.end..turn.end + prompt;
        let foot = prompt.end..prompt.end + foot;

        Self {
            transcript,
            turn,
            prompt,
            foot,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Bands, SHARE, Wants};

    /// Every band, top first, for the walks below.
    fn all(bands: &Bands) -> [&std::ops::Range<usize>; 4] {
        [&bands.transcript, &bands.turn, &bands.prompt, &bands.foot]
    }

    /// A session with something in every band: the shape the walks below are
    /// about, since a band nobody filled is one no order of surrender reaches.
    fn full(turn: usize, prompt: usize) -> Wants {
        Wants {
            turn,
            prompt,
            foot: 1,
        }
    }

    #[test]
    fn the_four_bands_are_the_window_exactly_once() {
        for rows in 0..80 {
            for turn in 0..4 {
                for prompt in 1..12 {
                    let bands = Bands::share(rows, full(turn, prompt));
                    let mut at = 0;
                    for band in all(&bands) {
                        assert_eq!(band.start, at, "a gap or an overlap at {rows} rows");
                        at = band.end;
                    }
                    assert_eq!(at, rows, "{rows} rows shared out as {at}");
                }
            }
        }
    }

    #[test]
    fn the_prompt_is_the_last_band_to_give_up_a_row() {
        for rows in 1..40 {
            let bands = Bands::share(rows, full(1, 3));
            assert!(!bands.prompt.is_empty(), "no prompt at {rows} rows");
        }
        assert_eq!(Bands::share(1, full(1, 3)).prompt, 0..1);
    }

    #[test]
    fn a_window_with_nothing_in_it_asks_for_nothing() {
        let bands = Bands::share(0, full(1, 3));
        for band in all(&bands) {
            assert!(band.is_empty());
        }
    }

    #[test]
    fn the_bands_give_up_in_the_order_the_module_says() {
        // One row at a time from the smallest window up, each band appearing
        // and never disappearing again. What this catches is a change to the
        // arithmetic that keeps the sum right and reorders the surrender.
        let seen = |rows| {
            let bands = Bands::share(rows, full(1, 1));
            (
                !bands.prompt.is_empty(),
                !bands.foot.is_empty(),
                !bands.turn.is_empty(),
                !bands.transcript.is_empty(),
            )
        };
        assert_eq!(seen(1), (true, false, false, false));
        assert_eq!(seen(2), (true, true, false, false));
        assert_eq!(seen(3), (true, true, true, false));
        assert_eq!(seen(4), (true, true, true, true));
    }

    #[test]
    fn a_prompt_taller_than_its_share_is_held_to_it() {
        let bands = Bands::share(24, full(0, 40));
        assert_eq!(bands.prompt.len(), 24 / SHARE);
        assert!(
            !bands.transcript.is_empty(),
            "the prompt took the whole window"
        );
    }

    #[test]
    fn a_prompt_shorter_than_its_share_takes_only_what_it_has() {
        let bands = Bands::share(24, full(0, 3));
        assert_eq!(bands.prompt.len(), 3);
        assert_eq!(bands.transcript.len(), 24 - 3 - 1);
    }

    #[test]
    fn a_band_nobody_put_anything_in_takes_no_rows() {
        // Including the prompt. A window is not owed a box before anything has
        // drawn one, and a row reserved for nothing is a row of transcript the
        // reader was not shown.
        let bands = Bands::share(24, Wants::default());
        assert_eq!(bands.transcript, 0..24);
        for band in all(&bands) {
            assert!(band.is_empty() || *band == bands.transcript);
        }
    }
}
