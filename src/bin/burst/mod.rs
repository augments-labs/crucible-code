//! How a burst is timed, shared by the two probes that time one.
//!
//! Both ask the same question in different words — one counts deltas appended to
//! a tail, the other whole-region redraws — and both are held to the same pair of
//! budgets: a floor in frames per second, and a closing rate that has not fallen
//! far behind the opening one. The floor catches a renderer that is slow. The
//! ratio catches the failure that matters more here and that a floor cannot see:
//! a redraw whose cost grows with what came before it, which is fast in the first
//! second and hopeless in the hundredth.
//!
//! What that ratio needs, and what it did not have, is for each of its two halves
//! to be a rate rather than a stopwatch reading. A phase timed once is one wall
//! clock measurement of a few milliseconds, and a machine that takes the thread
//! away for two of them has halved it — so a probe on a shared machine reported a
//! renderer that grows when nothing had grown. Each phase is therefore several
//! windows, and the phase's rate is their median: a window the machine
//! interrupted has to be most of them before it is the answer.

use std::time::Instant;

/// Frames in one timed window.
///
/// Long enough that the scheduler taking the thread away for a millisecond
/// changes a window rather than deciding it, short enough that nine of them per
/// phase still cost a fraction of a second.
pub(crate) const WINDOW: usize = 2_000;

/// Timed windows in each phase.
///
/// Odd, so the median is a window that was measured rather than the average of
/// two that were. Nine of them means five have to be disturbed together before
/// the phase reads as anything but what it was.
pub(crate) const WINDOWS: usize = 9;

/// Frames drawn between the two phases and timed by neither.
///
/// This is what makes the second phase *late*: whatever grows with what came
/// before it has these to grow by, and a probe that measured two adjacent phases
/// would be comparing a renderer with itself.
pub(crate) const BETWEEN: usize = 18_000;

/// Frames run and thrown away, so the measurement is not paying for the first
/// allocation of every reused buffer.
pub(crate) const WARMUP: usize = 2_000;

/// How far the sustained rate may fall behind the opening rate.
///
/// A renderer whose cost is bounded holds roughly level, so anything is slack.
/// A renderer whose cost grows with the transcript has already lost an order of
/// magnitude by the end of a burst this size, and far more by the end of a real
/// session — which is the failure this number exists to catch while the margin
/// is still recoverable.
pub(crate) const SUSTAINED_FRACTION: f64 = 0.5;

/// What one burst measured.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Burst {
    /// Frames per second over the opening phase.
    pub(crate) opening: f64,
    /// Frames per second over the closing phase.
    pub(crate) sustained: f64,
}

impl Burst {
    /// How much of the opening rate is left by the end.
    pub(crate) fn ratio(self) -> f64 {
        self.sustained / self.opening
    }
}

/// Runs a whole burst and returns what its two phases came to.
///
/// `frame` draws one frame and is handed the number of the frame it is drawing.
/// That number runs from zero and never repeats, so a probe whose picture depends
/// on it — a clock, a position in a script — moves as it would on screen.
///
/// # Errors
///
/// Whatever `frame` returns, at the first frame that fails.
pub(crate) fn measure<E>(mut frame: impl FnMut(usize) -> Result<(), E>) -> Result<Burst, E> {
    let mut index = 0;

    for _ in 0..WARMUP {
        frame(index)?;
        index += 1;
    }

    let opening = phase(&mut frame, &mut index)?;

    for _ in 0..BETWEEN {
        frame(index)?;
        index += 1;
    }

    let sustained = phase(&mut frame, &mut index)?;

    Ok(Burst { opening, sustained })
}

/// Times [`WINDOWS`] windows of [`WINDOW`] frames and returns their median rate.
fn phase<E>(frame: &mut impl FnMut(usize) -> Result<(), E>, index: &mut usize) -> Result<f64, E> {
    // An array rather than a vector, so the median below is an element that
    // exists by construction rather than one a caller has to be trusted to have
    // pushed.
    let mut rates = [0.0_f64; WINDOWS];

    for rate in &mut rates {
        let start = Instant::now();
        for _ in 0..WINDOW {
            frame(*index)?;
            *index += 1;
        }
        *rate = per_second(start.elapsed().as_secs_f64());
    }

    Ok(median(rates))
}

