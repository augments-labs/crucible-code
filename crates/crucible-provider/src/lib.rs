//! Provider implementations — one per LLM wire protocol.
//!
//! Depends on `crucible-core` alone. It must never reach `crucible-tools`,
//! `crucible-runner` or `crucible-tui`: a provider translates a turn into
//! requests and responses back into deltas, and knows nothing about what the
//! agent does with either.
//!
//! Each provider implements `Provider` from core and receives a resolved
//! `Credential`. Providers do not resolve credentials, read the environment or
//! touch the keyring — that happens once, during wiring, so that adding a new
//! way to authenticate does not edit a single provider.

mod anthropic;
mod endpoint;
mod json;
mod openai;
mod refusal;
mod sse;
mod stream;
mod transport;
mod unavailable;

pub use anthropic::Anthropic;
pub use endpoint::{Endpoint, EndpointError};
pub use openai::OpenAi;
pub use transport::http::Https;
pub use transport::{Response, Transport, TransportError};
pub use unavailable::Unavailable;
