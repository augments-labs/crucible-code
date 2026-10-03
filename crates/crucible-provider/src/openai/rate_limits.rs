//! What the plan backend says of a `ChatGPT` plan's usage windows.
//!
//! A plan is limited over windows of time, and some of its limits apply to
//! one model alone. The backend says how much of them is used in two places.
//!
//! In a response's headers, off a response crucible already receives,
//! wherever it carries them: a family of headers per limit, `x-<id>-primary-*`
//! and `x-<id>-secondary-*`, each window as a percentage used, its length in
//! minutes and the second it starts again. The `codex` family is the plan's
//! own; any other is the limit of the model whose slug its
//! `x-<id>-limit-name` says, else one named by its id. No more families are
//! read than a reading keeps groups, and a reading that left one out says it
//! is incomplete; the plan's own headers are kept ahead of any other's, so no
//! number of families arriving first leaves it out.
//!
//! In an answer to asking, at [`usage`]'s address with the credential the
//! provider already holds: `rate_limit` is the plan's own and each of
//! `additional_rate_limits` is one model's. It is asked only when a client
//! asks, and only on a sign-in to the plan.
//!
//! An API key's route reads neither: the published API says nothing of a plan.
//!
//! Which window is which is decided by its length, never by the header or
//! field that carried it or by any text in it. A window whose figures are not
//! numbers in range is left out, and a response that leaves every window out
//! reads as one that reported none.
//!
//! Which model a group limits is decided here, as the Codex CLI decides it:
//! by the limit's name, which is the model's slug, matched whole against the
//! id a request asks for. That is the group's key, kept apart from the name
//! it is drawn with; a limit with no name limits no model by name. Neither is
//! ever parsed.
//!
//! The header and field names are those the Codex CLI reads; no response
//! from the backend was captured to write this.
//!
//! The backend refuses a request whose plan is used up with a body of its own
//! shape, which [`used_up`] reads; the same refusal carries these headers, and
//! they name the window.

use std::time::{Duration, SystemTime};

use crucible_types::{
    GroupName, MAX_LIMIT_GROUPS, ModelGroup, ModelKey, PlanWindows, Scope, Window, WindowReading,
};
use serde_json::Value;

use super::Serving;
use crate::refusal::UsedUp;
use crate::responses::Usage;
use crate::transport::{Named, Reads, Wants};

/// Every header of a family opens with this.
const OPENS: &str = "x-";

/// The family whose limit is the plan's own rather than one model's.
const PLAN: &str = "codex";

/// The two windows a family reports, by the word in its headers' names.
const SLOTS: [&str; 2] = ["primary", "secondary"];

/// What follows a family's id and slot in the header of each figure a window
/// is reported in: the percentage used, the length in whole minutes, and the
/// second since the epoch it starts again.
const USED: &str = "used-percent";
const MINUTES: &str = "window-minutes";
const RESET: &str = "reset-at";

/// What follows a family's id in the header naming its limit.
const LIMIT_NAME: &str = "limit-name";

/// Whether `name` is a header of a family, `x-<id>-<slot>-<figure>` or
/// `x-<id>-limit-name`: the plan's own read ahead of any other, so that no
/// number of others arriving first leaves it out.
fn reads(name: &str) -> Wants {
    match id(name) {
        Some(PLAN) => Wants::Ahead,
        Some(_) => Wants::Kept,
        None => Wants::Not,
    }
}

/// The id of the family `name` is a header of, where it is one.
fn id(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(OPENS)?;
    rest.strip_suffix(LIMIT_NAME)
        .and_then(|id| id.strip_suffix('-'))
        .or_else(|| {
            [USED, MINUTES, RESET].iter().find_map(|figure| {
                rest.strip_suffix(figure)
                    .and_then(|slot| slot.strip_suffix('-'))
                    .and_then(|slot| SLOTS.iter().find_map(|each| slot.strip_suffix(each)))
                    .and_then(|id| id.strip_suffix('-'))
            })
        })
        .filter(|id| !id.is_empty())
}

/// The headers read on `route`: the plan backend's, and none on the API.
pub(super) fn headers(route: Serving) -> Option<Reads> {
    match route {
        Serving::Subscription => Some(reads),
        Serving::Api => None,
    }
}

