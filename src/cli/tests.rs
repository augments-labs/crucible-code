//! What the command line and the files together decide.

use clap::CommandFactory;

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

/// Which provider a flagless run lands on, on a machine nobody has logged in
/// from. The store is the other half of the same question and has its own
/// helper below; every test that is about a variable is asking this one.
fn lands(
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Served>, Fatal> {
    landing(settings, from, &Keys::default())
}

/// The same, on a machine that has.
fn landing(
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
    keys: &Keys,
) -> Result<Option<Served>, Fatal> {
    chosen(&Choice::default(), settings, from, keys)
}

#[test]
fn a_key_written_down_sets_a_provider_up_with_no_variable_exported() {
    // The whole point of `/login`: a key given once and a shell that says
    // nothing about it ever again.
    let sample = Sample::new("stored-only");
    let keys = sample.stored("openai");

    let found = landing(&Settings::default(), &holding(&[]), &keys).expect("one key, written down");

    assert_eq!(found.map(|found| found.name), Some("openai"));
}

#[test]
fn a_provider_holding_a_key_both_ways_is_one_provider_rather_than_two() {
    // Somebody who logged in and then exported the same key — which is what
    // happens the first time they put it in a shell profile. Counted twice it
    // is two providers to the question of which to ask, and the run they were
    // trying to set up ends by asking them to choose between openai and openai.
    let sample = Sample::new("stored-and-exported");
    let keys = sample.stored("openai");

    let found = landing(&Settings::default(), &holding(&["OPENAI_API_KEY"]), &keys)
        .expect("one provider, held twice");

    assert_eq!(found.map(|found| found.name), Some("openai"));
}

#[test]
fn a_machine_asked_which_provider_names_the_store_by_the_provider_it_holds() {
    // The sentence exists to be acted on, and what the reader does about a key
    // is unset the variable holding it. There is no variable behind a key that
    // was written down, so naming the vendor's usual one would send them to
    // look at something that was never set.
    let sample = Sample::new("stored-ambiguous");
    let keys = sample.stored("openai");

    let problem = landing(
        &Settings::default(),
        &holding(&["ANTHROPIC_API_KEY"]),
        &keys,
    )
    .expect_err("two providers, no choice");

    let said = problem.to_string();
    assert!(said.contains("ANTHROPIC_API_KEY"), "{said}");
    assert!(said.contains("openai"), "{said}");
    assert!(!said.contains("OPENAI_API_KEY"), "{said}");
}

#[test]
fn the_flag_names_the_model_over_anything_a_file_says() {
    let sample = Sample::new("model-flag");
    let settings = sample.settings(r#"{"providers": {"anthropic": {"model": "from-a-file"}}}"#);

    assert_eq!(
        wanted(
            &choice("claude-opus-5"),
            &settings,
            Some(serving("anthropic"))
        )
        .as_deref(),
        Some("claude-opus-5")
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
        wanted(&choice("openai/"), &settings, Some(serving("openai"))).as_deref(),
        Some("gpt-5.6")
    );
    assert_eq!(
        wanted(&Choice::default(), &settings, Some(serving("anthropic"))).as_deref(),
        Some("claude-opus-5")
    );
}

#[test]
fn a_model_named_nowhere_at_all_is_no_model() {
    // The rung that used to be a name written into this build. It sent whatever
    // model this binary was compiled with to whichever provider the key
    // belonged to, which is the pairing nobody asked for. There is no such rung
    // now, and the session says so rather than guessing.
    for one in PROVIDERS {
        let asked = wanted(
            &choice(&format!("{}/", one.name)),
            &Settings::default(),
            Some(one),
        );

        assert_eq!(asked, None, "{}", one.name);
    }
}

#[test]
fn a_model_configured_as_nothing_at_all_is_no_model() {
    // A key written and left empty is a file that says nothing rather than one
    // that asks for a model called "". Sent as it stands it would reach a
    // vendor as a request for a model with no name.
    let sample = Sample::new("model-blank");

    for blank in ["", "   "] {
        let settings = sample.settings(&format!(
            r#"{{"providers": {{"anthropic": {{"model": "{blank}"}}}}}}"#
        ));

        assert_eq!(
            wanted(&Choice::default(), &settings, Some(serving("anthropic"))),
            None,
            "{blank:?}"
        );
    }
}

#[test]
fn a_run_that_named_no_provider_goes_to_the_one_whose_key_is_set() {
    // The defect: `crucible` on a machine holding only OPENAI_API_KEY opened on
    // an Anthropic model, so the provider the session ran against was the one
    // there was nothing to authenticate with.
    for one in PROVIDERS {
        let found = lands(&Settings::default(), &holding(&[one.key])).expect("one key, no doubt");

        assert_eq!(found.map(|found| found.name), Some(one.name), "{}", one.key);
    }
}

#[test]
fn a_bare_model_name_goes_to_the_provider_whose_key_is_set_and_never_to_a_fallback() {
    // The same defect from the other side, and the one the user met:
    // `--model gpt-5.6-terra` with only OPENAI_API_KEY exported went to
    // Anthropic, because an unqualified name had a provider written into the
    // parser. A name is now served by whoever this machine can authenticate to.
    for one in PROVIDERS {
        let found = chosen(
            &choice("a-model-name"),
            &Settings::default(),
            &holding(&[one.key]),
            &Keys::default(),
        )
        .expect("one key, no doubt");

        assert_eq!(found.map(|found| found.name), Some(one.name), "{}", one.key);
    }
}

#[test]
fn a_machine_holding_no_key_at_all_has_no_provider() {
    // Not a fallback and not a refusal to start: there is nothing to ask, and
    // the session opens saying so with the prompt still there to set one up
    // from.
    assert!(
        lands(&Settings::default(), &holding(&[]))
            .expect("no key is not an error")
            .is_none()
    );
}

#[test]
fn a_machine_holding_every_key_and_choosing_none_is_asked_which() {
    // Two providers set up and nothing choosing between them. Picking one would
    // send the turn to a vendor over a coin toss, and the sentence back names
    // both variables so the answer is a flag away.
    let every = PROVIDERS.map(|one| one.key);

    let problem = lands(&Settings::default(), &holding(&every)).expect_err("two keys, no choice");

    let said = problem.to_string();
    for one in PROVIDERS {
        assert!(said.contains(one.key), "{said}");
    }
}

#[test]
fn a_machine_holding_every_key_takes_the_provider_a_model_was_chosen_for() {
    // What `/model` writes down is an answer to this question, so the run after
    // it does not ask again.
    let sample = Sample::new("model-decides");
    let settings = sample.settings(r#"{"providers": {"openai": {"model": "gpt-5.6"}}}"#);
    let every = PROVIDERS.map(|one| one.key);

    let found = lands(&settings, &holding(&every)).expect("the file chose");

    assert_eq!(found.map(|found| found.name), Some("openai"));
}

#[test]
fn a_model_named_on_the_flag_does_not_let_a_file_settle_the_provider() {
    // The file's answer is a choice of *model*, and the flag has already
    // overruled it. Reading it to pick the provider would send the name that
    // was typed to the vendor named beside a name that was not.
    let sample = Sample::new("model-overruled");
    let settings = sample.settings(r#"{"providers": {"openai": {"model": "gpt-5.6"}}}"#);
    let every = PROVIDERS.map(|one| one.key);

    assert!(
        chosen(
            &choice("claude-opus-5"),
            &settings,
            &holding(&every),
            &Keys::default()
        )
        .is_err(),
        "the flag named a model, so the file cannot say who serves it"
    );
}

#[test]
fn a_variable_exported_blank_holds_no_key() {
    // The shell it happens in: `ANTHROPIC_API_KEY=` is how a machine turns that
    // provider off, and it used to count as a key held. Both ways round, so
    // nothing is being tilted towards either provider, and every spelling of
    // blank.
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

            let found = lands(&Settings::default(), &exported(&machine)).expect("one real key");

            assert_eq!(
                found.map(|found| found.name),
                Some(one.name),
                "{} against {blank:?}",
                one.key
            );
        }
    }
}

#[test]
fn a_machine_whose_every_variable_is_blank_has_nothing_set_up() {
    let machine: Vec<(&str, &str)> = PROVIDERS.iter().map(|one| (one.key, "")).collect();

    assert!(
        lands(&Settings::default(), &exported(&machine))
            .expect("blank is not a key")
            .is_none()
    );
}

#[test]
fn the_variable_a_key_is_looked_for_in_is_the_one_the_configuration_names() {
    // A key kept under another name is still that provider's key. Reading only
    // the vendor's usual name would miss it and leave the machine looking as
    // though nothing were set up.
    let sample = Sample::new("keyed-variable");
    let settings =
        sample.settings(r#"{"providers": {"openai": {"apiKeyEnv": "WORK_OPENAI_KEY"}}}"#);

    let found = lands(&settings, &holding(&["WORK_OPENAI_KEY"])).expect("one key");

    assert_eq!(found.map(|found| found.name), Some("openai"));
}

#[test]
fn a_model_configured_for_another_provider_is_not_configured_for_this_one() {
    let sample = Sample::new("model-elsewhere");
    let elsewhere = sample.settings(r#"{"providers": {"openai": {"model": "gpt-5.6"}}}"#);

    assert_eq!(
        wanted(&Choice::default(), &elsewhere, Some(serving("anthropic"))),
        None
    );
}

#[test]
fn the_help_text_names_every_provider_this_build_serves_and_its_variable() {
    // The pair `PROVIDERS` and `long_about` can disagree, and a user meets
    // whichever of them is wrong: a provider the parser accepts and the help
    // text never mentions is one nobody finds, and a variable named in the help
    // text with no entry behind it is one they export and watch do nothing.
    let help = Cli::command().render_long_help().to_string();

    for one in PROVIDERS {
        assert!(
            help.contains(one.key),
            "the help text never says where {}'s key is read from",
            one.name
        );
    }
}

#[test]
fn a_display_name_is_the_typed_name_under_the_vendor_s_own_capitals() {
    // Two spellings of one provider, and only one of them is ever matched
    // against — so the one nobody types is the one free to drift. A panel
    // offering `OpenAl` writes its key down under `openai` and reads correctly
    // to everybody except the person deciding which vendor they are logging in
    // to.
    for one in PROVIDERS {
        assert!(
            one.shown.to_lowercase().starts_with(one.name),
            "{} is offered as {}",
            one.name,
            one.shown
        );
    }
}
