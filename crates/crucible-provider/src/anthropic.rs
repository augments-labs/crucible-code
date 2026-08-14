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
mod stream;
mod wire;

use crucible_core::{Cancel, Credential, DeltaStream, Outgoing, Provider, ProviderError, Request};

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

/// Anthropic's Messages API.
#[derive(Debug)]
pub struct Anthropic {
    credential: Box<dyn Credential>,
    transport: Box<dyn Transport>,
    endpoint: Endpoint,
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
        Self {
            credential,
            transport,
            endpoint,
        }
    }

    /// The headers every request carries, including the secret.
    fn headers(&self) -> Result<Outgoing, ProviderError> {
        let mut outgoing = Outgoing::new();
        outgoing.set_header("content-type", "application/json");
        outgoing.set_header("anthropic-version", VERSION);
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

impl Provider for Anthropic {
    fn name(&self) -> &'static str {
        NAME
    }

    fn stream(
        &self,
        request: Request,
        cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        // Nothing is sent for a turn the user has already abandoned. Once the
        // request is away, cancelling is the stream's business.
        if cancel.requested() {
            return Err(ProviderError::Cancelled(NAME));
        }

        let outgoing = self.headers()?;
        let body = body::build(&request).to_string();

        let response = self
            .transport
            .post(self.endpoint.as_str(), outgoing.headers(), &body)
            .map_err(|problem| ProviderError::Transport {
                provider: NAME,
                problem: problem.to_string().into(),
            })?;

        if response.status != 200 {
            return Err(refused(NAME, response.status, response.body));
        }

        Ok(Box::new(Stream::new(response.body, cancel.clone())))
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::{ApiKey, Delta, Header, HeaderKey, Message, StopReason, Transcript};

    use super::stream::tests::{ANSWER, deltas};
    use super::*;
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

    fn asking(text: &str) -> Request {
        let mut transcript = Transcript::new();
        transcript.push(Message::User(text.into()));

        Request {
            model: "claude-test".into(),
            transcript,
            tools: Vec::new(),
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
}
