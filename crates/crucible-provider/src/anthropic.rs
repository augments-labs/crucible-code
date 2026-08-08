//! The Anthropic provider.
//!
//! Four parts, and the split is the direction data travels: [`body`] builds a
//! request, [`wire`] reads one event of a response, [`stream`] delivers a whole
//! one, and this file is the request itself — the headers and the status.
//!
//! It names no HTTP client and no credential kind. A [`Transport`] is handed in
//! and so is a [`Credential`], which is what lets the whole protocol be tested
//! against recorded bytes.

mod body;
mod stream;
mod wire;

use std::io::Read;

use crucible_core::{Cancel, Credential, DeltaStream, Outgoing, Provider, ProviderError, Request};

use crate::anthropic::stream::Stream;
use crate::transport::Transport;

/// What this provider is called, in errors and in the status line.
const NAME: &str = "anthropic";

/// Where requests go.
const URL: &str = "https://api.anthropic.com/v1/messages";

/// The API version this speaks. Anthropic pins behaviour to it, so a new one is
/// a deliberate change here rather than something that drifts.
const VERSION: &str = "2023-06-01";

/// The most of a refusal to read before giving up on it.
///
/// A refusal is a sentence. Anything larger is a proxy's error page, and
/// reading all of it to print a paragraph of HTML helps nobody.
const MAX_REFUSAL: u64 = 8 * 1024;

/// Anthropic's Messages API.
#[derive(Debug)]
pub struct Anthropic {
    credential: Box<dyn Credential>,
    transport: Box<dyn Transport>,
}

impl Anthropic {
    /// A provider that authenticates with `credential` and sends over
    /// `transport`.
    #[must_use]
    pub fn new(credential: Box<dyn Credential>, transport: Box<dyn Transport>) -> Self {
        Self {
            credential,
            transport,
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
            .post(URL, outgoing.headers(), &body)
            .map_err(|problem| ProviderError::Transport {
                provider: NAME,
                problem: problem.to_string().into(),
            })?;

        if response.status != 200 {
            return Err(refused(response.status, response.body));
        }

        Ok(Box::new(Stream::new(response.body, cancel.clone())))
    }
}

/// A refusal, with the sentence the provider sent.
fn refused(status: u16, body: Box<dyn Read + Send>) -> ProviderError {
    let mut said = Vec::new();
    let read = body.take(MAX_REFUSAL).read_to_end(&mut said);

    let message = match read {
        // Lossy on purpose: this is already the failure path, and a message
        // that is not quite text is still better than no message.
        Ok(_) => explain(&String::from_utf8_lossy(&said)),
        Err(problem) => format!("the response could not be read: {problem}"),
    };

    ProviderError::Refused {
        provider: NAME,
        status,
        message: message.into(),
    }
}

/// The sentence inside a refusal body.
///
/// Falls back to the body itself, because a proxy or a gateway in front of the
/// API refuses in its own shape and that text is still what a user needs.
fn explain(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .as_ref()
        .and_then(|payload| payload.get("error"))
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .map_or_else(|| body.trim().to_owned(), ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use crucible_core::{ApiKey, Delta, HeaderKey, Message, StopReason, Transcript};

    use super::stream::tests::{ANSWER, deltas};
    use super::*;
    use crate::transport::{Replay, Sent};

    /// The exact key that must never appear anywhere but a header value.
    const SECRET: &str = "sk-ant-do-not-log-me";

    fn provider(status: u16, body: &str) -> (Anthropic, std::sync::Arc<Replay>) {
        let replay = std::sync::Arc::new(Replay::new(status, body));
        let credential = HeaderKey::new(ApiKey::new(SECRET), "x-api-key", "");

        (
            Anthropic::new(
                Box::new(credential),
                Box::new(std::sync::Arc::clone(&replay)),
            ),
            replay,
        )
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
        assert_eq!(sent.url, URL);
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
