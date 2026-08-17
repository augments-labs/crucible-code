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
use crate::credential::{CredentialError, Redactions};
use crate::ids::ToolId;
use crate::transcript::{StopReason, Transcript};

/// Why a provider could not produce a response.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// A provider response exceeded a retained-resource bound.
    #[error("{provider}: {limit} exceeded its limit of {maximum}")]
    Limit {
        /// Which provider produced the unbounded response.
        provider: &'static str,
        /// What grew too far.
        limit: ProviderLimit,
        /// The enforced maximum.
        maximum: usize,
    },

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

impl ProviderError {
    /// Removes request credentials from provider-controlled diagnostic text.
    ///
    /// Rebuilding the same variant preserves the typed failure while filtering
    /// every string field that could have crossed the provider boundary.
    #[must_use]
    pub fn redacted(self, redactions: &Redactions) -> Self {
        match self {
            Self::Limit { .. } | Self::Cancelled(_) => self,
            Self::Transport { provider, problem } => Self::Transport {
                provider,
                problem: redactions.redact(&problem).into(),
            },
            Self::Refused {
                provider,
                status,
                message,
            } => Self::Refused {
                provider,
                status,
                message: redactions.redact(&message).into(),
            },
            Self::Upstream {
                provider,
                kind,
                message,
            } => Self::Upstream {
                provider,
                kind: redactions.redact(&kind).into(),
                message: redactions.redact(&message).into(),
            },
            Self::Protocol { provider, problem } => Self::Protocol {
                provider,
                problem: redactions.redact(&problem).into(),
            },
            Self::Credential { provider, source } => Self::Credential {
                provider,
                source: match source {
                    CredentialError::NotInEnvironment(variable) => {
                        CredentialError::NotInEnvironment(redactions.redact(&variable).into())
                    }
                    CredentialError::NotRenewed(problem) => {
                        CredentialError::NotRenewed(redactions.redact(&problem).into())
                    }
                },
            },
            Self::Unconfigured(problem) => Self::Unconfigured(redactions.redact(&problem).into()),
        }
    }

    /// Whether asking again could reasonably get a different answer.
    ///
    /// The difference is whether the failure is about *this* request or about
    /// the moment it was made. A model name nobody has, a key without access, a
    /// response that did not parse and a bound this program enforces all say the
    /// same thing however many times they are asked; a socket that closed while
    /// the tools ran, a service that is overloaded and a gateway that gave up
    /// say it about one attempt.
    ///
    /// Every variant is named rather than caught by a rest arm: a failure added
    /// later is a decision about whether it is worth another go, and one waved
    /// through as permanent is a turn that fails where it did not have to.
    #[must_use]
    pub fn transient(&self) -> bool {
        match self {
            Self::Transport { .. } | Self::Upstream { .. } => true,

            // A status the service itself says is about now: it is busy, it
            // waited too long, or something behind it did. Every other status
            // is about the request, and the same request gets it again.
            Self::Refused { status, .. } => {
                matches!(status, 408 | 429) || matches!(status, 500..=599)
            }

            Self::Limit { .. }
            | Self::Protocol { .. }
            | Self::Credential { .. }
            | Self::Cancelled(_)
            | Self::Unconfigured(_) => false,
        }
    }
}

/// Which bounded part of one provider response grew too far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderLimit {
    /// Visible answer text, in bytes.
    Text,
    /// Tool argument text across the response, in bytes.
    ToolArguments,
    /// One provider-assigned tool call identifier, in bytes.
    ToolCallId,
    /// One provider-supplied tool name, in bytes.
    ToolCallName,
    /// Tool call identifiers and names across the response, in bytes.
    ToolCallMetadata,
    /// Tool calls across the response.
    ToolCalls,
    /// Tool calls across one turn.
    TurnToolCalls,
    /// Provider-controlled bytes retained across one turn.
    TurnResponseBytes,
    /// Provider responses across one turn.
    ProviderResponses,
}

impl fmt::Display for ProviderLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Text => "response text",
            Self::ToolArguments => "tool arguments",
            Self::ToolCallId => "tool call identifier",
            Self::ToolCallName => "tool call name",
            Self::ToolCallMetadata => "tool call metadata",
            Self::ToolCalls => "tool calls",
            Self::TurnToolCalls => "tool calls across the turn",
            Self::TurnResponseBytes => "response bytes across the turn",
            Self::ProviderResponses => "provider responses across the turn",
        })
    }
}

