//! Which proxy a request goes through, if any.
//!
//! The rules are the ones the previous client (ureq 3.4.2) applied, read from
//! its source rather than from any convention, because a user's proxy
//! settings were written against what it did:
//!
//! - The first of `ALL_PROXY`, `all_proxy`, `HTTPS_PROXY`, `https_proxy`,
//!   `HTTP_PROXY` and `http_proxy` that is set and parses serves every target,
//!   whatever its scheme. A value parses when it is a URI with an authority;
//!   no scheme means `http`; a scheme other than `http`, `https` or a SOCKS
//!   one does not parse, and the next variable is tried.
//! - The first of `NO_PROXY` and `no_proxy` that is set, even to nothing, is
//!   split on commas without trimming. An entry matches a target's host,
//!   ASCII case-insensitively and without its port: `*` matches everything;
//!   an entry that starts with `*` matches hosts ending in the rest of it,
//!   and one that starts with `.` hosts ending in the whole of it; one that
//!   ends with `*` matches hosts starting with the rest of it, and one that
//!   ends with `.` hosts starting with the whole of it; any other must be the
//!   whole host. There is no address-range matching.
//! - An `http://` or `https://` proxy is reached through `CONNECT`, with the
//!   user information of its address sent as a `Basic` credential. One whose
//!   address cannot be rebuilt without that user information, such as
//!   `http://[a@p]:1`, was chosen by the previous client and could not be
//!   connected to; it is chosen here too, and refused.
//! - The previous client was built without SOCKS: a `socks4://`, `socks://`
//!   or `socks5://` proxy, which it would have handed an address resolved
//!   here, was connected past, straight to the target; a `socks4a://` or
//!   `socks5h://` one, which was to resolve the target itself, left it no
//!   address, and the connection was refused.
//!
//! No platform proxy setting is read. [`ProxyEnv`] is read once, and choosing
//! for one target is a pure function of it, so every choice can be tested
//! against a table without touching the process environment.
//!
//! Here the credential is kept only in a header value marked sensitive, and
//! its encoding in the redactions of each request sent through the proxy
//! (so that text shown afterwards can have it taken out). The address a
//! tunnel is opened to keeps no user information, and the `Debug` of
//! [`ProxyEnv`] shows neither.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use crucible_credentials::Outgoing;
use hyper::Uri;
use hyper::header::{HeaderMap, HeaderValue, PROXY_AUTHORIZATION, USER_AGENT};
use hyper::http::uri::Scheme;

use crate::client::DEFAULT_USER_AGENT;

/// The variables a proxy is read from, in the order they are tried.
const PROXY_VARIABLES: [&str; 6] = [
    "ALL_PROXY",
    "all_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
];

/// The variables the hosts that go direct are read from, in order.
const NO_PROXY_VARIABLES: [&str; 2] = ["NO_PROXY", "no_proxy"];

/// The proxy settings of an environment, read once.
///
/// Its `Debug` says what kind of proxy was chosen and how many `NO_PROXY`
/// entries there are, and nothing of the proxy's address or credential.
#[derive(Clone)]
pub struct ProxyEnv {
    proxy: Option<Proxy>,
    no_proxy: Option<Vec<Bypass>>,
}

/// A proxy the environment names, by what the previous client did with it.
#[derive(Clone)]
enum Proxy {
    /// An `http://` or `https://` proxy, spoken to with `CONNECT`.
    Connect(ConnectProxy),
    /// A SOCKS proxy that would have been handed an address resolved here,
    /// and which the previous client connected past.
    Past,
    /// A SOCKS proxy that would have resolved the target, or an `http://` or
    /// `https://` one whose address cannot be rebuilt without its user
    /// information: either way the previous client made no connection.
    Refused,
}

/// A proxy spoken to with `CONNECT`, as a request would be sent to it.
///
/// Its fields are its own, set together by its parser, so an address and the
/// way its connection is spoken always agree.
#[derive(Clone)]
pub(crate) struct ConnectProxy {
    /// Scheme `http` or `https`, agreeing with `leg`; the host and any port
    /// as given, path `/`, and no user information.
    uri: Uri,
    /// How the connection to the proxy itself is spoken.
    leg: Leg,
    /// `Basic` and the encoded user information, when the address carries
    /// any; marked sensitive.
    authorization: Option<HeaderValue>,
}

/// How the connection to a proxy itself is spoken, fixed by its scheme when
/// it is read, so an `https://` proxy has no plaintext form.
#[derive(Clone)]
pub(crate) enum Leg {
    /// `http://`: plaintext, the `CONNECT` and its credential included.
    Plain,
    /// `https://`: TLS, verified for this host.
    Tls(String),
}

/// One `NO_PROXY` entry, lower-cased.
#[derive(Clone)]
enum Bypass {
    All,
    Exact(String),
    Prefix(String),
    Suffix(String),
}

/// How a connection to one target is made.
pub(crate) enum Route<'e> {
    Direct,
    Tunnel(&'e ConnectProxy),
    Refused,
}

impl ProxyEnv {
    /// The proxy settings of this process's environment, read now. A
    /// variable that is not valid Unicode counts as unset.
    #[must_use]
    pub fn capture() -> Self {
        Self::read(|name| std::env::var(name).ok())
    }

    /// The proxy settings `var` answers with, `None` being unset.
    pub(crate) fn read(var: impl Fn(&str) -> Option<String>) -> Self {
        let proxy = PROXY_VARIABLES
            .iter()
            .find_map(|name| var(name).and_then(|value| Proxy::parse(&value)));
        let no_proxy = NO_PROXY_VARIABLES
            .iter()
            .find_map(|name| var(name))
            .map(|list| list.split(',').map(Bypass::parse).collect());
        Self { proxy, no_proxy }
    }
}

