//! Asking Kimi Code how much of its plan is used, against recorded answers.
//!
//! The answers are shaped from what Kimi's own clients read, the current one
//! and the one before it; no answer from the service was captured to write
//! them.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::Asked;
use crucible_types::{PlanWindows, Used, Window, WindowReading};

use super::*;
use crate::transport::{Method, Replay};

/// What the plan is asked with, and nothing else: a key no answer may repeat.
const KEY: &str = "fabricated-kimi-usage-key";

/// The answer Kimi's current client reads, with every limit it names and the
/// booster wallet beside them.
const CURRENT: &str = include_str!("fixtures/usages/current.json");

/// The answer Kimi's earlier client reads, counting requests.
const OLDER: &str = include_str!("fixtures/usages/older.json");

/// The second `2026-10-03T15:40:00Z` is.
const FIVE_HOUR_RESET: u64 = 1_791_042_000;
/// The second `2026-10-05T09:00:00.443553353+02:00` is, to the whole second.
const WEEKLY_RESET: u64 = 1_791_183_600;
/// The second `2026-11-01T09:00:00Z` is.
const MONTHLY_RESET: u64 = 1_793_523_600;

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// What asking the plan behind a key at `endpoint` comes to, where it is
/// asked at all, when the service answers `status` with `body`; and the
/// transport that answered.
async fn asked_at(endpoint: Endpoint, status: u16, body: &str) -> (Option<Asked>, Arc<Replay>) {
    let replay = Arc::new(Replay::new(status, body));
    let provider = Moonshot::at(
        endpoint,
        Box::new(HeaderKey::new(ApiKey::new(KEY), Header::bearer())),
        Box::new(Arc::clone(&replay)),
    );
    let asked = match provider.ask_limits() {
        Some(asking) => Some(asking.await),
        None => None,
    };
    (asked, replay)
}

async fn asked(status: u16, body: &str) -> Asked {
    asked_at(Moonshot::CODING, status, body)
        .await
        .0
        .expect("a Kimi Code key is asked")
}

fn answered(asked: Asked) -> PlanWindows {
    match asked {
        Asked::Answered(windows) => windows,
        other => panic!("not an answer: {other:?}"),
    }
}

/// Every window of `windows`, by group and in order, with how much of it is
/// used; and that every group is the plan's own.
fn plan_wide(windows: &PlanWindows) -> Vec<(Window, Used)> {
    let mut groups = windows.groups();
    let Some(plan) = groups.next() else {
        return Vec::new();
    };
    assert_eq!(plan.scope(), &crucible_types::Scope::Plan);
    assert!(groups.next().is_none(), "Kimi's limits are the plan's own");
    plan.windows()
        .map(|(window, reading)| (window, reading.used()))
        .collect()
}

