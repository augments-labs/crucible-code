//! The Anthropic provider.
//!
//! Four parts, and the split is the direction data travels: [`body`] builds a
//! request, [`wire`] reads one event of a response, [`stream`] delivers a whole
//! one, and this file is the request itself — the headers and the status.
//!
//! Only two of those are this vendor's. Delivering a response is the same job
//! whoever sent it, so the loop that does it lives in `crate::stream` and
//! [`stream`] is where this protocol is handed to it.
//!
//! It names no HTTP client and no credential kind. A [`Transport`] is handed in
//! and so is a [`Credential`], which is what lets the whole protocol be tested
//! against recorded bytes.

mod body;
mod continuation;
#[cfg(test)]
mod continuation_tests;
mod diagnostics;
mod stream;
mod wire;

use crucible_core::{
    Cancel, Credential, CredentialScopeId, DeltaStream, Modalities, Modality, Outgoing, PriceRate,
    PricingCurrency, PricingDate, PricingError, PricingUnit, PromptCacheBoundary,
    PromptCacheCapabilities, PromptCacheContent, PromptCacheMechanismCapability,
    PromptCachePricing, PromptCacheProvenance, PromptCacheRates, PromptCacheRetentionClass,
    PromptCacheRoute, PromptCacheUsageReporting, Provider, ProviderError, Request,
    StatefulTransportCapability, UsageRate,
};

use crate::anthropic::stream::Stream;
use crate::endpoint::Endpoint;
use crate::refusal::refused;
use crate::transport::Transport;

/// What this provider is called, in errors and in the status line.
const NAME: &str = "anthropic";

/// Where requests go unless a setting says otherwise.
const VENDOR: Endpoint = Endpoint::fixed("https://api.anthropic.com/v1/messages");

/// The API version this speaks. Anthropic pins behaviour to it, so a new one is
/// a deliberate change here rather than something that drifts.
const VERSION: &str = "2023-06-01";
pub(super) const FABLE_51: &str = "claude-fable-5-1";

const ANTHROPIC_CACHE_CONTENT: &[PromptCacheContent] = &[
    PromptCacheContent::Text,
    PromptCacheContent::Tools,
    PromptCacheContent::Images,
    PromptCacheContent::Documents,
];
const ANTHROPIC_CACHE_BOUNDARIES: &[PromptCacheBoundary] = &[
    PromptCacheBoundary::AfterSystem,
    PromptCacheBoundary::AfterTools,
    PromptCacheBoundary::AfterMessage,
];
const ANTHROPIC_RETENTIONS: &[PromptCacheRetentionClass] = &[
    PromptCacheRetentionClass::ProviderDefault,
    PromptCacheRetentionClass::Ephemeral,
    PromptCacheRetentionClass::Extended,
];

const USD: PricingCurrency = PricingCurrency::new("USD");
const PRICING_REVIEWED: PricingDate = PricingDate::new(2026, 8, 31);
const FABLE_51_REVIEWED: PricingDate = PricingDate::new(2026, 9, 6);
const PRICING_SOURCE: &str = "https://platform.claude.com/docs/en/about-claude/pricing";

const fn anthropic_rates(input: u64, read: u64, write: u64, output: u64) -> PromptCacheRates {
    PromptCacheRates {
        uncached_input: UsageRate::priced(PriceRate::per_million(input)),
        cache_read: UsageRate::priced(PriceRate::per_million(read)),
        cache_write_or_creation: UsageRate::priced(PriceRate::per_million(write)),
        output: UsageRate::priced(PriceRate::per_million(output)),
        reasoning: UsageRate::NotApplicable,
        storage: UsageRate::NotApplicable,
        other: UsageRate::NotApplicable,
    }
}

/// Anthropic's Messages API.
#[derive(Debug)]
pub struct Anthropic {
    credential: Box<dyn Credential>,
    transport: Box<dyn Transport>,
    endpoint: Endpoint,
    credential_scope: CredentialScopeId,
}

impl Anthropic {
    /// The address this API is served at, for a caller with no reason to send
    /// anywhere else.
    pub const VENDOR: Endpoint = VENDOR;

