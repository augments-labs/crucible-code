//! How a burst is timed, shared by the two probes that time one.
//!
//! Both ask the same question in different words — one counts deltas folded into
//! the record, the other redraws of the band over the box — and both are held to
//! the same pair of budgets: a floor in frames per second, and a closing rate
//! that has not fallen far behind the opening one. The floor catches a renderer that is slow. The
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
//!
//! A rate is still a wall clock reading, though, and the two phases are
//! deliberately far apart in that clock — being late is what makes the second one
//! mean anything. So a machine that is not the same speed at both ends of a run
//! moves all nine windows of one phase together and none of the other, and a
//! median over windows that all moved cannot see it. A shared runner that boosts
//! at the start of a run, or loses a core to a neighbour at the end of one, is
//! read as a renderer that grew.
//!
//! So each window times one yardstick beside its frames — fixed work that cannot
//! grow with what came before it — and reports what a frame cost in *those* as
//! well as in seconds. The ratio is taken from the yardstick reading, where a
//! machine that halved has halved both halves and says nothing, and a frame that
//! got dearer has moved one of them alone. The floor stays a rate, because thirty
//! a second is a claim about somebody watching and only the clock can answer it.

use std::hint::black_box;
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

/// Steps in one yardstick.
///
/// Enough that measuring one takes a few hundred microseconds: far above the
/// resolution of the clock timing it, and a small fraction of the window it
/// stands beside, so the eighteen of them a burst runs are lost in what the
/// burst costs anyway.
const STEPS: usize = 200_000;

/// How far the sustained pace may fall behind the opening pace.
///
/// A renderer whose cost is bounded holds roughly level, so anything is slack.
/// A renderer whose cost grows with the transcript has already lost an order of
/// magnitude by the end of a burst this size, and far more by the end of a real
/// session — which is the failure this number exists to catch while the margin
/// is still recoverable.
pub(crate) const SUSTAINED_FRACTION: f64 = 0.5;

/// What one phase came to, read two ways.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Reading {
    /// Frames a second — what the machine managed, and what a floor is about.
    pub(crate) rate: f64,

    /// Frames per yardstick — the same phase with the machine divided out, and
    /// the only one of the two a ratio may be taken from.
    pub(crate) pace: f64,
}

/// What one burst measured.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Burst {
    /// The opening phase.
    pub(crate) opening: Reading,

    /// The closing phase.
    pub(crate) sustained: Reading,
}

impl Burst {
    /// How much of the opening pace is left by the end.
    pub(crate) fn ratio(self) -> f64 {
        self.sustained.pace / self.opening.pace
    }
}

/// One yardstick: fixed work, timed beside the frames.
///
/// A dependent chain of integer mixing, so what it costs is the machine's clock
/// and nothing else — no allocation, no memory it did not already hold, nothing
/// that could differ between the first of them and the last. [`black_box`] is
/// what keeps it that: without one the whole loop folds to a constant at the
/// optimisation level a probe is built at, and the yardstick measures nothing.
///
/// What it follows is what actually moves under a probe on a shared machine: the
/// clock frequency, and the thread being taken away. What it does not follow is
/// memory bandwidth, which the render path spends and this does not — so a
/// neighbour saturating the bus and nothing else would still read as a renderer
/// that grew. Written down rather than claimed away, because it is the one thing
/// the reading below does not divide out.
fn yardstick() -> u64 {
    mixing(STEPS)
}

