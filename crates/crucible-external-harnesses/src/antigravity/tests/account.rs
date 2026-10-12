//! A sign-in link is shown only when it is the sign-in page, and each
//! operation says no more than its steps established.

use crucible_execution::{Authentication, Failure};

use crate::Reading;
use crate::antigravity::Ended;
use crate::antigravity::account::{LINK_PREFIX, link};

/// A link as the agent prints it.
const SIGN_IN: &str = "https://accounts.google.com/o/oauth2/v2/auth?client_id=c&redirect_uri=http%3A%2F%2F127.0.0.1%3A40123%2F&response_type=code&state=s";

#[test]
fn the_sign_in_page_is_shown_as_the_agent_printed_it() {
    let shown = link(&format!("{LINK_PREFIX}{SIGN_IN}\r")).unwrap().unwrap();

    assert_eq!(shown.as_str(), SIGN_IN);
    assert!(link("Starting the server").is_none());
}

/// A link anywhere but the sign-in page, or one that could carry more than a
/// link, is refused rather than shown.
#[test]
fn a_link_that_is_not_the_sign_in_page_is_refused() {
    let long = format!("{SIGN_IN}&pad={}", "a".repeat(16 * 1024));
    for printed in [
        "http://accounts.google.com/o/oauth2/v2/auth?state=s",
        "https://accounts.google.com.example/o/oauth2/v2/auth?state=s",
        "https://accounts.google.com@example.com/o/oauth2/v2/auth?state=s",
        "https://example.com/?next=https://accounts.google.com/o/oauth2/v2/auth?",
        &format!("{SIGN_IN}#more"),
        &format!("{SIGN_IN} and more"),
        &format!("{SIGN_IN}\u{1b}[2J"),
        &long,
    ] {
        let refused = link(&format!("{LINK_PREFIX}{printed}"));
        assert!(matches!(refused, Some(Err(_))), "{printed}");
    }
}

#[test]
fn a_sign_in_the_status_after_it_confirms_says_signed_in() {
    let ended = Ended::signed_in(&Reading::Signed(Authentication::SignedIn));

    assert_eq!(ended.says(), "Signed in");
}

#[test]
fn a_sign_in_no_status_confirms_says_could_not_verify_sign_in() {
    for status in [
        Reading::Signed(Authentication::SignedOut),
        Reading::Unreadable(Failure::new("The agent did not answer in time.")),
    ] {
        assert_eq!(Ended::signed_in(&status).says(), "Could not verify sign-in");
    }
}

#[test]
fn a_logout_the_agent_confirms_says_signed_out_of_this_agent_profile() {
    let ended = Ended::signed_out(&Reading::Signed(Authentication::SignedOut));

    assert_eq!(ended.says(), "Signed out of this agent profile");
}

#[test]
fn a_sign_out_the_agent_cannot_confirm_says_external_sign_out_unverified() {
    let ended = Ended::signed_out(&Reading::Unreadable(Failure::new(
        "The agent offers no sign-out, so signing out is unverified.",
    )));

    assert_eq!(ended.says(), "Disconnected; external sign-out unverified");
}

#[test]
fn a_disconnect_says_the_account_remains_signed_in_only_when_that_was_known() {
    assert_eq!(
        Ended::disconnected(Authentication::SignedIn).says(),
        "Disconnected from Crucible. The external account remains signed in."
    );
    for known in [Authentication::Unknown, Authentication::SignedOut, Authentication::Unverifiable] {
        assert_eq!(
            Ended::disconnected(known).says(),
            "Disconnected from Crucible. External credentials were not changed."
        );
    }
}
