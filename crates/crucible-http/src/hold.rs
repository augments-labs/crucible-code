//! What may keep a request from leaving, and where a request is addressed as
//! that is asked.
//!
//! A client is handed at most one [`Hold`] by whoever builds it, and asks it
//! about every request before anything is dialled, looked up or written. This
//! crate names no vendor and no route: which addresses wait, and on what, is
//! the policy's, and the policy belongs to whoever knows what a route is.
//!
//! An [`Origin`] is the one form an address is compared in: the scheme and the
//! host in lower case, a host's trailing dot taken off, and the port written
//! out, the default one included. Two spellings of one place are one origin,
//! so a policy that holds an origin cannot be walked around by writing it
//! another way.

use std::fmt;

use hyper::Uri;

/// Where a request goes: its scheme, host and port.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Origin {
    scheme: Box<str>,
    host: Box<str>,
    port: u16,
}

impl Origin {
    /// The origin `url` is addressed to, and `None` for anything that is not
    /// an `http` or `https` URL naming a host.
    #[must_use]
    pub fn of(url: &str) -> Option<Self> {
        Self::from_uri(&url.parse::<Uri>().ok()?)
    }

    /// The origin of a URL already parsed.
    pub(crate) fn from_uri(uri: &Uri) -> Option<Self> {
        let scheme = uri.scheme_str()?.to_ascii_lowercase();
        let default = match scheme.as_str() {
            "http" => 80,
            "https" => 443,
            _ => return None,
        };
        let host = uri.host()?.to_ascii_lowercase();
        let host = host.strip_suffix('.').unwrap_or(&host);
        if host.is_empty() {
            return None;
        }
        Some(Self {
            scheme: scheme.into(),
            host: host.into(),
            port: uri.port_u16().unwrap_or(default),
        })
    }

    /// `http` or `https`.
    #[must_use]
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// The host, in lower case and with no trailing dot.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port, the scheme's default where the URL named none.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let default = matches!((&*self.scheme, self.port), ("http", 80) | ("https", 443));
        if default {
            write!(out, "{}://{}", self.scheme, self.host)
        } else {
            write!(out, "{}://{}:{}", self.scheme, self.host, self.port)
        }
    }
}

/// A policy asked, before each request leaves, whether it has to wait.
///
/// Asked on the task sending the request and answered at once: it reads what
/// it holds and does no work of its own.
pub trait Hold: Send + Sync + fmt::Debug {
    /// The name of what a request to `origin` waits on, where it waits, and
    /// `None` where it may leave.
    fn held(&self, origin: &Origin) -> Option<Box<str>>;
}

#[cfg(test)]
mod tests;
