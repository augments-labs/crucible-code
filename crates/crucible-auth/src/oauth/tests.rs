use super::*;

fn attempt() -> (
    LoginAttempt,
    mpsc::SyncSender<Result<LoginUpdate, OAuthError>>,
    mpsc::Receiver<Box<str>>,
) {
    let (updates, received) = mpsc::sync_channel(3);
    let (input, submitted) = mpsc::sync_channel(1);
    (
        LoginAttempt {
            updates: received,
            input,
            cancel: Cancel::new(),
        },
        updates,
        submitted,
    )
}

#[test]
fn method_names_are_owned_by_the_implementation_that_declares_them() {
    const DEVICE: LoginMethod = LoginMethod::new("device");

    assert_eq!(DEVICE.as_str(), "device");
    assert_eq!(format!("{DEVICE:?}"), "LoginMethod(\"device\")");
}

#[test]
fn authorization_uris_and_codes_are_redacted_from_debug_output() {
    let update = LoginUpdate::Authorize {
        browser_uri: "https://example.test/?secret=browser-canary".into(),
        shown_uri: "https://example.test/short-canary".into(),
        user_code: Some("code-canary".into()),
        manual: true,
    };

    let shown = format!("{update:?}");
    for canary in ["browser-canary", "short-canary", "code-canary"] {
        assert!(
            !shown.contains(canary),
            "authorization material reached Debug: {shown}"
        );
    }
}

#[test]
fn an_attempt_delivers_bounded_updates_without_busy_waiting() {
    let (attempt, updates, _) = attempt();
    updates.send(Ok(LoginUpdate::Complete)).unwrap();

    assert_eq!(
        attempt.wait(Duration::from_millis(10)).unwrap(),
        Some(LoginUpdate::Complete)
    );
    assert_eq!(attempt.wait(Duration::from_millis(1)).unwrap(), None);
}

#[test]
fn manual_input_is_trimmed_bounded_and_never_printed() {
    let (attempt, _, submitted) = attempt();

    attempt.submit("  authorization-canary  ").unwrap();
    assert_eq!(&*submitted.recv().unwrap(), "authorization-canary");
    assert!(matches!(
        attempt.submit("  "),
        Err(OAuthError::Invalid { .. })
    ));
    assert!(matches!(
        attempt.submit(&"x".repeat(16 * 1024 + 1)),
        Err(OAuthError::Invalid { .. })
    ));
    assert!(!format!("{attempt:?}").contains("authorization-canary"));
}

#[test]
fn dropping_an_attempt_requests_cancellation() {
    let (attempt, _, _) = attempt();
    let cancel = attempt.cancel.clone();

    drop(attempt);

    assert!(cancel.requested());
}
