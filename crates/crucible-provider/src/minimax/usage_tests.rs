//! Asking a Token Plan for its limits, held to the shape the vendor's own
//! command-line client reads.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::{Asked, Provider as _, ProviderError};
use crucible_types::{PlanWindows, Scope, Used, Window, WindowReading};
use serde_json::{Value, json};

use super::MiniMax;
use crate::endpoint::Endpoint;
use crate::transport::{Method, Replay};

/// The answer, in the shape of the vendor's command-line client: see
/// `fixtures/SOURCES.md`.
const REMAINS: &str = include_str!("fixtures/token-plan-remains.json");

/// A key made up for these tests, never one the vendor issued.
const KEY: &str = "fabricated-token-plan-key";

/// The fixture's 5-hour window ends here, in milliseconds since the epoch.
const INTERVAL_ENDS: u64 = 1_776_373_200_000;

/// The fixture's weekly window ends here, in milliseconds since the epoch.
const WEEK_ENDS: u64 = 1_776_614_400_000;

fn at(millis: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(millis)
}

fn signed() -> HeaderKey {
    HeaderKey::new(ApiKey::new(KEY), Header::bearer())
}

/// What a provider built on a Token Plan row at `endpoint` came to on being
/// asked, answered with `status` and `body`, and what it sent.
async fn asked_on_plan(
    endpoint: Endpoint,
    status: u16,
    body: &str,
) -> (Option<Asked>, Arc<Replay>) {
    let replay = Arc::new(Replay::new(status, body));
    let provider = MiniMax::on_plan(endpoint, Box::new(signed()), Box::new(Arc::clone(&replay)));
    let asked = match provider.ask_limits() {
        Some(asking) => Some(asking.await),
        None => None,
    };
    (asked, replay)
}

/// The windows an answer of `body` reads as, on the international site.
async fn read(body: &Value) -> PlanWindows {
    match asked_on_plan(MiniMax::IO, 200, &body.to_string()).await.0 {
        Some(Asked::Answered(windows)) => windows,
        other => panic!("not an answer: {other:?}"),
    }
}

fn remains() -> Value {
    serde_json::from_str(REMAINS).expect("the fixture is JSON")
}

/// The fixture with each of `fields` set on its first entry.
fn first_with(fields: &[(&str, Value)]) -> Value {
    let mut body = remains();
    let first = body
        .pointer_mut("/model_remains/0")
        .and_then(Value::as_object_mut)
        .expect("the fixture has an entry");
    for (field, value) in fields {
        first.insert((*field).to_owned(), value.clone());
    }
    body
}

/// Each group's name and windows, in the order the reading keeps them.
fn grouped(windows: &PlanWindows) -> Vec<(String, Vec<(Window, WindowReading)>)> {
    windows
        .groups()
        .map(|group| {
            let name = match group.scope() {
                Scope::Model(model) => model.name().as_str().to_owned(),
                Scope::Plan => "the plan".to_owned(),
            };
            (name, group.windows().collect())
        })
        .collect()
}

fn counted(used: u64, total: u64, ends: u64) -> WindowReading {
    WindowReading::counted(used, total, Some(at(ends))).expect("a total above 0")
}

#[tokio::test]
async fn minimax_usage_the_vendor_clients_answer_is_a_group_for_each_model() {
    let windows = read(&remains()).await;

    assert_eq!(
        grouped(&windows),
        [
            // The count is what is left: 228 of 1,500, so 1,272 are used. A
            // weekly total of 0 is no window.
            (
                "MiniMax-M*".to_owned(),
                vec![(Window::FiveHour, counted(1_272, 1_500, INTERVAL_ENDS))]
            ),
            (
                "speech-hd".to_owned(),
                vec![
                    (Window::FiveHour, counted(0, 9_000, INTERVAL_ENDS)),
                    (Window::Weekly, counted(0, 63_000, WEEK_ENDS)),
                ]
            ),
            (
                "image-01".to_owned(),
                vec![
                    (Window::FiveHour, counted(0, 100, INTERVAL_ENDS)),
                    (Window::Weekly, counted(0, 700, WEEK_ENDS)),
                ]
            ),
        ]
    );
    assert!(!windows.incomplete());
}

