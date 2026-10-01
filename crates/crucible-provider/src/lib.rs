//! Provider implementations — one per LLM wire protocol.
//!
//! Written against `crucible-models`, which owns the contract, and the shared
//! values beneath it. It must never reach `crucible-builtins`,
//! `crucible-runner` or `crucible-tui`: a provider translates a turn into
//! requests and responses back into deltas, and knows nothing about what the
//! agent does with either. Each vendor's capability tables, prices and wire
//! fields live beside its adapter here, never in the contract.
//!
//! Each provider implements `Provider` from `crucible-models` and receives a
//! resolved `Credential`. Providers do not resolve credentials, read the environment or
//! touch the keyring — that happens once, during wiring, so that adding a new
//! way to authenticate does not edit a single provider.

mod anthropic;
mod completions;
mod deepseek;
mod endpoint;
#[cfg(test)]
mod fake;
mod google;
mod history;
mod json;
mod meta;
mod mimo;
mod minimax;
mod moonshot;
mod openai;
mod qwen;
mod refusal;
mod responses;
mod sse;
mod stream;
mod transport;
mod unavailable;
mod web;
mod xai;
mod zai;

#[cfg(test)]
mod dialects_tests;
#[cfg(test)]
mod renewing_tests;

pub use anthropic::Anthropic;
pub use deepseek::DeepSeek;
pub use endpoint::{Endpoint, EndpointError};
pub use google::Google;
pub use meta::Meta;
pub use mimo::Mimo;
pub use minimax::MiniMax;
pub use moonshot::Moonshot;
pub use openai::OpenAi;
pub use qwen::Qwen;
pub use transport::http::HttpTurns;
pub use transport::{PostResponse, Transport, TransportError};
pub use unavailable::Unavailable;
pub use web::{AnthropicWeb, GoogleWeb, MetaWeb, MoonshotWeb, OpenAiWeb, XaiWeb};
pub use xai::Xai;
pub use zai::Zai;
