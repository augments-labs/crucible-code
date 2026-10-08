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
        ("https://[::1]", "https://[::1]"),
        (
            "https://[2001:db8::7]/v1",
            "https://[2001:db8::7]/[redacted]",
        ),
        (
            "https://host.example:8443/v1",
            "https://host.example:8443/[redacted]",
        ),
        (
            "https://host.example:/v1",
            "https://host.example:/[redacted]",
        ),
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
fn an_authority_that_is_not_a_host_and_a_numeric_port_is_not_shown() {
    // With the host left out, what is left of the user information is read
    // here as a host and a port, so a port that is not a number, a second
    // `:`, or anything but a port after a bracketed literal is not shown.
    for written in [
        "https://user:pa55word-fake",
        "https://user:pa55word-fake/v1",
        "https://user:pa55word-fake?q=1",
        "https://us%40x:pa55word-fake",
        "https://user:pa55word-fake:443/v1",
        "https://a@user:pa55word-fake/v1",
        "https://host.example:8443x/v1",
        "https://[::1]pa55word-fake/v1",
        "https://[::1]:pa55word-fake/v1",
        "https://[::1]:8080:pa55word-fake",
        "https://[user:pa55word-fake]/v1",
        "https://[::1/v1",
    ] {
        assert_eq!(redacted(written), HIDDEN, "{written:?}");
    }
}

#[test]
fn a_percent_encoded_host_is_not_shown() {
    // A URL parser decodes a host before reading it, so a host spelled with
    // a `%` is not the one it reads, and encoded user information is spelled
    // this way.
    for written in [
        "https://user%3Apa55word-fake",
        "https://user%3Apa55word-fake/v1",
        "https://host%2Eexample:8443/v1",
    ] {
        assert_eq!(redacted(written), HIDDEN, "{written:?}");
    }
}

#[test]
fn text_with_no_recipient_is_not_shown() {
    assert_eq!(redacted("gateway.example/v1"), HIDDEN);
}
