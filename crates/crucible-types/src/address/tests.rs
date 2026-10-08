//! What a diagnostic shows of an address.

use super::{HIDDEN, redacted};

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
    assert_eq!(
        redacted("https:////user:pa55word@host.example"),
        "https:///[redacted]"
    );
    assert_eq!(
        redacted("https://a:b@c:d@host.example:8443/v1?q=1"),
        "https://host.example:8443/[redacted]"
    );
    assert_eq!(
        redacted("https://host.example/v1/@path"),
        "https://host.example/[redacted]"
    );
}

#[test]
fn an_address_spelled_so_its_authority_cannot_be_read_is_not_shown() {
    // Nothing may be shown as a recipient that a URL parser would read as a
    // user or a path, so an address with no plain `scheme://` in front, or
    // whose authority holds a `\`, a space or a control, is not shown at all.
    for written in [
        "https:user:pa55word@host.example/v1",
        "user:pa55word@host.example",
        "https:user:pa55word@host.example/x://y",
        "https:tenant-fake/x://y",
        "://tenant-fake",
        "1https://tenant-fake",
        "localhost:8080/v1?api-key=not-a-real-key",
        "https://host.example\\tenant-fake",
        "https://\\/user:pa55word@host.example",
        "https://\t/user:pa55word@host.example/v1",
        "https://\n/user:pa55word@host.example/v1",
        "https://host.example /tenant-fake",
    ] {
        assert_eq!(redacted(written), HIDDEN, "{written:?}");
    }
}

#[test]
fn text_with_no_recipient_is_not_shown() {
    assert_eq!(redacted("gateway.example/v1"), HIDDEN);
}