/// The windows `named` report, read from a response that arrived at
/// `arrived`; `None` where they report none crucible knows.
///
/// Each family is found by a header reporting how much of a window is used,
/// and is no more than [`MAX_LIMIT_GROUPS`] of them: the plan's own first,
/// wherever its headers arrived, then the others in the order they did. A
/// reading that left a family out, or that the transport handed only some of
/// the headers it reads, says it is incomplete.
pub(super) fn read(named: &Named, arrived: SystemTime) -> Option<PlanWindows> {
    let mut families: Vec<&str> = Vec::new();
    for name in named.names() {
        let Some(family) = family(name) else {
            continue;
        };
        if families.contains(&family) {
            continue;
        }
        if family == PLAN {
            families.insert(0, family);
        } else {
            families.push(family);
        }
    }
    let cut = families.len() > MAX_LIMIT_GROUPS || named.left_out();
    families.truncate(MAX_LIMIT_GROUPS);
    let windows = families
        .iter()
        .filter_map(|family| Some((*family, scope(named, family)?)))
        .flat_map(|(family, scope)| {
            SLOTS.iter().filter_map(move |slot| {
                window(named, family, slot).map(|read| (scope.clone(), read))
            })
        })
        .fold(
            PlanWindows::new(arrived),
            |windows, (scope, (window, reading))| windows.within(scope, window, reading),
        );
    let windows = if cut { windows.cut() } else { windows };
    (!windows.is_empty()).then_some(windows)
}

/// The family a header reporting how much of a window is used belongs to:
/// the `<id>` of `x-<id>-<slot>-used-percent`.
fn family(name: &str) -> Option<&str> {
    let rest = name
        .strip_prefix(OPENS)?
        .strip_suffix(USED)?
        .strip_suffix('-')?;
    SLOTS
        .iter()
        .find_map(|slot| rest.strip_suffix(slot)?.strip_suffix('-'))
        .filter(|id| !id.is_empty())
}

/// Whose limit `family` is: the plan's own, or the model whose slug its
/// `x-<id>-limit-name` header says. One with no such header limits no model
/// by name, and is named by its id as the Codex CLI spells it, lowercase with
/// every `-` an `_`; `None` for an id that is no name.
fn scope(named: &Named, family: &str) -> Option<Scope> {
    if family == PLAN {
        return Some(Scope::Plan);
    }
    let id = family.to_ascii_lowercase().replace('-', "_");
    match named.get(&format!("{OPENS}{family}-{LIMIT_NAME}")) {
        Some(slug) => model(slug).or_else(|| unkeyed(&id)),
        None => unkeyed(&id),
    }
}

/// The group kept for the model whose slug is `slug`, as the backend names
/// a limit's model: it holds back a request to that model, matched whole as
/// the Codex CLI matches it, and is drawn as the slug says until a reader
/// knows the model by another name. `None` for a slug that is no name.
fn model(slug: &str) -> Option<Scope> {
    let name = GroupName::new(slug)?;
    Some(Scope::Model(ModelGroup::new(name, ModelKey::exact(slug))))
}

/// A group named `name` that limits no model by name.
fn unkeyed(name: &str) -> Option<Scope> {
    Some(Scope::Model(ModelGroup::new(GroupName::new(name)?, None)))
}

/// The window `family` reports in `slot`, where it has a length and each of
/// its figures is a number in range. A reset time may be absent, but one that
/// is present and is not a count of seconds since the epoch leaves the whole
/// window out.
fn window(named: &Named, family: &str, slot: &str) -> Option<(Window, WindowReading)> {
    let figure = |figure: &str| named.get(&format!("{OPENS}{family}-{slot}-{figure}"));
    let window = Window::of(figure(MINUTES)?.parse().ok()?)?;
    let percent = percent(figure(USED)?.parse().ok()?)?;
    let resets_at = match figure(RESET) {
        None => None,
        Some(reset) => {
            Some(SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(reset.parse().ok()?))?)
        }
    };
    Some((window, WindowReading::new(percent, resets_at)))
}

/// A percentage used, rounded down so it never says more is used than was,
/// and read as 100 past it; `None` for anything that is not a number from
/// zero up.
fn percent(used: f64) -> Option<u8> {
    if !used.is_finite() {
        return None;
    }
    (0..=100_u8).rev().find(|whole| f64::from(*whole) <= used)
}

/// Where the plan backend says how much of a plan is used.
const USAGE: &str = "https://chatgpt.com/backend-api/wham/usage";

/// The two windows a limit in the backend's usage answer reports.
const ANSWERED: [&str; 2] = ["primary_window", "secondary_window"];