/// The current answer with each of `limits`, a key and what it holds.
fn usages(limits: &[(&str, &str)]) -> String {
    let limits = limits
        .iter()
        .map(|(key, entry)| format!(r#""{key}":{entry}"#))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"usages":{{{limits}}}}}"#)
}

#[tokio::test]
async fn kimi_usage_all_three_windows_are_read_as_percentages() {
    let (asked, replay) = asked_at(Moonshot::CODING, 200, CURRENT).await;
    let windows = answered(asked.expect("asked"));

    assert_eq!(
        plan_wide(&windows),
        [
            (Window::FiveHour, Used::Percent(12)),
            (Window::Weekly, Used::Percent(31)),
            (Window::Monthly, Used::Percent(9)),
        ],
        "the monthly code share and the booster wallet are not read"
    );
    assert_eq!(
        windows.reading(Window::FiveHour),
        Some(WindowReading::new(12, Some(at(FIVE_HOUR_RESET))))
    );
    let weekly = windows
        .reading(Window::Weekly)
        .and_then(WindowReading::resets_at);
    // Both sides are built by the platform's own arithmetic, as a clock that
    // counts coarser than a nanosecond, as Windows's does, keeps what it can
    // of the fraction alike on each.
    assert_eq!(
        weekly,
        at(WEEKLY_RESET).checked_add(Duration::from_nanos(443_553_353)),
        "an offset and a fraction of a second are read"
    );
    assert_eq!(
        windows.reading(Window::Monthly),
        Some(WindowReading::new(9, Some(at(MONTHLY_RESET))))
    );
    assert!(!windows.incomplete());

    let sent = replay.sent();
    assert_eq!(sent.method, Method::Get);
    assert_eq!(sent.url, "https://api.kimi.com/coding/v1/usages");
    assert!(sent.body.is_empty());
    let header = |name: &str| {
        sent.headers
            .iter()
            .find(|(present, _)| present == name)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(
        header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_eq!(header("accept"), Some("application/json"));
    assert_eq!(replay.sent_count(), Some(1));
}

/// A key or sign-in of kimi.ai is asked at the global site's own address,
/// since the other refuses it.
#[tokio::test]
async fn kimi_usage_the_global_site_is_asked_at_its_own_address() {
    let (asked, replay) = asked_at(Moonshot::CODING_AI, 200, CURRENT).await;
    assert!(matches!(asked, Some(Asked::Answered(_))), "{asked:?}");
    assert_eq!(replay.sent().url, "https://api.kimi.ai/coding/v1/usages");
}

/// A newer member's plan has no weekly window: the two it has are read, and
/// none is assumed.
#[tokio::test]
async fn kimi_usage_only_the_five_hour_and_monthly_windows_reported_are_read() {
    let body = usages(&[
        (
            "limit_5h",
            r#"{"used_ratio":0.5,"reset_time":"2026-10-03T15:40:00Z"}"#,
        ),
        (
            "limit_month_total",
            r#"{"used_ratio":0.25,"reset_time":"2026-11-01T09:00:00Z"}"#,
        ),
    ]);
    assert_eq!(
        plan_wide(&answered(asked(200, &body).await)),
        [
            (Window::FiveHour, Used::Percent(50)),
            (Window::Monthly, Used::Percent(25)),
        ]
    );
}

/// A ratio may come as text, and is read as the number it spells.
#[tokio::test]
async fn kimi_usage_a_ratio_given_as_text_is_read() {
    let body = usages(&[(
        "limit_7d",
        r#"{"used_ratio":"0.4","reset_time":"2026-10-05T07:00:00Z"}"#,
    )]);
    let windows = answered(asked(200, &body).await);
    assert_eq!(
        windows.reading(Window::Weekly),
        Some(WindowReading::new(40, Some(at(WEEKLY_RESET))))
    );
}

/// A ratio is a share rounded down, never past what was used, and a ratio a
/// float cannot hold exactly is not rounded below what it says.
#[tokio::test]
async fn kimi_usage_a_ratio_is_a_share_rounded_down() {
    let body = usages(&[
        ("limit_5h", r#"{"used_ratio":0.29}"#),
        ("limit_7d", r#"{"used_ratio":0.125}"#),
        ("limit_month_total", r#"{"used_ratio":0}"#),
    ]);
    assert_eq!(
        plan_wide(&answered(asked(200, &body).await)),
        [
            (Window::FiveHour, Used::Percent(29)),
            (Window::Weekly, Used::Percent(12)),
            (Window::Monthly, Used::Percent(0)),
        ]
    );
}

/// A ratio past one is a window used up, which stops a turn on any model
/// until it starts again: Kimi's limits are the plan's own. So is a ratio too
/// large to be multiplied out as a percentage.
#[tokio::test]
async fn kimi_usage_a_ratio_past_one_is_a_window_used_up() {
    let body = usages(&[(
        "limit_5h",
        r#"{"used_ratio":1.7,"reset_time":"2026-10-03T15:40:00Z"}"#,
    )]);
    let windows = answered(asked(200, &body).await);
    let reading = windows.reading(Window::FiveHour).expect("read");
    assert_eq!(reading.used(), Used::Percent(100));
    assert!(reading.spent());
    let before = at(FIVE_HOUR_RESET - 60);
    for model in ["kimi-for-coding", "k3"] {
        assert_eq!(
            windows.exhausted(model, before),
            Some((Window::FiveHour, at(FIVE_HOUR_RESET))),
            "{model}"
        );
    }

    let body = usages(&[("limit_5h", r#"{"used_ratio":1e307}"#)]);
    assert_eq!(
        answered(asked(200, &body).await).reading(Window::FiveHour),
        Some(WindowReading::new(100, None))
    );
}

/// A reset time that is not one leaves the window read without a reset, as
/// Kimi's own client shows it; so does one before 1970, which no window of a
/// plan asked now starts again at, the zero time some services write for
/// none among them.
#[tokio::test]
async fn kimi_usage_a_bad_reset_time_is_a_window_with_no_reset() {
    for reset in [
        r#""soon""#,
        r#""2026-13-45T25:00:00Z""#,
        r#""2026-10-03T15:40:00""#,
        r#""""#,
        "1791042000",
        "null",
        r#""0001-01-01T00:00:00Z""#,
        r#""1600-12-31T23:59:59Z""#,
        r#""1969-12-31T23:59:59.5Z""#,
    ] {
        let body = usages(&[(
            "limit_5h",
            &format!(r#"{{"used_ratio":0.12,"reset_time":{reset}}}"#),
        )]);
        assert_eq!(
            answered(asked(200, &body).await).reading(Window::FiveHour),
            Some(WindowReading::new(12, None)),
            "{reset}"
        );
    }
}

/// A limit whose ratio is not a number from zero up is left out, and a key
/// missing is a window not reported.
#[tokio::test]
async fn kimi_usage_a_ratio_that_is_not_a_share_leaves_its_window_out() {
    for ratio in [r#""a lot""#, "-0.2", r#""NaN""#, r#""inf""#, "null", "true"] {
        let body = usages(&[
            ("limit_5h", &format!(r#"{{"used_ratio":{ratio}}}"#)),
            ("limit_7d", r#"{"used_ratio":0.31}"#),
        ]);
        assert_eq!(
            plan_wide(&answered(asked(200, &body).await)),
            [(Window::Weekly, Used::Percent(31))],
            "{ratio}"
        );
    }
    let empty = answered(asked(200, r#"{"usages":{}}"#).await);
    assert!(empty.is_empty());
}

/// The earlier answer counts requests: each limit is a window as long as it
/// says, used out of its total, in the plan-wide group. What it says beside
/// its limits names no window, so it is not read.
#[tokio::test]
async fn kimi_usage_the_older_answer_is_read_as_counts() {
    assert_eq!(
        plan_wide(&answered(asked(200, OLDER).await)),
        [
            (
                Window::FiveHour,
                Used::Counted {
                    used: 412,
                    total: 1500
                }
            ),
            (
                Window::Weekly,
                Used::Counted {
                    used: 0,
                    total: 9000
                }
            ),
        ]
    );
}

/// An earlier answer's limit is left out where its window is in a unit not
/// read, or its total is none.
#[tokio::test]
async fn kimi_usage_an_older_limit_with_no_length_or_total_is_left_out() {
    let body = r#"{"limits":[
        {"detail":{"limit":100,"used":1},"window":{"duration":30,"timeUnit":"TIME_UNIT_FORTNIGHT"}},
        {"detail":{"limit":0,"used":0},"window":{"duration":5,"timeUnit":"TIME_UNIT_HOUR"}},
        {"detail":{"limit":100,"used":1}},
        {"detail":{"limit":100,"remaining":120},"window":{"duration":1,"timeUnit":"TIME_UNIT_DAY"}}
    ]}"#;
    assert_eq!(
        plan_wide(&answered(asked(200, body).await)),
        [(
            Window::Daily,
            Used::Counted {
                used: 0,
                total: 100
            }
        )]
    );
}

/// An answer in neither shape leaves what was known.
#[tokio::test]
async fn kimi_usage_an_answer_in_neither_shape_is_a_failure() {
    for body in ["not json", "[1,2]", "{}", r#"{"usages":"none"}"#, ""] {
        let failed = asked(200, body).await;
        assert!(matches!(failed, Asked::Failed(_)), "{body}: {failed:?}");
    }
}

/// A key of the open platform has no plan to ask, nor has a gateway at an
/// address of its own.
#[tokio::test]
async fn kimi_usage_a_platform_key_or_a_gateway_is_never_asked() {
    for endpoint in [
        Moonshot::PLATFORM,
        Endpoint::fixed("https://gateway.example/coding/v1/chat/completions"),
    ] {
        let (asked, replay) = asked_at(endpoint, 200, CURRENT).await;
        assert!(asked.is_none(), "{asked:?}");
        assert_eq!(replay.sent_count(), Some(0));
    }
}

/// A refusal of the credential, or no source there, closes asking for the
/// session; anything else leaves it open. Neither repeats the key.
#[tokio::test]
async fn kimi_usage_a_401_closes_asking_and_a_500_does_not() {
    for status in [401, 403, 404] {
        let closed = asked(status, KEY).await;
        assert!(matches!(closed, Asked::Closed), "{status}: {closed:?}");
    }
    for status in [500, 429, 502] {
        let failed = asked(status, KEY).await;
        assert!(matches!(failed, Asked::Failed(_)), "{status}: {failed:?}");
        assert!(!format!("{failed:?}").contains(KEY), "{failed:?}");
    }
}
