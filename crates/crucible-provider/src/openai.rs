//! The OpenAI provider.
//!
//! Four parts, and the split is the direction data travels: [`body`] builds a
//! request, [`wire`] reads one event of a response, [`stream`] delivers a whole
//! one, and this file is the request itself — the address, the headers and the
//! status. Two addresses serve this protocol and they do not accept the same
//! body, so the address is also what [`Serving`] is read off on the way out.
//!
//! Only two of those are this vendor's. Delivering a response is the same job
//! whoever sent it, so the loop that does it lives in `crate::stream` and
//! [`stream`] is where this protocol is handed to it.
//!
//! It names no HTTP client and no credential kind. A [`Transport`] is handed in
//! and so is a [`Credential`], which is what lets the whole protocol be tested
//! against recorded bytes.
//!
//! Responses rather than Chat Completions, and not for the newer fields. A
//! model that reasons before answering refuses function tools on the older
//! endpoint outright — it answers a request carrying both with
//!
//! ```text
//! Function tools with reasoning_effort are not supported for <model> in
//! /v1/chat/completions. To use function tools, use /v1/responses or set
//! reasoning_effort to 'none'.
//! ```
//!
//! and the effort it names is the vendor's own default rather than anything
//! sent from here. A harness whose whole purpose is calling tools has two
//! answers to that: turn the reasoning off, or move. Turning it off would leave
//! every OpenAI session running a thinking model told not to think, which is
//! the worse of the two by some way.
//!
//! What that costs is the other vendors serving an OpenAI-compatible API, which
//! implement the older endpoint and not this one. They are reached by a
//! provider of their own the day one is written, and the seam is already the
//! right shape for it: a wire protocol is a module here, and nothing above this
//! crate learns which endpoint answered.

mod body;
#[cfg(test)]
mod model_tests;
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

use crate::endpoint::Endpoint;
use crate::openai::stream::Stream;
use crate::refusal::refused;
use crate::transport::Transport;

/// What this provider is called, in errors and in the status line.
const NAME: &str = "openai";
const ASTRA: &str = "gpt-6-astra";

/// Models whose cache options and inclusive usage include cache writes.
fn cache_writes(model: &str) -> bool {
    model == ASTRA || model.starts_with("gpt-5.6-")
}

/// Where requests go unless a setting says otherwise.
const VENDOR: Endpoint = Endpoint::fixed("https://api.openai.com/v1/responses");

/// Where `ChatGPT` subscription credentials serve the Responses protocol.
const SUBSCRIPTION: Endpoint = Endpoint::fixed("https://chatgpt.com/backend-api/codex/responses");

const OPENAI_CACHE_CONTENT: &[PromptCacheContent] = &[
    PromptCacheContent::Text,
    PromptCacheContent::Tools,
    PromptCacheContent::Images,
    PromptCacheContent::Documents,
];
const OPENAI_EXPLICIT_BOUNDARIES: &[PromptCacheBoundary] = &[PromptCacheBoundary::AfterMessage];
const OPENAI_56_RETENTIONS: &[PromptCacheRetentionClass] = &[
    PromptCacheRetentionClass::ProviderDefault,
    PromptCacheRetentionClass::Ephemeral,
];
const OPENAI_55_RETENTIONS: &[PromptCacheRetentionClass] = &[
    PromptCacheRetentionClass::ProviderDefault,
    PromptCacheRetentionClass::Extended,
];

const USD: PricingCurrency = PricingCurrency::new("USD");
const PRICING_REVIEWED: PricingDate = PricingDate::new(2026, 8, 31);
const ASTRA_REVIEWED: PricingDate = PricingDate::new(2026, 9, 6);
const PRICING_SOURCE: &str = "https://developers.openai.com/api/docs/pricing";
const MODEL_55_PRICING_SOURCE: &str = "https://developers.openai.com/api/docs/models/gpt-5.5";

const fn rate(nanocurrency: u64) -> UsageRate {
    UsageRate::priced(PriceRate::per_million(nanocurrency))
}

const fn optional_rate(nanocurrency: u64) -> UsageRate {
    UsageRate::optional(PriceRate::per_million(nanocurrency))
}

const fn openai_rates(input: u64, read: u64, write: UsageRate, output: u64) -> PromptCacheRates {
    PromptCacheRates {
        uncached_input: rate(input),
        cache_read: rate(read),
        cache_write_or_creation: write,
        output: rate(output),
        reasoning: UsageRate::NotApplicable,
        storage: UsageRate::NotApplicable,
        other: UsageRate::NotApplicable,
    }
}