    /// A provider that authenticates with `credential`, sends over `transport`
    /// and posts to `endpoint`.
    ///
    /// The address is named by the caller rather than defaulted here, because
    /// the wiring is where a decision like that belongs and there is one
    /// constructor rather than a defaulting one beside an explicit one. It is
    /// an [`Endpoint`] rather than a string because what decides who receives
    /// the key is checked before it is one.
    #[must_use]
    pub fn at(
        endpoint: Endpoint,
        credential: Box<dyn Credential>,
        transport: Box<dyn Transport>,
    ) -> Self {
        let credential_scope = credential.scope();
        Self {
            credential,
            transport,
            endpoint,
            credential_scope,
        }
    }

    /// The headers every request carries, including the secret.
    fn headers(&self, model: &str) -> Result<Outgoing, ProviderError> {
        let mut outgoing = Outgoing::new();
        outgoing.set_header("content-type", "application/json");
        outgoing.set_header("anthropic-version", VERSION);
        outgoing.set_header("accept", "text/event-stream");
        if model == FABLE_51 {
            outgoing.set_header(
                "anthropic-beta",
                "thinking-binding-controls-2026-08-01,mid-conversation-output-config-2026-07-01",
            );
        }

        self.credential
            .authorize(&mut outgoing)
            .map_err(|source| ProviderError::Credential {
                provider: NAME,
                source,
            })?;

        Ok(outgoing)
    }
}

