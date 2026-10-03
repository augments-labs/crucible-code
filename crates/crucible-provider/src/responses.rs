//! Responses, for every vendor that serves it.
//!
//! The request is written in [`body`], one event of a response is read in
//! [`wire`], and [`Responses`] is the provider that sends one and hands back the
//! other. None of the three has a case for any one vendor. What differs between
//! the vendors that serve this wire is a [`Dialect`]: what the provider is
//! called, the addresses the vendor serves and which of its services each one
//! is, the headers it asks for beside the credential, the parts of the body
//! only its own routes accept, how it asks for a fast answer and refuses one,
//! how it counts what a response cost, how it words a failure, what its
//! prompt cache is known to do and to cost, and where its plan says how much
//! of its limits is used ([`asking`], which [`crate::completions`] asks
//! through too).
//!
//! A vendor on this wire is a dialect and nothing else: a type that implements
//! [`Dialect`], and a name for `Responses` over it. Every hook but the ones
//! that say who the vendor is has a default, and the default is the plain
//! wire: the body every vendor here reads, no fast form, and nothing claimed
//! about a cache.
//!
//! One vendor's models hand back items of their own that a later request must
//! carry unchanged. That is a [`Replay`], which a dialect names and the writer
//! and reader hand a request over to. A vendor that keeps nothing names
//! [`Plain`].
//!
//! This is not [`crate::completions`], which posts to Chat Completions and
//! reads the older shape: a transcript there is a list of messages, and here it
//! is a flat list of items narrated back item by item.
//!
//! It names no HTTP client and no credential kind. A [`Transport`] is handed in
//! and so is a [`Credential`], which is what lets the whole protocol be tested
//! against recorded bytes.

pub(crate) mod asking;
pub(crate) mod body;
pub(crate) mod wire;

use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::SystemTime;

use crucible_credentials::{Credential, Outgoing, Redactions};
use crucible_models::{
    Asked, Delta, DeltaStream, FastForm, PromptCacheCapabilities, PromptCachePricing,
    PromptCacheProvenance, PromptCacheRoute, Provider, ProviderError, Request, Served, Speed,
};
use crucible_runtime::{BoxFuture, Cancel};
use crucible_types::{
    ContinuationScope, CredentialScopeId, Modalities, PlanWindows, PricingDate, PricingError,
    PromptCacheEncoding, PromptCacheRetentionClass,
};
use serde_json::Value;

use crate::endpoint::Endpoint;
use crate::json::Object;
use crate::refusal::{Plan, PlanRule, Rules, refused_at};
use crate::sse::SseEvent;
use crate::stream::{Limited, Response};
use crate::transport::{Named, Reads, Transport, reads_none};

/// What writes a field the vendor's automatic prefix cache reads, into the
/// body being written.
pub(crate) type Hint = fn(&mut Object<'_>);

/// Where a vendor's plan says how much of its limits is used, and how what it
/// says there is read.
#[derive(Debug, Clone, Copy)]
pub struct Usage {
    /// The address asked, on the vendor's own host.
    pub(crate) url: &'static str,
    /// The windows an answer says, read from one that arrived at the instant
    /// given; `None` where it is not the shape the vendor answers in.
    pub(crate) read: fn(&Value, SystemTime) -> Option<PlanWindows>,
}

/// What a price is asked for: one model, at one revision, for a prompt of
/// one size kept for one time, on one day.
#[derive(Debug, Clone, Copy)]
pub struct Priced<'a> {
    /// The model, as requests name it.
    pub model: &'a str,
    /// The revision the cache records name it by.
    pub revision: Option<&'a str>,
    /// How much the request carries, where that is known.
    pub input_tokens: Option<u64>,
    /// How long its prefix is asked to be kept.
    pub retention: PromptCacheRetentionClass,
    /// The day the price is asked for.
    pub at: PricingDate,
}

