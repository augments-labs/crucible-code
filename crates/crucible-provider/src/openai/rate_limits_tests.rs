//! The plan backend's usage windows, read off the head of a response.
//!
//! The header names are shaped from the strings of the Codex CLI's binary, the
//! program the backend serves, rather than from a captured response.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::{Asked, RequestPurpose};
use crucible_types::{
    LimitGroup, MAX_LIMIT_GROUPS, Message, PlanWindows, Scope, Transcript, Window, WindowReading,
};

use super::*;
use crate::transport::Replay;

/// A plain answer, so every reading below is a reading of the head alone.
const ANSWER: &str = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n\
     data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-limits\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":1,\"total_tokens\":11}}}\n\n";

/// A second some days after this was written, as the backend spells one.
const RESET: u64 = 1_793_000_000;

fn asking() -> Request<'static> {
    let mut transcript = Transcript::new();
    transcript.push(Message::said("hello")).unwrap();
    Request {
        model: "gpt-5.5",
        purpose: RequestPurpose::Turn,
        transcript: Box::leak(Box::new(transcript)),
        tools: &[],
        system: None,
        max_tokens: 8192,
        effort: None,
        attached: &[],
        prompt_cache: None,
    }
}

/// What a response from `endpoint` whose head carries `headers` reports.
fn limits_at(endpoint: Endpoint, headers: &[(&'static str, &str)]) -> Option<PlanWindows> {
    let replay = headers
        .iter()
        .fold(Replay::new(200, ANSWER), |replay, (name, value)| {
            replay.answering(name, *value)
        });
    let replay = Arc::new(replay);
    let credential = HeaderKey::new(ApiKey::new("synthetic-limits-key"), Header::bearer());
    let provider = OpenAi::at(
        endpoint,
        Box::new(credential),
        Box::new(Arc::clone(&replay)),
    );
    let cancel = Cancel::new();
    let stream = crucible_runtime::answered!(provider.stream(asking(), &cancel)).unwrap();
    stream.limits()
}

fn limits(headers: &[(&'static str, &str)]) -> Option<PlanWindows> {
    limits_at(SUBSCRIPTION, headers)
}

fn percents(windows: Option<PlanWindows>) -> Vec<(Window, u8)> {
    windows
        .map(|windows| {
            windows
                .groups()
                .flat_map(LimitGroup::windows)
                .map(|(window, reading)| (window, reading.percent()))
                .collect()
        })
        .unwrap_or_default()
}

/// Each group a reading holds, by the model it names (none for the plan's
/// own), with its windows' percentages.
type Grouped = Vec<(Option<String>, Vec<(Window, u8)>)>;

fn grouped(windows: Option<PlanWindows>) -> Grouped {
    windows
        .map(|windows| {
            windows
                .groups()
                .map(|group| {
                    let model = match group.scope() {
                        Scope::Plan => None,
                        Scope::Model(group) => Some(group.name().as_str().to_owned()),
                    };
                    let read = group
                        .windows()
                        .map(|(window, reading)| (window, reading.percent()))
                        .collect();
                    (model, read)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

#[test]
fn rate_limit_windows_a_subscription_response_reports_are_read() {
    let before = SystemTime::now();
    let reset = RESET.to_string();
    let later = (RESET + 86_400).to_string();
    let windows = limits(&[
        ("x-codex-primary-used-percent", "12.7"),
        ("x-codex-primary-window-minutes", "300"),
        ("x-codex-primary-reset-at", &reset),
        ("x-codex-secondary-used-percent", "31"),
        ("x-codex-secondary-window-minutes", "10080"),
        ("x-codex-secondary-reset-at", &later),
    ])
    .expect("two windows reported");

    assert_eq!(
        windows.reading(Window::FiveHour),
        Some(WindowReading::new(12, Some(at(RESET))))
    );
    assert_eq!(
        windows.reading(Window::Weekly),
        Some(WindowReading::new(31, Some(at(RESET + 86_400))))
    );
    assert_eq!(windows.reading(Window::Monthly), None);
    assert!(windows.arrived() >= before);
}

/// No window is assumed: a weekly window reported alone is the only one
/// read, and so is a monthly one, whichever header carried it.
#[test]
fn rate_limit_a_window_is_named_by_its_length_alone() {
    assert_eq!(
        percents(limits(&[
            ("x-codex-primary-used-percent", "31"),
            ("x-codex-primary-window-minutes", "10080"),
        ])),
        [(Window::Weekly, 31)]
    );
    assert_eq!(
        percents(limits(&[
            ("x-codex-secondary-used-percent", "9"),
            ("x-codex-secondary-window-minutes", "43200"),
        ])),
        [(Window::Monthly, 9)]
    );
    assert_eq!(
        percents(limits(&[
            ("x-codex-primary-used-percent", "9"),
            ("x-codex-primary-window-minutes", "44640"),
            ("x-codex-secondary-used-percent", "4"),
            ("x-codex-secondary-window-minutes", "300"),
        ])),
        [(Window::FiveHour, 4), (Window::Monthly, 9)]
    );
}

/// A window of a length that is none of the named ones is kept, and named
/// by that length; one of a day is daily.
#[test]
fn rate_limit_a_window_of_any_other_length_is_kept_under_its_length() {
    assert_eq!(
        percents(limits(&[
            ("x-codex-primary-used-percent", "12"),
            ("x-codex-primary-window-minutes", "180"),
            ("x-codex-secondary-used-percent", "31"),
            ("x-codex-secondary-window-minutes", "1440"),
        ])),
        [(Window::Lasting(180), 12), (Window::Daily, 31)]
    );
    assert_eq!(
        percents(limits(&[
            ("x-codex-primary-used-percent", "12"),
            ("x-codex-primary-window-minutes", "60"),
        ])),
        [(Window::Lasting(60), 12)]
    );
}

/// A family of headers other than the plan's own is a limit of its own,
/// named by the header that names it: `x-codex-bengalfox-*` beside
/// `x-codex-bengalfox-limit-name`.
#[test]
fn rate_limit_a_family_named_for_a_model_is_read_as_a_group_of_its_own() {
    let reset = RESET.to_string();
    let windows = limits(&[
        ("x-codex-primary-used-percent", "31"),
        ("x-codex-primary-window-minutes", "10080"),
        ("x-codex-bengalfox-primary-used-percent", "12"),
        ("x-codex-bengalfox-primary-window-minutes", "300"),
        ("x-codex-bengalfox-primary-reset-at", &reset),
        ("x-codex-bengalfox-secondary-used-percent", "4"),
        ("x-codex-bengalfox-secondary-window-minutes", "10080"),
        ("x-codex-bengalfox-limit-name", "gpt-5.3-codex-spark"),
    ]);

    assert_eq!(
        grouped(windows.clone()),
        [
            (None, vec![(Window::Weekly, 31)]),
            (
                Some("gpt-5.3-codex-spark".to_owned()),
                vec![(Window::FiveHour, 12), (Window::Weekly, 4)]
            ),
        ]
    );
    let spark = windows
        .as_ref()
        .and_then(|windows| windows.groups().nth(1))
        .and_then(|group| group.reading(Window::FiveHour));
    assert_eq!(spark, Some(WindowReading::new(12, Some(at(RESET)))));
}

/// A family's `limit-name` is the slug of the model it limits, and a used-up
/// window of it holds back a request to that model, matched whole as it is
/// spelled, and to no other. A family the head names by its id alone limits
/// no model by name, whatever its id reads like.
#[test]
fn rate_limit_a_used_up_model_family_stops_its_model_and_no_other() {
    let reset = RESET.to_string();
    let before = at(RESET - 60);
    let spent = [
        ("x-codex-primary-used-percent", "31"),
        ("x-codex-primary-window-minutes", "10080"),
        ("x-codex-bengalfox-primary-used-percent", "100"),
        ("x-codex-bengalfox-primary-window-minutes", "300"),
        ("x-codex-bengalfox-primary-reset-at", reset.as_str()),
    ];
    let mut named = spent.to_vec();
    named.push(("x-codex-bengalfox-limit-name", "gpt-5.3-codex-spark"));

    let windows = limits(&named).expect("a plan-wide and a model's window");
    assert_eq!(
        windows.exhausted("gpt-5.3-codex-spark", before),
        Some((Window::FiveHour, at(RESET)))
    );
    for other in ["gpt-5.5", "GPT-5.3-Codex-Spark", "gpt-5.3-codex"] {
        assert_eq!(windows.exhausted(other, before), None, "{other}");
    }

    let unnamed = limits(&spent).expect("a plan-wide and a family's window");
    for model in ["codex_bengalfox", "codex-bengalfox", "gpt-5.3-codex-spark"] {
        assert_eq!(unnamed.exhausted(model, before), None, "{model}");
    }
}

/// A family whose limit is not named is named by its own id, as the Codex
/// CLI names it; a name that is nothing once its control characters go is
/// no name.
#[test]
fn rate_limit_a_family_with_no_limit_name_is_named_by_its_id() {
    for name in [None, Some("\u{1b}\u{7}")] {
        let mut headers = vec![
            ("x-codex-other-primary-used-percent", "9"),
            ("x-codex-other-primary-window-minutes", "300"),
        ];
        headers.extend(name.map(|name| ("x-codex-other-limit-name", name)));
        assert_eq!(
            grouped(limits(&headers)),
            [(Some("codex_other".to_owned()), vec![(Window::FiveHour, 9)])],
            "{name:?}"
        );
    }
}

/// Every header of a whole family `id`, as the backend sends one: both
/// windows' three figures, and the limit's name.
fn family_of(id: &str, name: &str, primary: &str, secondary: &str) -> Vec<(&'static str, String)> {
    let header =
        |rest: &str| -> &'static str { Box::leak(format!("x-{id}-{rest}").into_boxed_str()) };
    vec![
        (header("primary-used-percent"), primary.to_owned()),
        (header("primary-window-minutes"), "300".to_owned()),
        (header("primary-reset-at"), RESET.to_string()),
        (header("secondary-used-percent"), secondary.to_owned()),
        (header("secondary-window-minutes"), "10080".to_owned()),
        (header("secondary-reset-at"), RESET.to_string()),
        (header("limit-name"), name.to_owned()),
    ]
}

/// More whole families than a reading keeps groups for, the plan's own
/// arriving last: the first ones are read, and the plan's own is never the
/// one left out, however many headers arrived before it.
#[test]
fn rate_limit_more_families_than_the_ceiling_keep_no_more_than_it() {
    let mut sent: Vec<(&'static str, String)> = (0..MAX_LIMIT_GROUPS + 4)
        .flat_map(|family| {
            family_of(
                &format!("codex-f{family}"),
                &format!("model-f{family}"),
                "5",
                "6",
            )
        })
        .collect();
    sent.extend(family_of("codex", "codex", "12", "31"));
    let headers: Vec<(&'static str, &str)> = sent
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();

    let read = limits(&headers);
    assert!(read.as_ref().is_some_and(PlanWindows::incomplete));
    let groups = grouped(read);

    assert_eq!(groups.len(), MAX_LIMIT_GROUPS, "{groups:?}");
    assert_eq!(
        groups.first(),
        Some(&(None, vec![(Window::FiveHour, 12), (Window::Weekly, 31)])),
        "{groups:?}"
    );
    assert_eq!(
        groups.get(1),
        Some(&(
            Some("model-f0".to_owned()),
            vec![(Window::FiveHour, 5), (Window::Weekly, 6)]
        )),
        "{groups:?}"
    );
}

/// The plan's own family and `models` more, each whole.
fn families(models: usize) -> Vec<(&'static str, String)> {
    let mut sent: Vec<(&'static str, String)> = (0..models)
        .flat_map(|family| {
            family_of(
                &format!("codex-f{family}"),
                &format!("model-f{family}"),
                "5",
                "6",
            )
        })
        .collect();
    sent.extend(family_of("codex", "codex", "12", "31"));
    sent
}

/// As many families as a reading keeps groups are all of them; one more is
/// a reading that says it left some out.
#[test]
fn rate_limit_one_family_past_the_ceiling_is_a_reading_said_to_be_incomplete() {
    for (models, incomplete) in [(MAX_LIMIT_GROUPS - 1, false), (MAX_LIMIT_GROUPS, true)] {
        let sent = families(models);
        let headers: Vec<(&'static str, &str)> = sent
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let read = limits(&headers).unwrap();
        assert_eq!(read.groups().count(), MAX_LIMIT_GROUPS, "{models}");
        assert_eq!(read.incomplete(), incomplete, "{models}");
    }
}

#[test]
fn rate_limit_headers_absent_report_no_windows() {
    assert_eq!(limits(&[]), None);
    assert_eq!(
        limits(&[("x-codex-credits-has-credits", "true")]),
        None,
        "a header that is no window's"
    );
}

#[test]
fn rate_limit_headers_malformed_report_no_windows() {
    let reset = RESET.to_string();
    for (used, minutes, reset) in [
        ("plenty", "300", reset.as_str()),
        ("NaN", "300", &reset),
        ("inf", "300", &reset),
        ("-1", "300", &reset),
        ("", "300", &reset),
        ("12", "0", &reset),
        ("12", "five hours", &reset),
        ("12", "-300", &reset),
        ("12", "300", "tomorrow"),
        ("12", "300", "-5"),
    ] {
        assert_eq!(
            limits(&[
                ("x-codex-primary-used-percent", used),
                ("x-codex-primary-window-minutes", minutes),
                ("x-codex-primary-reset-at", reset),
            ]),
            None,
            "{used:?} {minutes:?} {reset:?}"
        );
    }
    assert_eq!(
        limits(&[("x-codex-primary-used-percent", "12")]),
        None,
        "a window of no stated length"
    );
    assert_eq!(
        percents(limits(&[
            ("x-codex-primary-used-percent", "plenty"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-secondary-used-percent", "31"),
            ("x-codex-secondary-window-minutes", "10080"),
        ])),
        [(Window::Weekly, 31)],
        "one window malformed leaves the other"
    );
}

#[test]
fn rate_limit_a_window_with_no_reset_time_is_read_without_one() {
    let windows = limits(&[
        ("x-codex-primary-used-percent", "31"),
        ("x-codex-primary-window-minutes", "10080"),
    ])
    .expect("a window reported");
    assert_eq!(
        windows.reading(Window::Weekly),
        Some(WindowReading::new(31, None))
    );
}

#[test]
fn rate_limit_a_window_past_full_reads_as_full_and_a_fraction_never_rounds_up() {
    for (used, read) in [("150", 100), ("100", 100), ("99.9", 99), ("0", 0)] {
        assert_eq!(
            percents(limits(&[
                ("x-codex-primary-used-percent", used),
                ("x-codex-primary-window-minutes", "300"),
            ])),
            [(Window::FiveHour, read)],
            "{used}"
        );
    }
}

#[test]
fn rate_limit_headers_are_not_read_for_an_api_key_or_a_gateway() {
    let headers = [
        ("x-codex-primary-used-percent", "31"),
        ("x-codex-primary-window-minutes", "10080"),
    ];
    assert_eq!(limits_at(VENDOR, &headers), None);
    assert_eq!(
        limits_at(
            Endpoint::fixed("https://gateway.example/backend-api/codex/responses"),
            &headers
        ),
        None
    );
}

/// The plan backend's refusal of a used-up plan, as the Codex CLI reads it:
/// `error.type` and the reset, as a second or as seconds still to wait. The
/// message carries a marker no error may repeat.
fn used_up(reset: &str) -> String {
    format!(
        r#"{{"error":{{"type":"usage_limit_reached","message":"PRIVATE-MARKER limit","plan_type":"plus"{reset}}}}}"#
    )
}

/// What one refused request from `endpoint` for `model` fails with, and how
/// many requests reached the transport.
fn refused_at(
    endpoint: Endpoint,
    model: &'static str,
    status: u16,
    body: &str,
    headers: &[(&'static str, &str)],
) -> (ProviderError, Option<usize>) {
    let replay = headers
        .iter()
        .fold(Replay::new(status, body), |replay, (name, value)| {
            replay.answering(name, *value)
        });
    let replay = Arc::new(replay);
    let credential = HeaderKey::new(ApiKey::new("synthetic-limits-key"), Header::bearer());
    let provider = OpenAi::at(
        endpoint,
        Box::new(credential),
        Box::new(Arc::clone(&replay)),
    );
    let cancel = Cancel::new();
    let request = Request { model, ..asking() };
    let Err(error) = crucible_runtime::answered!(provider.stream(request, &cancel)) else {
        panic!("a refusal answered");
    };
    (error, replay.sent_count())
}

fn refused(body: &str) -> (ProviderError, Option<usize>) {
    refused_at(SUBSCRIPTION, "gpt-5.5", 429, body, &[])
}

/// What a failure says of a plan: whether it is a used-up one, and its reset.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    UsedUp(Option<SystemTime>),
    Not,
}

fn plan_reset(error: &ProviderError) -> Plan {
    match error {
        ProviderError::PlanLimit { resets_at, .. } => Plan::UsedUp(*resets_at),
        _ => Plan::Not,
    }
}

/// The reset of a used-up plan, where it was given.
fn reset_of(error: &ProviderError) -> Option<SystemTime> {
    match plan_reset(error) {
        Plan::UsedUp(reset) => reset,
        Plan::Not => None,
    }
}

#[test]
fn plan_limit_a_used_up_plan_with_its_reset_is_refused_once_and_never_again() {
    let (error, sent) = refused(&used_up(&format!(
        r#","resets_at":{RESET},"resets_in_seconds":3600"#
    )));

    assert_eq!(
        plan_reset(&error),
        Plan::UsedUp(Some(at(RESET))),
        "{error:?}"
    );
    assert!(!error.transient());
    assert_eq!(sent, Some(1));
    assert!(!error.to_string().contains("PRIVATE-MARKER"), "{error}");
    assert!(
        !format!("{error:?}").contains("PRIVATE-MARKER"),
        "{error:?}"
    );
}

#[test]
fn plan_limit_with_only_seconds_to_wait_resets_that_long_after_it_arrived() {
    let before = SystemTime::now();
    let (error, sent) = refused(&used_up(r#","resets_in_seconds":3600"#));
    let after = SystemTime::now();

    let reset = reset_of(&error);
    let hour = Duration::from_hours(1);
    assert!(
        reset.is_some_and(|reset| reset >= before + hour && reset <= after + hour),
        "{error:?}"
    );
    assert!(!error.transient());
    assert_eq!(sent, Some(1));
}

#[test]
fn plan_limit_with_a_malformed_reset_is_still_a_used_up_plan_with_none() {
    for reset in [
        r#","resets_at":"soon""#,
        r#","resets_at":-5"#,
        r#","resets_at":1.5"#,
        r#","resets_in_seconds":"an hour""#,
        r#","resets_in_seconds":-60"#,
        r#","resets_at":null,"resets_in_seconds":null"#,
        r#","resets_at":18446744073709551615"#,
    ] {
        let (error, sent) = refused(&used_up(reset));
        assert_eq!(plan_reset(&error), Plan::UsedUp(None), "{reset}: {error:?}");
        assert!(!error.transient(), "{reset}");
        assert_eq!(sent, Some(1), "{reset}");
    }
}

#[test]
fn plan_limit_with_no_reset_at_all_says_it_was_not_reported() {
    let (error, sent) = refused(&used_up(""));

    assert!(
        matches!(
            error,
            ProviderError::PlanLimit {
                window: None,
                resets_at: None,
                reading: None,
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(sent, Some(1));
}

#[test]
fn plan_limit_a_bad_reset_falls_back_to_the_seconds_still_to_wait() {
    let before = SystemTime::now();
    let (error, _) = refused(&used_up(r#","resets_at":"soon","resets_in_seconds":60"#));

    let reset = reset_of(&error);
    assert!(
        reset.is_some_and(|reset| reset >= before + Duration::from_mins(1)),
        "{error:?}"
    );
}

#[test]
fn plan_limit_names_the_window_the_refusals_own_head_reports_used_up() {
    let reset = RESET.to_string();
    let (error, _) = refused_at(
        SUBSCRIPTION,
        "gpt-5.5",
        429,
        &used_up(&format!(r#","resets_at":{RESET}"#)),
        &[
            ("x-codex-primary-used-percent", "40"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-primary-reset-at", "1792990000"),
            ("x-codex-secondary-used-percent", "100"),
            ("x-codex-secondary-window-minutes", "10080"),
            ("x-codex-secondary-reset-at", &reset),
        ],
    );

    let ProviderError::PlanLimit {
        window, reading, ..
    } = error
    else {
        panic!("not a used-up plan: {error:?}");
    };
    assert_eq!(window, Some(Window::Weekly));
    assert_eq!(
        percents(reading.map(|reading| *reading)),
        vec![(Window::FiveHour, 40), (Window::Weekly, 100)]
    );
}

#[test]
fn plan_limit_a_private_model_still_says_its_plan_is_used_up() {
    let (error, sent) = refused_at(
        SUBSCRIPTION,
        ASTRA,
        429,
        &used_up(&format!(r#","resets_at":{RESET}"#)),
        &[],
    );

    assert_eq!(
        plan_reset(&error),
        Plan::UsedUp(Some(at(RESET))),
        "{error:?}"
    );
    assert_eq!(sent, Some(1));
}

#[test]
fn plan_limit_a_rate_limit_or_an_overload_is_still_asked_again() {
    for kind in ["rate_limit_reached_error", "engine_overloaded_error"] {
        let body = format!(
            r#"{{"error":{{"type":"{kind}","message":"slow down","resets_in_seconds":20}}}}"#
        );
        let (error, _) = refused(&body);
        assert!(
            matches!(error, ProviderError::Refused { status: 429, .. }),
            "{kind}: {error:?}"
        );
        assert!(error.transient(), "{kind}");
    }
}

#[test]
fn plan_limit_is_not_read_from_a_refusal_that_is_not_the_plan_backends() {
    let body = used_up(&format!(r#","resets_at":{RESET}"#));
    for endpoint in [
        VENDOR,
        Endpoint::fixed("https://gateway.example/backend-api/codex/responses"),
    ] {
        let (error, _) = refused_at(endpoint, "gpt-5.5", 429, &body, &[]);
        assert!(
            matches!(error, ProviderError::Refused { status: 429, .. }),
            "{error:?}"
        );
    }
}

#[test]
fn plan_limit_a_body_that_is_not_json_stays_the_refusal_it_was() {
    let (error, _) = refused("usage_limit_reached");
    assert!(
        matches!(error, ProviderError::Refused { status: 429, .. }),
        "{error:?}"
    );
}

/// What the plan backend is asked with, and nothing else: a key no answer
/// may repeat.
const ASKING_KEY: &str = "synthetic-asking-key";

/// What asking the plan behind a credential on `endpoint` comes to, where it
/// is asked at all, when the backend answers `status` with `body`; and the
/// transport that answered.
async fn asked_at(endpoint: Endpoint, status: u16, body: &str) -> (Option<Asked>, Arc<Replay>) {
    let replay = Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(ApiKey::new(ASKING_KEY), Header::bearer());
    let provider = OpenAi::at(
        endpoint,
        Box::new(credential),
        Box::new(Arc::clone(&replay)),
    );
    let asked = match provider.ask_limits() {
        Some(asking) => Some(asking.await),
        None => None,
    };
    (asked, replay)
}

async fn asked(status: u16, body: &str) -> Asked {
    asked_at(SUBSCRIPTION, status, body)
        .await
        .0
        .expect("the plan backend is where a plan sign-in is asked")
}

fn answered(asked: Asked) -> PlanWindows {
    match asked {
        Asked::Answered(windows) => windows,
        other => panic!("not an answer: {other:?}"),
    }
}

/// One window as the plan backend's usage answer spells it.
fn wham_window(percent: u8, seconds: u64, reset: u64) -> String {
    format!(
        r#"{{"used_percent":{percent},"limit_window_seconds":{seconds},"reset_after_seconds":3600,"reset_at":{reset}}}"#
    )
}

/// A whole answer, with the fields this does not read beside the ones it does.
fn wham(rate_limit: &str, additional: &str) -> String {
    format!(
        r#"{{"plan_type":"plus","rate_limit":{rate_limit},"credits":{{"has_credits":false,"unlimited":false,"balance":"0"}},"spend_control":null{additional}}}"#
    )
}

#[tokio::test]
async fn plan_limit_asked_both_windows_of_the_plan_are_read() {
    let body = wham(
        &format!(
            r#"{{"allowed":true,"limit_reached":false,"primary_window":{},"secondary_window":{}}}"#,
            wham_window(12, 18_000, RESET),
            wham_window(31, 604_800, RESET + 86_400),
        ),
        "",
    );
    let (asked, replay) = asked_at(SUBSCRIPTION, 200, &body).await;
    let windows = answered(asked.expect("asked"));

    assert_eq!(
        grouped(Some(windows.clone())),
        [(None, vec![(Window::FiveHour, 12), (Window::Weekly, 31)])]
    );
    assert_eq!(
        windows.reading(Window::Weekly),
        Some(WindowReading::new(31, Some(at(RESET + 86_400))))
    );
    let sent = replay.sent();
    assert_eq!(sent.method, crate::transport::Method::Get);
    assert_eq!(sent.url, "https://chatgpt.com/backend-api/wham/usage");
    assert!(sent.body.is_empty());
    assert!(
        sent.headers
            .iter()
            .any(|(name, value)| name == "authorization" && value.contains(ASKING_KEY)),
        "the credential already given goes with it"
    );
    assert_eq!(replay.sent_count(), Some(1));
}

/// A plan with no five-hour window answers with its weekly one alone, in
/// whichever slot; the slot never names it.
#[tokio::test]
async fn plan_limit_asked_a_weekly_window_alone_is_the_only_one_read() {
    for (primary, secondary) in [
        (wham_window(31, 604_800, RESET), "null".to_owned()),
        ("null".to_owned(), wham_window(31, 604_800, RESET)),
    ] {
        let body = wham(
            &format!(r#"{{"primary_window":{primary},"secondary_window":{secondary}}}"#),
            "",
        );
        assert_eq!(
            grouped(Some(answered(asked(200, &body).await))),
            [(None, vec![(Window::Weekly, 31)])],
            "{body}"
        );
    }
}

/// An additional limit is a group named for its model; one with no limit of
/// its own is no group.
#[tokio::test]
async fn plan_limit_asked_an_additional_limit_is_a_group_named_for_its_model() {
    let body = wham(
        &format!(
            r#"{{"primary_window":null,"secondary_window":{}}}"#,
            wham_window(31, 604_800, RESET)
        ),
        &format!(
            r#","additional_rate_limits":[{{"limit_name":"gpt-5.3-codex-spark","metered_feature":"codex_bengalfox","rate_limit":{{"allowed":true,"limit_reached":false,"primary_window":{},"secondary_window":{}}}}},{{"limit_name":"unmetered","metered_feature":"codex_other"}}]"#,
            wham_window(12, 18_000, RESET),
            wham_window(4, 604_800, RESET),
        ),
    );

    assert_eq!(
        grouped(Some(answered(asked(200, &body).await))),
        [
            (None, vec![(Window::Weekly, 31)]),
            (
                Some("gpt-5.3-codex-spark".to_owned()),
                vec![(Window::FiveHour, 12), (Window::Weekly, 4)]
            ),
        ]
    );
}

/// An additional limit's `limit_name` is the slug of the model it limits: a
/// used-up window of it stops a request to that model and no other. One with
/// only a `metered_feature` is named by it and limits no model by name.
#[tokio::test]
async fn plan_limit_asked_a_used_up_model_limit_stops_its_model_and_no_other() {
    let before = at(RESET - 60);
    let spent = format!(
        r#"{{"primary_window":{},"secondary_window":null}}"#,
        wham_window(100, 18_000, RESET)
    );
    let plan = format!(
        r#"{{"primary_window":null,"secondary_window":{}}}"#,
        wham_window(31, 604_800, RESET)
    );

    let named = wham(
        &plan,
        &format!(
            r#","additional_rate_limits":[{{"limit_name":"gpt-5.3-codex-spark","metered_feature":"codex_bengalfox","rate_limit":{spent}}}]"#
        ),
    );
    let windows = answered(asked(200, &named).await);
    assert_eq!(
        windows.exhausted("gpt-5.3-codex-spark", before),
        Some((Window::FiveHour, at(RESET)))
    );
    for other in ["gpt-5.5", "GPT-5.3-Codex-Spark", "codex_bengalfox"] {
        assert_eq!(windows.exhausted(other, before), None, "{other}");
    }

    let metered = wham(
        &plan,
        &format!(
            r#","additional_rate_limits":[{{"metered_feature":"codex_bengalfox","rate_limit":{spent}}}]"#
        ),
    );
    let windows = answered(asked(200, &metered).await);
    assert_eq!(
        grouped(Some(windows.clone())),
        [
            (None, vec![(Window::Weekly, 31)]),
            (
                Some("codex_bengalfox".to_owned()),
                vec![(Window::FiveHour, 100)]
            ),
        ]
    );
    assert_eq!(windows.exhausted("codex_bengalfox", before), None);
}

/// As many additional limits as a reading keeps groups beside the plan's own
/// are all of them; one more is an answer that says it left some out.
#[tokio::test]
async fn plan_limit_asked_one_limit_past_the_ceiling_is_an_answer_said_to_be_incomplete() {
    let limit = |each: usize| {
        format!(
            r#"{{"limit_name":"model-{each}","metered_feature":"feature_{each}","rate_limit":{{"primary_window":{},"secondary_window":null}}}}"#,
            wham_window(5, 18_000, RESET)
        )
    };
    for (additional, incomplete) in [
        (MAX_LIMIT_GROUPS - 1, false),
        (MAX_LIMIT_GROUPS, true),
        (MAX_LIMIT_GROUPS + 3, true),
    ] {
        let limits: Vec<String> = (0..additional).map(limit).collect();
        let body = wham(
            &format!(
                r#"{{"primary_window":null,"secondary_window":{}}}"#,
                wham_window(31, 604_800, RESET)
            ),
            &format!(r#","additional_rate_limits":[{}]"#, limits.join(",")),
        );
        let read = answered(asked(200, &body).await);
        assert_eq!(read.groups().count(), MAX_LIMIT_GROUPS, "{additional}");
        assert_eq!(read.incomplete(), incomplete, "{additional}");
    }

    // With no limit of the plan's own, the ceiling is all models'.
    for (additional, incomplete) in [(MAX_LIMIT_GROUPS, false), (MAX_LIMIT_GROUPS + 1, true)] {
        let limits: Vec<String> = (0..additional).map(limit).collect();
        let body = wham(
            "null",
            &format!(r#","additional_rate_limits":[{}]"#, limits.join(",")),
        );
        let read = answered(asked(200, &body).await);
        assert_eq!(read.groups().count(), MAX_LIMIT_GROUPS, "{additional}");
        assert_eq!(read.incomplete(), incomplete, "no plan, {additional}");
    }
}

/// Nulls, and windows of no length, are windows not reported: the answer is
/// still an answer, of none.
#[tokio::test]
async fn plan_limit_asked_nulls_are_an_answer_of_no_windows() {
    for body in [
        wham("null", r#","additional_rate_limits":null"#),
        wham(r#"{"primary_window":null,"secondary_window":null}"#, ""),
        wham(
            &format!(
                r#"{{"primary_window":{},"secondary_window":null}}"#,
                wham_window(12, 0, RESET)
            ),
            r#","additional_rate_limits":[{"limit_name":"x","metered_feature":"x","rate_limit":null}]"#,
        ),
        "{}".to_owned(),
    ] {
        let windows = answered(asked(200, &body).await);
        assert!(windows.is_empty(), "{body}: {windows:?}");
    }
}

/// A window with no reset time is read with the seconds still to wait,
/// counted from when the answer arrived.
#[tokio::test]
async fn plan_limit_asked_with_only_seconds_to_wait_resets_that_long_after() {
    let before = SystemTime::now();
    let body = wham(
        r#"{"primary_window":{"used_percent":12,"limit_window_seconds":18000,"reset_after_seconds":3600}}"#,
        "",
    );
    let reset = answered(asked(200, &body).await)
        .reading(Window::FiveHour)
        .and_then(WindowReading::resets_at);
    assert!(
        reset.is_some_and(|reset| reset >= before + Duration::from_hours(1)),
        "{reset:?}"
    );
}

/// An answer that is not JSON, or not the backend's shape, leaves what was
/// known; a window whose figures are not numbers is left out of an answer.
#[tokio::test]
async fn plan_limit_asked_a_malformed_answer_is_a_failure_and_a_malformed_window_is_left_out() {
    for body in ["not json", "[1,2]", r#""usage""#, "", r#"{"rate_limit":"#] {
        let failed = asked(200, body).await;
        assert!(matches!(failed, Asked::Failed(_)), "{body}: {failed:?}");
    }
    let body = wham(
        &format!(
            r#"{{"primary_window":{{"used_percent":"a lot","limit_window_seconds":18000}},"secondary_window":{}}}"#,
            wham_window(31, 604_800, RESET)
        ),
        "",
    );
    assert_eq!(
        grouped(Some(answered(asked(200, &body).await))),
        [(None, vec![(Window::Weekly, 31)])]
    );
}

/// A refusal of the credential, or no source there, closes asking; anything
/// else leaves it open. Neither repeats the credential.
#[tokio::test]
async fn plan_limit_asked_a_401_closes_and_a_500_does_not() {
    for status in [401, 403, 404] {
        let closed = asked(status, ASKING_KEY).await;
        assert!(matches!(closed, Asked::Closed), "{status}: {closed:?}");
    }
    for status in [500, 429, 502] {
        let failed = asked(status, ASKING_KEY).await;
        assert!(matches!(failed, Asked::Failed(_)), "{status}: {failed:?}");
        assert!(!format!("{failed:?}").contains(ASKING_KEY), "{failed:?}");
    }
}

/// An API key, and a gateway at an address of its own, have no plan to ask.
#[tokio::test]
async fn plan_limit_asked_never_for_an_api_key_or_a_gateway() {
    for endpoint in [
        VENDOR,
        Endpoint::fixed("https://gateway.example/backend-api/codex/responses"),
    ] {
        let (asked, replay) = asked_at(endpoint, 200, "{}").await;
        assert!(asked.is_none());
        assert_eq!(replay.sent_count(), Some(0));
    }
}
