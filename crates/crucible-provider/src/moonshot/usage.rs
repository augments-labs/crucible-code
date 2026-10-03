//! What Kimi Code says of a plan's usage windows, when asked.
//!
//! A Kimi Code key or sign-in is asked at `usages` under the coding address
//! of the site it belongs to, with the credential the provider already holds;
//! a key of the open platform has no plan there and is never asked. It is
//! asked only when a client asks.
//!
//! The answer comes in one of two shapes, the ones Kimi's current and earlier
//! clients read, and both are read here. In the current one, `usages` names
//! each window by its key — `limit_5h`, `limit_7d` and `limit_month_total` —
//! with the share of it used as a ratio from 0 to 1, possibly written as text,
//! and the instant it starts again as RFC 3339 text. In the earlier one, each
//! of `limits` counts requests: its total, and how many are used or remain, in
//! a window whose length it says. What the earlier shape says beside `limits`
//! names no window's length, and the share of the month spent on code and the
//! booster wallet are not limits on a window, so none of those is read.
//!
//! Every window is the plan's own: Kimi reports no limit of one model. A key
//! missing is a window not reported, and a window whose figures are not
//! numbers in range is left out. A reset that is not an instant, or is one
//! before 1970, leaves its window read with no reset, as Kimi's own client
//! shows it.
//!
//! The field names are those Kimi's clients read; no answer from the service
//! was captured to write this.

use std::time::{Duration, SystemTime};

use crucible_types::{PlanWindows, Window, WindowReading};
use serde_json::{Map, Value};

use crate::endpoint::Endpoint;
use crate::responses::Usage;

/// Where the plan behind a credential of the kimi.com site is asked.
const CODING: &str = "https://api.kimi.com/coding/v1/usages";

/// Where the plan behind a credential of the kimi.ai site is asked.
const CODING_AI: &str = "https://api.kimi.ai/coding/v1/usages";

/// The windows of the current answer, by the key that names each.
const KEYED: [(&str, Window); 3] = [
    ("limit_5h", Window::FiveHour),
    ("limit_7d", Window::Weekly),
    ("limit_month_total", Window::Monthly),
];

/// Minutes in each unit a window of the earlier answer is measured in, by the
/// word its unit's name holds, in the order Kimi's earlier client looks.
const UNITS: [(&str, u64); 3] = [("MINUTE", 1), ("HOUR", 60), ("DAY", 24 * 60)];

/// How far below a whole percentage a ratio may fall and still be read as
/// it: a share multiplied out in floating point can land a hair under the
/// whole it was written as, as `0.29` does.
const SLACK: f64 = 1e-9;

/// Where the plan behind a credential sent to `endpoint` is asked: the
/// coding address of its own site, and nowhere on the open platform.
pub(super) fn usage(endpoint: &Endpoint) -> Option<Usage> {
    let url = if *endpoint == super::CODING {
        CODING
    } else if *endpoint == super::CODING_AI {
        CODING_AI
    } else {
        return None;
    };
    Some(Usage {
        url,
        read: |body, arrived| asked(body, arrived).into(),
    })
}

/// The windows `body` says, read from an answer that arrived at `arrived`;
/// `None` where it is in neither shape.
fn asked(body: &Value, arrived: SystemTime) -> Option<PlanWindows> {
    let body = body.as_object()?;
    if let Some(usages) = body.get("usages") {
        return Some(current(usages.as_object()?, arrived));
    }
    if body.get("usage").is_some_and(Value::is_object)
        || body.get("limits").is_some_and(Value::is_array)
    {
        return Some(older(body, arrived));
    }
    None
}

/// The current answer's windows, each a share used.
fn current(usages: &Map<String, Value>, arrived: SystemTime) -> PlanWindows {
    KEYED
        .iter()
        .filter_map(|(key, window)| Some((*window, shared(usages.get(*key)?)?)))
        .fold(PlanWindows::new(arrived), |windows, (window, reading)| {
            windows.with(window, reading)
        })
}

/// One window of the current answer: its `used_ratio`, a number or text
/// spelling one, and its `reset_time`.
fn shared(limit: &Value) -> Option<WindowReading> {
    let ratio = match limit.get("used_ratio")? {
        Value::Number(ratio) => ratio.as_f64()?,
        Value::String(ratio) => ratio.trim().parse().ok()?,
        _ => return None,
    };
    let resets_at = limit
        .get("reset_time")
        .and_then(Value::as_str)
        .and_then(|reset| reset.parse::<jiff::Timestamp>().ok())
        .and_then(instant);
    Some(WindowReading::new(percent(ratio)?, resets_at))
}

/// The instant `reset` is, where it is one from 1970 on; `None` before.
///
/// Built from its seconds rather than through `jiff`'s conversion, which
/// panics where the platform's clock cannot hold the instant, as Windows's
/// cannot before 1601; and no window of a plan asked now starts again
/// before 1970, so an instant there, such as the zero time some services
/// write for none, says no reset.
fn instant(reset: jiff::Timestamp) -> Option<SystemTime> {
    let seconds = u64::try_from(reset.as_second()).ok()?;
    let nanos = u32::try_from(reset.subsec_nanosecond()).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::new(seconds, nanos))
}

/// A ratio used as a whole percentage, rounded down so it never says more is
/// used than was, and read as 100 past it, however far; `None` for anything
/// that is not a number from zero up.
fn percent(ratio: f64) -> Option<u8> {
    if !ratio.is_finite() {
        return None;
    }
    // A ratio too large to multiply out is past one all the same: the product
    // is infinite, and every whole percentage is under it.
    let used = ratio * 100.0 + SLACK;
    (0..=100_u8).rev().find(|whole| f64::from(*whole) <= used)
}

/// The earlier answer's windows, each a count used of a total. Its `limits`
/// is read whole: the answer is bounded before it is read, and a group keeps
/// no more windows than its ceiling.
fn older(body: &Map<String, Value>, arrived: SystemTime) -> PlanWindows {
    body.get("limits")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(counted)
        .fold(PlanWindows::new(arrived), |windows, (window, reading)| {
            windows.with(window, reading)
        })
}

/// One limit of the earlier answer: the length its `window` says, and how
/// many of its `detail`'s `limit` are `used`, or else are not `remaining`.
/// A limit whose figures sit beside its window rather than under `detail` is
/// read the same, as Kimi's earlier client reads it.
fn counted(limit: &Value) -> Option<(Window, WindowReading)> {
    let window = limit.get("window")?;
    let duration = whole(window.get("duration")?)?;
    let unit = window.get("timeUnit")?.as_str()?;
    let (_, minutes) = UNITS.iter().find(|(word, _)| unit.contains(word))?;
    let window = Window::of(duration.checked_mul(*minutes)?)?;

    let detail = limit
        .get("detail")
        .filter(|detail| detail.is_object())
        .unwrap_or(limit);
    let total = whole(detail.get("limit")?)?;
    let used = match detail.get("used").and_then(whole) {
        Some(used) => used,
        None => total.saturating_sub(whole(detail.get("remaining")?)?),
    };
    Some((window, WindowReading::counted(used, total, None)?))
}

/// A count, written as a whole number or as text spelling one.
fn whole(count: &Value) -> Option<u64> {
    match count {
        Value::Number(count) => count.as_u64(),
        Value::String(count) => count.trim().parse().ok(),
        _ => None,
    }
}