/// What a vendor's cache is known to do where it caches a prompt's prefix on
/// its own, takes a key that routes requests to the same cache, and reports
/// what it read: the record for `revision`, reviewed at `provenance`, assuming
/// no hit below `minimum` tokens.
pub(crate) fn automatic(
    revision: &'static str,
    provenance: PromptCacheProvenance,
    minimum: u32,
) -> PromptCacheCapabilities {
    let routed = crucible_models::PromptCacheMechanismCapability::automatic_prefix(
        minimum,
        true,
        false,
        &[
            crucible_models::PromptCacheContent::Text,
            crucible_models::PromptCacheContent::Tools,
        ],
    );
    PromptCacheCapabilities::supported(
        provenance.record_version(),
        Some(revision),
        provenance,
        crucible_models::StatefulTransportCapability::Unsupported,
        &[routed],
        crucible_types::PromptCacheUsageReporting::ReadTokens,
    )
}

/// What one vendor on this wire does its own way.
///
/// Everything here is about the vendor rather than about one request, so none
/// of it takes `self`: a dialect is a type, and [`Responses`] is generic over
/// it.
pub trait Dialect: Sized + Send + Sync + 'static {
    /// What the provider is called, in errors and in the status line.
    const NAME: &'static str;

    /// What the provider is shown as in `Debug`.
    const TITLE: &'static str;

    /// The addresses the vendor serves, its own first. Any other is one a
    /// setting named, and nothing is assumed about what it does.
    const ADDRESSES: &'static [Endpoint];

    /// What the prompt cache records the protocol as.
    const PROTOCOL: &'static str;

    /// What the prompt cache records a request's shape as, so a change to the
    /// shape is not read as the same cache.
    const SHAPE: &'static str;

    /// Whether a function tool says `strict: false` rather than leaving the
    /// vendor's default to decide how its arguments are checked.
    const STRICT: bool = true;

    /// Whether what the model said before calling a tool goes back marked as
    /// commentary rather than as an answer.
    const COMMENTARY: bool = false;

    /// What `service_tier` says in a request asking for a fast answer.
    const FAST_TIER: Option<&'static str> = None;

    /// Whether the vendor may close a stream with a `[DONE]` line after the
    /// event that ended the response.
    const SENTINEL: bool = false;

    /// Which of the vendor's services an address is, where they do not accept
    /// the same body. `()` where there is one.
    type Route: Copy + Eq + fmt::Debug + Send + Sync + 'static;

    /// Items the vendor's models hand back to be sent again unchanged.
    /// [`Plain`] where there are none.
    type Replay: Replay;

    /// Which service `endpoint` is; an address a setting named is read as the
    /// service a key is sent to.
    fn route(endpoint: &Endpoint) -> Self::Route;

    /// The kinds of file the vendor's models take.
    fn spells() -> Modalities;

    /// Headers the vendor asks for beside the content type, the stream it
    /// answers in and the credential, which [`Responses`] sets itself.
    fn headers(outgoing: &mut Outgoing) {
        let _ = outgoing;
    }

    /// Whether `route` takes `max_output_tokens`.
    fn ceiling(route: Self::Route) -> bool {
        let _ = route;
        true
    }

    /// Whether `route` takes an explicit cache breakpoint.
    fn breakpoints(route: Self::Route) -> bool {
        let _ = route;
        false
    }

    /// What asks the automatic prefix cache to keep `model`'s prefix for
    /// `retention`, or nothing where the vendor has no field for it.
    fn retention(model: &str, retention: PromptCacheRetentionClass) -> Option<Hint> {
        let _ = (model, retention);
        None
    }

    /// Whether `model`'s usage counts what was written to the cache.
    fn cache_writes(model: &str) -> bool {
        let _ = model;
        false
    }

    /// What a response cost, from the event that ended it.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Protocol`] where the counts contradict each other.
    fn usage(payload: &Value, cache_writes: bool) -> Result<Option<Delta>, ProviderError> {
        wire::usage::<Self>(payload, cache_writes)
    }

    /// Where a failure event keeps its code and its words.
    fn failure(event: &Value) -> &Value {
        event
    }

    /// A refused request's error, as the vendor's body is read; `redactions`
    /// hold what any words lifted out of it must not show.
    fn refusal(error: ProviderError, redactions: &Redactions) -> ProviderError {
        let _ = redactions;
        error
    }

    /// How `model` is asked to answer fast on `route`.
    fn fast(route: Self::Route, model: &str) -> FastForm {
        let _ = (route, model);
        FastForm::None
    }

    /// The vendor's refusal of a request too large for the model's window,
    /// where it sends that refusal with no code: a refusal this answers yes
    /// for, from its status and its body read whole, is
    /// [`ProviderError::WindowExceeded`], which the session is compacted for.
    /// Written as the shape that refusal comes in and one exact phrase, case
    /// and all, anchored where the vendor puts it; every other vendor's
    /// refusals are told by their code alone.
    /// None, by default.
    const OVERLONG: Option<fn(u16, &Value) -> bool> = None;

    /// What tells a refusal of the fast tier on `route` from any other.
    fn fast_refused(route: Self::Route) -> Option<fn(u16, &str) -> bool> {
        let _ = route;
        None
    }

    /// What tells the vendor's refusal of a used-up plan on `route` from any
    /// other: a refusal that answers yes is [`ProviderError::PlanLimit`],
    /// which is never retried. None, by default, where the vendor has no
    /// such refusal or its shape is not known.
    fn plan_refused(route: Self::Route) -> Option<PlanRule> {
        let _ = route;
        None
    }

    /// The tier an event says the answer was served at, where it says one.
    fn served(data: &str) -> Option<Served> {
        let _ = data;
        None
    }

    /// Which response headers `route` reports its subscription's usage
    /// windows in, which the transport hands back and nothing else. None, by
    /// default.
    fn limit_headers(route: Self::Route) -> Option<Reads> {
        let _ = route;
        None
    }

    /// Where `route`'s plan says how much of its limits is used, and how its
    /// answer is read. None, by default: a route with no such source is never
    /// asked.
    fn usage_source(route: Self::Route) -> Option<Usage> {
        let _ = route;
        None
    }

    /// The usage windows `named`, the headers [`Self::limit_headers`] reads,
    /// say, read from a response that arrived at `arrived`; `None` where they
    /// report no window crucible knows.
    fn limits(named: &Named, arrived: SystemTime) -> Option<PlanWindows> {
        let _ = (named, arrived);
        None
    }

    /// What the vendor's cache is known to do for `model` on `route`.
    fn prompt_cache(route: Self::Route, model: &str) -> PromptCacheCapabilities {
        let _ = (route, model);
        PromptCacheCapabilities::unknown("unreviewed model")
    }

    /// What `asked` costs on `route`, or nothing where `route` is not one of
    /// the vendor's own or the price is not known.
    ///
    /// # Errors
    ///
    /// [`PricingError`] where a reviewed price does not make a valid record.
    fn prompt_cache_pricing(
        route: Option<Self::Route>,
        asked: Priced<'_>,
    ) -> Result<Option<PromptCachePricing>, PricingError> {
        let _ = (route, asked);
        Ok(None)
    }
}