/// Where `route`'s plan is asked how much of it is used: the plan backend's
/// own, and nowhere on the API.
pub(super) fn usage(route: Serving) -> Option<Usage> {
    match route {
        Serving::Subscription => Some(Usage {
            url: USAGE,
            read: |body, arrived| asked(body, arrived).into(),
        }),
        Serving::Api => None,
    }
}

/// The windows the plan backend's usage answer `body` says, read from one that
/// arrived at `arrived`; `None` where it is not an object.
///
/// `rate_limit` is the plan's own, and each of `additional_rate_limits` is a
/// limit of its own: the model's whose slug its `limit_name` says, else one
/// named by its `metered_feature` that limits no model by name. No more of
/// them are read than a reading keeps groups, and an answer with more says it
/// is incomplete. Nothing else in the answer is read: not the plan's type, its
/// credits or its spend control. A window that is null, of no length, or whose
/// figures are not numbers in range is left out, and an answer that leaves
/// every window out is an answer of none.
fn asked(body: &Value, arrived: SystemTime) -> Option<PlanWindows> {
    let body = body.as_object()?;
    let mut windows = PlanWindows::new(arrived);
    if let Some(limit) = body.get("rate_limit") {
        windows = limit_windows(windows, &Scope::Plan, limit, arrived);
    }
    let additional = body
        .get("additional_rate_limits")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if additional.len() > MAX_LIMIT_GROUPS {
        windows = windows.cut();
    }
    for limit in additional.iter().take(MAX_LIMIT_GROUPS) {
        let field = |field: &str| limit.get(field).and_then(Value::as_str);
        let scope = field("limit_name")
            .and_then(model)
            .or_else(|| field("metered_feature").and_then(unkeyed));
        if let (Some(scope), Some(rate_limit)) = (scope, limit.get("rate_limit")) {
            windows = limit_windows(windows, &scope, rate_limit, arrived);
        }
    }
    Some(windows)
}

/// `windows`, with each window `limit` reports read into `scope`.
fn limit_windows(
    windows: PlanWindows,
    scope: &Scope,
    limit: &Value,
    arrived: SystemTime,
) -> PlanWindows {
    ANSWERED
        .iter()
        .filter_map(|slot| answered(limit.get(*slot)?, arrived))
        .fold(windows, |windows, (window, reading)| {
            windows.within(scope.clone(), window, reading)
        })
}

/// One window of a usage answer: its length is `limit_window_seconds`, in
/// whole minutes rounded up as the Codex CLI rounds them, and it starts again
/// at `reset_at`, a second since the epoch, or else `reset_after_seconds`
/// after the answer arrived.
fn answered(window: &Value, arrived: SystemTime) -> Option<(Window, WindowReading)> {
    let seconds = window.get("limit_window_seconds")?.as_u64()?;
    let length = Window::of(seconds.div_ceil(60))?;
    let percent = percent(window.get("used_percent")?.as_f64()?)?;
    let seconds = |field: &str| {
        window
            .get(field)
            .and_then(Value::as_u64)
            .map(Duration::from_secs)
    };
    let resets_at = seconds("reset_at")
        .filter(|since| !since.is_zero())
        .and_then(|since| SystemTime::UNIX_EPOCH.checked_add(since))
        .or_else(|| seconds("reset_after_seconds").and_then(|wait| arrived.checked_add(wait)));
    Some((length, WindowReading::new(percent, resets_at)))
}

/// Whether `body`, a refused response that arrived at `arrived`, is the plan
/// backend refusing a used-up plan, and when it says the plan starts again.
///
/// Told by `error.type` alone, as the Codex CLI tells it, and never by the
/// message beside it. The reset is `error.resets_at`, a second since the
/// epoch, or else `error.resets_in_seconds` counted from `arrived`; either
/// that is not a whole count of seconds is as good as absent, and a refusal
/// with neither is still a used-up plan whose reset was not reported.
pub(super) fn used_up(body: &Value, arrived: SystemTime) -> Option<UsedUp> {
    let error = body.get("error")?;
    if error.get("type")?.as_str()? != "usage_limit_reached" {
        return None;
    }
    let seconds = |field: &str| {
        error
            .get(field)
            .and_then(Value::as_u64)
            .map(Duration::from_secs)
    };
    let resets_at = seconds("resets_at")
        .and_then(|since| SystemTime::UNIX_EPOCH.checked_add(since))
        .or_else(|| seconds("resets_in_seconds").and_then(|wait| arrived.checked_add(wait)));
    Some(UsedUp { resets_at })
}
