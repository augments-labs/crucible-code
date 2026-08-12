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

/// A machine holding these variables and no others.
fn holding<'a>(set: &'a [&'a str]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| set.contains(&name).then(|| "a-key".to_owned())
}

/// The same, for a machine where what a variable holds is the point.
///
/// A name and a value rather than a name, because a variable exported blank is
/// set and holds no key, and which of those two facts wins is what the tests
/// below are about.
fn exported<'a>(set: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| {
        set.iter()
            .find(|(exported, _)| *exported == name)
            .map(|(_, value)| (*value).to_owned())
    }
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
fn a_run_that_named_no_provider_goes_to_the_one_whose_key_is_set() {
    // The defect: `crucible` on a machine holding only OPENAI_API_KEY opened on
    // an Anthropic model, so the provider the session ran against was the one
    // there was nothing to authenticate with.
    for one in PROVIDERS {
        assert_eq!(
            keyed(&Settings::default(), &holding(&[one.key])),
            one.name,
            "{}",
            one.key
        );
    }
}

#[test]
fn a_machine_holding_every_key_or_none_answers_the_same_way_every_run() {
    // Neither says which provider was meant, and an answer read off whichever
    // variable came first would move between two runs of the same command.
    let every = PROVIDERS.map(|one| one.key);

    assert_eq!(keyed(&Settings::default(), &holding(&every)), FALLBACK);
    assert_eq!(keyed(&Settings::default(), &holding(&[])), FALLBACK);
}

#[test]
fn a_variable_exported_blank_loses_to_one_that_holds_a_key() {
    // The defect, and the shell it happens in: `ANTHROPIC_API_KEY=` is how a
    // machine turns that provider off, and it counted as a key held. A session
    // set up with a real OPENAI_API_KEY beside it opened on a Claude and then
    // refused over the variable holding nothing. Both ways round, so the tie is
    // not being broken towards either provider, and every spelling of blank.
    for blank in ["", " ", "\n"] {
        for one in PROVIDERS {
            let machine: Vec<(&str, &str)> = PROVIDERS
                .iter()
                .map(|other| {
                    (
                        other.key,
                        if other.name == one.name {
                            "a-key"
                        } else {
                            blank
                        },
                    )
                })
                .collect();

            assert_eq!(
                keyed(&Settings::default(), &exported(&machine)),
                one.name,
                "{} against {blank:?}",
                one.key
            );
        }
    }
}

#[test]
fn a_machine_whose_only_variable_is_blank_is_still_that_provider() {
    // Nothing holds a key, so the blank one is the only evidence there is. The
    // refusal that follows names the variable already in the shell rather than
    // one the user has never typed.
    for one in PROVIDERS {
        assert_eq!(
            keyed(&Settings::default(), &exported(&[(one.key, "")])),
            one.name,
            "{}",
            one.key
        );
    }
}

#[test]
fn every_variable_exported_blank_answers_the_same_way_every_run() {
    // No key anywhere and nothing to tell the two apart, which is the machine
    // that has never been set up at all.
    let machine: Vec<(&str, &str)> = PROVIDERS.iter().map(|one| (one.key, "")).collect();

    assert_eq!(keyed(&Settings::default(), &exported(&machine)), FALLBACK);
}

#[test]
fn the_variable_a_key_is_looked_for_in_is_the_one_the_configuration_names() {
    // A key kept under another name is still that provider's key. Reading only
    // the vendor's usual name would miss it and land on the other provider.
    let sample = Sample::new("keyed-variable");
    let settings =
        sample.settings(r#"{"providers": {"openai": {"apiKeyEnv": "WORK_OPENAI_KEY"}}}"#);

    assert_eq!(keyed(&settings, &holding(&["WORK_OPENAI_KEY"])), "openai");
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