/// Items a model handed back, sent again as they were.
///
/// Such a model's earlier turns are not written from the transcript: they are
/// the items it answered with, kept whole beside the turn, and its answers are
/// read so that they can be. The writer and the reader hand a request for such
/// a model over to this, and to nothing else.
pub trait Replay: fmt::Debug + Send + Sized + 'static {
    /// Whether `model`'s turns go back as the items it answered with.
    fn replays(model: &str) -> bool;

    /// A reader for one answer to `request`, bound to `scope`.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Protocol`] where what is kept cannot be started.
    fn reading(request: &Request<'_>, scope: ContinuationScope) -> Result<Self, ProviderError>;

    /// What one event of that answer means.
    ///
    /// # Errors
    ///
    /// Whatever the answer did that cannot be kept.
    fn deltas(&mut self, event: &SseEvent) -> Result<Vec<Delta>, ProviderError>;

    /// Writes how hard to think and the input, for a request that replays.
    /// `explicit` is the message an explicit cache breakpoint marks.
    ///
    /// # Errors
    ///
    /// Whatever in the kept items cannot be sent again.
    fn write(
        body: &mut Object<'_>,
        request: &Request<'_>,
        scope: ContinuationScope,
        explicit: Option<usize>,
    ) -> Result<(), ProviderError>;

    /// What a refusal of a request that replays may say.
    fn refusal(error: ProviderError) -> ProviderError {
        error
    }
}

