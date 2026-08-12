//! What the command line and the files together decide.

use super::*;
use crate::cli::sample::Sample;

fn choice(flag: &str) -> Choice {
    Choice::parse(flag).expect("a provider")
}

/// The entry `run` resolves before it asks for a model.
fn serving(named: &str) -> Served {
    served(named).expect("a provider this build has")
}

#[test]
fn the_flag_names_the_model_over_anything_a_file_says() {
    let sample = Sample::new("model-flag");
    let settings = sample.settings(r#"{"providers": {"anthropic": {"model": "from-a-file"}}}"#);

    assert_eq!(
        &*wanted(&choice("claude-opus-5"), &settings, serving("anthropic")),
        "claude-opus-5"
    );
}

#[test]
fn a_provider_with_no_model_after_it_takes_the_one_configured_for_it() {
    // The reason `--model openai/` parses at all. Without it every way of
    // choosing a provider names a model in the same breath, and the model in
    // the file could never be the one asked for.
    let sample = Sample::new("model-file");
    let settings = sample.settings(
        r#"{"providers": {"anthropic": {"model": "claude-opus-5"},
                          "openai": {"model": "gpt-5.6"}}}"#,
    );

    assert_eq!(
        &*wanted(&choice("openai/"), &settings, serving("openai")),
        "gpt-5.6"
    );

    // And the default provider, which is what an absent flag resolves to.
    assert_eq!(
        &*wanted(&choice(""), &settings, serving("anthropic")),
        "claude-opus-5"
    );
}

#[test]
fn a_model_named_nowhere_at_all_is_the_one_that_provider_is_built_with() {
    // The rung that used to be one name for every provider, which made an
    // openai run ask openai for a Claude. It is read from the same array the
    // provider was found in, so a provider added later cannot be given one.
    for one in PROVIDERS {
        let asked = wanted(
            &choice(&format!("{}/", one.name)),
            &Settings::default(),
            one,
        );

        assert_eq!(&*asked, one.model, "{}", one.name);
    }
}

#[test]
fn a_model_configured_for_another_provider_is_not_configured_for_this_one() {
    let sample = Sample::new("model-elsewhere");
    let elsewhere = sample.settings(r#"{"providers": {"openai": {"model": "gpt-5.6"}}}"#);
    let anthropic = serving("anthropic");

    assert_eq!(
        &*wanted(&choice(""), &elsewhere, anthropic),
        anthropic.model
    );
}