impl Provider for Anthropic {
    fn name(&self) -> &'static str {
        NAME
    }

    fn spells(&self) -> Modalities {
        // Messages spells an attachment as an `image` or `document` block, and
        // this module writes both. What a document holds beyond a PDF -- plain
        // text, and a vendor-side conversion -- is not offered, because a
        // modality is what is declared here and a PDF is the only one that
        // block carries.
        Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf)
    }

    fn prompt_cache_capabilities(&self, model: &str) -> PromptCacheCapabilities {
        if self.endpoint != VENDOR {
            return PromptCacheCapabilities::unknown("custom endpoint");
        }
        let (minimum, revision) = match model {
            FABLE_51 => (512, FABLE_51),
            "claude-fable-5" => (512, "claude-fable-5"),
            "claude-opus-5" => (512, "claude-opus-5"),
            "claude-sonnet-5" => (1_024, "claude-sonnet-5"),
            "claude-haiku-4-5" => (4_096, "claude-haiku-4-5-20251001"),
            _ => return PromptCacheCapabilities::unknown("unreviewed model"),
        };
        let automatic = PromptCacheMechanismCapability::automatic_prefix(
            minimum,
            false,
            true,
            ANTHROPIC_CACHE_CONTENT,
        )
        .with_retentions(ANTHROPIC_RETENTIONS);
        let explicit = PromptCacheMechanismCapability::explicit_breakpoints(
            minimum,
            4,
            ANTHROPIC_CACHE_BOUNDARIES,
            ANTHROPIC_CACHE_CONTENT,
        )
        .with_retentions(ANTHROPIC_RETENTIONS);
        let (date, version) = if model == FABLE_51 {
            ("2026-09-06", "anthropic-prompt-cache-2026-09-06")
        } else {
            ("2026-08-31", "anthropic-prompt-cache-2026-08-31")
        };
        PromptCacheCapabilities::supported(
            version,
            Some(revision),
            PromptCacheProvenance::new(
                "https://platform.claude.com/docs/en/build-with-claude/prompt-caching",
                date,
                version,
            ),
            StatefulTransportCapability::Unsupported,
            &[automatic, explicit],
            PromptCacheUsageReporting::ReadAndWriteTokens,
        )
    }

    fn prompt_cache_pricing(
        &self,
        model: &str,
        revision: Option<&str>,
        input_tokens: Option<u64>,
        retention: PromptCacheRetentionClass,
        at: PricingDate,
    ) -> Result<Option<PromptCachePricing>, PricingError> {
        let reviewed = if model == FABLE_51 {
            FABLE_51_REVIEWED
        } else {
            PRICING_REVIEWED
        };
        if self.endpoint != VENDOR || at < reviewed || input_tokens.is_none() {
            return Ok(None);
        }
        if !matches!(
            retention,
            PromptCacheRetentionClass::ProviderDefault
                | PromptCacheRetentionClass::Ephemeral
                | PromptCacheRetentionClass::Extended
        ) {
            return Ok(None);
        }
        let (model, resolved_revision, input, read, short_write, extended_write, output) =
            match (model, revision) {
                (FABLE_51, Some(FABLE_51)) => (
                    FABLE_51,
                    FABLE_51,
                    10_000_000_000,
                    250_000_000,
                    12_500_000_000,
                    20_000_000_000,
                    50_000_000_000,
                ),
                ("claude-fable-5", Some("claude-fable-5")) => (
                    "claude-fable-5",
                    "claude-fable-5",
                    10_000_000_000,
                    1_000_000_000,
                    12_500_000_000,
                    20_000_000_000,
                    50_000_000_000,
                ),
                ("claude-opus-5", Some("claude-opus-5")) => (
                    "claude-opus-5",
                    "claude-opus-5",
                    5_000_000_000,
                    500_000_000,
                    6_250_000_000,
                    10_000_000_000,
                    25_000_000_000,
                ),
                ("claude-sonnet-5", Some("claude-sonnet-5")) => (
                    "claude-sonnet-5",
                    "claude-sonnet-5",
                    2_000_000_000,
                    200_000_000,
                    2_500_000_000,
                    4_000_000_000,
                    10_000_000_000,
                ),
                ("claude-haiku-4-5", Some("claude-haiku-4-5-20251001")) => (
                    "claude-haiku-4-5",
                    "claude-haiku-4-5-20251001",
                    1_000_000_000,
                    100_000_000,
                    1_250_000_000,
                    2_000_000_000,
                    5_000_000_000,
                ),
                _ => return Ok(None),
            };
        let extended = retention == PromptCacheRetentionClass::Extended;
        let write = if extended {
            extended_write
        } else {
            short_write
        };
        Ok(Some(
            PromptCachePricing::new(
                "anthropic-messages",
                "https://api.anthropic.com/v1/messages",
                model,
                Some(resolved_revision),
                reviewed,
                if model == FABLE_51 && extended {
                    "anthropic-direct-1h-2026-09-06"
                } else if model == FABLE_51 {
                    "anthropic-direct-5m-2026-09-06"
                } else if extended {
                    "anthropic-direct-1h-2026-08-31"
                } else {
                    "anthropic-direct-5m-2026-08-31"
                },
                PRICING_SOURCE,
                USD,
                PricingUnit::MillionTokens,
                anthropic_rates(input, read, write, output),
            )
            .with_retention(retention),
        ))
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: "anthropic-messages",
            endpoint: self.endpoint.as_str(),
            custom_endpoint: self.endpoint != VENDOR,
            credential_scope: self.credential_scope,
            account: None,
            project: None,
            request_shape_version: "anthropic-messages-2023-06-01-v1",
        }
    }

    fn prompt_cache_encoding(&self, request: &Request<'_>) -> crucible_core::PromptCacheEncoding {
        body::prompt_cache_encoding(request)
    }

    fn stream(
        &self,
        request: Request<'_>,
        cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        // Nothing is sent for a turn the user has already abandoned. Once the
        // request is away, cancelling is the stream's business.
        if cancel.requested() {
            return Err(ProviderError::Cancelled(NAME));
        }

        let outgoing = self.headers(request.model)?;
        let redactions = outgoing.redactions();
        let scope =
            crucible_core::ContinuationScope::new(self.credential_scope, self.endpoint.as_str());
        let body = body::serialize(&request, (request.model == FABLE_51).then_some(scope))?;

        let response = self
            .transport
            .post(self.endpoint.as_str(), outgoing, body, cancel)
            .map_err(|problem| problem.for_provider(NAME).redacted(&redactions))?;

        if response.status != 200 {
            let error = refused(NAME, response.status, response.body, &redactions, cancel);
            return Err(if request.model == FABLE_51 {
                diagnostics::refusal(error)
            } else {
                error
            });
        }

        Ok(Box::new(Stream::with_wire(
            response.body,
            cancel.clone(),
            redactions,
            wire::Messages::for_request(request.model, scope, request.effort)?,
        )))
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::{
        ApiKey, Delta, Header, HeaderKey, Message, PriceRate, PricingDate,
        PromptCacheRetentionClass, StopReason, Transcript, UsageRate,
    };

    use super::stream::tests::{ANSWER, deltas};
    use super::*;
    use crate::fake::output_usage;
    use crate::transport::{Replay, Sent};

    /// The exact key that must never appear anywhere but a header value.
    const SECRET: &str = "sk-ant-do-not-log-me";

    fn provider(status: u16, body: &str) -> (Anthropic, std::sync::Arc<Replay>) {
        let replay = std::sync::Arc::new(Replay::new(status, body));
        let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bare("x-api-key"));

        (
            Anthropic::at(
                Anthropic::VENDOR,
                Box::new(credential),
                Box::new(std::sync::Arc::clone(&replay)),
            ),
            replay,
        )
    }

    #[test]
    fn fable_51_requests_use_adaptive_prefix_binding_for_every_effort_and_omission() {
        use crucible_core::{Effort, RequestPurpose};
        use serde_json::{Value, json};
        let (provider, replay) = provider(200, ANSWER);
        let mut transcript = Transcript::new();
        transcript.push(Message::said("hello")).unwrap();
        for effort in [
            None,
            Some(Effort::Low),
            Some(Effort::Medium),
            Some(Effort::High),
            Some(Effort::Xhigh),
            Some(Effort::Max),
        ] {
            provider
                .stream(
                    Request {
                        model: "claude-fable-5-1",
                        purpose: RequestPurpose::Turn,
                        transcript: &transcript,
                        tools: &[],
                        max_tokens: 8192,
                        system: None,
                        effort,
                        attached: &[],
                        prompt_cache: None,
                    },
                    &Cancel::new(),
                )
                .unwrap();
            let sent = replay.sent();
            assert_eq!(sent.url, "https://api.anthropic.com/v1/messages");
            let body: Value = serde_json::from_str(&sent.body).unwrap();
            assert_eq!(
                body.get("model").and_then(Value::as_str),
                Some("claude-fable-5-1")
            );
            assert_eq!(
                body.get("thinking"),
                Some(&json!({
                    "type":"adaptive", "block_binding":{"prefix_mismatch_behavior":"drop_block"}
                }))
            );
            assert_eq!(
                body.pointer("/output_config/effort")
                    .and_then(Value::as_str),
                effort.map(Effort::as_str)
            );
            assert!(body.get("tool_choice").is_none());
            assert_eq!(body.get("max_tokens").and_then(Value::as_u64), Some(8192));
            assert!(sent.headers.iter().any(|(key, value)| {
                key == "anthropic-beta"
                    && value
                        .split(',')
                        .any(|part| part.trim() == "thinking-binding-controls-2026-08-01")
            }));
            assert!(
                sent.headers
                    .iter()
                    .all(|(key, _)| !key.eq_ignore_ascii_case("authorization"))
            );
            assert!(!sent.body.contains(SECRET));
        }
    }

    #[test]
    fn exact_model_and_retention_select_current_first_party_prices() {
        let (provider, _) = provider(200, ANSWER);
        let five_minutes = provider
            .prompt_cache_pricing(
                "claude-fable-5",
                Some("claude-fable-5"),
                Some(1_000),
                PromptCacheRetentionClass::ProviderDefault,
                PricingDate::new(2026, 8, 31),
            )
            .unwrap()
            .unwrap();
        let one_hour = provider
            .prompt_cache_pricing(
                "claude-fable-5",
                Some("claude-fable-5"),
                Some(1_000),
                PromptCacheRetentionClass::Extended,
                PricingDate::new(2026, 8, 31),
            )
            .unwrap()
            .unwrap();
        let sonnet = provider
            .prompt_cache_pricing(
                "claude-sonnet-5",
                Some("claude-sonnet-5"),
                Some(1_000),
                PromptCacheRetentionClass::ProviderDefault,
                PricingDate::new(2026, 9, 1),
            )
            .unwrap()
            .unwrap();

        assert_eq!(
            five_minutes.rates().cache_write_or_creation,
            UsageRate::priced(PriceRate::per_million(12_500_000_000))
        );
        assert_eq!(
            one_hour.rates().cache_write_or_creation,
            UsageRate::priced(PriceRate::per_million(20_000_000_000))
        );
        assert_eq!(
            sonnet.rates().uncached_input,
            UsageRate::priced(PriceRate::per_million(2_000_000_000))
        );
    }

    #[test]
    fn reviewed_models_default_to_native_automatic_caching_and_custom_routes_are_unknown() {
        let (provider, _) = provider(200, ANSWER);
        let reviewed = provider.prompt_cache_capabilities("claude-fable-5");

        assert_eq!(
            reviewed.support(),
            crucible_core::PromptCacheSupport::Supported
        );
        assert_eq!(
            reviewed
                .mechanisms()
                .first()
                .map(crucible_core::PromptCacheMechanismCapability::mechanism),
            Some(crucible_core::PromptCacheMechanism::AutomaticPrefix)
        );

        let custom = Anthropic::at(
            Endpoint::parse("https://proxy.invalid/v1/messages").unwrap(),
            Box::new(HeaderKey::new(
                ApiKey::new(SECRET),
                Header::bare("x-api-key"),
            )),
            Box::new(Replay::new(200, ANSWER)),
        );
        assert_eq!(
            custom.prompt_cache_capabilities("claude-fable-5").support(),
            crucible_core::PromptCacheSupport::Unknown
        );
    }

    #[test]
    fn a_configured_endpoint_is_where_the_request_goes() {
        // The point of the setting: a gateway standing in for the vendor. What
        // this asserts is that the address reaches the transport, because a
        // provider that read it and still posted to the constant would be a
        // setting that looks applied and does nothing.
        let replay = std::sync::Arc::new(Replay::new(200, ANSWER));
        let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bare("x-api-key"));
        let endpoint = Endpoint::parse("http://localhost:8080/v1").expect("a local address");

        let provider = Anthropic::at(
            endpoint,
            Box::new(credential),
            Box::new(std::sync::Arc::clone(&replay)),
        );

        provider.stream(asking("hello"), &Cancel::new()).unwrap();

        assert_eq!(replay.sent().url, "http://localhost:8080/v1");
    }

    fn asking(text: &str) -> Request<'static> {
        let mut transcript = Transcript::new();
        transcript
            .push(Message::said(text))
            .expect("valid fixture transcript");

        Request {
            purpose: crucible_core::RequestPurpose::Turn,
            model: "claude-test",
            transcript: Box::leak(Box::new(transcript)),
            tools: &[],
            attached: &[],
            max_tokens: 1024,
            system: None,
            effort: None,
            prompt_cache: None,
        }
    }

    fn header<'a>(sent: &'a Sent, name: &str) -> &'a str {
        sent.headers
            .iter()
            .find(|(present, _)| present == name)
            .map_or("<no such header>", |(_, value)| value)
    }

    #[test]
    fn a_request_carries_the_version_and_asks_for_a_stream() {
        let (anthropic, replay) = provider(200, ANSWER);

        anthropic.stream(asking("hello"), &Cancel::new()).unwrap();

        let sent = replay.sent();
        assert_eq!(sent.url, Anthropic::VENDOR.as_str());
        assert_eq!(header(&sent, "anthropic-version"), VERSION);
        assert_eq!(header(&sent, "accept"), "text/event-stream");
        assert_eq!(header(&sent, "content-type"), "application/json");
    }

    #[test]
    fn a_request_is_authorised_by_the_credential_it_was_given() {
        // The provider names the header and the prefix; it never sees the key.
        let (anthropic, replay) = provider(200, ANSWER);

        anthropic.stream(asking("hello"), &Cancel::new()).unwrap();

        assert_eq!(header(&replay.sent(), "x-api-key"), SECRET);
    }

    #[test]
    fn a_provider_does_not_show_its_credential_in_its_debug() {
        // `Provider` is held by the runner and appears in its `Debug`.
        let (anthropic, _) = provider(200, ANSWER);

        assert!(
            !format!("{anthropic:?}").contains(SECRET),
            "the key leaked through the provider"
        );
    }

    #[test]
    fn an_accepted_request_is_handed_back_as_the_answer_it_returned() {
        // The end of the round trip: a body that arrived over the transport
        // reaches the caller as deltas, with nothing in between to arrange it.
        let (anthropic, _) = provider(200, ANSWER);

        let mut stream = anthropic.stream(asking("hello"), &Cancel::new()).unwrap();

        assert_eq!(
            deltas(stream.as_mut()),
            vec![
                Delta::Text("Hello".into()),
                Delta::Text(", world".into()),
                output_usage(4),
                Delta::Stopped(StopReason::Yielded),
            ]
        );
    }

    #[test]
    fn a_refusal_carries_the_status_and_the_sentence_that_explains_it() {
        let said =
            r#"{"type":"error","error":{"type":"not_found_error","message":"model: claude-nope"}}"#;
        let (anthropic, _) = provider(404, said);

        let problem = anthropic
            .stream(asking("hello"), &Cancel::new())
            .unwrap_err();

        assert_eq!(
            problem.to_string(),
            "anthropic: HTTP 404: model: claude-nope"
        );
    }

    #[test]
    fn a_refusal_that_is_not_the_api_still_says_what_it_said() {
        // A gateway in front of the API refuses in its own shape, and that text
        // is the only clue the user gets.
        let (anthropic, _) = provider(502, "  upstream connect error  ");

        let problem = anthropic
            .stream(asking("hello"), &Cancel::new())
            .unwrap_err();

        assert_eq!(
            problem.to_string(),
            "anthropic: HTTP 502: upstream connect error"
        );
    }

    #[test]
    fn a_refusal_cannot_repeat_the_applied_credential() {
        let said = format!(
            r#"{{"error":{{"message":"gateway repeated {SECRET}; model remains useful"}}}}"#
        );
        let (anthropic, _) = provider(401, &said);

        let problem = anthropic
            .stream(asking("hello"), &Cancel::new())
            .unwrap_err();
        let displayed = problem.to_string();
        let debugged = format!("{problem:?}");

        assert!(!displayed.contains(SECRET));
        assert!(!debugged.contains(SECRET));
        assert!(displayed.contains("model remains useful"));
    }

    #[test]
    fn a_stream_error_cannot_repeat_the_applied_credential() {
        let body = format!(
            "event: error\ndata: {{\"error\":{{\"type\":\"gateway\",\"message\":\"{SECRET}; model remains useful\"}}}}\n\n"
        );
        let (anthropic, _) = provider(200, &body);

        let mut stream = anthropic.stream(asking("hello"), &Cancel::new()).unwrap();
        let problem = stream.next().unwrap().unwrap_err();
        let displayed = problem.to_string();
        let debugged = format!("{problem:?}");

        assert!(!displayed.contains(SECRET));
        assert!(!debugged.contains(SECRET));
        assert!(displayed.contains("model remains useful"));
    }

    #[test]
    fn a_cancelled_turn_is_never_sent() {
        let (anthropic, replay) = provider(200, ANSWER);
        let cancel = Cancel::new();
        cancel.request();

        let problem = anthropic.stream(asking("hello"), &cancel).unwrap_err();

        assert!(matches!(problem, ProviderError::Cancelled(_)));
        assert!(
            replay.sent().url.is_empty(),
            "a request went out for a turn the user had abandoned"
        );
    }

    /// What a wire protocol declares is what its body writes, which is now text, a
    /// picture and a PDF. A declaration that ran ahead of the body would be read as
    /// permission to send bytes this module has no shape for — and one that lagged
    /// behind it would refuse a file at the prompt that the request could carry.
    #[test]
    fn anthropic_spells_no_more_than_its_body_can_write_today() {
        let (provider, _replay) = provider(200, ANSWER);

        assert_eq!(
            provider.spells(),
            Modalities::empty()
                .insert(Modality::Text)
                .insert(Modality::Image)
                .insert(Modality::Pdf),
        );
    }
}
