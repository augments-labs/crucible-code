//! What a diagnostic shows of an address.

use super::redacted;

#[test]
fn the_recipient_is_shown_and_nothing_after_it() {
    for (written, shown) in [
        (
            "https://gateway.example/v1/messages",
            "https://gateway.example/[redacted]",
        ),
        ("https://gateway.example", "https://gateway.example"),
        (
            "https://gateway.example:8443?key=not-a-real-key",
            "https://gateway.example:8443/[redacted]",
        ),
        ("http://127.0.0.1:9/v1", "http://127.0.0.1:9/[redacted]"),
        ("https://[::1]:8080/x", "https://[::1]:8080/[redacted]"),
        (
            "https://host.example#fragment",
            "https://host.example/[redacted]",
        ),
    ] {
        assert_eq!(redacted(written), shown, "{written}");
    }
}

#[test]
fn user_information_is_never_shown() {
    assert_eq!(
        redacted("https://user:pa55word@host.example/v1"),
        "https://host.example/[redacted]"
    );
    // A user a URL parser would read after a run of slashes is in what
    // follows the empty authority, so it goes with the path.
    assert_eq!(
        redacted("https:///user:pa55word@host.example/v1"),
        "https:///[redacted]"
    );
}

#[test]
fn text_with_no_recipient_is_not_shown() {
    assert_eq!(redacted("gateway.example/v1"), "[redacted address]");
}
