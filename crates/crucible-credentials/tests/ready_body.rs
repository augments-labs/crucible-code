//! Every shipped `authorize` future answers the first time it is polled.
//!
//! A synchronous caller crosses through `crucible-runtime`'s `Bridge`, which
//! this crate cannot name — it keeps `crucible-types` as its only internal
//! dependency. Standing in for that one poll here, in an integration test
//! rather than in the crate's own shipped source, is what keeps the poll
//! machinery out of the files `scripts/python/bridge-ledger.py` treats as
//! polling a future by hand instead of crossing a bridge.

use std::task::{Context, Poll, Waker};

use crucible_credentials::{
    ApiKey, Authorization, Credential, CredentialError, Header, HeaderKey, Outgoing,
};
use crucible_types::CredentialScopeId;

/// The exact string that must never appear anywhere but the header value.
const SECRET: &str = "sk-ant-do-not-log-me";

/// Polls `authorizing` once and panics if it was not ready: every
/// implementation this crate ships answers at its first poll, which is what
/// keeps a synchronous crossing that polls once prerequisite-free.
#[allow(clippy::panic)] // A credential that would wait is a test failure.
#[track_caller]
fn authorized(authorizing: Authorization<'_>) -> Result<(), CredentialError> {
    let mut authorizing = authorizing;
    match authorizing
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(answer) => answer,
        Poll::Pending => panic!("a credential shipped here would have had to wait"),
    }
}

/// One header's value, by name.
fn header<'a>(request: &'a Outgoing, name: &str) -> &'a str {
    request
        .headers()
        .iter()
        .find(|(present, _)| &**present == name)
        .map_or("<no such header>", |(_, value)| value)
}

#[test]
fn a_header_keys_future_answers_the_first_time_it_is_polled() {
    // A tool run's crossing polls a future once; a `HeaderKey` that pended
    // there would be refused on every request it signs.
    let mut request = Outgoing::new();
    let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bearer());
    let mut authorizing = credential.authorize(&mut request);

    assert!(
        matches!(
            authorizing
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Ok(()))
        ),
        "the future was still pending after one poll"
    );
}

#[test]
fn a_variable_padded_by_a_paste_sends_the_key_and_not_the_padding() {
    // The two shapes a key arrives padded in: a space from a paste, and a
    // CRLF from a key file written on Windows. The header value has to be
    // what a key set correctly would send, since the alternative is a 401
    // or a rejected header that names neither the variable nor the space.
    let mut padded = Outgoing::new();
    let key = ApiKey::from_lookup("KEY", |_| Some(format!(" {SECRET} \r\n"))).unwrap();
    authorized(HeaderKey::new(key, Header::bare("x-api-key")).authorize(&mut padded)).unwrap();

    assert_eq!(header(&padded, "x-api-key"), SECRET);
}

#[test]
fn anthropic_and_openai_send_the_same_key_differently() {
    // One credential type, two wire conventions — this is why a credential
    // is handed a `Header` instead of hard-coding either.
    let key = ApiKey::new(SECRET);

    let mut anthropic = Outgoing::new();
    authorized(HeaderKey::new(key.clone(), Header::bare("x-api-key")).authorize(&mut anthropic))
        .unwrap();
    assert_eq!(header(&anthropic, "x-api-key"), SECRET);

    let mut openai = Outgoing::new();
    authorized(HeaderKey::new(key, Header::bearer()).authorize(&mut openai)).unwrap();
    assert_eq!(header(&openai, "authorization"), format!("Bearer {SECRET}"));
}

/// A credential that has to renew something before it can answer, and
/// cannot. The shape one takes when what it holds has expired and renewing
/// it was refused.
#[derive(Debug)]
struct Stale;

impl Credential for Stale {
    fn scope(&self) -> CredentialScopeId {
        CredentialScopeId::new()
    }

    fn authorize<'a>(&'a self, _request: &'a mut Outgoing) -> Authorization<'a> {
        Box::pin(std::future::ready(Err(CredentialError::NotRenewed(
            "the login has expired".into(),
        ))))
    }
}

#[test]
fn a_credential_that_cannot_renew_says_so_and_writes_nothing() {
    // Authentication fails at two moments, not one. This is the second: the
    // credential was found, and what it holds is no longer good. A request
    // half-authorised is worse than one never sent, so a failure here leaves
    // the headers as they were.
    let mut request = Outgoing::new();

    let problem = authorized(Stale.authorize(&mut request)).unwrap_err();

    assert_eq!(problem.to_string(), "the login has expired");
    assert!(request.headers().is_empty(), "the request was written to");
}
