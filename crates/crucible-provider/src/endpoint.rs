//! Where a provider's requests go.
//!
//! Each provider has one it uses unless told otherwise, and what tells it
//! otherwise is a setting — so this is a value parsed from text somebody wrote,
//! and it is parsed once, here, into a type the providers can only be built
//! with. A provider never sees the string.
//!
//! What the parse is *for* is the credential. Every request carries the key in
//! a header, so the address decides who receives it: an endpoint that is not
//! the vendor's is a key handed to whoever answers, and one reached over plain
//! HTTP is a key on the wire in the clear. The first of those is the user's
//! call to make — a gateway or a proxy is the whole reason to set this — and
//! the second is not, so the scheme is checked rather than trusted.
//!
//! `http` is accepted for a loopback address alone. That is not a softening of
//! the rule but the same rule: those bytes reach no network, so there is no
//! wire for them to be in the clear on. It is also what makes a local server a
//! thing crucible can be pointed at, which is what the whole-screen tests need
//! to drive a turn.
//!
//! Where the setting may be written is decided a layer up, in
//! `crucible-config`: `.crucible/config.json` travels with a checkout and may
//! not say this, because a repository that could would be a repository that
//! reads the key of everyone who clones it.

use std::borrow::Cow;
use std::fmt;

use thiserror::Error;

/// Why an address was refused.
#[derive(Debug, Error)]
pub enum EndpointError {
    /// Neither `https` nor a loopback `http`.
    #[error(
        "{0} is not an address crucible will send a key to: it must be https, \
         or http on localhost"
    )]
    Insecure(Box<str>),

    /// No host at all — `https:///v1/messages`, or a bare path.
    #[error("{0} names no host to send to")]
    Hostless(Box<str>),

    /// User information changes which part of an authority is the host, and a
    /// fragment has no place in an HTTP request target. Neither is accepted or
    /// repeated, because each is also a conventional place to put a secret.
    #[error("the provider address contains user information or a fragment")]
    Unsafe,
}

/// An address a provider posts to.
///
/// Built by [`Endpoint::parse`] or by the constant each provider keeps, so a
/// value of this type is one that has been through the check above. There is no
/// way to make one from a string that has not.
/// Borrowed for the address a provider ships with and owned for one somebody
/// configured, so the ordinary run — nothing configured — allocates nothing and
/// the constant can be built in a `const`.
#[derive(Clone, PartialEq, Eq)]
pub struct Endpoint(Cow<'static, str>);

impl Endpoint {
    /// The address as a provider sends to it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Reads an address somebody configured.
    ///
    /// # Errors
    ///
    /// [`EndpointError`] when the scheme would put a key somewhere it must not
    /// go, or when there is no host to send to.
    pub fn parse(text: &str) -> Result<Self, EndpointError> {
        let text = text.trim_end_matches('/');

        if text.contains('#') || user_information(text) {
            return Err(EndpointError::Unsafe);
        }

        let rest = if let Some(rest) = text.strip_prefix("https://") {
            rest
        } else if let Some(rest) = text.strip_prefix("http://") {
            rest
        } else {
            return Err(EndpointError::Insecure(redacted(text).into()));
        };

        let authority = authority(rest);
        let Some(host) = host(authority) else {
            return Err(EndpointError::Hostless(redacted(text).into()));
        };

        if text.starts_with("http://") && !loopback(host) {
            return Err(EndpointError::Insecure(redacted(text).into()));
        }

        Ok(Self(Cow::Owned(text.to_owned())))
    }

    /// The address a provider ships with, which has no parse to fail.
    ///
    /// `const` so each provider's default is a constant rather than something
    /// built at startup, and private to this crate so the only address reaching
    /// it is one written in this repository. Everything from outside arrives
    /// through [`Endpoint::parse`] and is checked.
    pub(crate) const fn fixed(url: &'static str) -> Self {
        Self(Cow::Borrowed(url))
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&redacted(&self.0))
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Endpoint").field(&redacted(&self.0)).finish()
    }
}

/// The authority at the front of what follows a scheme.
fn authority(rest: &str) -> &str {
    rest.split(['/', '?', '#']).next().unwrap_or("")
}

/// The host inside an authority, if it is unambiguous and well formed.
fn host(authority: &str) -> Option<&str> {
    if authority.is_empty()
        || authority.contains(['@', '\\'])
        || authority.chars().any(char::is_whitespace)
    {
        return None;
    }

    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let host = authority.get(..=end)?;
        let after = authority.get(end + 1..)?;
        return bracket_port(after).then_some(host);
    }

    let (host, port) = authority
        .split_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    let port = port.is_none_or(|port| {
        !port.is_empty() && !port.contains(':') && port.bytes().all(|byte| byte.is_ascii_digit())
    });

    (!host.is_empty() && port).then_some(host)
}

