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
