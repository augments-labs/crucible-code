//! An HTTP client built for crucible's outgoing requests to share, and the
//! policy a request sent through it is sent under. Nothing uses it yet.
//!
//! [`Http`] is a pooled HTTP/1.1 client. What it will and will not do is fixed
//! here rather than by each caller, because each of these is a promise about
//! where a credential can go:
//!
//! - **TLS** is rustls over the compiled-in Mozilla roots ([`Tls`]), TLS 1.2 and
//!   1.3, with no platform verifier, no system store, no client certificate and
//!   no ALPN, so HTTP/1.1 is all that is ever spoken. Plaintext is spoken only
//!   to an `http` target or an `http://` proxy; any other scheme, or a URL
//!   with no host, is refused before it is dialled.
//! - **Redirects are answers.** A 3xx comes back as the response it is and is
//!   never followed: a redirect chooses a new recipient, and a header such as
//!   `x-api-key` is not one a client knows to strip on the way there. Every
//!   other status comes back the same way.
//! - **A proxy** is chosen from the environment by the rules the previous
//!   client applied ([`ProxyEnv`]). An `http://` or `https://` one is
//!   reached through `CONNECT`; a `socks4://`, `socks://` or `socks5://` one
//!   is connected past, straight to the target; a `socks4a://` or
//!   `socks5h://` one, or an `http://` or `https://` one whose address
//!   cannot be rebuilt without its user information, is refused. A
//!   credential is sent only to the proxy and registered for redaction on
//!   the request's headers.
//! - **A hostname lookup** runs on a bounded blocking worker ([`Lookups`]).
//!   A poisoned owner looks up under a 5 s deadline and a [`Poison`]:
//!   one lookup that outlives it fails every later one at once, because the
//!   platform call cannot be cancelled and a stuck resolver would otherwise
//!   collect workers. A plain owner has neither, and a proxy's host is looked
//!   up only with one ([`PlainLookups`]).
//! - **Deadlines**: 15 s to connect, then a minute each for the request head,
//!   its body and the response head ([`Phase`]). hyper hands the head over
//!   before writing it, so its write falls inside the body's or the answer's
//!   minute: about two minutes before the response head at worst, where the
//!   previous client gave three. The body of a response has no clock here.
//! - **The response head** is refused once 64 KiB of it is buffered without
//!   its end, or when it has more than 128 fields. A head that ends inside the
//!   read crossing 64 KiB still parses, so the largest accepted is under
//!   128 KiB, not exactly 64 KiB as before.
//! - **Nothing is logged here.** This crate installs no logger or subscriber
//!   and writes nothing to a terminal. hyper-util emits `tracing` events that
//!   name hosts and addresses; they go nowhere only while nothing in the
//!   process installs a subscriber.
//!
//! Nothing here builds a runtime, and the environment is read only by
//! [`ProxyEnv::capture`]: whoever builds a client is to hand it the TLS
//! configuration, a lookup owner for its targets and a plain one for proxy
//! hosts, the proxy settings and, for poisoned lookups, the poison. The tasks
//! hyper-util spawns for a client are owned by it and aborted when it is
//! dropped; a lookup already on a blocking worker is not a task of the
//! client's, and runs until the platform answers, holding its owner's permit.

mod client;
mod connect;
mod dns;
mod proxy;
mod tasks;

pub use client::{DEFAULT_USER_AGENT, Http, HttpError, Phase};
pub use connect::{ConnectError, Tls};
pub use dns::{LookupError, Lookups, PlainLookups, Poison};
pub use proxy::ProxyEnv;
