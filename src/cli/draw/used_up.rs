//! The notice a turn leaves when the plan behind the sign-in is used up.
//!
//! One block, standing where the turn's last row would: a head saying the
//! limit is reached, which window and when it resets, and under it what became
//! of the turn and what to do next. Every word is this program's. The reset is
//! the reader's own wall clock with its date, read the way `/usage` reads one,
//! and a reset the vendor did not report is said to be not reported rather
//! than guessed.
//!
//! It says "before sending" only where nothing was sent, and "refused the
//! request" only where the vendor said no: a reader on a flaky network is owed
//! the difference between the two.
//!
//! Where the head does not fit on one row, the window and the reset stand on
//! the row under it, or on a row each where the two do not fit on one, and the
//! prose loses its first sentence: what is left is the part a narrow reader can
//! act on.

use std::time::SystemTime;

use crucible_runner::PlanLimitStop;
use crucible_tui::{Glyphs, Row, Slot, columns, fold};
use crucible_types::Window;

use crate::cli::converse::command::Clock;

/// Where the head's mark stands, and where every row under it starts.
const HEAD: usize = 2;
const UNDER: usize = 4;

/// The rows of the notice at `width` columns, for the window that was used
/// up and when it resets, either of which the vendor may not have said.
pub(crate) fn rows(
    (window, resets_at): (Option<Window>, Option<SystemTime>),
    stopped: PlanLimitStop,
    width: usize,
    glyphs: Glyphs,
    clock: &Clock,
) -> Vec<Row> {
    let title = format!("{} Usage limit reached", glyphs.stopped());
    let reset = resets_at.and_then(|at| clock.reset_by(at));
    let parts = window
        .map(Window::named)
        .into_iter()
        .chain(Some(
            reset
                .clone()
                .unwrap_or_else(|| "resets: not reported".to_owned()),
        ))
        .collect::<Vec<_>>();
    let detail = parts.join(&format!(" {} ", glyphs.dot()));
    let next = if reset.is_some() {
        "Nothing was lost; send a prompt after the reset to continue."
    } else {
        "Nothing was lost; send a prompt later to continue."
    };

    let dot = format!(" {} ", glyphs.dot());
    if 2 * HEAD + columns(&title) + columns(&dot) + columns(&detail) <= width {
        let head = Row::new()
            .then(Slot::Plain, " ".repeat(HEAD))
            .then(Slot::Strong, title)
            .then(Slot::Quiet, format!("{dot}{detail}"));
        let what = match stopped {
            PlanLimitStop::BeforeSending => "The turn stopped before sending.",
            PlanLimitStop::Refused => "The vendor refused the request.",
        };
        let mut rows = vec![head];
        rows.extend(folded(Slot::Quiet, &format!("{what} {next}"), UNDER, width));
        return rows;
    }

    let mut rows = folded(Slot::Strong, &title, HEAD, width);
    // The window and the reset on a row each where the two do not fit on one,
    // so a fold never parts a reset's date from its time.
    if 2 * UNDER + columns(&detail) <= width {
        rows.extend(folded(Slot::Quiet, &detail, UNDER, width));
    } else {
        for part in &parts {
            rows.extend(folded(Slot::Quiet, part, UNDER, width));
        }
    }
    rows.extend(folded(Slot::Quiet, next, UNDER, width));
    rows
}

