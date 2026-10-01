//! What Kimi is called when it is shown, and what its cache is keyed by: two
//! things no request or response carries, held to what they were before the
//! wire was shared.

use crucible_credentials::{ApiKey, Header, HeaderKey};

use super::*;
use crate::transport::Replay;

/// Kimi at `endpoint`, over a transport nothing is asked of.
fn kimi(endpoint: Endpoint) -> Moonshot {
    Moonshot::at(
        endpoint,
        Box::new(HeaderKey::new(
            ApiKey::new("fabricated-kimi-key"),
            Header::bearer(),
        )),
        Box::new(Replay::new(200, "")),
    )
}

#[test]
fn kimi_is_shown_under_its_own_name_with_the_fields_it_always_had() {
    // Each field as it shows itself, made again rather than read back out of
    // the provider: the whole text is the name and those four, in that order,
    // as the struct Kimi was before its wire was shared printed it.
    let credential = HeaderKey::new(ApiKey::new("fabricated-kimi-key"), Header::bearer());
    let expected = format!(
        "Moonshot {{ credential: {credential:?}, transport: {:?}, endpoint: {:?}, \
         credential_scope: {:?} }}",
        Replay::new(200, ""),
        Moonshot::CODING,
        crucible_credentials::Credential::scope(&credential),
    );

    let shown = format!("{:?}", kimi(Moonshot::CODING));

    assert_eq!(shown, expected);
    assert!(!shown.contains("fabricated-kimi-key"), "{shown}");
}

#[test]
fn kimi_keys_its_cache_by_the_shape_and_protocol_it_always_had() {
    for endpoint in [Moonshot::CODING, Moonshot::CODING_AI, Moonshot::PLATFORM] {
        let provider = kimi(endpoint.clone());
        let route = provider.prompt_cache_route();
        assert_eq!(route.protocol, "openai-chat-completions");
        assert_eq!(route.request_shape_version, "moonshot-chat-completions-v1");
        assert_eq!(route.endpoint, endpoint.as_str());
        assert!(!route.custom_endpoint);
    }
    let elsewhere = kimi(Endpoint::fixed(
        "https://gateway.example/v1/chat/completions",
    ));
    assert!(elsewhere.prompt_cache_route().custom_endpoint);
}
