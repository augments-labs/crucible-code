use crucible_core::{
    ApiKey, Carried, Delta, Header, HeaderKey, Message, Spend, StopReason, Transcript,
};

use super::stream::tests::{ANSWER, deltas};
use super::*;
use crate::transport::{Replay, Sent};

/// The exact key that must never appear anywhere but a header value.
const SECRET: &str = "sk-do-not-log-me";

fn provider(endpoint: Endpoint, body: &str, status: u16) -> (Moonshot, std::sync::Arc<Replay>) {
    let replay = std::sync::Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(ApiKey::new(SECRET), Header::bearer());

    (
        Moonshot::at(
            endpoint,
            Box::new(credential),
            Box::new(std::sync::Arc::clone(&replay)),
        ),
        replay,
    )
}

fn asking(text: &str) -> Request<'static> {
    let mut transcript = Transcript::new();
    transcript.push(Message::said(text));

    Request {
        model: "kimi-test",
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
fn a_key_reaches_whichever_of_the_two_addresses_it_belongs_to() {
    // The one thing about this vendor that is not like the others: a key is
    // issued against one console and refused by the other, and nothing in the
    // key says which. Both addresses are named, and the caller's choice is what
    // reaches the transport — a provider that read it and posted to a constant
    // would send half its users' keys to the wrong host.
    for endpoint in [Moonshot::CODING, Moonshot::PLATFORM] {
        let address = endpoint.as_str().to_owned();
        let (moonshot, replay) = provider(endpoint, ANSWER, 200);

        moonshot.stream(asking("hello"), &Cancel::new()).unwrap();

        assert_eq!(replay.sent().url, address);
    }

    assert_ne!(Moonshot::CODING.as_str(), Moonshot::PLATFORM.as_str());
}

#[test]
fn a_request_says_what_is_calling_and_asks_for_a_stream() {
    // The identifier is this vendor's terms rather than politeness: a client
    // that misreports itself is in violation of them.
    let (moonshot, replay) = provider(Moonshot::CODING, ANSWER, 200);

    moonshot.stream(asking("hello"), &Cancel::new()).unwrap();

    let sent = replay.sent();
    assert_eq!(header(&sent, "user-agent"), AGENT);
    assert!(
        header(&sent, "user-agent").starts_with("crucible/"),
        "the identifier does not name this harness"
    );
    assert_eq!(header(&sent, "accept"), "text/event-stream");
    assert_eq!(header(&sent, "content-type"), "application/json");
}

#[test]
fn a_request_is_authorised_by_the_credential_it_was_given() {
    let (moonshot, replay) = provider(Moonshot::CODING, ANSWER, 200);

    moonshot.stream(asking("hello"), &Cancel::new()).unwrap();

    assert_eq!(
        header(&replay.sent(), "authorization"),
        format!("Bearer {SECRET}")
    );
}

#[test]
fn a_provider_does_not_show_its_credential_in_its_debug() {
    let (moonshot, _) = provider(Moonshot::CODING, ANSWER, 200);

    assert!(
        !format!("{moonshot:?}").contains(SECRET),
        "the key leaked through the provider"
    );
}

#[test]
fn an_accepted_request_is_handed_back_as_the_answer_it_returned() {
    let (moonshot, _) = provider(Moonshot::CODING, ANSWER, 200);

    let mut stream = moonshot.stream(asking("hello"), &Cancel::new()).unwrap();

    assert_eq!(
        deltas(stream.as_mut()),
        vec![
            Delta::Text("Hello".into()),
            Delta::Text(", world".into()),
            Delta::Stopped(StopReason::Yielded),
            Delta::Carried(Carried::new(9)),
            Delta::Spent(Spend::new(4)),
        ]
    );
}

#[test]
fn a_refusal_carries_the_status_and_the_sentence_that_explains_it() {
    let said =
        r#"{"error":{"type":"invalid_authentication_error","message":"Invalid Authentication"}}"#;
    let (moonshot, _) = provider(Moonshot::PLATFORM, said, 401);

    let problem = moonshot
        .stream(asking("hello"), &Cancel::new())
        .unwrap_err();

    assert_eq!(
        problem.to_string(),
        "moonshot: HTTP 401: Invalid Authentication"
    );
}

#[test]
fn a_refusal_cannot_repeat_raw_or_bearer_credentials() {
    let said = format!(
        r#"{{"error":{{"message":"Bearer {SECRET}; raw {SECRET}; model remains useful"}}}}"#
    );
    let (moonshot, _) = provider(Moonshot::PLATFORM, &said, 401);

    let problem = moonshot
        .stream(asking("hello"), &Cancel::new())
        .unwrap_err();
    let displayed = problem.to_string();
    let debugged = format!("{problem:?}");

    assert!(!displayed.contains(SECRET));
    assert!(!debugged.contains(SECRET));
    assert!(displayed.contains("model remains useful"));
}

#[test]
fn a_stream_error_cannot_repeat_raw_or_bearer_credentials() {
    let body = format!(
        "data: {{\"error\":{{\"type\":\"gateway\",\"message\":\"Bearer {SECRET}; raw {SECRET}; model remains useful\"}}}}\n\n"
    );
    let (moonshot, _) = provider(Moonshot::PLATFORM, &body, 200);

    let mut stream = moonshot.stream(asking("hello"), &Cancel::new()).unwrap();
    let problem = stream.next().unwrap().unwrap_err();
    let displayed = problem.to_string();
    let debugged = format!("{problem:?}");

    assert!(!displayed.contains(SECRET));
    assert!(!debugged.contains(SECRET));
    assert!(displayed.contains("model remains useful"));
}

#[test]
fn a_cancelled_turn_is_never_sent() {
    let (moonshot, replay) = provider(Moonshot::CODING, ANSWER, 200);
    let cancel = Cancel::new();
    cancel.request();

    let problem = moonshot.stream(asking("hello"), &cancel).unwrap_err();

    assert!(matches!(problem, ProviderError::Cancelled(_)));
    assert!(
        replay.sent().url.is_empty(),
        "a request went out for a turn the user had abandoned"
    );
}
/// What a wire protocol declares is what its body writes, and until a body
/// writes an attachment that is text and nothing else. A declaration that
/// ran ahead of the body would be read as permission to send bytes this
/// module has no shape for.
#[test]
fn moonshot_spells_no_more_than_its_body_can_write_today() {
    let (provider, _replay) = provider(Moonshot::CODING, ANSWER, 200);

    assert_eq!(
        provider.spells(),
        Modalities::empty().insert(Modality::Text),
    );
}
