//! What a Token Plan says of its limits, asked of the site that issued the
//! key.
//!
//! The answer is shaped as the vendor's own command-line client reads it: a
//! group for each model or family of models (`model_remains`), each with a
//! 5-hour window (`current_interval_*`) and a weekly one (`current_weekly_*`)
//! counted in requests, each ending at an instant in milliseconds since the
//! epoch. A status of 3 is a window with no limit, a total of 0 is no window,
//! and a model with neither window is none the plan includes.
//!
//! The vendor's counts are ambiguous: older answers give what is left under
//! `*_usage_count`, and newer ones may give what is used. As that client does,
//! a share left (`*_remaining_percent`) is read for what is used where it is
//! given, and the count is otherwise read as what is left.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crucible_types::{GroupName, ModelGroup, ModelKey, PlanWindows, Scope, Window, WindowReading};
use serde_json::{Map, Value};

use crate::responses::Usage;

/// Where a minimax.io key's plan is asked.
const IO: &str = "https://api.minimax.io/v1/token_plan/remains";

/// Where a minimaxi.com key's plan is asked.
const CN: &str = "https://api.minimax.cn/v1/token_plan/remains";

/// The status a window with no limit carries.
const UNLIMITED: u64 = 3;

/// Where the plan of a key served at `endpoint` is asked: on the site that
/// served it, and nowhere for an address that is not one of the vendor's.
pub(super) fn source(endpoint: &crate::endpoint::Endpoint) -> Option<Usage> {
    let url = if *endpoint == super::IO {
        IO
    } else if *endpoint == super::CN {
        CN
    } else {
        return None;
    };
    Some(Usage { url, read })
}

/// The windows an answer that arrived at `arrived` says; `None` for one that
/// is not the shape the vendor answers in, or whose status is not 0: a
/// refusal, which says nothing of the plan.
fn read(body: &Value, arrived: SystemTime) -> Option<PlanWindows> {
    let body = body.as_object()?;
    let status = body.get("base_resp")?.get("status_code")?.as_i64()?;
    if status != 0 {
        return None;
    }
    let mut windows = PlanWindows::new(arrived);
    for entry in body.get("model_remains")?.as_array()? {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let Some(scope) = entry
            .get("model_name")
            .and_then(Value::as_str)
            .and_then(group)
        else {
            continue;
        };
        if WINDOWS
            .iter()
            .all(|(_, prefix, _)| not_in_plan(entry, prefix))
        {
            continue;
        }
        for (length, prefix, ends) in WINDOWS {
            if let Some(reading) = window(entry, prefix, ends) {
                windows = windows.within(scope.clone(), length, reading);
            }
        }
    }
    Some(windows)
}

/// Each window an entry reports: its length, what its fields open with, and
/// the field it ends at.
const WINDOWS: [(Window, &str, &str); 2] = [
    (Window::FiveHour, "current_interval", "end_time"),
    (Window::Weekly, "current_weekly", "weekly_end_time"),
];

/// The group a model name stands for: every model it starts where it ends
/// in `*`, and the one model it names otherwise.
fn group(name: &str) -> Option<Scope> {
    let key = if name.ends_with('*') {
        ModelKey::prefixed(name)
    } else {
        ModelKey::exact(name)
    };
    Some(Scope::Model(ModelGroup::new(GroupName::new(name)?, key)))
}

/// One of an entry's figures for the window whose fields open with `prefix`.
fn figure<'a>(entry: &'a Map<String, Value>, prefix: &str, figure: &str) -> Option<&'a Value> {
    entry.get(&format!("{prefix}_{figure}"))
}

/// Whether the window under `prefix` is as the vendor reports one for a
/// model the plan does not include: a total of 0 with no limit.
fn not_in_plan(entry: &Map<String, Value>, prefix: &str) -> bool {
    figure(entry, prefix, "total_count").and_then(Value::as_u64) == Some(0)
        && figure(entry, prefix, "status").and_then(Value::as_u64) == Some(UNLIMITED)
}

/// The window an entry reports under `prefix`, ending at the instant under
/// `ends`; `None` for a total of 0 or a figure that is not read.
fn window(entry: &Map<String, Value>, prefix: &str, ends: &str) -> Option<WindowReading> {
    if figure(entry, prefix, "status").and_then(Value::as_u64) == Some(UNLIMITED) {
        return Some(WindowReading::unlimited());
    }
    let total = figure(entry, prefix, "total_count")?.as_u64()?;
    let used = match figure(entry, prefix, "remaining_percent") {
        Some(left) => used_of(total, left.as_f64()?)?,
        None => total.saturating_sub(figure(entry, prefix, "usage_count")?.as_u64()?),
    };
    let resets_at = entry
        .get(ends)
        .and_then(Value::as_u64)
        .and_then(|millis| UNIX_EPOCH.checked_add(Duration::from_millis(millis)));
    WindowReading::counted(used, total, resets_at)
}

/// What is used of `total` with `left` percent of it left; `None` for a share
/// that is not from 0 to 100. The share is read to the hundredth of a percent
/// at or above it, so a window is never read as more used than it is, and as
/// all used only where nothing is left.
fn used_of(total: u64, left: f64) -> Option<u64> {
    if !(0.0..=100.0).contains(&left) {
        return None;
    }
    let hundredths = (0..=10_000_u16).find(|share| f64::from(*share) >= left * 100.0)?;
    let left = total.checked_mul(u64::from(hundredths))?.div_ceil(10_000);
    Some(total.saturating_sub(left))
}