/// `steps` of that chain.
///
/// Parted from the yardstick so the tests can ask for a different amount of the
/// same work, which is how they stand up a frame that costs what they say it
/// costs: work is what a frame spends, and it is the one thing on this path that
/// costs the same on a machine whose clock is dear to read.
fn mixing(steps: usize) -> u64 {
    let mut held = 0x243f_6a88_85a3_08d3_u64;

    for _ in 0..steps {
        held = held
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        held ^= held >> 33;
        held = black_box(held);
    }

    held
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
pub(crate) fn measure<E>(frame: impl FnMut(usize) -> Result<(), E>) -> Result<Burst, E> {
    measure_with(frame, || {
        black_box(yardstick());
    })
}

/// [`measure`], told what to measure the frames against.
///
/// Split out for the tests, which have to be able to slow the yardstick down
/// alongside the frames: that is a machine losing speed, and it is the one thing
/// the reading is built to say nothing about.
fn measure_with<E>(
    mut frame: impl FnMut(usize) -> Result<(), E>,
    mut yard: impl FnMut(),
) -> Result<Burst, E> {
    let mut index = 0;

    for _ in 0..WARMUP {
        frame(index)?;
        index += 1;
    }

    let opening = phase(&mut frame, &mut yard, &mut index)?;

    for _ in 0..BETWEEN {
        frame(index)?;
        index += 1;
    }

    let sustained = phase(&mut frame, &mut yard, &mut index)?;

    Ok(Burst { opening, sustained })
}

/// Times [`WINDOWS`] windows of [`WINDOW`] frames and a yardstick apiece, and
/// returns the middle of each.
///
/// The two medians are taken over the same nine windows and independently of one
/// another, which is what makes a stalled window cost the phase nothing twice
/// over rather than once.
fn phase<E>(
    frame: &mut impl FnMut(usize) -> Result<(), E>,
    yard: &mut impl FnMut(),
    index: &mut usize,
) -> Result<Reading, E> {
    // Arrays rather than vectors, so the medians below are elements that exist
    // by construction rather than ones a caller has to be trusted to have
    // pushed.
    let mut rates = [0.0_f64; WINDOWS];
    let mut paces = [0.0_f64; WINDOWS];

    for (rate, pace) in rates.iter_mut().zip(paces.iter_mut()) {
        let drawing = Instant::now();
        for _ in 0..WINDOW {
            frame(*index)?;
            *index += 1;
        }
        let drawn = drawing.elapsed().as_secs_f64();

        // Immediately after the frames it stands for, because adjacent in the
        // wall clock is the whole of what makes it a yardstick: a machine that
        // changes speed between the two has to change it in the middle of a
        // window before either reading is worth less than the other.
        let measuring = Instant::now();
        yard();
        let measured = measuring.elapsed().as_secs_f64();

        *rate = per_second(drawn);

        // Frames a second times seconds a yardstick, which is frames a
        // yardstick — the clock cancels, and with it whatever the machine was
        // doing at the time.
        *pace = *rate * measured;
    }

    Ok(Reading {
        rate: median(rates),
        pace: median(paces),
    })
}

/// The middle of what the windows came to.
///
/// The whole reason a phase is more than one window. A mean would carry a
/// stalled window into the answer in proportion to how bad it was; the median
/// carries it not at all until most of the windows are stalled, at which point
/// the machine really was that slow and the budget is entitled to say so.
fn median(mut readings: [f64; WINDOWS]) -> f64 {
    // `total_cmp` rather than `partial_cmp`: a window drew frames and ran a
    // yardstick, so neither of the two things measured over it took no time at
    // all, and a sort that cannot be handed a NaN needs no branch for one.
    readings.sort_by(f64::total_cmp);

    // Indexed rather than fetched: the array's length is [`WINDOWS`], so the
    // middle of it is an element the type system already knows is there.
    readings[WINDOWS / 2]
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
    use std::cell::Cell;

    use super::*;

    /// The frame the closing phase begins at.
    const LATE: usize = WARMUP + WINDOWS * WINDOW;

    /// Nine windows at `fast`, with `stalled` of them dropped to a tenth of it.
    fn windows(stalled: usize) -> [f64; WINDOWS] {
        let mut readings = [1_000.0_f64; WINDOWS];
        for reading in readings.iter_mut().take(stalled) {
            *reading = 100.0;
        }
        readings
    }

    #[test]
    fn a_few_stalled_windows_do_not_decide_the_phase() {
        // The reason a phase is more than one window, and the failure that
        // change existed for: under a single-window phase one stall of a couple
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

        assert!(burst.opening.rate.is_finite(), "{burst:?}");
        assert!(burst.sustained.rate.is_finite(), "{burst:?}");
        assert_eq!(drawn, WARMUP + WINDOWS * WINDOW * 2 + BETWEEN);
    }

    #[test]
    fn a_yardstick_is_measured_in_every_window_and_nowhere_else() {
        // Not in the warmup and not in what lies between the phases: those are
        // frames nobody is timing, so a yardstick beside them would be work the
        // burst pays for and no reading is made of.
        let mut measured = 0;
        let _ = measure_with::<()>(|_| Ok(()), || measured += 1).expect("a burst");

        assert_eq!(measured, WINDOWS * 2);
    }

    #[test]
    fn a_yardstick_is_the_same_work_every_time_it_is_measured() {
        // What lets it stand for the machine rather than for the burst. Work
        // that differed between the first of them and the last would be a
        // second thing changing across a run, and the reading below divides by
        // it.
        assert_eq!(yardstick(), yardstick());
    }

    /// A burst on a machine where a frame costs `frame` steps of mixing and a
    /// yardstick costs `yard`, both told how far into the burst they are being
    /// run.
    ///
    /// Steps rather than a duration, because a duration here has to be spent by
    /// watching the clock and the clock is not free to read. A machine where one
    /// reading costs a microsecond rounds a frame asked for one up to two and a
    /// frame asked for three up to four, so a test that meant *three times as
    /// dear* stands up a machine a third slower — and the reading it then makes
    /// is true of a machine nobody described. Work is what a frame actually
    /// spends, and how much of it a step is does not change between the two ends
    /// of a burst.
    fn machine(frame: impl Fn(usize) -> usize, yard: impl Fn(usize) -> usize) -> Burst {
        // The yardstick is not handed the index, because on a real machine it
        // is not told one either. It reads how far the frames have got, which
        // is the same thing the machine it stands for would know.
        let reached = Cell::new(0_usize);

        measure_with::<()>(
            |index| {
                reached.set(index);
                black_box(mixing(frame(index)));
                Ok(())
            },
            || {
                black_box(mixing(yard(reached.get())));
            },
        )
        .expect("a burst")
    }

    /// What a frame costs in the bursts below, in steps of mixing.
    ///
    /// Enough that a window of them is milliseconds — the same thing [`WINDOW`]
    /// is sized for, so a scheduler taking the thread away changes a window
    /// rather than deciding it — and few enough that a whole burst is a fraction
    /// of a second.
    const FRAME: usize = 1_000;

    /// What a yardstick costs in the bursts below.
    ///
    /// The real one, so a window's two readings stand in the same proportion
    /// here as they do under a probe.
    const YARD: usize = STEPS;

    #[test]
    fn the_opening_phase_times_the_early_frames_and_the_closing_one_the_late() {
        // What makes the ratio mean anything. If both phases timed the same
        // stretch of the burst it would be comparing a renderer with itself,
        // and the budget would hold however much the renderer had grown. So
        // make the early frames the expensive ones and check that the opening
        // rate is the one that shows it.
        let burst = machine(|index| usize::from(index < LATE) * FRAME, |_| 0);

        assert!(
            burst.opening.rate < burst.sustained.rate,
            "the two phases did not cover different frames: {burst:?}"
        );
    }

    #[test]
    fn a_machine_that_slowed_between_the_phases_is_not_a_renderer_that_grew() {
        // The failure the yardstick exists for, and the one that had this probe
        // failing on a shared runner. Everything after the opening phase costs
        // three times as much — the frames and the yardstick alike, which is
        // what a machine losing two thirds of its speed does and what a renderer
        // that grew does not.
        let slower =
            |quick: usize| move |index: usize| if index < LATE { quick } else { quick * 3 };

        let burst = machine(slower(FRAME), slower(YARD));

        assert!(
            burst.sustained.rate / burst.opening.rate < SUSTAINED_FRACTION,
            "the machine did not slow down, so this proves nothing: {burst:?}"
        );
        assert!(
            burst.ratio() > 0.8,
            "a machine that slowed was read as a renderer that grew: {burst:?}"
        );
    }

    #[test]
    fn a_frame_that_got_dearer_late_in_the_burst_still_is_one() {
        // The other half, and what keeps the yardstick from being a way to pass.
        // Here the machine held its speed and only the frames got dearer, which
        // is exactly what the ratio is for.
        let burst = machine(
            |index| if index < LATE { FRAME } else { FRAME * 3 },
            |_| YARD,
        );

        assert!(
            burst.ratio() < SUSTAINED_FRACTION,
            "a renderer that grew went unnoticed: {burst:?}"
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
    fn a_ratio_is_what_is_left_of_the_opening_pace() {
        let burst = Burst {
            // The rates say the machine ended four times as fast as it began.
            // The yardstick says a frame ended costing four times as much. Only
            // the second of those is a renderer, and only the second is what a
            // ratio may be taken from.
            opening: Reading {
                rate: 50.0,
                pace: 200.0,
            },
            sustained: Reading {
                rate: 200.0,
                pace: 50.0,
            },
        };

        assert!((burst.ratio() - 0.25).abs() < f64::EPSILON);
    }
}
