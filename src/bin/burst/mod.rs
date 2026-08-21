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
//! renderer that grows when nothing had grown. Each phase is therefore many
//! windows.
//!
//! A machine interferes with a burst two ways, and one window cannot be read
//! against both.
//!
//! It can be slower for a while — a core clocked down, a sibling thread taking
//! half the pipeline — which every microsecond of a window feels alike. Against
//! that, each window times a yardstick beside its frames: fixed work that cannot
//! grow with what came before it, and that a slower machine slows by the same
//! factor as the frames. The reading the ratio is taken from is what a frame cost
//! in *yardsticks*, where a machine that halved has halved both halves and says
//! nothing, and a frame that got dearer has moved one of them alone.
//!
//! Or it can take the thread away entirely, which is not a slower machine at all:
//! it is the same machine, absent. That lands in whatever was running at the
//! moment it happened, and a yardstick is a rounding error of a window — so it
//! lands in the frames, essentially always, and the yardstick reads a machine
//! that never faltered. No amount of interleaving fixes that, because charging
//! the gap in proportion to how much of the window each reading holds is exactly
//! what happens already, and the reading holding four per cent of the window
//! catches four per cent of the gaps.
//!
//! What answers that one is the *best* window rather than the middle one. Being
//! taken away can only make a window worse, never better, so the quickest of many
//! is the machine at liberty — and a window short enough to fall between two
//! gaps means there are such windows to find. That is why a window here is a
//! duration and becomes a count only once a frame has been timed: the two probes
//! sharing this file differ by an order of magnitude in what a frame costs, and a
//! frame count that is brief for one is longer than any gap for the other.
//!
//! How many windows there are to look through is the other half of that, and it
//! belongs to the phase rather than the window. A phase of a few dozen is shorter
//! than one stretch of a machine being taken away, so every window in it can be
//! one the machine was absent for, and the best of them is the best of a bad lot.
//! A phase here is hundreds of windows for that reason — long enough to outlast
//! such a stretch, so that even the phase with fewest untouched windows to offer
//! still has one.
//!
//! The floor stays the middle window and stays a rate, because thirty a second is
//! a claim about somebody watching, and somebody watching sees the machine they
//! have rather than the machine at liberty.

use std::hint::black_box;
use std::time::{Duration, Instant};

/// How long one timed window should last.
///
/// Short enough that a window can fall entirely between two of the moments a
/// scheduler takes the thread away — which is the whole of what makes the best
/// of many windows a reading of the machine at liberty. Long enough that the
/// pair of clock readings around it is a rounding error on a machine where
/// reading the clock is dear.
const WINDOW: Duration = Duration::from_micros(400);

/// Frames timed, once, to find out how many of them fill a [`WINDOW`].
///
/// Drawn after the warmup, so what they cost is what a frame costs rather than
/// what the first of every reused buffer costs.
const CALIBRATE: usize = 256;

/// The fewest and the most frames a window may hold, whatever that timing said.
///
/// Neither bound is expected to bind: they are here so that a probe whose frames
/// are far cheaper or far dearer than either of the two this file serves gets a
/// window that is merely the wrong length, rather than one of no frames at all or
/// one that is the whole burst.
const FEWEST: usize = 8;
const MOST: usize = 512;

/// Timed windows in each phase.
///
/// Odd, so the median is a window that was measured rather than the average of
/// two that were.
///
/// This many, because a phase is only worth reading if it holds a window the
/// machine left alone, and a shared runner is taken away in stretches long
/// enough to cover a phase of a few dozen whole. At this count a phase spans
/// hundreds of milliseconds and outlasts one — in the phase the thread is taken
/// away from throughout, which has fewest untouched windows to offer, as much as
/// in the phase it is not.
pub(crate) const WINDOWS: usize = 513;

/// Frames drawn between the two phases and timed by neither.
///
/// This is what makes the second phase *late*: whatever grows with what came
/// before it has these to grow by, and a probe that measured two adjacent phases
/// would be comparing a renderer with itself.
pub(crate) const BETWEEN: usize = 34_000;

/// Frames run and thrown away, so the measurement is not paying for the first
/// allocation of every reused buffer.
pub(crate) const WARMUP: usize = 2_000;

