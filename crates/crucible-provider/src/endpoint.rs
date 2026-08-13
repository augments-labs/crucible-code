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
}

/// An address a provider posts to.
///
/// Built by [`Endpoint::parse`] or by the constant each provider keeps, so a
/// value of this type is one that has been through the check above. There is no
/// way to make one from a string that has not.
/// Borrowed for the address a provider ships with and owned for one somebody
/// configured, so the ordinary run — nothing configured — allocates nothing and
/// the constant can be built in a `const`.
#[derive(Debug, Clone, PartialEq, Eq)]
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

        let host = if let Some(rest) = text.strip_prefix("https://") {
            rest
        } else if let Some(rest) = text.strip_prefix("http://") {
            if !loopback(rest) {
                return Err(EndpointError::Insecure(text.into()));
            }
            rest
        } else {
            return Err(EndpointError::Insecure(text.into()));
        };

        // A host ends at the first `/`, `:` or `?`. Empty means the scheme was
        // followed straight by the path, which is a URL with nowhere to go.
        if host.split(['/', ':', '?']).next().unwrap_or("").is_empty() {
            return Err(EndpointError::Hostless(text.into()));
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
        f.write_str(&self.0)
    }
}

/// Whether what follows `http://` is this machine talking to itself.
///
/// The three spellings the loader of a local server would use, and nothing
/// clever: a name that merely *resolves* to a loopback address is not one of
/// them, because what resolves it is DNS and what DNS answers can change
/// between this check and the request.
fn loopback(rest: &str) -> bool {
    let host = rest.split(['/', '?']).next().unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(before, _)| before);

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
    fn a_refusal_says_what_was_written() {
        // Somebody is looking at the file this came from, so the message has to
        // name the value they wrote rather than describe it.
        let problem = Endpoint::parse("http://gateway.example").expect_err("this to be refused");

        assert!(
            problem.to_string().contains("http://gateway.example"),
            "{problem}"
        );
    }
}