#[tokio::test]
async fn minimax_usage_is_asked_of_the_keys_own_site_with_the_key_and_nothing_else() {
    for (site, address) in [
        (MiniMax::IO, "https://api.minimax.io/v1/token_plan/remains"),
        (MiniMax::CN, "https://api.minimax.cn/v1/token_plan/remains"),
    ] {
        let (asked, replay) = asked_on_plan(site, 200, REMAINS).await;

        assert!(matches!(asked, Some(Asked::Answered(_))), "{asked:?}");
        let sent = replay.sent();
        assert_eq!(sent.method, Method::Get);
        assert_eq!(sent.url, address);
        assert!(sent.body.is_empty());
        // The key, what is asked for and who asks: any other header fails here.
        let mut names = sent
            .headers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        names.sort_unstable();
        assert_eq!(names, ["accept", "authorization", "user-agent"]);
        assert!(
            sent.headers
                .iter()
                .any(|(name, value)| name == "authorization" && value == &format!("Bearer {KEY}"))
        );
        assert_eq!(replay.sent_count(), Some(1));
    }
}

#[tokio::test]
async fn minimax_usage_a_pay_as_you_go_key_never_asks() {
    for site in [MiniMax::IO, MiniMax::CN] {
        let replay = Arc::new(Replay::new(200, REMAINS));
        // Built as a pay-as-you-go row builds it.
        let provider = MiniMax::at(site, Box::new(signed()), Box::new(Arc::clone(&replay)));

        assert!(provider.ask_limits().is_none());
        assert_eq!(replay.sent_count(), Some(0));
    }
}

#[tokio::test]
async fn minimax_usage_a_plan_key_at_an_address_a_setting_named_never_asks() {
    let gateway = Endpoint::parse("https://proxy.invalid/v1/chat/completions").unwrap();

    let (asked, replay) = asked_on_plan(gateway, 200, REMAINS).await;

    assert!(asked.is_none(), "{asked:?}");
    assert_eq!(replay.sent_count(), Some(0));
}

#[tokio::test]
async fn minimax_usage_a_weekly_status_of_3_is_unlimited() {
    let windows = read(&first_with(&[("current_weekly_status", json!(3))])).await;

    assert_eq!(
        grouped(&windows).first(),
        Some(&(
            "MiniMax-M*".to_owned(),
            vec![
                (Window::FiveHour, counted(1_272, 1_500, INTERVAL_ENDS)),
                (Window::Weekly, WindowReading::unlimited()),
            ]
        ))
    );
}

#[tokio::test]
async fn minimax_usage_a_model_with_both_totals_0_is_not_shown() {
    let none = [
        ("current_interval_total_count", json!(0)),
        ("current_weekly_total_count", json!(0)),
    ];
    // With no status, and as the vendor marks a model the plan does not
    // include.
    let not_in_plan = [
        ("current_interval_status", json!(3)),
        ("current_weekly_status", json!(3)),
    ];
    for fields in [none.to_vec(), [none, not_in_plan].concat()] {
        let windows = read(&first_with(&fields)).await;

        let names: Vec<String> = grouped(&windows)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, ["speech-hd", "image-01"], "{fields:?}");
    }
}

