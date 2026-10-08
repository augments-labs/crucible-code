//! How an address a provider's requests go to is shown.
//!
//! Every request carries a key, so the address says who receives it, and a
//! reader of a diagnostic needs that much: the scheme, the host and the port.
//! The rest is not shown. User information is a password by convention, and a
//! gateway's path and query are commonly where a tenant identifier or a token
//! is put, so everything after the authority becomes one `/[redacted]`.
//!
//! What is shown as the recipient is only what a URL parser would read as one.
//! An address spelled so that the two readings differ, where what is split off
//! here as a scheme or a host is a user or a path to the parser, is shown as
//! [`HIDDEN`] and nothing else.
//!
//! Owned here because the two writers of an address, the provider refusing or
//! printing the one it sends to and the configuration printing the one that
//! was written, may not name each other.

#[cfg(test)]
mod tests;

/// What is shown in place of an address that names no recipient a diagnostic
/// can repeat.
pub const HIDDEN: &str = "<redacted>";

/// The authority at the front of what follows a scheme: everything up to the
/// first `/`, `?` or `#`.
#[must_use]
pub fn authority(rest: &str) -> &str {
    rest.split(['/', '?', '#']).next().unwrap_or("")
}

/// `text` as a diagnostic may show it: the scheme and the authority without
/// any user information in it, and `/[redacted]` in place of whatever follows
/// the authority.
///
/// The authority is read from the first `://` to the first `/`, `?` or `#`,
/// and a URL parser starts it there too and ends it at one of those three or
/// sooner, so anything it reads as a user or a path is hidden with them. Two
/// spellings break that, and an address spelled either way is [`HIDDEN`]
/// whole, as is text with no `://` at all. A scheme that is not a plain one
/// holds whatever was written before the first `://`, a user or a path among
/// it. And a parser ends the authority at a `\`, drops a tab or a line break
/// wherever one is written, and refuses a space or another control, so an
/// authority holding any of them is not one it reads. A run of `/` after the
/// scheme, which a parser skips, leaves an empty authority here, and
/// everything after it is hidden.
#[must_use]
pub fn redacted(text: &str) -> String {
    let Some((scheme, rest)) = text.split_once("://") else {
        return HIDDEN.to_owned();
    };
    let named = authority(rest);
    if !plain(scheme) || named.contains(unread) {
        return HIDDEN.to_owned();
    }
    let authority = named.rsplit_once('@').map_or(named, |(_, host)| host);
    let mut shown = format!("{scheme}://{authority}");

    if rest.len() > named.len() {
        shown.push_str("/[redacted]");
    }
    shown
}

/// Whether `scheme` is one a URL parser reads as written: a letter, then
/// letters, digits, `+`, `-` and `.`.
fn plain(scheme: &str) -> bool {
    scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Whether `c` in an authority means a URL parser does not read it as
/// written.
fn unread(c: char) -> bool {
    c == '\\' || c.is_whitespace() || c.is_control()
}
