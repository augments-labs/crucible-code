//! The plan backend's usage windows, read off the head of a response.
//!
//! The header names are shaped from the strings of the Codex CLI's binary, the
//! program the backend serves, rather than from a captured response.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::RequestPurpose;
use crucible_types::{Message, PlanWindows, Transcript, Window, WindowReading};

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
                .reported()
                .map(|(window, reading)| (window, reading.percent()))
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
        ("12", "60", &reset),
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
