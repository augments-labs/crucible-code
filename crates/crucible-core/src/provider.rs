//! What a provider is, from the runner's side.
//!
//! Providers are an open set: adding one must not edit this crate. A provider
//! translates a request into its vendor's wire shape and the response back
//! into [`Delta`]s, and knows nothing about what the agent does with either.
//!
//! Streaming is pull-based rather than callback-based. The runner owns the
//! stream on the provider thread and turns each delta into an event, which
//! keeps the render path free of provider code.

use std::fmt;

use crate::cancel::Cancel;
use crate::credential::CredentialError;
use crate::ids::ToolId;
use crate::transcript::{StopReason, Transcript};

/// Why a provider could not produce a response.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// The request never reached the provider, or the connection broke.
    #[error("{provider}: {problem}")]
    Transport {
        /// Which provider.
        provider: &'static str,
        /// What went wrong at the transport level.
        problem: Box<str>,
    },

    /// The provider answered with a status the request cannot recover from.
    ///
    /// Carries the status and the provider's own message, which is what tells
    /// a user that a model name is wrong or a key lacks access.
    #[error("{provider}: HTTP {status}: {message}")]
    Refused {
        /// Which provider.
        provider: &'static str,
        /// The HTTP status.
        status: u16,
        /// The provider's message.
        message: Box<str>,
    },

    /// The provider accepted the request and then reported a failure inside the
    /// response it was already streaming.
    ///
    /// Distinct from [`Self::Refused`] because there is no status to report: the
    /// response was a success and the failure arrived as content. Being
    /// overloaded is the usual reason, and it is the one worth retrying.
    #[error("{provider}: {kind}: {message}")]
    Upstream {
        /// Which provider.
        provider: &'static str,
        /// What the provider called the failure.
        kind: Box<str>,
        /// The provider's message.
        message: Box<str>,
    },

    /// The response did not match the shape this provider expects.
    #[error("{provider}: unexpected response: {problem}")]
    Protocol {
        /// Which provider.
        provider: &'static str,
        /// What did not fit.
        problem: Box<str>,
    },

    /// Authentication could not be applied.
    #[error("{provider}: {source}")]
    Credential {
        /// Which provider.
        provider: &'static str,
        /// The underlying failure.
        source: CredentialError,
    },

    /// The user cancelled mid-stream.
    #[error("{0}: cancelled")]
    Cancelled(&'static str),

    /// There is nothing set up to send the turn to.
    ///
    /// No provider is named, because that is the whole of the problem: no
    /// credential was found for any of them. It reaches the user as a failed
    /// turn rather than as a refusal to start, so that the session is still
    /// there to set one up in.
    #[error("{0}")]
    Unconfigured(Box<str>),
}

/// One turn's worth of input to a model.
#[derive(Debug, Clone)]
pub struct Request {
    /// Which model to ask.
    pub model: Box<str>,
    /// The transcript so far.
    pub transcript: Transcript,
    /// The tools the model may call, as JSON Schema.
    pub tools: Vec<ToolSchema>,
    /// Ceiling on the response length.
    pub max_tokens: u32,
    /// The system prompt, if the session has one.
    pub system: Option<Box<str>>,
}

/// A tool as advertised to a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSchema {
    /// The name the model calls.
    pub name: &'static str,
    /// The JSON Schema for the arguments.
    pub schema: &'static str,
}

/// One piece of streamed output.
///
/// A tool call arrives across several: the name comes first, then the arguments
/// spread over as many deltas as the provider chose to send, then a close. The
/// runner assembles them, because every provider splits them differently and
/// the assembly is the same either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta {
    /// Prose, to be shown as it arrives.
    Text(Box<str>),
    /// The model started asking for a tool.
    ToolStarted {
        /// The provider's identifier for this call.
        id: ToolId,
        /// Which tool.
        name: Box<str>,
    },
    /// More of the current tool call's argument JSON.
    ToolArgs(Box<str>),
    /// The model stopped, and why.
    Stopped(StopReason),
}

/// A stream of deltas from one request.
///
/// `next` blocks on the socket, so it runs on the provider thread and never on
/// the render path.
pub trait DeltaStream: Send {
    /// The next delta, or `None` when the stream is finished.
    fn next(&mut self) -> Option<Result<Delta, ProviderError>>;
}

/// One LLM backend adapter.
pub trait Provider: Send + Sync {
    /// The provider's name, for errors and for the status line.
    fn name(&self) -> &'static str;

    /// Starts a request and returns its stream of deltas.
    ///
    /// # Errors
    ///
    /// [`ProviderError`] if the request could not be sent or was refused. A
    /// failure part-way through the response arrives through the stream
    /// instead.
    fn stream(
        &self,
        request: Request,
        cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError>;
}

impl fmt::Debug for dyn Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Provider({})", self.name())
    }
}

impl fmt::Debug for dyn DeltaStream {
    /// Nothing to show. A stream is a socket part-way through a response, and
    /// the only way to describe its contents is to consume them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeltaStream")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_carries_what_the_provider_said() {
        let err = ProviderError::Refused {
            provider: "anthropic",
            status: 404,
            message: "model: claude-nope not found".into(),
        };
        assert_eq!(
            err.to_string(),
            "anthropic: HTTP 404: model: claude-nope not found"
        );
    }

    #[test]
    fn a_credential_error_does_not_carry_the_secret_forward() {
        let err = ProviderError::Credential {
            provider: "anthropic",
            source: CredentialError::NotInEnvironment("ANTHROPIC_API_KEY".into()),
        };
        let shown = err.to_string();
        assert_eq!(shown, "anthropic: ANTHROPIC_API_KEY is not set");
        assert!(
            shown.contains("ANTHROPIC_API_KEY"),
            "the variable name helps"
        );
    }
}
