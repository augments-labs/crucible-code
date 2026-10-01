//! The window a session is managed against: the model's own, held under what
//! its route takes, unless a setting names another.

use super::*;

#[test]
fn a_model_this_build_knows_starts_at_its_native_window() {
    let settings = Settings::default();

    for (provider, model, held) in [
        ("anthropic", "claude-fable-5-1", 1_000_000),
        ("anthropic", "claude-sonnet-5", 1_000_000),
        ("anthropic", "claude-haiku-4-5", 200_000),
        ("deepseek", "deepseek-v4-pro", 1_000_000),
        ("google", "gemini-3.1-pro-preview", 1_048_576),
        ("meta", "muse-spark-1.3", 1_048_576),
        ("meta", "muse-spark-1.3-contributor", 1_048_576),
        ("meta", "muse-spark-1.2", 1_048_576),
        ("meta", "muse-spark-1.2-contributor", 1_048_576),
        ("mimo", "mimo-v2.6-pro", 1_048_576),
        ("minimax", "MiniMax-M3", 1_000_000),
        ("minimax", "MiniMax-M2.7", 204_800),
        ("moonshot", "k3", 1_048_576),
        ("moonshot", "kimi-for-coding-highspeed", 262_144),
        ("qwen", "qwen3.8-max", 1_000_000),
        ("xai", "grok-4.7", 500_000),
        ("zai", "glm-5.3", 1_000_000),
        // Held under the most the vendor's own client manages against on the
        // sign-in, which serves the same names a key does.
        ("openai", "gpt-5.6-sol", 872_000),
        ("openai", "gpt-6-astra", 872_000),
    ] {
        assert_eq!(
            window(&catalogue(), serving(provider), model, &settings),
            held,
            "{provider}/{model}"
        );
    }
}

#[test]
fn a_configured_window_still_wins_over_the_native_one() {
    let sample = Sample::new("context-window-lowered");
    let settings = sample.settings(
        r#"{"providers":{"meta":{"defaultContextWindow":200000},"minimax":{"contextWindow":{"MiniMax-M3":300000}}}}"#,
    );

    assert_eq!(
        window(&catalogue(), serving("meta"), "muse-spark-1.3", &settings),
        200_000
    );
    assert_eq!(
        window(&catalogue(), serving("minimax"), "MiniMax-M3", &settings),
        300_000
    );
    assert_eq!(
        window(&catalogue(), serving("minimax"), "MiniMax-M2.7", &settings),
        204_800
    );
}

#[test]
fn unknown_models_do_not_bypass_the_providers_default_operational_window() {
    let settings = Settings::default();

    assert_eq!(
        window(
            &catalogue(),
            serving("anthropic"),
            "claude-future",
            &settings
        ),
        200_000
    );
    assert_eq!(
        window(&catalogue(), serving("openai"), "gpt-future", &settings),
        272_000
    );
    assert_eq!(
        window(&catalogue(), serving("moonshot"), "kimi-future", &settings),
        262_144
    );
    // A name with no native window known starts where it always did, beside
    // the provider's known names that now start at theirs.
    assert_eq!(
        window(&catalogue(), serving("meta"), "muse-future", &settings),
        200_000
    );
    assert_eq!(
        window(
            &catalogue(),
            serving("minimax"),
            "MiniMax-future",
            &settings
        ),
        200_000
    );
}

#[test]
fn an_explicit_context_window_can_opt_back_into_a_larger_window() {
    let sample = Sample::new("context-window-opt-in");
    let settings = sample.settings(
        r#"{"providers":{"anthropic":{"contextWindow":{"claude-sonnet-5":1000000}},"openai":{"defaultContextWindow":872000},"moonshot":{"contextWindow":{"k3":1048576}}}}"#,
    );

    assert_eq!(
        window(
            &catalogue(),
            serving("anthropic"),
            "claude-sonnet-5",
            &settings
        ),
        1_000_000
    );
    assert_eq!(
        window(&catalogue(), serving("openai"), "gpt-5.6-sol", &settings),
        872_000
    );
    assert_eq!(
        window(&catalogue(), serving("openai"), "gpt-future", &settings),
        872_000
    );
    assert_eq!(
        window(&catalogue(), serving("moonshot"), "k3", &settings),
        1_048_576
    );
}