/// The middle of what the windows came to.
///
/// The whole reason a phase is more than one window. A mean would carry a
/// stalled window into the answer in proportion to how bad it was; the median
/// carries it not at all until most of the windows are stalled, at which point
/// the machine really was that slow and the budget is entitled to say so.
fn median(mut rates: [f64; WINDOWS]) -> f64 {
    // `total_cmp` rather than `partial_cmp`: these are rates, so none of them is
    // a NaN, and a sort that cannot be handed one needs no branch for it.
    rates.sort_by(f64::total_cmp);

    // Indexed rather than fetched: the array's length is [`WINDOWS`], so the
    // middle of it is an element the type system already knows is there.
    rates[WINDOWS / 2]
}

/// A window's frame count over the seconds it took.
fn per_second(seconds: f64) -> f64 {
    // A window this size takes milliseconds, so the precision lost converting the
    // count is far below the noise in the measurement.
    #[allow(clippy::cast_precision_loss)]
    let frames = WINDOW as f64;

    frames / seconds
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Nine windows at `fast`, with `stalled` of them dropped to a tenth of it.
    fn windows(stalled: usize) -> [f64; WINDOWS] {
        let mut rates = [1_000.0_f64; WINDOWS];
        for rate in rates.iter_mut().take(stalled) {
            *rate = 100.0;
        }
        rates
    }

    #[test]
    fn a_few_stalled_windows_do_not_decide_the_phase() {
        // The reason a phase is more than one window, and the failure this
        // change exists for: under a single-window phase one stall of a couple
        // of milliseconds was the entire reading, and reported a renderer that
        // had grown when nothing had.
        assert!((median(windows(4)) - 1_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn enough_of_them_still_do() {
        // The other half. This is a median, not a best-of: a machine slow for
        // most of a phase is one the budget is entitled to notice.
        assert!((median(windows(5)) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn every_frame_of_both_phases_and_what_lies_between_is_drawn() {
        let mut drawn = 0;
        let burst = measure::<()>(|_| {
            drawn += 1;
            Ok(())
        })
        .expect("a burst");

        assert!(burst.opening.is_finite(), "{burst:?}");
        assert!(burst.sustained.is_finite(), "{burst:?}");
        assert_eq!(drawn, WARMUP + WINDOWS * WINDOW * 2 + BETWEEN);
    }

    /// Busies the thread for `taken`. A sleep is far too coarse for this.
    fn spin(taken: Duration) {
        let until = Instant::now() + taken;
        while Instant::now() < until {
            std::hint::spin_loop();
        }
    }

    #[test]
    fn the_opening_phase_times_the_early_frames_and_the_closing_one_the_late() {
        // What makes the ratio mean anything. If both phases timed the same
        // stretch of the burst it would be comparing a renderer with itself,
        // and the budget would hold however much the renderer had grown. So
        // make the early frames the expensive ones and check that the opening
        // rate is the one that shows it.
        let early = WARMUP + WINDOWS * WINDOW;
        let burst = measure::<()>(|index| {
            if index < early {
                spin(Duration::from_micros(2));
            }
            Ok(())
        })
        .expect("a burst");

        assert!(
            burst.opening < burst.sustained,
            "the two phases did not cover different frames: {burst:?}"
        );
    }

    #[test]
    fn a_frame_that_fails_stops_the_burst_rather_than_being_averaged_in() {
        let mut drawn = 0;
        let stopped = measure(|_| {
            drawn += 1;
            if drawn == WARMUP + 1 {
                return Err("the renderer gave up");
            }
            Ok(())
        });

        assert_eq!(stopped.err(), Some("the renderer gave up"));
        assert_eq!(drawn, WARMUP + 1);
    }

    #[test]
    fn a_ratio_is_what_is_left_of_the_opening_rate() {
        let burst = Burst {
            opening: 200.0,
            sustained: 50.0,
        };

        assert!((burst.ratio() - 0.25).abs() < f64::EPSILON);
    }
}