/// `text` folded to fit `width` with `indent` columns before every row and as
/// many kept clear after it, so the notice reads as a block set in from both
/// edges rather than prose run out to the window's; or with neither where the
/// window is too narrow to spare them.
fn folded(slot: Slot, text: &str, indent: usize, width: usize) -> Vec<Row> {
    let indent = if 2 * indent < width { indent } else { 0 };
    fold(text, width.saturating_sub(2 * indent).max(1))
        .into_iter()
        .map(|row| {
            Row::new()
                .then(Slot::Plain, " ".repeat(indent))
                .then(slot, row)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use crucible_tui::{Recording, Renderer};
    use jiff::tz::{Offset, TimeZone};

    use super::*;

    /// Friday 2 October 2026, 12:00 UTC.
    const NOW: i64 = 1_790_942_400;

    /// Monday 5 October 2026, 09:00 UTC.
    const MONDAY: u64 = 1_791_190_800;

    fn monday() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(MONDAY)
    }

    fn utc() -> Clock {
        Clock::at(NOW, TimeZone::UTC)
    }

    /// What a window `width` columns wide shows once the notice is drawn.
    fn shown(
        window: Option<Window>,
        resets_at: Option<SystemTime>,
        stopped: PlanLimitStop,
        width: usize,
        glyphs: Glyphs,
    ) -> Vec<String> {
        let mut renderer = Renderer::new(Recording::new(width, 24));
        let drawn = rows((window, resets_at), stopped, width, glyphs, &utc());
        renderer.present(&drawn).expect("the notice to draw");
        renderer.terminal().picture().said()
    }

    #[test]
    fn plan_limit_notice_at_80_columns_names_the_window_and_the_reset_on_one_row() {
        assert_eq!(
            shown(
                Some(Window::Weekly),
                Some(monday()),
                PlanLimitStop::BeforeSending,
                80,
                Glyphs::Unicode,
            ),
            [
                "  ■ Usage limit reached · weekly window · resets 5 Oct 09:00",
                "    The turn stopped before sending. Nothing was lost; send a prompt after",
                "    the reset to continue.",
            ]
        );
    }

    #[test]
    fn plan_limit_notice_at_80_columns_with_no_reset_says_it_was_not_reported() {
        assert_eq!(
            shown(None, None, PlanLimitStop::Refused, 80, Glyphs::Unicode),
            [
                "  ■ Usage limit reached · resets: not reported",
                "    The vendor refused the request. Nothing was lost; send a prompt later to",
                "    continue.",
            ]
        );
    }

    #[test]
    fn plan_limit_notice_at_40_columns_stands_the_reset_under_the_head() {
        assert_eq!(
            shown(
                Some(Window::Weekly),
                Some(monday()),
                PlanLimitStop::BeforeSending,
                40,
                Glyphs::Unicode,
            ),
            [
                "  ■ Usage limit reached",
                "    weekly window",
                "    resets 5 Oct 09:00",
                "    Nothing was lost; send a prompt",
                "    after the reset to continue.",
            ]
        );
    }

    #[test]
    fn plan_limit_notice_at_40_columns_with_no_reset_says_so_under_the_head() {
        assert_eq!(
            shown(None, None, PlanLimitStop::Refused, 40, Glyphs::Unicode),
            [
                "  ■ Usage limit reached",
                "    resets: not reported",
                "    Nothing was lost; send a prompt",
                "    later to continue.",
            ]
        );
    }

    #[test]
    fn plan_limit_notice_says_refused_only_for_a_refusal_with_its_reset() {
        assert_eq!(
            shown(
                Some(Window::FiveHour),
                Some(monday()),
                PlanLimitStop::Refused,
                80,
                Glyphs::Unicode,
            ),
            [
                "  ■ Usage limit reached · 5-hour window · resets 5 Oct 09:00",
                "    The vendor refused the request. Nothing was lost; send a prompt after",
                "    the reset to continue.",
            ]
        );
    }

    #[test]
    fn plan_limit_notice_falls_back_to_ascii_marks() {
        let shown = shown(
            Some(Window::Weekly),
            Some(monday()),
            PlanLimitStop::BeforeSending,
            80,
            Glyphs::Ascii,
        );

        assert_eq!(
            shown.first().map(String::as_str),
            Some("  # Usage limit reached - weekly window - resets 5 Oct 09:00")
        );
        assert!(shown.iter().all(|row| row.is_ascii()), "{shown:?}");
    }

    #[test]
    fn plan_limit_notice_reads_the_reset_on_the_local_clock_and_soon_once_reached() {
        let east = Clock::at(NOW, TimeZone::fixed(Offset::constant(2)));
        let head = |clock: &Clock, at| {
            rows(
                (Some(Window::Weekly), Some(at)),
                PlanLimitStop::BeforeSending,
                80,
                Glyphs::Unicode,
                clock,
            )
            .first()
            .map(Row::text)
        };
        let past = UNIX_EPOCH + Duration::from_secs(MONDAY - 7 * 24 * 3600);
        let now = UNIX_EPOCH + Duration::from_secs(NOW.unsigned_abs());
        let later_today = now + Duration::from_hours(3);

        assert_eq!(
            head(&east, monday()).as_deref(),
            Some("  ■ Usage limit reached · weekly window · resets 5 Oct 11:00")
        );
        // Dated even later today, as `/usage` draws it, so the two never name
        // one reset two ways.
        assert_eq!(
            head(&utc(), later_today).as_deref(),
            Some("  ■ Usage limit reached · weekly window · resets 2 Oct 15:00")
        );
        for reached in [past, now] {
            assert_eq!(
                head(&utc(), reached).as_deref(),
                Some("  ■ Usage limit reached · weekly window · resets soon")
            );
        }
    }

    #[test]
    fn plan_limit_notice_titles_strong_and_says_the_rest_quietly() {
        let drawn = rows(
            (Some(Window::Weekly), Some(monday())),
            PlanLimitStop::BeforeSending,
            80,
            Glyphs::Unicode,
            &utc(),
        );

        let (head, under) = drawn.split_first().expect("a head row");
        let strong: String = head
            .spans()
            .filter(|(slot, _)| *slot == Slot::Strong)
            .map(|(_, text)| text)
            .collect();
        let quiet: String = head
            .spans()
            .filter(|(slot, _)| *slot == Slot::Quiet)
            .map(|(_, text)| text)
            .collect();
        assert_eq!(strong, "■ Usage limit reached");
        assert_eq!(quiet, " · weekly window · resets 5 Oct 09:00");
        assert!(!under.is_empty());
        for row in under {
            assert!(
                row.spans()
                    .all(|(slot, text)| slot == Slot::Quiet || text.trim().is_empty()),
                "{:?}",
                row.text()
            );
        }
    }

    #[test]
    fn plan_limit_notice_never_runs_past_the_window() {
        for width in 8..=120 {
            for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
                for (window, resets_at) in [(Some(Window::Monthly), Some(monday())), (None, None)] {
                    for row in rows(
                        (window, resets_at),
                        PlanLimitStop::Refused,
                        width,
                        glyphs,
                        &utc(),
                    ) {
                        assert!(
                            row.columns() <= width,
                            "{width} {glyphs:?}: {:?}",
                            row.text()
                        );
                    }
                }
            }
        }
    }
}
