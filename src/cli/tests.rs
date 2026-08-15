//! What the command line and the files together decide.

use clap::CommandFactory;
#[cfg(unix)]
use std::ffi::OsString;

use super::*;

#[test]
fn startup_distinguishes_no_credential_from_an_unselected_provider() {
    assert_eq!(opening_unasked(None, false), NOTHING_TO_ASK);
    assert_eq!(opening_unasked(None, true), NO_PROVIDER_CHOSEN);
    assert_eq!(opening_unasked(Some(PROVIDERS[0]), true), NO_MODEL_CHOSEN);
}

use crate::cli::sample::Sample;

fn choice(flag: &str) -> Choice {
    Choice::parse(flag).expect("a provider")
}

#[cfg(unix)]
#[test]
fn existing_user_configuration_is_private_before_settings_can_read_it() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    let sample = Sample::new("protect-user-config");
    let directory = sample.root();
    let config = directory.join("config.json");
    fs::write(&config, r#"{"env":{"DEPLOY_TOKEN":"secret"}}"#).expect("a user configuration");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("directory mode");
    fs::set_permissions(&config, fs::Permissions::from_mode(0o644)).expect("file mode");
    let home =
        Home::find(&|name| (name == crucible_config::HOME).then(|| OsString::from(&directory)))
            .expect("an absolute user home");

    protect_user_config(&home).expect("the private boundary");

    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o600
    );
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
    landing(settings, from, &StoredCredentials::default())
}