/// No items handed back: every turn is written from the transcript.
#[derive(Debug, Default)]
pub struct Plain;

impl Replay for Plain {
    fn replays(model: &str) -> bool {
        let _ = model;
        false
    }

    fn reading(request: &Request<'_>, scope: ContinuationScope) -> Result<Self, ProviderError> {
        let _ = (request, scope);
        Ok(Self)
    }

    fn deltas(&mut self, event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
        let _ = event;
        Ok(Vec::new())
    }

    fn write(
        body: &mut Object<'_>,
        request: &Request<'_>,
        scope: ContinuationScope,
        explicit: Option<usize>,
    ) -> Result<(), ProviderError> {
        let _ = (body, request, scope, explicit);
        Ok(())
    }
}

/// A Responses provider, speaking `D`'s dialect.
pub struct Responses<D: Dialect> {
    credential: Arc<dyn Credential>,
    transport: Arc<dyn Transport>,
    endpoint: Endpoint,
    credential_scope: CredentialScopeId,
    dialect: PhantomData<D>,
}

impl<D: Dialect> fmt::Debug for Responses<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(D::TITLE)
            .field("credential", &self.credential)
            .field("transport", &self.transport)
            .field("endpoint", &self.endpoint)
            .field("credential_scope", &self.credential_scope)
            .finish()
    }
}

impl<D: Dialect> Responses<D> {
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
            credential: credential.into(),
            transport: transport.into(),
            endpoint,
            credential_scope,
            dialect: PhantomData,
        }
    }

    /// How `model` is asked to answer fast at the vendor's own address: what
    /// a list of models says before any provider is set up.
    #[must_use]
    pub fn fast_at_vendor(model: &str) -> FastForm {
        D::ADDRESSES
            .first()
            .map_or(FastForm::None, |address| D::fast(D::route(address), model))
    }

    /// The vendor's service this provider posts to, or `None` where a setting
    /// named the address: what a gateway does is not the vendor's to say.
    fn vendor(&self) -> Option<D::Route> {
        D::ADDRESSES
            .contains(&self.endpoint)
            .then(|| D::route(&self.endpoint))
    }

    /// The headers every request carries, including the secret.
    async fn headers(&self, cancel: &Cancel) -> Result<Outgoing, ProviderError> {
        let mut outgoing = Outgoing::new();
        outgoing.set_header("content-type", "application/json");
        outgoing.set_header("accept", "text/event-stream");
        D::headers(&mut outgoing);

        // Raced against the turn's cancel: a credential renewing its token
        // waits for a renewal that is the renewal's own work, so a turn
        // stopped meanwhile stops waiting here and leaves it to finish.
        cancel
            .race(self.credential.authorize(&mut outgoing))
            .await
            .ok_or(ProviderError::Cancelled(D::NAME))?
            .map_err(|source| ProviderError::Credential {
                provider: D::NAME,
                source,
            })?;

        Ok(outgoing)
    }
}

