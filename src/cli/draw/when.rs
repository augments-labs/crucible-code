//! How long ago something was, in as few characters as say it.
//!
//! The words are here rather than in the component that draws them, because
//! `crucible-tui` is handed rows and never asks a clock. They are here rather
//! than in the runner for the opposite reason: a session log knows when a
//! session started, and nothing below the wiring decides what that is called.
//!
//! Every rung is approximate above the day, and deliberately so. What somebody
//! reading a list of four sessions needs is which one they were in this morning
//! and which one was last month; a date to the minute is in the log.

use std::time::SystemTime;

/// The rungs, in seconds.
const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

/// Anything under two days that is not today is yesterday.
const YESTERDAY: u64 = 2 * DAY;

const WEEK: u64 = 7 * DAY;

/// A month as a round number of days, and a year as a round number of those.
/// Nothing here is a calendar, and a session two hundred days old reads the
/// same either way.
const MONTH: u64 = 30 * DAY;
const YEAR: u64 = 12 * MONTH;

/// How long before `now` something at `started` was.
///
/// A clock that went backwards — a machine that resynchronised, a log copied
/// from somewhere else — reads as just now rather than as a negative age or a
/// panic. It is the one answer that is never misleading about a session
/// somebody can see the file of.
pub(crate) fn ago(started: SystemTime, now: SystemTime) -> String {
    let seconds = now.duration_since(started).unwrap_or_default().as_secs();

    if seconds < MINUTE {
        return "just now".to_owned();
    }
    if seconds < HOUR {
        return format!("{}m ago", seconds / MINUTE);
    }
    if seconds < DAY {
        return format!("{}h ago", seconds / HOUR);
    }
    if seconds < YESTERDAY {
        return "yesterday".to_owned();
    }
    if seconds < WEEK {
        return format!("{}d ago", seconds / DAY);
    }
    if seconds < MONTH {
        return format!("{}w ago", seconds / WEEK);
    }
    if seconds < YEAR {
        return format!("{}mo ago", seconds / MONTH);
    }

    format!("{}y ago", seconds / YEAR)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// What something this many seconds old is called.
    ///
    /// Far enough after the epoch that a century of age is still an instant a
    /// `SystemTime` can hold, since taking one away from it is subtraction.
    fn old(seconds: u64) -> String {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(200 * YEAR);
        ago(now - Duration::from_secs(seconds), now)
    }

    #[test]
    fn every_rung_says_what_it_is_and_none_of_them_says_it_twice() {
        // The whole ladder in one place, because what matters about it is that
        // reading two rows tells them apart — not what any one of them says.
        assert_eq!(old(0), "just now");
        assert_eq!(old(59), "just now");
        assert_eq!(old(MINUTE), "1m ago");
        assert_eq!(old(59 * MINUTE), "59m ago");
        assert_eq!(old(HOUR), "1h ago");
        assert_eq!(old(23 * HOUR), "23h ago");
        assert_eq!(old(DAY), "yesterday");
        assert_eq!(old(2 * DAY), "2d ago");
        assert_eq!(old(6 * DAY), "6d ago");
        assert_eq!(old(WEEK), "1w ago");
        assert_eq!(old(4 * WEEK), "4w ago");
        assert_eq!(old(MONTH), "1mo ago");
        assert_eq!(old(11 * MONTH), "11mo ago");
        assert_eq!(old(YEAR), "1y ago");
        assert_eq!(old(30 * YEAR), "30y ago");
    }

    #[test]
    fn nothing_it_says_is_wider_than_the_column_it_is_drawn_in() {
        // The title beside it gives up columns to make room; a time that grew
        // without a bound would take them all. Nine characters is `yesterday`,
        // which is the longest of them, and every number that reaches this side
        // of the ladder is at most two digits.
        for seconds in [0, 59, 61, 3601, DAY, 3 * DAY, WEEK, MONTH, 99 * YEAR] {
            let said = old(seconds);
            assert!(said.len() <= 9, "{seconds}: {said:?}");
        }
    }

    #[test]
    fn a_clock_that_went_backwards_is_not_a_negative_age() {
        // A machine that resynchronised while crucible was closed, or a log
        // copied from another one. Subtraction alone would be an error here and
        // this is a row on a screen.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(YEAR);
        let later = now + Duration::from_secs(WEEK);

        assert_eq!(ago(later, now), "just now");
    }
}