/// The same, on a machine that has.
fn landing(
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
    keys: &StoredCredentials,
) -> Result<Option<Served>, Fatal> {
    chosen(settings, from, keys, &Subscriptions::production())
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
fn a_subscription_written_down_sets_up_its_provider_too() {
    // A stored account login is a credential the same way an exported key is:
    // a machine whose only authentication is a subscription still opens on its
    // provider rather than being told nothing is set up.
    let sample = Sample::new("subscription-only");
    let keys = sample.subscribed("openai");

    let found = landing(&Settings::default(), &holding(&[]), &keys)
        .expect("one subscription, no ambiguity");

    assert_eq!(found.map(|found| found.name), Some("openai"));
}

#[test]
fn a_subscription_is_available_only_to_a_registered_account_route() {
    let sample = Sample::new("unsupported-subscription");
    let stored = sample.subscribed("anthropic");

    let found = landing(&Settings::default(), &holding(&[]), &stored)
        .expect("an unsupported stored shape is not a credential");

    assert!(found.is_none());
}

#[test]
fn a_subscription_does_not_authenticate_a_custom_api_key_audience() {
    let sample = Sample::new("subscription-custom-audience");
    let stored = sample.subscribed("openai");
    let settings =
        sample.user(r#"{"providers":{"openai":{"baseUrl":"https://gateway.example/v1"}}}"#);

    let found = landing(&settings, &holding(&[]), &stored)
        .expect("an account token is not sent to a custom address");

    assert!(found.is_none());
}

#[test]
fn kimi_product_names_keep_their_wire_identifiers_and_effort_sets() {
    let moonshot = serving("moonshot");
    let offered: Vec<_> = moonshot
        .models
        .iter()
        .map(|model| (model.name, model.shown, model.rungs))
        .collect();

    assert_eq!(
        offered,
        [
            ("k3", "K3", KIMI),
            ("k3-256k", "K3-256k", KIMI),
            ("kimi-for-coding", "K2.7 Coding", KIMI),
            ("kimi-for-coding-highspeed", "K2.7 Coding Highspeed", KIMI),
        ]
    );
    for model in [
        "k3",
        "k3-256k",
        "kimi-for-coding",
        "kimi-for-coding-highspeed",
    ] {
        assert_eq!(
            rungs("moonshot", model),
            [Effort::Low, Effort::High, Effort::Max]
        );
    }
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
fn a_stored_key_and_an_exported_key_leave_the_provider_for_model_to_choose() {
    let sample = Sample::new("stored-ambiguous");
    let keys = sample.stored("openai");

    let found = landing(
        &Settings::default(),
        &holding(&["ANTHROPIC_API_KEY"]),
        &keys,
    )
    .expect("several credentials are not a startup failure");

    assert!(found.is_none());
}

#[test]
fn the_source_a_credential_came_from_is_the_one_construction_would_use() {
    // `/logout` names what remains after a removal from this answer, so it has
    // to be the same order `startup::provider` resolves in: a deliberate
    // account login, then the inherited variable, then the stored key.
    let subscriptions = Subscriptions::production();

    let sample = Sample::new("source-subscription");
    let stored = sample.subscribed("openai");
    let source = credential_source(
        serving("openai"),
        &Settings::default(),
        &holding(&["OPENAI_API_KEY"]),
        &stored,
        &subscriptions,
    );
    assert_eq!(source, Some(CredentialSource::Subscription));

    let source = credential_source(
        serving("anthropic"),
        &Settings::default(),
        &holding(&["ANTHROPIC_API_KEY"]),
        &StoredCredentials::default(),
        &subscriptions,
    );
    assert_eq!(
        source,
        Some(CredentialSource::Environment("ANTHROPIC_API_KEY".into()))
    );

    let sample = Sample::new("source-stored");
    let stored = sample.stored("openai");
    let source = credential_source(
        serving("openai"),
        &Settings::default(),
        &holding(&[]),
        &stored,
        &subscriptions,
    );
    assert_eq!(source, Some(CredentialSource::StoredKey));

    // A plan's token is the vendor's: pointed at a configured gateway, the
    // subscription is no source at all.
    let settings =
        sample.user(r#"{"providers": {"openai": {"baseUrl": "https://gateway.example/v1"}}}"#);
    let stored = sample.subscribed("openai");
    let source = credential_source(
        serving("openai"),
        &settings,
        &holding(&[]),
        &stored,
        &subscriptions,
    );
    assert_eq!(source, None);
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
fn the_flag_says_how_hard_to_think_over_anything_a_file_says() {
    let sample = Sample::new("effort-flag");
    let settings = sample.settings(r#"{"providers": {"anthropic": {"effort": "low"}}}"#);

    assert_eq!(
        thinking(Some(Effort::Max), &settings, Some(serving("anthropic"))),
        Some(Effort::Max)
    );
}

#[test]
fn a_run_that_says_nothing_takes_the_rung_configured_for_the_provider_it_is_going_to() {
    // Per provider rather than one answer for the machine, because which rungs
    // exist is the vendor's business: a file that chose `xhigh` for the one
    // serving it has said nothing about the one that would refuse it.
    let sample = Sample::new("effort-file");
    let settings = sample.settings(
        r#"{"providers": {"anthropic": {"effort": "xhigh"},
                          "openai": {"effort": "low"}}}"#,
    );

    assert_eq!(
        thinking(None, &settings, Some(serving("anthropic"))),
        Some(Effort::Xhigh)
    );
    assert_eq!(
        thinking(None, &settings, Some(serving("openai"))),
        Some(Effort::Low)
    );
    assert_eq!(thinking(None, &settings, Some(serving("moonshot"))), None);
}

#[test]
fn a_run_nobody_told_how_hard_to_think_asks_for_no_rung_at_all() {
    // Not the middle one, and not the rung the picker opens on. Every vendor
    // here chose a default per model, and one asked for on somebody's behalf
    // would reach the models that do not take the field at all — turning a
    // session nobody configured into a refusal from a vendor they did not
    // knowingly ask anything of.
    assert_eq!(
        thinking(None, &Settings::default(), Some(serving("anthropic"))),
        None
    );
    assert_eq!(thinking(None, &Settings::default(), None), None);
}

#[test]
fn a_rung_that_is_not_one_is_refused_with_the_rungs_that_are() {
    // The flag is parsed before there is anything on screen to look at, so the
    // sentence is the whole of what somebody who mistyped gets back.
    let refused =
        Cli::try_parse_from(["crucible", "--effort", "maximum"]).expect_err("a rung nobody serves");

    let said = refused.to_string();
    assert!(said.contains("no effort called maximum"), "{said}");
    assert!(said.contains("low, medium, high, xhigh, max"), "{said}");
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
fn a_machine_holding_every_key_starts_with_the_provider_open() {
    // Authentication makes every row reachable; it does not pick one by
    // declaration order. `/model` is the interactive place that chooses both
    // halves, and a launch that refuses to start would strand a machine one
    // command away from that choice.
    let every = PROVIDERS.map(|one| one.key);
    let found = lands(&Settings::default(), &holding(&every)).expect("several keys are usable");

    assert!(found.is_none());
}

#[test]
fn a_machine_holding_every_key_asks_the_provider_it_was_told_to() {
    // The one setting that chooses a vendor, and the whole of what settles a
    // machine set up for two.
    let sample = Sample::new("provider-decides");
    let settings = sample.user(r#"{"provider": "openai"}"#);
    let every = PROVIDERS.map(|one| one.key);

    let found = lands(&settings, &holding(&every)).expect("the file chose");

    assert_eq!(found.map(|found| found.name), Some("openai"));
}

#[test]
fn an_unavailable_configured_provider_does_not_replace_itself_with_another() {
    // A remembered choice cannot authenticate itself. Falling through to the
    // other key would send a turn to a provider nobody chose; retaining the
    // unavailable choice would make a normal launch fail before `/login` can
    // repair it. The run therefore opens with no provider selected.
    let sample = Sample::new("named-over-keyed");
    let settings = sample.user(r#"{"provider": "anthropic"}"#);

    let found = lands(&settings, &holding(&["OPENAI_API_KEY"])).expect("the file chose");

    assert!(found.is_none());
}

#[test]
fn a_remembered_provider_without_any_credential_does_not_stop_startup() {
    let sample = Sample::new("named-without-key");
    let settings = sample.user(
        r#"{"provider":"openai","providers":{"openai":{"model":"gpt-5.6-sol","effort":"high"}}}"#,
    );

    let launch = launch(
        &Cli::try_parse_from(["crucible"]).unwrap(),
        &settings,
        &|_| None,
        &sample.store().read(),
        &Subscriptions::production(),
    )
    .expect("an unavailable remembered provider is an interactive setup state");

    assert!(launch.serving.is_none());
    assert!(launch.model.is_none());
    assert!(launch.effort.is_none());
    assert_eq!(launch.unasked, NOTHING_TO_ASK);
}

#[test]
fn a_model_written_under_a_provider_never_chooses_that_provider() {
    // The failure this key was added for. `providers.openai.model` says what to
    // ask openai for, and it used to be read as saying to ask openai — so a
    // machine holding two keys was sent to whichever vendor a model had been
    // picked for weeks earlier, with nothing on screen saying so.
    let sample = Sample::new("model-is-not-a-provider");
    let settings = sample.settings(r#"{"providers": {"openai": {"model": "gpt-5.6"}}}"#);
    let every = PROVIDERS.map(|one| one.key);

    assert!(
        lands(&settings, &holding(&every)).unwrap().is_none(),
        "a model is what to ask a provider for, not which provider to ask"
    );
}

#[test]
fn a_provider_this_build_does_not_serve_is_refused_before_anything_is_drawn() {
    // Written by hand, or written by a later crucible and read back by this
    // one. Either way it is the same sentence the flag gets, naming what this
    // build has — and not a silent fall through to whichever key is exported.
    let sample = Sample::new("named-nobody");
    let settings = sample.user(r#"{"provider": "gemini"}"#);

    let problem = lands(&settings, &holding(&["OPENAI_API_KEY"])).expect_err("no such provider");

    assert!(problem.to_string().contains("gemini"), "{problem}");
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
    let settings = sample.user(r#"{"providers": {"openai": {"apiKeyEnv": "WORK_OPENAI_KEY"}}}"#);

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

#[test]
fn every_provider_offers_a_few_models_and_never_a_list_to_scroll() {
    // The panel is a handful to look down, not a catalogue. Five is where a
    // list stops being read and starts being searched — and a name that is not
    // on it is still typed, which is what keeps the ceiling a ceiling rather
    // than a claim about what the vendor serves.
    for one in PROVIDERS {
        assert!(!one.models.is_empty(), "{}", one.name);
        assert!(one.models.len() <= 5, "{}: {}", one.name, one.models.len());
    }
}

#[test]
fn every_model_serves_its_rungs_weakest_first_and_names_none_of_them_twice() {
    // The ladder is drawn between two ends — faster on the left, smarter on the
    // right — and those ends are a claim about the order of what is between
    // them. A set written down out of order draws a track whose ends are wrong,
    // which is worse than a missing rung: nothing on screen says so.
    for one in PROVIDERS {
        for model in one.models {
            let ladder: Vec<usize> = model
                .rungs
                .iter()
                .map(|rung| {
                    Effort::LADDER
                        .iter()
                        .position(|known| known == rung)
                        .expect("a rung of the ladder")
                })
                .collect();

            // Strictly, which is what makes this the uniqueness check as well:
            // a rung written down twice is a rung that stands still under an
            // arrow key.
            assert!(
                ladder.is_sorted_by(|here, next| here < next),
                "{}: {:?}",
                model.name,
                model.rungs
            );
        }
    }
}

#[test]
fn a_model_nobody_wrote_down_is_offered_every_rung_rather_than_none() {
    // The table is read off somebody else's documentation and goes stale
    // between releases. What is not in it is not a model that serves nothing —
    // it is a model this build knows nothing about, and the choice is between
    // offering a rung its vendor may refuse and withholding one it serves. The
    // first is a sentence back from the vendor; the second is crucible deciding
    // what a model it has never heard of can do.
    assert_eq!(rungs("anthropic", "claude-opus-9"), Effort::LADDER);

    // Including under a provider this build does not serve, which is the same
    // ignorance arrived at from the other side.
    assert_eq!(rungs("ollama", "llama-4"), Effort::LADDER);

    // And a name is only known under the provider it was written down for: two
    // vendors serving one name serve it on their own terms.
    assert_eq!(rungs("openai", "claude-haiku-4-5"), Effort::LADDER);
    assert!(rungs("anthropic", "claude-haiku-4-5").is_empty());
}
