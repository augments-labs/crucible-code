//! What the plan backend says of a `ChatGPT` plan's usage windows.
//!
//! A plan is limited over windows of time, and the backend it is served by
//! can say, in a response's headers, how much of up to two of them has been
//! used: a *primary* and a *secondary* window, each as a percentage used, its
//! length in minutes and the second it starts again. They are read here,
//! off a response crucible already receives, wherever it carries them;
//! nothing here assumes every response does. Nothing is ever asked to learn
//! them, and an API key's route is not read at all: the published API says
//! nothing of a plan.
//!
//! Which window is which is decided by its length, never by the header that
//! carried it or by any text in it: five hours, a week, or a month of 28 to 31
//! days. A window of any other length, or one whose figures are not numbers
//! in range, is left out, and a response that leaves every window out reads
//! as one that reported none.
//!
//! The header names are those the Codex CLI reads, as its binary's strings
//! spell them; no response from the backend was captured to write this.

use std::time::{Duration, SystemTime};

use crucible_types::{PlanWindows, Window, WindowReading};

use super::Serving;
use crate::transport::Named;

/// The headers one of the two windows is reported in.
struct Reported {
    /// How much of it is used, as a percentage that may have a fraction.
    used: &'static str,
    /// How long it is, in whole minutes.
    minutes: &'static str,
    /// When it starts again, in whole seconds since the Unix epoch.
    reset: &'static str,
}

const PRIMARY: Reported = Reported {
    used: "x-codex-primary-used-percent",
    minutes: "x-codex-primary-window-minutes",
    reset: "x-codex-primary-reset-at",
};

const SECONDARY: Reported = Reported {
    used: "x-codex-secondary-used-percent",
    minutes: "x-codex-secondary-window-minutes",
    reset: "x-codex-secondary-reset-at",
};

/// Every header read, and the only ones the transport hands back.
const HEADERS: &[&str] = &[
    PRIMARY.used,
    PRIMARY.minutes,
    PRIMARY.reset,
    SECONDARY.used,
    SECONDARY.minutes,
    SECONDARY.reset,
];

/// The headers read on `route`: the plan backend's, and none on the API.
pub(super) fn headers(route: Serving) -> &'static [&'static str] {
    match route {
        Serving::Subscription => HEADERS,
        Serving::Api => &[],
    }
}

/// The windows `named` report, read from a response that arrived at
/// `arrived`; `None` where they report none crucible knows.
pub(super) fn read(named: &Named, arrived: SystemTime) -> Option<PlanWindows> {
    let windows = [PRIMARY, SECONDARY]
        .iter()
        .filter_map(|reported| window(named, reported))
        .fold(PlanWindows::new(arrived), |windows, (window, reading)| {
            windows.with(window, reading)
        });
    (!windows.is_empty()).then_some(windows)
}

/// The window `reported` names, where its length is one crucible knows and
/// each of its figures is a number in range. A reset time may be absent, but
/// one that is present and is not a count of seconds since the epoch leaves
/// the whole window out.
fn window(named: &Named, reported: &Reported) -> Option<(Window, WindowReading)> {
    let window = length(named.get(reported.minutes)?.parse().ok()?)?;
    let percent = percent(named.get(reported.used)?)?;
    let resets_at = match named.get(reported.reset) {
        None => None,
        Some(reset) => {
            Some(SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(reset.parse().ok()?))?)
        }
    };
    Some((window, WindowReading::new(percent, resets_at)))
}

/// Which window lasts `minutes`: five hours, a week, or a month of 28 to 31
/// days.
fn length(minutes: u64) -> Option<Window> {
    const DAY: u64 = 24 * 60;
    match minutes {
        300 => Some(Window::FiveHour),
        10_080 => Some(Window::Weekly),
        minutes if (28 * DAY..=31 * DAY).contains(&minutes) => Some(Window::Monthly),
        _ => None,
    }
}

/// A percentage used, rounded down so it never says more is used than was,
/// and read as 100 past it; `None` for anything that is not a number from
/// zero up.
fn percent(text: &str) -> Option<u8> {
    let used: f64 = text.parse().ok()?;
    if !used.is_finite() {
        return None;
    }
    (0..=100_u8).rev().find(|whole| f64::from(*whole) <= used)
}