#[tokio::test]
async fn minimax_usage_a_remaining_percent_says_what_is_used_and_its_absence_leaves_the_count_as_left()
 {
    // A quarter left of 1,500 is 1,125 used, whatever the count says.
    let given = read(&first_with(&[
        ("current_interval_remaining_percent", json!(25)),
        ("current_interval_usage_count", json!(375)),
    ]))
    .await;
    assert_eq!(
        given
            .groups()
            .next()
            .map(|group| group.windows().collect::<Vec<_>>()),
        Some(vec![(
            Window::FiveHour,
            counted(1_125, 1_500, INTERVAL_ENDS)
        )])
    );

    // With none, the count is what is left.
    let absent = read(&first_with(&[("current_interval_usage_count", json!(375))])).await;
    assert_eq!(
        absent
            .groups()
            .next()
            .map(|group| group.windows().collect::<Vec<_>>()),
        Some(vec![(
            Window::FiveHour,
            counted(1_125, 1_500, INTERVAL_ENDS)
        )])
    );

    // Nothing left is all of it used.
    let spent = read(&first_with(&[(
        "current_interval_remaining_percent",
        json!(0),
    )]))
    .await;
    assert_eq!(
        spent
            .groups()
            .next()
            .and_then(|group| group.reading(Window::FiveHour))
            .map(WindowReading::used),
        Some(Used::Counted {
            used: 1_500,
            total: 1_500
        })
    );
}

/// An answer that came with a 200 and carries `code` under `base_resp`.
fn refused_with(code: i64, said: &str) -> String {
    json!({
        "base_resp": {"status_code": code, "status_msg": said},
        "model_remains": remains().get("model_remains").cloned(),
    })
    .to_string()
}

#[tokio::test]
async fn minimax_usage_a_nonzero_base_resp_is_a_refusal_and_no_reading() {
    let (asked, _) = asked_on_plan(MiniMax::IO, 200, &refused_with(1002, "rate limit")).await;

    assert!(
        matches!(
            asked,
            Some(Asked::Failed(ProviderError::Protocol {
                provider: "minimax",
                ..
            }))
        ),
        "{asked:?}"
    );
}

#[tokio::test]
async fn minimax_usage_a_key_refused_inside_a_200_closes_the_source() {
    // The vendor refuses a key it does not accept with a 200 and its own code,
    // where another vendor answers 401: it is not asked again that session.
    let (asked, _) = asked_on_plan(MiniMax::IO, 200, &refused_with(1004, "login fail")).await;

    assert!(matches!(asked, Some(Asked::Closed)), "{asked:?}");
}

#[tokio::test]
async fn minimax_usage_a_family_group_holds_its_models_and_an_exact_one_its_own() {
    let windows = read(&remains()).await;
    let holding = |model: &str| -> Vec<String> {
        windows
            .groups()
            .filter_map(|group| match group.scope() {
                Scope::Model(group) if group.holds(model) => Some(group.name().as_str().to_owned()),
                _ => None,
            })
            .collect()
    };

    assert_eq!(holding("MiniMax-M2.7"), ["MiniMax-M*"]);
    assert_eq!(holding("MiniMax-M3"), ["MiniMax-M*"]);
    assert_eq!(holding("speech-2.8-hd"), Vec::<String>::new());
    assert_eq!(holding("speech-hd"), ["speech-hd"]);
}

#[tokio::test]
async fn minimax_usage_a_reset_past_any_window_is_no_reset_and_stops_no_turn() {
    let now = SystemTime::now();
    let millis = |at: SystemTime| {
        u64::try_from(
            at.duration_since(UNIX_EPOCH)
                .expect("after the epoch")
                .as_millis(),
        )
        .expect("within u64")
    };
    // The family's 5-hour window spent, ending where each case says.
    let spent_until = |ends: u64| {
        first_with(&[
            ("current_interval_total_count", json!(10)),
            ("current_interval_usage_count", json!(0)),
            ("end_time", json!(ends)),
        ])
    };

    let soon = read(&spent_until(millis(now + Duration::from_hours(1)))).await;
    let never = read(&spent_until(u64::MAX)).await;

    let five_hour = |windows: &PlanWindows| {
        windows
            .groups()
            .next()
            .and_then(|group| group.reading(Window::FiveHour))
            .map(WindowReading::resets_at)
    };
    assert!(matches!(five_hour(&soon), Some(Some(_))));
    assert!(soon.exhausted("MiniMax-M2.7", now).is_some());
    assert_eq!(five_hour(&never), Some(None));
    assert_eq!(never.exhausted("MiniMax-M2.7", now), None);
}
