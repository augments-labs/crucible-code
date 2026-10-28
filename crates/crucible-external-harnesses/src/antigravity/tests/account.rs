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
        &format!("{SIGN_IN}\u{202e}gro.elgoog"),
        &format!("{SIGN_IN}&hint=\u{e9}"),
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

/// A logout is told as done only once a status step after it reads signed
/// out.
#[test]
fn a_logout_a_status_confirms_says_signed_out_of_this_agent_profile() {
    let out = Reading::Signed(Authentication::SignedOut);
    let ended = Ended::signed_out(&out, &out);

    assert_eq!(ended.says(), "Signed out of this agent profile");
}

/// A logout the agent refused, or one the status after it does not read as
/// signed out, is unverified, and keeps why.
#[test]
fn a_sign_out_nothing_confirms_says_external_sign_out_unverified() {
    let out = Reading::Signed(Authentication::SignedOut);
    let refused = Reading::Unreadable(Failure::new("The agent offers no sign-out."));
    for (logout, status) in [
        (&refused, &out),
        (&out, &Reading::Signed(Authentication::SignedIn)),
        (&out, &refused),
    ] {
        let ended = Ended::signed_out(logout, status);
        assert_eq!(ended.says(), "Disconnected; external sign-out unverified");
        assert!(matches!(ended, Ended::SignOutUnverified(_)), "{ended:?}");
    }
}

#[test]
fn a_disconnect_says_the_account_remains_signed_in_only_when_that_was_known() {
    assert_eq!(
        Ended::disconnected(Authentication::SignedIn).says(),
        "Disconnected from Crucible. The external account remains signed in."
    );
    for known in [
        Authentication::Unknown,
        Authentication::SignedOut,
        Authentication::Unverifiable,
    ] {
        assert_eq!(
            Ended::disconnected(known).says(),
            "Disconnected from Crucible. External credentials were not changed."
        );
    }
}

/// The four methods are offered in the agent's order: two it carries out once
/// asked, and two keys, each given in the variable the agent reads; a business
/// sign-in needs a project and location, which the agent reads from no
/// variable, and nothing offers the machine's own cloud credentials.
#[test]
fn the_four_methods_are_offered_with_the_variables_each_needs() {
    use crate::antigravity::{methods, needs};
    use crate::catalog::Method;

    let offered: Vec<(String, Option<String>)> = methods()
        .into_iter()
        .map(|method| {
            let var = match method.method {
                Method::EnvVar { var } => Some(var.to_string()),
                Method::Agent | Method::Terminal { .. } => None,
            };
            (method.id.to_string(), var)
        })
        .collect();

    assert_eq!(
        offered,
        [
            ("oauth-personal".to_owned(), None),
            ("oauth-business".to_owned(), None),
            (
                "gemini-api-key".to_owned(),
                Some("GEMINI_API_KEY".to_owned())
            ),
            (
                "agent-platform".to_owned(),
                Some("GOOGLE_API_KEY".to_owned())
            ),
        ]
    );
    assert_eq!(
        needs("oauth-business"),
        Some("a Google Cloud project and location for Gemini Enterprise")
    );
    for id in ["oauth-personal", "gemini-api-key", "agent-platform"] {
        assert!(needs(id).is_none(), "{id}");
    }
    assert!(
        offered
            .iter()
            .all(|(_, var)| var.as_deref() != Some("GOOGLE_APPLICATION_CREDENTIALS"))
    );
}