/// Whether the part after a bracketed host is absent or a numeric port.
fn bracket_port(after: &str) -> bool {
    after.is_empty()
        || after
            .strip_prefix(':')
            .is_some_and(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Whether an address carries credentials before its host.
fn user_information(text: &str) -> bool {
    text.split_once("://")
        .is_some_and(|(_, rest)| authority(rest).contains('@'))
}

/// An address safe to put in diagnostics.
fn redacted(text: &str) -> String {
    let Some((scheme, rest)) = text.split_once("://") else {
        return "[redacted address]".to_owned();
    };
    let named = authority(rest);
    let authority = named.rsplit_once('@').map_or(named, |(_, host)| host);
    let mut shown = format!("{scheme}://{authority}");

    // Gateway paths are commonly tenant- or token-bearing, just as queries
    // are. Diagnostics therefore show the recipient but no request target.
    if rest.len() > named.len() {
        shown.push_str("/[redacted]");
    }
    shown
}

/// Whether a parsed host is this machine talking to itself.
///
/// The three spellings the loader of a local server would use, and nothing
/// clever: a name that merely *resolves* to a loopback address is not one of
/// them, because what resolves it is DNS and what DNS answers can change
/// between this check and the request.
fn loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_https_address_is_taken_as_written() {
        let endpoint = Endpoint::parse("https://gateway.example/v1/messages")
            .expect("an https address to be accepted");

        assert_eq!(endpoint.as_str(), "https://gateway.example/v1/messages");
    }

    #[test]
    fn a_trailing_slash_is_not_part_of_the_address() {
        // Two spellings of one address must not become two addresses: a request
        // built by joining a path onto this would otherwise carry `//`.
        let endpoint = Endpoint::parse("https://gateway.example/v1/").expect("this to be accepted");

        assert_eq!(endpoint.as_str(), "https://gateway.example/v1");
    }

    #[test]
    fn plain_http_to_somewhere_else_is_refused() {
        // The refusal that matters: every request carries the key in a header,
        // so this is the key on somebody's network in the clear.
        let problem = Endpoint::parse("http://gateway.example/v1/messages")
            .expect_err("plain http to be refused");

        assert!(matches!(problem, EndpointError::Insecure(_)), "{problem:?}");
    }

    #[test]
    fn plain_http_to_this_machine_is_allowed() {
        for address in [
            "http://localhost:8080/v1/messages",
            "http://127.0.0.1:8080/v1",
            "http://[::1]:8080/v1",
        ] {
            assert!(
                Endpoint::parse(address).is_ok(),
                "{address} to be accepted: it reaches no network"
            );
        }
    }

    #[test]
    fn a_name_that_merely_looks_local_is_refused() {
        // `localhost.evil.example` starts with the string and is not this
        // machine. Matching the host exactly is what keeps that true.
        let problem = Endpoint::parse("http://localhost.evil.example/v1")
            .expect_err("a longer name beginning with localhost to be refused");

        assert!(matches!(problem, EndpointError::Insecure(_)), "{problem:?}");
    }

    #[test]
    fn user_information_cannot_disguise_a_remote_host_as_loopback() {
        for address in [
            "http://localhost:8080@evil.example/v1",
            "http://127.0.0.1:80@evil.example/v1",
            "http://[::1]@evil.example/v1",
        ] {
            let problem = Endpoint::parse(address).expect_err("user information to be refused");
            assert!(matches!(problem, EndpointError::Unsafe), "{problem:?}");
        }
    }

    #[test]
    fn secrets_in_an_address_never_reach_diagnostics() {
        let accepted =
            Endpoint::parse("https://gateway.example/tenant-hunter2/v1?token=query-hunter2")
                .expect("an https address with a query");
        for canary in ["tenant-hunter2", "query-hunter2"] {
            assert!(!accepted.to_string().contains(canary));
            assert!(!format!("{accepted:?}").contains(canary));
        }
        assert_eq!(accepted.to_string(), "https://gateway.example/[redacted]");

        for address in [
            "http://person:hunter2@evil.example/v1",
            "https://gateway.example/v1#hunter2",
            "ftp://gateway.example/v1?token=hunter2",
        ] {
            let problem = Endpoint::parse(address).expect_err("an unsafe address");
            assert!(!problem.to_string().contains("hunter2"), "{problem}");
            assert!(!format!("{problem:?}").contains("hunter2"));
        }
    }

    #[test]
    fn a_scheme_crucible_does_not_speak_is_refused() {
        for address in ["ftp://gateway.example", "file:///etc/passwd", "gateway"] {
            assert!(
                Endpoint::parse(address).is_err(),
                "{address} to be refused: it is not a way to reach a provider"
            );
        }
    }

    #[test]
    fn an_address_with_no_host_is_refused() {
        let problem =
            Endpoint::parse("https:///v1/messages").expect_err("an address with no host to fail");

        assert!(matches!(problem, EndpointError::Hostless(_)), "{problem:?}");
    }

    #[test]
    fn a_refusal_names_the_recipient_without_repeating_the_target() {
        let problem = Endpoint::parse("http://gateway.example/tenant-secret")
            .expect_err("this to be refused");

        assert!(
            problem.to_string().contains("http://gateway.example"),
            "{problem}"
        );
        assert!(!problem.to_string().contains("tenant-secret"), "{problem}");
    }
}
