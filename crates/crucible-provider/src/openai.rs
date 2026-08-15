//! The OpenAI provider.
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
mod stream;
mod wire;

use crucible_core::{Cancel, Credential, DeltaStream, Outgoing, Provider, ProviderError, Request};

use crate::endpoint::Endpoint;
use crate::openai::stream::Stream;
use crate::refusal::refused;
use crate::transport::Transport;

/// What this provider is called, in errors and in the status line.
const NAME: &str = "openai";

/// Where requests go unless a setting says otherwise.
const VENDOR: Endpoint = Endpoint::fixed("https://api.openai.com/v1/responses");

/// OpenAI's Responses API.
#[derive(Debug)]
pub struct OpenAi {
    credential: Box<dyn Credential>,
    transport: Box<dyn Transport>,
    endpoint: Endpoint,
}

impl OpenAi {
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
        Self {
            credential,
            transport,
            endpoint,
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
        let body = body::serialize(&request);

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

        Ok(Box::new(Stream::new(
            response.body,
            cancel.clone(),
            redactions,
        )))
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::{ApiKey, Delta, Header, HeaderKey, Message, StopReason, Transcript};

    use super::stream::tests::{ANSWER, deltas};
    use super::*;
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

    fn asking(text: &str) -> Request<'static> {
        let mut transcript = Transcript::new();
        transcript.push(Message::User(text.into()));

        Request {
            model: "gpt-test",
            transcript: Box::leak(Box::new(transcript)),
            tools: &[],
            max_tokens: 1024,
            system: None,
            effort: None,
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
}