/// Steps in one window's yardstick.
///
/// Enough that measuring one takes tens of microseconds: far above the
/// resolution of the clock timing it, and a small fraction of the window it
/// stands beside, so the hundred and thirty of them a burst runs are lost in what
/// the burst costs anyway.
const STEPS: usize = 20_000;

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
    /// Frames a second in the middle window — what the machine managed, and what
    /// a floor is about.
    pub(crate) rate: f64,

    /// Frames per yardstick in the best window — the same phase with the machine
    /// divided out both ways it interferes, and the only one of the two a ratio
    /// may be taken from.
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
/// What it follows is the one thing about a shared machine that a stretch of time
/// this short can follow: the clock frequency. The thread being taken away is not
/// its job and cannot be — a yardstick is too small a part of a window to be
/// running when it happens — and is answered by taking the best of many windows
/// instead. What neither of them follows is memory bandwidth, which the render
/// path spends and this does not, so a neighbour saturating the bus and nothing
/// else would still read as a renderer that grew. Written down rather than
/// claimed away, because it is the one thing left that the reading below does not
/// divide out.
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

    let window = sized(&mut frame, &mut index)?;
    let opening = phase(&mut frame, &mut yard, &mut index, window)?;

    for _ in 0..BETWEEN {
        frame(index)?;
        index += 1;
    }

    let sustained = phase(&mut frame, &mut yard, &mut index, window)?;

    Ok(Burst { opening, sustained })
}

/// How many frames fill a [`WINDOW`] on the machine this burst is running on.
///
/// Both phases are handed the same answer, timed once and before either of them.
/// A window re-sized between the phases would be two different measurements
/// wearing one name, and the ratio takes one from the other.
fn sized<E>(frame: &mut impl FnMut(usize) -> Result<(), E>, index: &mut usize) -> Result<usize, E> {
    let timing = Instant::now();
    for _ in 0..CALIBRATE {
        frame(*index)?;
        *index += 1;
    }
    let took = timing.elapsed().as_secs_f64();

    // A count and a duration, both far above what a clock can resolve, so the
    // precision lost converting either is far below the noise in the timing.
    #[allow(clippy::cast_precision_loss)]
    let drawn = CALIBRATE as f64;
    let fills = drawn * WINDOW.as_secs_f64() / took;

    // Truncating, and a machine so quick that `took` rounded to nothing lands on
    // the ceiling rather than on a window of every frame the burst has.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frames = if fills.is_finite() {
        fills as usize
    } else {
        MOST
    };

    Ok(frames.clamp(FEWEST, MOST))
}

/// Times [`WINDOWS`] windows of `window` frames and a yardstick apiece, and reads
/// them twice: the middle window for the rate, the best for the pace.
///
/// Twice, because the two readings are answering different people. A floor is
/// about somebody watching a screen, and what they get is the machine they have
/// — so the rate is the middle window, and a machine slow for most of a phase is
/// one the floor is entitled to notice. A ratio is about whether a frame costs
/// more than it did, which is a question about the renderer alone — so the pace
/// is the quickest window over the quickest yardstick, each being the reading
/// that stretch of the burst made when nothing was interrupting it.
fn phase<E>(
    frame: &mut impl FnMut(usize) -> Result<(), E>,
    yard: &mut impl FnMut(),
    index: &mut usize,
    window: usize,
) -> Result<Reading, E> {
    // Arrays rather than vectors, so the medians below are elements that exist
    // by construction rather than ones a caller has to be trusted to have
    // pushed.
    let mut rates = [0.0_f64; WINDOWS];
    let mut yards = [0.0_f64; WINDOWS];

    for (rate, measured) in rates.iter_mut().zip(yards.iter_mut()) {
        let drawing = Instant::now();
        for _ in 0..window {
            frame(*index)?;
            *index += 1;
        }
        let drawn = drawing.elapsed().as_secs_f64();

        // Immediately after the frames it stands for, because adjacent in the
        // wall clock is what makes it a yardstick for *this* window: a machine
        // that changes speed between the two has to change it in the middle of a
        // window before either reading is worth less than the other.
        let measuring = Instant::now();
        yard();
        *measured = measuring.elapsed().as_secs_f64();

        *rate = per_second(window, drawn);
    }

    Ok(Reading {
        rate: median(rates),

        // Frames a second times seconds a yardstick, which is frames a
        // yardstick — the clock cancels, and with it whatever the machine was
        // doing at the time. Both halves are the best window rather than the
        // same window, because the two are answering the same question about the
        // machine at liberty and neither is helped by being paired with a window
        // the machine was not.
        pace: quickest(&rates, f64::gt) * quickest(&yards, f64::lt),
    })
}