/// Which of the two services a request is bound for.
///
/// They speak the same protocol and do not accept the same body: the published
/// API takes the whole Responses request, and the backend a plan is served by
/// implements a part of it and answers a field it does not know with a 400 that
/// ends the turn. So the difference is carried into [`body`] rather than left to
/// the address.
///
/// Read off the address rather than stored beside it, because being on that
/// service *is* posting there — a field saying which one would be a second
/// answer to a question the endpoint already answers, and the two would drift
/// the first time one of them was set without the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Serving {
    /// `api.openai.com`, or whatever gateway a key was pointed at.
    Api,

    /// The backend a `ChatGPT` plan is served by.
    Subscription,
}

impl Serving {
    /// Which service `endpoint` belongs to.
    fn of(endpoint: &Endpoint) -> Self {
        if *endpoint == SUBSCRIPTION {
            Self::Subscription
        } else {
            Self::Api
        }
    }
}

/// OpenAI's Responses API.
#[derive(Debug)]
pub struct OpenAi {
    credential: Box<dyn Credential>,
    transport: Box<dyn Transport>,
    endpoint: Endpoint,
    credential_scope: CredentialScopeId,
}

impl OpenAi {
    /// The address this API is served at, for a caller with no reason to send
    /// anywhere else.
    pub const VENDOR: Endpoint = VENDOR;

    /// The fixed endpoint that accepts a `ChatGPT` subscription credential.
    ///
    /// Kept distinct from [`OpenAi::VENDOR`]: an API key may be redirected to a
    /// configured compatible gateway, while a subscription token is minted for
    /// this audience and must never follow that setting.
    pub const SUBSCRIPTION: Endpoint = SUBSCRIPTION;

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
    ///
    /// No version header: this API is versioned by its path, and behaviour
    /// changes arrive under new model names rather than new dates.
    fn headers(&self) -> Result<Outgoing, ProviderError> {
        let mut outgoing = Outgoing::new();
        outgoing.set_header("content-type", "application/json");
        outgoing.set_header("accept", "text/event-stream");

        self.credential
            .authorize(&mut outgoing)
            .map_err(|source| ProviderError::Credential {
                provider: NAME,
                source,
            })?;

        Ok(outgoing)
    }
}