/// One turn's worth of input to a model.
///
/// The large fields are borrowed from the runner. A provider consumes them
/// before [`Provider::stream`] returns; the returned stream owns only the
/// response, so starting a request does not duplicate the transcript.
#[derive(Clone, Copy)]
pub struct Request<'a> {
    /// Which model to ask.
    pub model: &'a str,
    /// The transcript so far.
    pub transcript: &'a Transcript,
    /// The tools the model may call, as JSON Schema.
    pub tools: &'a [ToolSchema],
    /// Ceiling on the response length.
    pub max_tokens: u32,
    /// The system prompt, if the session has one.
    pub system: Option<&'a str>,
    /// How hard to think, where somebody said.
    ///
    /// `None` is not a rung and does not mean the middle one: it is the field
    /// left off, which is the vendor's own default. Every vendor here has one
    /// and each picked it for the models it serves, so a session nobody has
    /// said anything to about effort is one this program has not overridden.
    pub effort: Option<Effort>,
}

impl fmt::Debug for Request<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("model", &self.model)
            .field("transcript", &"[redacted]")
            .field("tools", &self.tools.len())
            .field("max_tokens", &self.max_tokens)
            .field("system", &self.system.map(|_| "[redacted]"))
            .field("effort", &self.effort)
            .finish()
    }
}

/// How hard a model is asked to think before it answers.
///
/// crucible's ladder rather than any one vendor's, so a rung means the same
/// thing after `/model` moves the session to another of them. Where a vendor
/// serves fewer rungs than these five, the ones it serves are what gets
/// offered — and a rung named on the command line that its vendor does not
/// serve comes back as that vendor's own refusal, which is the bargain a model
/// name is already on.
///
/// Two words one vendor serves are deliberately not rungs here: `none` and
/// `minimal`. What they buy is a model that calls tools without thinking about
/// them first, and a harness whose whole purpose is calling tools has no use
/// for that — the same argument that put this crate's OpenAI provider on the
/// endpoint where reasoning and function tools can be asked for together.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Effort {
    /// The fewest tokens: short, scoped work where speed is the point.
    Low,
    /// Some of the thinking, for a saving on all of it.
    Medium,
    /// What a session asks for when somebody opens the panel and takes the
    /// rung it opens on, and what most of these vendors default to unasked.
    #[default]
    High,
    /// More than high, for work that runs long enough to be worth it.
    Xhigh,
    /// Everything the model has, with no ceiling on what it spends getting it.
    Max,
}

impl Effort {
    /// Every rung, weakest first — what a picker walks and what a refusal
    /// lists.
    pub const LADDER: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max];

    /// The rung as all three vendors spell it, which is the same word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// The word given for a rung was not one.
///
/// Names what was typed and then every rung there is, because this is reached
/// where there is nothing on screen to look at: a flag being parsed before the
/// first frame, or a key in a file somebody is reading with an editor open.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no effort called {named}; crucible takes {}", Effort::LADDER.map(Effort::as_str).join(", "))]
pub struct EffortError {
    /// What was asked for.
    pub named: Box<str>,
}

impl std::str::FromStr for Effort {
    type Err = EffortError;

    /// Trimmed and lowercased first. A word this short is typed at a shell in
    /// whatever case the person was already in, and refusing `--effort HIGH`
    /// teaches nothing that accepting it does not.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let named = text.trim().to_ascii_lowercase();

        Self::LADDER
            .into_iter()
            .find(|rung| rung.as_str() == named)
            .ok_or_else(|| EffortError {
                named: text.trim().into(),
            })
    }
}

/// A tool as advertised to a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSchema {
    /// The name the model calls.
    pub name: &'static str,
    /// The JSON Schema for the arguments.
    pub schema: &'static str,
}

/// What a response has cost, counted in the tokens the model produced.
///
/// Output only. What a request carries is settled before it is sent and is the
/// same however long the answer takes, so it says nothing about the answer
/// somebody is currently waiting on — and one number that goes up while you
/// watch it is worth more than two that need adding.
///
/// A provider says this about the response it is in the middle of, as often as
/// it likes, each reading replacing the last. What a whole turn spent is the
/// sum over its responses, which is the runner's to add because the turn is
/// the runner's: a provider is asked several times and is told nothing about
/// the turn around those requests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Spend(u64);

impl Spend {
    /// Nothing spent yet.
    pub const NONE: Self = Self(0);

    /// A reading of `tokens` produced.
    #[must_use]
    pub const fn new(tokens: u64) -> Self {
        Self(tokens)
    }

    /// How many tokens that is.
    #[must_use]
    pub const fn tokens(self) -> u64 {
        self.0
    }