/// The best of what the windows came to, `better` saying which way that is.
///
/// A rate is better when it is higher and a yardstick when it is lower, and both
/// mean the same thing: the window where the machine was least in the way. This
/// is what makes the pace a reading of the renderer — being interrupted can only
/// make a window worse, so the best window of many is the one nothing happened
/// to, and a phase where *every* window is worse than the phase before it is a
/// phase where the frames themselves got dearer.
fn quickest(readings: &[f64; WINDOWS], better: fn(&f64, &f64) -> bool) -> f64 {
    // Folded rather than sorted: the answer is one element and the order of the
    // rest is nobody's business. The first element seeds it, so the answer is a
    // window that was measured even where every comparison says no.
    let mut best = readings[0];
    for reading in readings {
        if better(reading, &best) {
            best = *reading;
        }
    }
    best
}

/// The middle of what the windows came to.
///
/// The whole reason a phase is more than one window. A mean would carry a
/// stalled window into the answer in proportion to how bad it was; the median
/// carries it not at all until most of the windows are stalled, at which point
/// the machine really was that slow and the floor is entitled to say so.
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
fn per_second(window: usize, seconds: f64) -> f64 {
    // A window holds hundreds of frames at most, so the precision lost converting
    // the count is none at all.
    #[allow(clippy::cast_precision_loss)]
    let frames = window as f64;

    frames / seconds
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// [`WINDOWS`] windows at `fast`, with `stalled` of them dropped to a tenth
    /// of it.
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
        assert!((median(windows(WINDOWS / 2)) - 1_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn enough_of_them_still_do() {
        // The other half. The rate is a median, not a best-of: a machine slow
        // for most of a phase is one the floor is entitled to notice, because a
        // floor is about what somebody watching gets.
        assert!((median(windows(WINDOWS / 2 + 1)) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn one_window_the_machine_left_alone_is_the_best_of_them_however_few() {
        // And the other reading is a best-of for the opposite reason: a pace is
        // about whether a frame costs more than it did, which is a question
        // about the renderer, and every window but one being stalled says
        // nothing about the renderer at all.
        let all_but_one = windows(WINDOWS - 1);

        assert!((quickest(&all_but_one, f64::gt) - 1_000.0).abs() < f64::EPSILON);
        assert!((quickest(&all_but_one, f64::lt) - 100.0).abs() < f64::EPSILON);
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

        // How many frames a window holds is the machine's answer rather than
        // this file's, so what is checked is the shape: the frames nobody times
        // are all there, and what is left over is two phases of equal windows
        // within the bounds a window is held to.
        let timed = drawn - (WARMUP + CALIBRATE + BETWEEN);
        assert_eq!(timed % (WINDOWS * 2), 0, "{drawn} frames");
        assert!(
            (FEWEST..=MOST).contains(&(timed / (WINDOWS * 2))),
            "{drawn} frames"
        );
    }

    #[test]
    fn a_yardstick_is_measured_in_every_window_and_nowhere_else() {
        // Not in the warmup, not in the frames a window is sized by, and not in
        // what lies between the phases: those are frames nobody is timing, so a
        // yardstick beside them would be work the burst pays for and no reading
        // is made of.
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

    /// How far into a burst a frame or a yardstick is being run, counted in the
    /// windows already timed behind it.
    #[derive(Debug, Clone, Copy)]
    struct Where(usize);

    impl Where {
        /// Whether the opening phase is over.
        fn late(self) -> bool {
            self.0 >= WINDOWS
        }

        /// Which window of the closing phase, counted from zero.
        fn closing(self) -> usize {
            self.0.saturating_sub(WINDOWS)
        }
    }

    /// A burst on a machine where a frame costs `frame` steps of mixing and a
    /// yardstick costs `yard`, both told where in the burst they are being run.
    ///
    /// Steps rather than a duration, because a duration here has to be spent by
    /// watching the clock and the clock is not free to read. A machine where one
    /// reading costs a microsecond rounds a frame asked for one up to two and a
    /// frame asked for three up to four, so a test that meant *three times as
    /// dear* stands up a machine a third slower — and the reading it then makes
    /// is true of a machine nobody described. Work is what a frame actually
    /// spends, and how much of it a step is does not change between the two ends
    /// of a burst.
    ///
    /// Windows rather than frames, because how many frames a window holds is
    /// decided by the machine the test is running on and no test may depend on
    /// the answer. Counting the yardsticks is how a burst says where it has got
    /// to in terms the reading itself is made in — one of them is measured per
    /// window and nowhere else, which the test above holds it to.
    fn machine(frame: impl Fn(Where) -> usize, yard: impl Fn(Where) -> usize) -> Burst {
        let timed = Cell::new(0_usize);

        measure_with::<()>(
            |_| {
                black_box(mixing(frame(Where(timed.get()))));
                Ok(())
            },
            || {
                black_box(mixing(yard(Where(timed.get()))));
                timed.set(timed.get() + 1);
            },
        )
        .expect("a burst")
    }

    /// What a frame costs in the bursts below, in steps of mixing.
    ///
    /// Cheap enough that a burst of them is a fraction of a second, and dear
    /// enough that a window holds hundreds rather than the ceiling — so the
    /// bursts below are read the way a probe's is.
    const FRAME: usize = 500;

    /// How much dearer a frame or a yardstick gets late in these bursts.
    ///
    /// Six rather than the three it was, because one of the bursts below begins
    /// by checking that the wall clock alone would have condemned the run — and
    /// the wall clock is the thing every machine here disagrees about. A runner
    /// that reaches its clock speed some way into a run is already most of the
    /// way to that check on its own, and a factor of three left it deciding
    /// whether a machine warming up counted as a machine that slowed. Six is far
    /// enough from [`SUSTAINED_FRACTION`] that only a machine changing speed
    /// threefold could reach it, and a machine doing that is not one any reading
    /// here is about.
    const DEARER: usize = 6;

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
        let burst = machine(|at| usize::from(!at.late()) * FRAME, |_| 0);

        assert!(
            burst.opening.rate < burst.sustained.rate,
            "the two phases did not cover different frames: {burst:?}"
        );
    }

    #[test]
    fn a_machine_that_slowed_between_the_phases_is_not_a_renderer_that_grew() {
        // The failure the yardstick exists for, and one of the two that had
        // this probe failing on a shared runner. Everything after the opening
        // phase costs [`DEARER`] times as much — the frames and the yardstick
        // alike, which is what a machine losing most of its speed does and what
        // a renderer that grew does not.
        let slower = |quick: usize| {
            move |at: Where| {
                if at.late() { quick * DEARER } else { quick }
            }
        };

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

    /// One window of the closing phase in [`TAKEN`] is left alone.
    ///
    /// A thread being taken away is not a slower machine: it lands on some
    /// windows of a phase and not on others, and the yardstick beside a window
    /// it did not land on reads a machine that never faltered. Most of them,
    /// so the middle window is one it happened to and the rate says the phase
    /// was slow — which is true, and is what a floor is for. Not all of them, so
    /// there is still a window the machine left alone for the pace to be read
    /// from.
    const TAKEN: usize = 3;

    #[test]
    fn a_thread_taken_away_during_the_closing_phase_is_not_a_renderer_that_grew() {
        // The other failure that had this probe failing on a shared runner, and
        // the one no yardstick can answer: a neighbour arriving partway through
        // a run takes the thread away in gaps, and a yardstick is too small a
        // part of a window to be running when one happens. What tells this apart
        // from the burst below is not how dear the frames got but how many
        // windows it reached.
        let burst = machine(
            |at| {
                if at.late() && at.closing() % TAKEN != 0 {
                    FRAME * DEARER
                } else {
                    FRAME
                }
            },
            |_| YARD,
        );

        assert!(
            burst.sustained.rate / burst.opening.rate < SUSTAINED_FRACTION,
            "the thread was never taken away, so this proves nothing: {burst:?}"
        );
        assert!(
            burst.ratio() > 0.8,
            "a thread taken away was read as a renderer that grew: {burst:?}"
        );
    }

    #[test]
    fn a_frame_that_got_dearer_late_in_the_burst_still_is_one() {
        // What keeps both of those from being a way to pass. Here the machine
        // held its speed and never took the thread away, and only the frames got
        // dearer — every one of them, in every window there is, which is exactly
        // what the ratio is for.
        let burst = machine(
            |at| if at.late() { FRAME * DEARER } else { FRAME },
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