impl Provider for OpenAi {
    fn name(&self) -> &'static str {
        NAME
    }

    fn spells(&self) -> Modalities {
        // Responses spells an attachment as an `input_image` or `input_file`
        // part, and this module writes both. An `input_file` carries other
        // kinds of file too, and none of them is offered: what is declared
        // here is a modality, and a PDF is the only one this harness attaches
        // that the part is documented to read.
        Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf)
    }

    fn prompt_cache_capabilities(&self, model: &str) -> PromptCacheCapabilities {
        if self.endpoint != VENDOR && self.endpoint != SUBSCRIPTION {
            return PromptCacheCapabilities::unknown("custom endpoint");
        }

        let (minimum, revision, retentions, usage): (u32, &'static str, &[_], _) = match model {
            ASTRA => (
                1_024,
                ASTRA,
                OPENAI_56_RETENTIONS,
                PromptCacheUsageReporting::ReadAndWriteTokens,
            ),
            "gpt-5.6-sol" => (
                1_024,
                "gpt-5.6-sol",
                OPENAI_56_RETENTIONS,
                PromptCacheUsageReporting::ReadAndWriteTokens,
            ),
            "gpt-5.6-terra" => (
                1_024,
                "gpt-5.6-terra",
                OPENAI_56_RETENTIONS,
                PromptCacheUsageReporting::ReadAndWriteTokens,
            ),
            "gpt-5.6-luna" => (
                1_024,
                "gpt-5.6-luna",
                OPENAI_56_RETENTIONS,
                PromptCacheUsageReporting::ReadAndWriteTokens,
            ),
            "gpt-5.5" => (
                2_048,
                "gpt-5.5",
                OPENAI_55_RETENTIONS,
                PromptCacheUsageReporting::ReadTokens,
            ),
            _ => return PromptCacheCapabilities::unknown("unreviewed model"),
        };
        let automatic = PromptCacheMechanismCapability::automatic_prefix(
            minimum,
            true,
            true,
            OPENAI_CACHE_CONTENT,
        )
        .with_retentions(retentions);
        let mechanisms = if self.endpoint == VENDOR && cache_writes(model) {
            vec![
                automatic,
                PromptCacheMechanismCapability::explicit_breakpoints(
                    minimum,
                    4,
                    OPENAI_EXPLICIT_BOUNDARIES,
                    OPENAI_CACHE_CONTENT,
                )
                .with_retentions(OPENAI_56_RETENTIONS),
            ]
        } else {
            vec![automatic]
        };
        let (reviewed, version) = if model == ASTRA {
            ("2026-09-06", "openai-prompt-cache-2026-09-06")
        } else {
            ("2026-08-31", "openai-prompt-cache-2026-08-31")
        };
        PromptCacheCapabilities::supported(
            version,
            Some(revision),
            PromptCacheProvenance::new(
                "https://developers.openai.com/api/docs/guides/prompt-caching",
                reviewed,
                version,
            ),
            StatefulTransportCapability::Unsupported,
            &mechanisms,
            usage,
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
        let reviewed = if model == ASTRA {
            ASTRA_REVIEWED
        } else {
            PRICING_REVIEWED
        };
        if self.endpoint != VENDOR || at < reviewed {
            return Ok(None);
        }
        let Some(input_tokens) = input_tokens else {
            return Ok(None);
        };
        let (model, source, short, long) = match (model, revision) {
            (ASTRA, Some(ASTRA)) => (
                ASTRA,
                "https://developers.openai.com/api/docs/models/gpt-6-astra",
                openai_rates(
                    10_000_000_000,
                    1_000_000_000,
                    rate(12_500_000_000),
                    50_000_000_000,
                ),
                openai_rates(
                    20_000_000_000,
                    2_000_000_000,
                    rate(25_000_000_000),
                    75_000_000_000,
                ),
            ),
            ("gpt-5.6-sol", Some("gpt-5.6-sol")) => (
                "gpt-5.6-sol",
                PRICING_SOURCE,
                openai_rates(
                    4_000_000_000,
                    400_000_000,
                    rate(5_000_000_000),
                    20_000_000_000,
                ),
                openai_rates(
                    8_000_000_000,
                    800_000_000,
                    rate(10_000_000_000),
                    30_000_000_000,
                ),
            ),
            ("gpt-5.6-terra", Some("gpt-5.6-terra")) => (
                "gpt-5.6-terra",
                PRICING_SOURCE,
                openai_rates(
                    2_000_000_000,
                    200_000_000,
                    rate(2_500_000_000),
                    12_000_000_000,
                ),
                openai_rates(
                    4_000_000_000,
                    400_000_000,
                    rate(5_000_000_000),
                    18_000_000_000,
                ),
            ),
            ("gpt-5.6-luna", Some("gpt-5.6-luna")) => (
                "gpt-5.6-luna",
                PRICING_SOURCE,
                openai_rates(200_000_000, 20_000_000, rate(250_000_000), 1_200_000_000),
                openai_rates(400_000_000, 40_000_000, rate(500_000_000), 1_800_000_000),
            ),
            ("gpt-5.5", Some("gpt-5.5")) => (
                "gpt-5.5",
                MODEL_55_PRICING_SOURCE,
                openai_rates(
                    5_000_000_000,
                    500_000_000,
                    optional_rate(5_000_000_000),
                    30_000_000_000,
                ),
                openai_rates(
                    10_000_000_000,
                    1_000_000_000,
                    optional_rate(10_000_000_000),
                    45_000_000_000,
                ),
            ),
            _ => return Ok(None),
        };
        let allowed_retention = if model == "gpt-5.5" {
            matches!(
                retention,
                PromptCacheRetentionClass::ProviderDefault | PromptCacheRetentionClass::Extended
            )
        } else {
            matches!(
                retention,
                PromptCacheRetentionClass::ProviderDefault | PromptCacheRetentionClass::Ephemeral
            )
        };
        if !allowed_retention {
            return Ok(None);
        }
        let (short_version, long_version) = if model == ASTRA {
            (
                "openai-standard-short-2026-09-06",
                "openai-standard-long-2026-09-06",
            )
        } else {
            (
                "openai-standard-short-2026-08-31",
                "openai-standard-long-2026-08-31",
            )
        };
        let (rates, minimum, maximum, version) = if input_tokens <= 272_000 {
            (short, 0, Some(272_000), short_version)
        } else {
            (long, 272_001, None, long_version)
        };
        Ok(Some(
            PromptCachePricing::new(
                "openai-responses",
                "https://api.openai.com/v1/responses",
                model,
                Some(model),
                reviewed,
                version,
                source,
                USD,
                PricingUnit::MillionTokens,
                rates,
            )
            .with_input_band(minimum, maximum)
            .with_retention(retention),
        ))
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: "openai-responses",
            endpoint: self.endpoint.as_str(),
            custom_endpoint: self.endpoint != VENDOR && self.endpoint != SUBSCRIPTION,
            credential_scope: self.credential_scope,
            account: None,
            project: None,
            request_shape_version: "openai-responses-v1",
        }
    }

    fn prompt_cache_encoding(&self, request: &Request<'_>) -> crucible_core::PromptCacheEncoding {
        body::prompt_cache_encoding(request, Serving::of(&self.endpoint))
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

        let outgoing = self.headers()?;
        let redactions = outgoing.redactions();
        let body = body::serialize(&request, Serving::of(&self.endpoint));

        let response = self
            .transport
            .post(self.endpoint.as_str(), outgoing, body, cancel)
            .map_err(|problem| problem.for_provider(NAME).redacted(&redactions))?;

        if response.status != 200 {
            return Err(refused(
                NAME,
                response.status,
                response.body,
                &redactions,
                cancel,
            ));
        }

        Ok(Box::new(Stream::with_wire(
            response.body,
            cancel.clone(),
            redactions,
            wire::Responses::for_model(request.model),
        )))
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::{
        ApiKey, Delta, Header, HeaderKey, Message, PriceRate, PricingDate,
        PromptCacheRetentionClass, PromptCacheUsageReporting, StopReason, Transcript, UsageRate,
    };

    use super::stream::tests::{ANSWER, deltas};
    use super::*;
    use crate::fake::inclusive_usage;
    use crate::transport::{Replay, Sent};

    /// The exact key that must never appear anywhere but a header value.
    const SECRET: &str = "sk-proj-do-not-log-me";

    fn provider(status: u16, body: &str) -> (OpenAi, std::sync::Arc<Replay>) {
        let replay = std::sync::Arc::new(Replay::new(status, body));
        let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bearer());

        (
            OpenAi::at(
                OpenAi::VENDOR,
                Box::new(credential),
                Box::new(std::sync::Arc::clone(&replay)),
            ),
            replay,
        )
    }

    #[test]
    fn reconstructed_providers_keep_only_the_same_credentials_cache_scope() {
        let replay = || Box::new(Replay::new(200, ANSWER));
        let first = OpenAi::at(
            OpenAi::VENDOR,
            Box::new(HeaderKey::new(ApiKey::new(SECRET), Header::bearer())),
            replay(),
        );
        let reconstructed = OpenAi::at(
            OpenAi::VENDOR,
            Box::new(HeaderKey::new(ApiKey::new(SECRET), Header::bearer())),
            replay(),
        );
        let other = OpenAi::at(
            OpenAi::VENDOR,
            Box::new(HeaderKey::new(ApiKey::new("sk-other"), Header::bearer())),
            replay(),
        );

        assert_eq!(
            first.prompt_cache_route().credential_scope,
            reconstructed.prompt_cache_route().credential_scope
        );
        assert_ne!(
            first.prompt_cache_route().credential_scope,
            other.prompt_cache_route().credential_scope
        );
    }

    #[test]
    fn exact_model_and_input_band_select_current_standard_token_prices() {
        let (provider, _) = provider(200, ANSWER);
        let date = PricingDate::new(2026, 8, 31);
        let short = provider
            .prompt_cache_pricing(
                "gpt-5.6-sol",
                Some("gpt-5.6-sol"),
                Some(272_000),
                PromptCacheRetentionClass::ProviderDefault,
                date,
            )
            .unwrap()
            .unwrap();
        let long = provider
            .prompt_cache_pricing(
                "gpt-5.6-sol",
                Some("gpt-5.6-sol"),
                Some(272_001),
                PromptCacheRetentionClass::ProviderDefault,
                date,
            )
            .unwrap()
            .unwrap();

        assert_eq!(
            short.rates().uncached_input,
            UsageRate::priced(PriceRate::per_million(4_000_000_000))
        );
        assert_eq!(
            short.rates().cache_write_or_creation,
            UsageRate::priced(PriceRate::per_million(5_000_000_000))
        );
        assert_eq!(
            long.rates().uncached_input,
            UsageRate::priced(PriceRate::per_million(8_000_000_000))
        );
    }

    #[test]
    fn older_model_reporting_and_subscription_pricing_remain_exactly_unknown() {
        let (provider, _) = provider(200, ANSWER);
        assert_eq!(
            provider.prompt_cache_capabilities("gpt-5.5").usage(),
            PromptCacheUsageReporting::ReadTokens
        );

        let replay = std::sync::Arc::new(Replay::new(200, ANSWER));
        let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bearer());
        let subscription = OpenAi::at(
            OpenAi::SUBSCRIPTION,
            Box::new(credential),
            Box::new(std::sync::Arc::clone(&replay)),
        );
        assert!(
            subscription
                .prompt_cache_pricing(
                    "gpt-5.6-sol",
                    Some("gpt-5.6-sol"),
                    Some(1_000),
                    PromptCacheRetentionClass::ProviderDefault,
                    PricingDate::new(2026, 8, 31),
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn current_models_advertise_implicit_and_explicit_cache_boundaries() {
        let (provider, _) = provider(200, ANSWER);
        let current = provider.prompt_cache_capabilities("gpt-5.6-sol");
        let older = provider.prompt_cache_capabilities("gpt-5.5");

        let replay = std::sync::Arc::new(Replay::new(200, ANSWER));
        let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bearer());
        let subscription = OpenAi::at(OpenAi::SUBSCRIPTION, Box::new(credential), Box::new(replay));
        let subscription = subscription.prompt_cache_capabilities("gpt-5.6-sol");

        let [automatic, explicit] = current.mechanisms() else {
            panic!("current OpenAI models need automatic and explicit mechanisms");
        };
        assert_eq!(
            automatic.mechanism(),
            crucible_core::PromptCacheMechanism::AutomaticPrefix
        );
        assert_eq!(
            explicit.mechanism(),
            crucible_core::PromptCacheMechanism::ExplicitBreakpoints
        );
        assert_eq!(explicit.maximum_breakpoints(), 4);
        assert_eq!(older.mechanisms().len(), 1);
        assert_eq!(
            subscription
                .mechanisms()
                .first()
                .map(crucible_core::PromptCacheMechanismCapability::mechanism),
            Some(crucible_core::PromptCacheMechanism::AutomaticPrefix)
        );
    }

    #[test]
    fn a_custom_responses_route_never_inherits_first_party_cache_controls() {
        let custom = OpenAi::at(
            Endpoint::parse("https://proxy.invalid/v1/responses").unwrap(),
            Box::new(HeaderKey::new(ApiKey::new(SECRET), Header::bearer())),
            Box::new(Replay::new(200, ANSWER)),
        );

        assert_eq!(
            custom.prompt_cache_capabilities("gpt-5.6-sol").support(),
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

        let provider = OpenAi::at(
            endpoint,
            Box::new(credential),
            Box::new(std::sync::Arc::clone(&replay)),
        );

        provider.stream(asking("hello"), &Cancel::new()).unwrap();

        assert_eq!(replay.sent().url, "http://localhost:8080/v1");
    }

    #[test]
    fn a_subscription_credential_is_served_at_its_own_fixed_address() {
        // The same constructor, the other fixed address: which of the two a
        // credential belongs to is decided by whoever wires it up, and what
        // this asserts is that the choice reaches the transport intact.
        let replay = std::sync::Arc::new(Replay::new(200, ANSWER));
        let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bearer());

        let provider = OpenAi::at(
            OpenAi::SUBSCRIPTION,
            Box::new(credential),
            Box::new(std::sync::Arc::clone(&replay)),
        );

        provider.stream(asking("hello"), &Cancel::new()).unwrap();

        assert_eq!(
            replay.sent().url,
            "https://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn which_service_a_request_is_bound_for_is_read_off_where_it_is_going() {
        // The body differs between the two, and the address is the only thing
        // that says which is receiving it. A provider that built one body for
        // both would be signed in with a plan and refused on every turn.
        let replay = std::sync::Arc::new(Replay::new(200, ANSWER));
        let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bearer());

        let provider = OpenAi::at(
            OpenAi::SUBSCRIPTION,
            Box::new(credential),
            Box::new(std::sync::Arc::clone(&replay)),
        );

        provider.stream(asking("hello"), &Cancel::new()).unwrap();

        assert!(
            !replay.sent().body.contains("max_output_tokens"),
            "the plan backend refuses the whole request over that field"
        );
    }

    fn asking(text: &str) -> Request<'static> {
        let mut transcript = Transcript::new();
        transcript
            .push(Message::said(text))
            .expect("valid fixture transcript");

        Request {
            purpose: crucible_core::RequestPurpose::Turn,
            model: "gpt-test",
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
    fn a_request_goes_to_responses_and_asks_for_a_stream() {
        let (openai, replay) = provider(200, ANSWER);

        openai.stream(asking("hello"), &Cancel::new()).unwrap();

        let sent = replay.sent();
        assert_eq!(sent.url, OpenAi::VENDOR.as_str());
        assert_eq!(header(&sent, "accept"), "text/event-stream");
        assert_eq!(header(&sent, "content-type"), "application/json");
    }

    #[test]
    fn a_request_is_authorised_by_the_credential_it_was_given() {
        // The provider names the header and the prefix; it never sees the key.
        // Same credential kind as the other provider, a different header — the
        // point of keeping authentication off the protocol axis.
        let (openai, replay) = provider(200, ANSWER);

        openai.stream(asking("hello"), &Cancel::new()).unwrap();

        assert_eq!(
            header(&replay.sent(), "authorization"),
            format!("Bearer {SECRET}")
        );
    }

    #[test]
    fn a_provider_does_not_show_its_credential_in_its_debug() {
        // `Provider` is held by the runner and appears in its `Debug`.
        let (openai, _) = provider(200, ANSWER);

        assert!(
            !format!("{openai:?}").contains(SECRET),
            "the key leaked through the provider"
        );
    }

    #[test]
    fn an_accepted_request_is_handed_back_as_the_answer_it_returned() {
        // The end of the round trip: a body that arrived over the transport
        // reaches the caller as deltas, with nothing in between to arrange it.
        let (openai, _) = provider(200, ANSWER);

        let mut stream = openai.stream(asking("hello"), &Cancel::new()).unwrap();

        assert_eq!(
            deltas(stream.as_mut()),
            vec![
                Delta::Text("Hello".into()),
                Delta::Text(", world".into()),
                inclusive_usage(Some(9), None, Some(4)),
                Delta::Stopped(StopReason::Yielded),
            ]
        );
    }

    #[test]
    fn a_refusal_carries_the_status_and_the_sentence_that_explains_it() {
        let said = r#"{"error":{"message":"The model `gpt-nope` does not exist","type":"invalid_request_error"}}"#;
        let (openai, _) = provider(404, said);

        let problem = openai.stream(asking("hello"), &Cancel::new()).unwrap_err();

        assert_eq!(
            problem.to_string(),
            "openai: HTTP 404: The model `gpt-nope` does not exist"
        );
    }

    #[test]
    fn a_refusal_cannot_repeat_raw_or_bearer_credentials() {
        let said = format!(
            r#"{{"error":{{"message":"Bearer {SECRET}; raw {SECRET}; model remains useful"}}}}"#
        );
        let (openai, _) = provider(401, &said);

        let problem = openai.stream(asking("hello"), &Cancel::new()).unwrap_err();
        let displayed = problem.to_string();
        let debugged = format!("{problem:?}");

        assert!(!displayed.contains(SECRET));
        assert!(!debugged.contains(SECRET));
        assert!(displayed.contains("model remains useful"));
    }

    #[test]
    fn a_stream_error_cannot_repeat_raw_or_bearer_credentials() {
        let body = format!(
            "data: {{\"type\":\"error\",\"code\":\"gateway\",\"message\":\"Bearer {SECRET}; raw {SECRET}; model remains useful\"}}\n\n"
        );
        let (openai, _) = provider(200, &body);

        let mut stream = openai.stream(asking("hello"), &Cancel::new()).unwrap();
        let problem = stream.next().unwrap().unwrap_err();
        let displayed = problem.to_string();
        let debugged = format!("{problem:?}");

        assert!(!displayed.contains(SECRET));
        assert!(!debugged.contains(SECRET));
        assert!(displayed.contains("model remains useful"));
    }

    #[test]
    fn a_cancelled_turn_is_never_sent() {
        let (openai, replay) = provider(200, ANSWER);
        let cancel = Cancel::new();
        cancel.request();

        let problem = openai.stream(asking("hello"), &cancel).unwrap_err();

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
    fn openai_spells_no_more_than_its_body_can_write_today() {
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