    /// This and `other` together.
    ///
    /// Saturating, because a count that wrapped would read as a turn that spent
    /// nothing at the moment it spent the most.
    #[must_use]
    pub const fn and(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
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
    /// What this response has cost so far, replacing whatever it last said.
    Spent(Spend),
    /// The model stopped, and why.
    Stopped(StopReason),
}

/// A stream of deltas from one request.
///
/// `next` blocks on the socket, so it runs on the provider thread and never on
/// the render path.
pub trait DeltaStream: Send {
    /// The next delta, or `None` when the stream is finished.
    ///
    /// A stream that returns `None` has already delivered a
    /// [`Delta::Stopped`], or has already reported a failure. Ending without
    /// either is the one thing an implementation may not do: silence is what a
    /// finished response and a response that stopped arriving have in common,
    /// and the stop reason is the only thing that tells them apart. A
    /// truncated answer that ends quietly reads as a complete one, which is
    /// the failure the user cannot see for themselves — so a response that
    /// stops arriving is a [`ProviderError::Transport`] to report, not a
    /// stream that finished.
    ///
    /// The runner holds this rather than trusting it, and a stream that ends
    /// with nothing said fails the turn.
    fn next(&mut self) -> Option<Result<Delta, ProviderError>>;
}

/// One LLM backend adapter.
pub trait Provider: Send + Sync {
    /// The provider's name, for errors and for the status line.
    fn name(&self) -> &'static str;

    /// Starts a request and returns its stream of deltas.
    ///
    /// The borrowed request must be consumed before this method returns. The
    /// stream may retain the response, but no request field.
    ///
    /// # Errors
    ///
    /// [`ProviderError`] if the request could not be sent or was refused. A
    /// failure part-way through the response arrives through the stream
    /// instead.
    fn stream(
        &self,
        request: Request<'_>,
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
    fn every_rung_is_read_back_from_the_word_it_is_written_as() {
        for rung in [
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::Xhigh,
            Effort::Max,
        ] {
            assert_eq!(rung.as_str().parse(), Ok(rung));
        }
    }

    #[test]
    fn a_rung_nobody_serves_is_refused_with_the_ones_that_are() {
        // The sentence is the whole of what somebody who mistyped gets: there
        // is no list on screen at the moment a flag is parsed, and `--effort
        // maximum` is the obvious thing to type.
        let refused = "maximum".parse::<Effort>().expect_err("not a rung");

        assert_eq!(
            refused.to_string(),
            "no effort called maximum; crucible takes low, medium, high, xhigh, max"
        );
    }

    #[test]
    fn a_rung_is_read_however_it_was_capitalised() {
        // A word this short, typed at a shell, is typed in whatever case the
        // person was already in. Refusing `--effort HIGH` teaches nothing.
        assert_eq!("HIGH".parse(), Ok(Effort::High));
        assert_eq!(" Max ".parse(), Ok(Effort::Max));
    }

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
    fn a_resource_failure_names_what_was_bounded() {
        let err = ProviderError::Limit {
            provider: "moonshot",
            limit: ProviderLimit::ToolArguments,
            maximum: 1024,
        };

        assert_eq!(
            err.to_string(),
            "moonshot: tool arguments exceeded its limit of 1024"
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

    #[test]
    fn redaction_preserves_error_kinds_and_useful_provider_text() {
        let mut outgoing = crate::Outgoing::new();
        outgoing.protect("credential-canary");
        let error = ProviderError::Upstream {
            provider: "openai",
            kind: "rate_limit".into(),
            message: "account credential-canary is over quota".into(),
        }
        .redacted(&outgoing.redactions());

        assert_eq!(
            error.to_string(),
            "openai: rate_limit: account <redacted> is over quota"
        );
        assert!(!format!("{error:?}").contains("credential-canary"));
    }

    #[test]
    fn a_failure_about_the_moment_is_worth_asking_again_and_one_about_the_request_is_not() {
        let moment = [
            ProviderError::Transport {
                provider: "openai",
                problem: "connection closed before any data was read".into(),
            },
            ProviderError::Upstream {
                provider: "openai",
                kind: "overloaded_error".into(),
                message: "the engine is currently overloaded".into(),
            },
        ];

        for failure in moment {
            assert!(failure.transient(), "{failure}");
        }

        let request = [
            ProviderError::Protocol {
                provider: "openai",
                problem: "no `type` on the event".into(),
            },
            ProviderError::Limit {
                provider: "openai",
                limit: ProviderLimit::ToolArguments,
                maximum: 1024,
            },
            ProviderError::Credential {
                provider: "openai",
                source: CredentialError::NotInEnvironment("OPENAI_API_KEY".into()),
            },
            ProviderError::Cancelled("openai"),
            ProviderError::Unconfigured("no credential for any provider".into()),
        ];

        for failure in request {
            assert!(!failure.transient(), "{failure}");
        }
    }

    #[test]
    fn a_refusal_is_worth_asking_again_only_where_the_status_is_about_the_moment() {
        // The line this draws is the one a user meets: 429 and 503 clear on
        // their own, and 401 and 404 are the same answer for as long as the key
        // or the model name stays what it is.
        for status in [408, 429, 500, 503, 529] {
            assert!(refusal(status).transient(), "HTTP {status}");
        }

        for status in [400, 401, 403, 404, 413, 422] {
            assert!(!refusal(status).transient(), "HTTP {status}");
        }
    }

    fn refusal(status: u16) -> ProviderError {
        ProviderError::Refused {
            provider: "openai",
            status,
            message: "no".into(),
        }
    }
}