impl<D: Dialect> Provider for Responses<D> {
    fn name(&self) -> &'static str {
        D::NAME
    }

    fn spells(&self) -> Modalities {
        D::spells()
    }

    fn prompt_cache_capabilities(&self, model: &str) -> PromptCacheCapabilities {
        match self.vendor() {
            Some(route) => D::prompt_cache(route, model),
            None => PromptCacheCapabilities::unknown("custom endpoint"),
        }
    }

    fn prompt_cache_pricing(
        &self,
        model: &str,
        revision: Option<&str>,
        input_tokens: Option<u64>,
        retention: PromptCacheRetentionClass,
        at: PricingDate,
    ) -> Result<Option<PromptCachePricing>, PricingError> {
        D::prompt_cache_pricing(
            self.vendor(),
            Priced {
                model,
                revision,
                input_tokens,
                retention,
                at,
            },
        )
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: D::PROTOCOL,
            endpoint: self.endpoint.as_str(),
            custom_endpoint: self.vendor().is_none(),
            credential_scope: self.credential_scope,
            account: None,
            project: None,
            request_shape_version: D::SHAPE,
        }
    }

    fn prompt_cache_encoding(&self, request: &Request<'_>) -> PromptCacheEncoding {
        body::prompt_cache_encoding::<D>(request, D::route(&self.endpoint))
    }

    fn fast(&self, model: &str) -> FastForm {
        self.vendor()
            .map_or(FastForm::None, |route| D::fast(route, model))
    }

    fn stream<'a>(
        &'a self,
        request: Request<'a>,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Box<dyn DeltaStream>, ProviderError>> {
        self.stream_at(request, Speed::Standard, cancel)
    }

    fn ask_limits(&self) -> Option<BoxFuture<'static, Asked>> {
        // Only the vendor's own services are asked: a gateway's address is not
        // where the vendor keeps a plan.
        let usage = self.vendor().and_then(D::usage_source)?;
        Some(Box::pin(asking::ask(
            D::NAME,
            usage,
            Arc::clone(&self.credential),
            Arc::clone(&self.transport),
            D::headers,
        )))
    }

    fn stream_at<'a>(
        &'a self,
        request: Request<'a>,
        speed: Speed,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Box<dyn DeltaStream>, ProviderError>> {
        Box::pin(async move {
            // Nothing is sent for a turn the user has already abandoned. Once the
            // request is away, cancelling is the stream's business.
            if cancel.requested() {
                return Err(ProviderError::Cancelled(D::NAME));
            }

            // Fast only where it was asked and this model on this route has a
            // form the request carries; everywhere else the request is the
            // ordinary one.
            let fast = speed == Speed::Fast && self.fast(request.model).switched();
            let mut outgoing = self.headers(cancel).await?;
            let scope = ContinuationScope::new(self.credential_scope, self.endpoint.as_str());
            let replays = D::Replay::replays(request.model);
            let body = body::serialize::<D>(
                &request,
                D::route(&self.endpoint),
                replays.then_some(scope),
                fast,
            )?;

            // Only the vendor's own services are read for their windows: what a
            // gateway puts in a header is not the vendor's to say.
            let response = self
                .transport
                .post_reading(
                    self.endpoint.as_str(),
                    &mut outgoing,
                    body,
                    cancel,
                    self.vendor()
                        .and_then(D::limit_headers)
                        .unwrap_or(reads_none),
                )
                .await;
            let redactions = outgoing.redactions();
            let response =
                response.map_err(|problem| problem.for_provider(D::NAME).redacted(&redactions))?;

            // Read before the status is, because a vendor that reports its
            // windows reports them on a refusal too, and a refusal of a
            // used-up plan is when they matter most.
            let arrived = SystemTime::now();
            let limits = self
                .vendor()
                .and_then(D::limit_headers)
                .and_then(|_| D::limits(response.named(), arrived));

            if response.status() != 200 {
                // A refusal of the tier is read for only where one is
                // documented, and only for a request that asked for it.
                let rule = self.vendor().filter(|_| fast).and_then(D::fast_refused);
                let plan = self.vendor().and_then(D::plan_refused).map(|rule| Plan {
                    rule,
                    arrived,
                    reading: limits,
                    model: request.model,
                });
                let error = refused_at(
                    D::NAME,
                    Rules {
                        fast: rule,
                        overlong: D::OVERLONG,
                        plan,
                    },
                    response,
                    &redactions,
                    cancel,
                )
                .await;
                let error = D::refusal(error, &redactions);
                return Err(if replays {
                    D::Replay::refusal(error)
                } else {
                    error
                });
            }

            let response = Response::with_wire(
                response.into_reader(),
                cancel.clone(),
                redactions,
                wire::Narration::<D>::for_request(&request, scope)?,
            );
            Ok(match limits {
                Some(limits) => Box::new(Limited::new(response, limits)) as Box<dyn DeltaStream>,
                None => Box::new(response),
            })
        })
    }
}