impl Proxy {
    fn parse(value: &str) -> Option<Self> {
        let given: Uri = value.parse().ok()?;
        let authority = given.authority()?;
        let scheme = given.scheme_str().unwrap_or("http").to_ascii_lowercase();
        let secure = match scheme.as_str() {
            "http" => false,
            "https" => true,
            "socks4" | "socks" | "socks5" => return Some(Self::Past),
            "socks4a" | "socks5h" => return Some(Self::Refused),
            _ => return None,
        };
        let connect = ConnectProxy::parse(authority.as_str(), secure);
        Some(connect.map_or(Self::Refused, Self::Connect))
    }

    /// What kind of proxy this is, for `Debug`.
    fn kind(&self) -> &'static str {
        match self {
            Self::Connect(_) => "Connect",
            Self::Past => "Past",
            Self::Refused => "Refused",
        }
    }
}

impl ConnectProxy {
    /// The address a tunnel is opened to: scheme, host and port.
    pub(crate) fn uri(&self) -> &Uri {
        &self.uri
    }

    /// How the connection to the proxy itself is spoken.
    pub(crate) fn leg(&self) -> &Leg {
        &self.leg
    }

    /// The proxy at `authority`, over TLS when `secure`; `None` when its
    /// address cannot be rebuilt without its user information.
    fn parse(authority: &str, secure: bool) -> Option<Self> {
        // The user information ends at the last `@`. The previous client
        // split it again at its last `:` and sent `user:password`, with an
        // empty password when there was no `:`, which is the user
        // information itself whenever it has one.
        let (userinfo, place) = match authority.rsplit_once('@') {
            Some((userinfo, place)) => (Some(userinfo), place),
            None => (None, authority),
        };
        let uri = Uri::builder()
            .scheme(if secure { Scheme::HTTPS } else { Scheme::HTTP })
            .authority(place)
            .path_and_query("/")
            .build()
            .ok()?;
        let leg = if secure {
            Leg::Tls(uri.host()?.to_owned())
        } else {
            Leg::Plain
        };
        let authorization = match userinfo {
            Some(userinfo) => {
                let pair = if userinfo.contains(':') {
                    userinfo.to_owned()
                } else {
                    format!("{userinfo}:")
                };
                let basic = format!("Basic {}", STANDARD.encode(pair));
                let mut value = HeaderValue::try_from(basic).ok()?;
                value.set_sensitive(true);
                Some(value)
            }
            None => None,
        };
        Some(Self {
            uri,
            leg,
            authorization,
        })
    }

    /// The fields a `CONNECT` carries besides its target: the previous
    /// client's `user-agent` whatever the request's own, a request to keep
    /// the connection, and the credential.
    ///
    /// `hyper-util` writes them lower-case in the map's order, where the
    /// previous client wrote the same fields title-cased in this order.
    pub(crate) fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(DEFAULT_USER_AGENT));
        headers.insert("proxy-connection", HeaderValue::from_static("Keep-Alive"));
        if let Some(basic) = &self.authorization {
            headers.insert(PROXY_AUTHORIZATION, basic.clone());
        }
        headers
    }

    /// Registers the encoded credential this proxy is sent on `request`, so
    /// that text shown after a request made through it can have it taken out.
    pub(crate) fn protect(&self, request: &mut Outgoing) {
        let token = self
            .authorization
            .as_ref()
            .and_then(|basic| basic.to_str().ok())
            .and_then(|basic| basic.strip_prefix("Basic "));
        if let Some(token) = token {
            request.protect(token);
        }
    }
}

impl Bypass {
    fn parse(entry: &str) -> Self {
        let entry = entry.to_ascii_lowercase();
        if entry == "*" {
            Self::All
        } else if let Some(rest) = entry.strip_prefix('*') {
            Self::Suffix(rest.to_owned())
        } else if entry.starts_with('.') {
            Self::Suffix(entry)
        } else if let Some(rest) = entry.strip_suffix('*') {
            Self::Prefix(rest.to_owned())
        } else if entry.ends_with('.') {
            Self::Prefix(entry)
        } else {
            Self::Exact(entry)
        }
    }

    fn matches(&self, host: &str) -> bool {
        let host = host.as_bytes();
        match self {
            Self::All => true,
            Self::Exact(name) => host.eq_ignore_ascii_case(name.as_bytes()),
            Self::Prefix(start) => host
                .get(..start.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(start.as_bytes())),
            Self::Suffix(end) => host
                .len()
                .checked_sub(end.len())
                .and_then(|at| host.get(at..))
                .is_some_and(|tail| tail.eq_ignore_ascii_case(end.as_bytes())),
        }
    }
}

/// How a connection to `target` is made under `env`.
pub(crate) fn select<'e>(env: &'e ProxyEnv, target: &Uri) -> Route<'e> {
    let Some(proxy) = &env.proxy else {
        return Route::Direct;
    };
    let bypassed = env
        .no_proxy
        .as_ref()
        .zip(target.host())
        .is_some_and(|(list, host)| list.iter().any(|entry| entry.matches(host)));
    match proxy {
        _ if bypassed => Route::Direct,
        Proxy::Connect(to) => Route::Tunnel(to),
        Proxy::Past => Route::Direct,
        Proxy::Refused => Route::Refused,
    }
}

impl fmt::Debug for ProxyEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyEnv")
            .field("proxy", &self.proxy.as_ref().map(Proxy::kind))
            .field("bypasses", &self.no_proxy.as_ref().map(Vec::len))
            .finish()
    }
}

#[cfg(test)]
mod tests;
