//! How an address a provider's requests go to is shown.
//!
//! Every request carries a key, so the address says who receives it, and a
//! reader of a diagnostic needs that much: the scheme, the host and the port.
//! The rest is not shown. User information is a password by convention, and a
//! gateway's path and query are commonly where a tenant identifier or a token
//! is put, so everything after the authority becomes one `/[redacted]`.
//!
//! Owned here because the two writers of an address, the provider refusing or
//! printing the one it sends to and the configuration printing the one that
//! was written, may not name each other.

#[cfg(test)]
mod tests;

/// The authority at the front of what follows a scheme: everything up to the
/// first `/`, `?` or `#`.
#[must_use]
pub fn authority(rest: &str) -> &str {
    rest.split(['/', '?', '#']).next().unwrap_or("")
}

/// `text` as a diagnostic may show it: the scheme and the authority without
/// any user information in it, and `/[redacted]` in place of whatever follows
/// the authority. Text with no `://` has no recipient to name and is not shown
/// at all.
#[must_use]
pub fn redacted(text: &str) -> String {
    let Some((scheme, rest)) = text.split_once("://") else {
        return "[redacted address]".to_owned();
    };
    let named = authority(rest);
    let authority = named.rsplit_once('@').map_or(named, |(_, host)| host);
    let mut shown = format!("{scheme}://{authority}");

    if rest.len() > named.len() {
        shown.push_str("/[redacted]");
    }
    shown
}
