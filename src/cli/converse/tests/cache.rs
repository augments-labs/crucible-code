//! `/cache` in a whole conversation over each provider this build added: it
//! answers with the vendor's own cache record, and with no error.
//!
//! Each provider is the one the registry builds from a key in its variable,
//! over a transport that sends nothing, so the test reaches no vendor.

use crucible_app::providers::{NOTHING_TO_ASK, offered, providers};
use crucible_app::startup::{self, ProviderAuth};
use crucible_app::subscription::Subscriptions;
use crucible_auth::{Renewals, StoredCredentials};
use crucible_config::Settings;
use crucible_models::Provider;
use crucible_provider::HttpTurns;
use crucible_runner::Runner;

use super::*;

/// `named`'s provider, as the registry builds it from a key in its variable.
fn built(named: &str) -> Box<dyn Provider> {
    let catalogue = providers()
        .expect("the built-in providers register")
        .snapshot();
    let serving = offered(&catalogue)
        .find(|one| one.name == named)
        .expect("a provider this build serves");
    let settings = Settings::default();
    let from = |_: &str| Some("fabricated-cache-test-key".to_owned());
    let stored = StoredCredentials::default();
    let subscriptions = Subscriptions::production(&Renewals::new());
    startup::provider(
        Some(serving),
        NOTHING_TO_ASK,
        ProviderAuth {
            settings: &settings,
            from: &from,
            stored: &stored,
            subscriptions: &subscriptions,
        },
        &HttpTurns::unavailable(),
    )
    .expect("a provider from a key")
}

/// What the terminal holds after `/cache` over `named` asking `model`.
fn inspected(named: &'static str, model: &str) -> String {
    let provider = built(named);
    let conversation =
        Conversation::recording(Arc::new(Session::nowhere()), Some(named), |session| {
            Runner::new(
                provider,
                Tools::new(),
                Agent::new(
                    AgentId::new("test"),
                    Model {
                        name: model.into(),
                        max_tokens: 64,
                        window: None,
                        accepts: None,
                        effort: None,
                    },
                ),
                crucible_context::ContextInputs::new(std::env::temp_dir()),
                session,
            )
        });
    let mut renderer = Renderer::new(Recording::new(200, 24));
    let mut input = Cursor::new(b"/cache\n".to_vec());

    converse(
        conversation,
        &mut renderer,
        &plain(),
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .expect("the loop to finish");
    renderer.terminal().written().to_string()
}

/// `/cache` over `named` names the record reviewed at `source`, says nothing
/// is held, and says no line of trouble.
fn answers(named: &'static str, model: &str, source: &str) {
    let written = inspected(named, model);

    assert!(
        written.contains("cache policy: mode="),
        "{named}: {written}"
    );
    assert!(written.contains("declared support:"), "{named}: {written}");
    assert!(
        written.contains(&format!(
            "capability provenance: reviewed 2026-10-01 from {source}"
        )),
        "{named}: {written}"
    );
    assert!(
        written.contains("last attempt: none yet"),
        "{named}: {written}"
    );
    assert!(
        written.contains("persistent resources: none"),
        "{named}: {written}"
    );
    assert!(!written.contains("! cache"), "{named}: {written}");
    assert!(!written.contains("! /cache"), "{named}: {written}");
}

#[test]
fn cache_answers_for_meta() {
    answers(
        "meta",
        "muse-spark-1.3",
        "https://dev.meta.ai/docs/prompt-caching",
    );
}

#[test]
fn cache_answers_for_xai() {
    answers(
        "xai",
        "grok-4.7",
        "https://docs.x.ai/developers/advanced-api-usage/prompt-caching",
    );
}

#[test]
fn cache_answers_for_deepseek() {
    answers(
        "deepseek",
        "deepseek-flash",
        "https://api-docs.deepseek.com/guides/kv_cache",
    );
}

#[test]
fn cache_answers_for_zai() {
    answers(
        "zai",
        "glm-5.3",
        "https://docs.z.ai/guides/capabilities/cache",
    );
}

#[test]
fn cache_answers_for_qwen() {
    answers(
        "qwen",
        "qwen3.8-max",
        "https://www.alibabacloud.com/help/en/model-studio/context-cache",
    );
}

#[test]
fn cache_answers_for_mimo() {
    answers(
        "mimo",
        "mimo-v2.6-pro",
        "https://mimo.mi.com/static/docs/price/pay-as-you-go.md",
    );
}

#[test]
fn cache_answers_for_minimax() {
    answers(
        "minimax",
        "MiniMax-M3",
        "https://platform.minimax.io/docs/api-reference/text-prompt-caching",
    );
}
