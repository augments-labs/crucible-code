//! What the command line and the files together decide.

use clap::CommandFactory;
use crucible_core::Modality;
#[cfg(unix)]
use std::ffi::OsString;

use super::*;

#[test]
fn startup_distinguishes_no_credential_from_an_unselected_provider() {
    assert_eq!(opening_unasked(None, false), NOTHING_TO_ASK);
    assert_eq!(opening_unasked(None, true), NO_PROVIDER_CHOSEN);
    assert_eq!(opening_unasked(Some(first()), true), NO_MODEL_CHOSEN);
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

/// The built-in providers, as one generation the tests read against.
fn catalogue() -> Providers {
    providers()
        .expect("the built-in providers register")
        .snapshot()
}

/// Every record the registry holds, in the order it holds them.
fn every() -> Vec<Served> {
    offered(&catalogue()).collect()
}

/// The first of them, which is the one a fresh build opens the panels on.
fn first() -> Served {
    every().first().copied().expect("a provider is registered")
}

/// The record `run` resolves before it asks for a model.
fn serving(named: &str) -> Served {
    served(&catalogue(), named).expect("a provider this build has")
}

/// The credential sources a test resolves a provider against.
fn authenticating<'a>(
    settings: &'a Settings,
    from: &'a dyn Fn(&str) -> Option<String>,
    stored: &'a StoredCredentials,
    subscriptions: &'a Subscriptions,
) -> startup::ProviderAuth<'a> {
    startup::ProviderAuth {
        settings,
        from,
        stored,
        subscriptions,
    }
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
    chosen(
        &catalogue(),
        authenticating(settings, from, keys, &Subscriptions::production()),
    )
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
            rungs(&catalogue(), "moonshot", model),
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
        authenticating(
            &Settings::default(),
            &holding(&["OPENAI_API_KEY"]),
            &stored,
            &subscriptions,
        ),
    );
    assert_eq!(source, Some(CredentialSource::Subscription));

    let source = credential_source(
        serving("anthropic"),
        authenticating(
            &Settings::default(),
            &holding(&["ANTHROPIC_API_KEY"]),
            &StoredCredentials::default(),
            &subscriptions,
        ),
    );
    assert_eq!(
        source,
        Some(CredentialSource::Environment("ANTHROPIC_API_KEY".into()))
    );

    let sample = Sample::new("source-stored");
    let stored = sample.stored("openai");
    let source = credential_source(
        serving("openai"),
        authenticating(&Settings::default(), &holding(&[]), &stored, &subscriptions),
    );
    assert_eq!(source, Some(CredentialSource::StoredKey));

    // A plan's token is the vendor's: pointed at a configured gateway, the
    // subscription is no source at all.
    let settings =
        sample.user(r#"{"providers": {"openai": {"baseUrl": "https://gateway.example/v1"}}}"#);
    let stored = sample.subscribed("openai");
    let source = credential_source(
        serving("openai"),
        authenticating(&settings, &holding(&[]), &stored, &subscriptions),
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
    for one in every() {
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
    for one in every() {
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
    let keys: Vec<&str> = every().iter().map(|one| one.key).collect();
    let found = lands(&Settings::default(), &holding(&keys)).expect("several keys are usable");

    assert!(found.is_none());
}

#[test]
fn a_machine_holding_every_key_asks_the_provider_it_was_told_to() {
    // The one setting that chooses a vendor, and the whole of what settles a
    // machine set up for two.
    let sample = Sample::new("provider-decides");
    let settings = sample.user(r#"{"provider": "openai"}"#);
    let keys: Vec<&str> = every().iter().map(|one| one.key).collect();

    let found = lands(&settings, &holding(&keys)).expect("the file chose");

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
        &catalogue(),
        authenticating(
            &settings,
            &|_| None,
            &sample.store().read(),
            &Subscriptions::production(),
        ),
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
    let keys: Vec<&str> = every().iter().map(|one| one.key).collect();

    assert!(
        lands(&settings, &holding(&keys)).unwrap().is_none(),
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
        for one in every() {
            let machine: Vec<(&str, &str)> = every()
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
    let machine: Vec<(&str, &str)> = every().iter().map(|one| (one.key, "")).collect();

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
    // The registry and `long_about` can disagree, and a user meets
    // whichever of them is wrong: a provider the parser accepts and the help
    // text never mentions is one nobody finds, and a variable named in the help
    // text with no entry behind it is one they export and watch do nothing.
    let help = Cli::command().render_long_help().to_string();

    for one in every() {
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
    for one in every() {
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
    for one in every() {
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
    for one in every() {
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
    assert_eq!(
        rungs(&catalogue(), "anthropic", "claude-opus-9"),
        Effort::LADDER
    );

    // Including under a provider this build does not serve, which is the same
    // ignorance arrived at from the other side.
    assert_eq!(rungs(&catalogue(), "ollama", "llama-4"), Effort::LADDER);

    // And a name is only known under the provider it was written down for: two
    // vendors serving one name serve it on their own terms.
    assert_eq!(
        rungs(&catalogue(), "openai", "claude-haiku-4-5"),
        Effort::LADDER
    );
    assert!(rungs(&catalogue(), "anthropic", "claude-haiku-4-5").is_empty());
}

/// Every model crucible offers, and what the generated table says it reads.
///
/// Named by provider rather than one by one: a model added to `OFFERED` later
/// is covered by these the moment the table is regenerated, which is the point
/// at which somebody is reading the diff anyway.
fn accepting(one: Modality) -> Vec<&'static str> {
    models::FACTS
        .iter()
        .filter(|facts| facts.accepts.contains(one))
        .map(|facts| facts.provider)
        .collect()
}

#[test]
fn the_models_table_has_a_row_for_every_model_crucible_offers_and_no_others() {
    // What is checked in cannot be reproduced here: the database it was read
    // from is served over the network and this repository keeps no copy, so no
    // test can say the numbers are right. This says the shape is, which is the
    // half that goes wrong by hand — a model added to the catalogue and never
    // read from the database is one crucible offers and knows no window for,
    // and a row left behind by a model that was withdrawn is a limit nothing
    // will ever be held to.
    let offered: Vec<(&str, &str)> = every()
        .iter()
        .flat_map(|one| one.models.iter().map(move |model| (one.name, model.name)))
        .collect();

    for (provider, model) in &offered {
        assert!(
            facts(provider, model).is_some(),
            "{provider}/{model} is offered and has no row — run generate-models"
        );
    }

    for row in models::FACTS {
        assert!(
            offered.contains(&(row.provider, row.model)),
            "{}/{} has a row and is offered by nobody",
            row.provider,
            row.model
        );
    }
}

#[test]
fn the_models_table_says_every_model_crucible_offers_reads_text_and_an_image() {
    assert!(
        !models::FACTS.is_empty(),
        "the table is generated and is never empty"
    );
    for facts in models::FACTS {
        let named = facts.model;
        assert!(facts.accepts.contains(Modality::Text), "{named} reads text");
        assert!(
            facts.accepts.contains(Modality::Image),
            "{named} reads an image"
        );
    }
}

#[test]
fn the_models_table_gives_a_pdf_to_anthropic_and_openai_and_not_to_moonshot() {
    for facts in models::FACTS {
        let expected = matches!(facts.provider, "anthropic" | "openai");
        assert_eq!(
            facts.accepts.contains(Modality::Pdf),
            expected,
            "{} reading a PDF",
            facts.model,
        );
    }
    assert!(accepting(Modality::Pdf).contains(&"anthropic"));
}

#[test]
fn the_models_table_gives_video_to_moonshot_alone_and_audio_to_nobody() {
    for facts in models::FACTS {
        assert_eq!(
            facts.accepts.contains(Modality::Video),
            facts.provider == "moonshot",
            "{} reading a video",
            facts.model,
        );
        assert!(
            !facts.accepts.contains(Modality::Audio),
            "{} reads audio, which no model crucible offers did when this was written",
            facts.model,
        );
    }
    assert_eq!(accepting(Modality::Audio), Vec::<&str>::new());
}

#[test]
fn resume_and_continue_cannot_be_asked_for_together() {
    // Each names a different session to pick up. Refused at the parser, so
    // whichever the user meant, nothing is opened on the other's behalf.
    let refused = Cli::try_parse_from(["crucible", "--resume", "some-id", "--continue"])
        .expect_err("two ways back at once");

    let said = refused.to_string();
    assert!(said.contains("cannot be used with"), "{said}");
}

#[test]
fn resume_round_trip() {
    use crucible_runner::Session;

    let sample = Sample::new("resume-round-trip");
    let workspace = sample.workspace();
    let session =
        Session::start(&sample.logs(), &workspace, None).expect("a new session to record");
    let id = session.id().expect("a recorded session has a name").clone();
    let path = session.path().to_owned();
    session.append(&crucible_core::Message::said("keep this turn"));
    drop(session);

    // The parting message names the command that comes back to this session.
    let mut renderer = crucible_tui::Renderer::new(crucible_tui::Recording::new(80, 24));
    draw::parting(
        &mut renderer,
        &converse::Parting::Kept(path),
        style::Style::plain(),
    )
    .expect("a parting to draw");
    let written = renderer.terminal().written().to_string();
    assert!(
        written.contains(&format!("crucible --resume {}", id.as_str())),
        "{written}"
    );

    // The recorded id reopens the session it names, with its transcript.
    let (reopened, transcript) =
        startup::reopening(&sample.logs(), &workspace, &id).expect("the session named");
    assert_eq!(transcript.len(), 1);
    drop(reopened);

    // An id nothing here answers to is told so in one sentence.
    let stranger = crucible_core::SessionId::new();
    let refused = startup::reopening(&sample.logs(), &workspace, &stranger)
        .expect_err("a session nobody recorded");
    assert_eq!(
        refused.to_string(),
        format!("no session {} in this workspace", stranger.as_str())
    );
}

#[test]
fn the_registry_holds_every_built_in_provider_as_a_built_in_source() {
    // What a provider from somewhere else will be read beside. A row whose
    // provenance said anything but "built in" would be one this build compiled
    // in and then reported as somebody else's.
    let providers = catalogue();

    for one in providers.entries() {
        assert_eq!(
            one.provenance().id(),
            format!("crucible:{}", one.id()),
            "{}",
            one.id()
        );
        assert_eq!(one.provenance().kind(), SourceKind::Builtin, "{}", one.id());
    }

    assert_eq!(
        providers.entries().len(),
        every().len(),
        "every built-in record is registered exactly once"
    );
}

#[test]
fn two_records_answering_to_one_name_are_refused_and_both_sources_are_named() {
    // A provider is named on a flag, in a file and on a panel. Two arms under
    // one name is a key sent to whichever of them registered last, decided by
    // load order and visible nowhere — so the second one is refused instead.
    let registry = providers().expect("the built-in providers register");
    let mut staged = registry.stage();
    let again = Arm::builtin(serving("anthropic")).expect("a record of the same name");

    let problem = staged
        .register(again)
        .expect_err("a second arm under a registered name");

    let said = problem.to_string();
    assert!(said.contains("anthropic"), "{said}");
    assert!(
        said.contains("crucible:anthropic"),
        "{said} names neither source"
    );
}

#[test]
fn the_names_a_refusal_offers_are_the_ones_the_registry_holds_now() {
    // The sentence is built from the generation in force rather than from the
    // list this build was compiled with, so a provider registered later is one
    // a mistyped name is told about.
    let providers = catalogue();
    let said = names(&providers);

    assert_eq!(
        said,
        every()
            .iter()
            .map(|one| one.name)
            .collect::<Vec<_>>()
            .join(", ")
    );
}

#[test]
fn a_provider_built_from_its_record_is_the_one_the_vendor_module_makes() {
    // The registry moved which arm builds a provider; it did not move what the
    // arm builds. Prompt caching is the fact a session's spend is decided by,
    // and it is read off the provider object rather than off the name, so a
    // record wired to the wrong factory would be silently cheap or silently
    // expensive rather than wrong on screen.
    let stored = StoredCredentials::default();
    let subscriptions = Subscriptions::production();
    let settings = Settings::default();
    let from = holding(&["ANTHROPIC_API_KEY"]);

    let built = startup::provider(
        Some(serving("anthropic")),
        NOTHING_TO_ASK,
        authenticating(&settings, &from, &stored, &subscriptions),
    )
    .expect("a key is exported for it");

    let direct = crucible_provider::Anthropic::at(
        crucible_provider::Anthropic::VENDOR,
        Box::new(crucible_core::HeaderKey::new(
            crucible_core::ApiKey::from_lookup("ANTHROPIC_API_KEY", &from).expect("the key"),
            crucible_core::Header::bare("x-api-key"),
        )),
        Box::new(crucible_provider::Https::new()),
    );

    assert_eq!(built.name(), direct.name());
    assert_eq!(
        built.prompt_cache_capabilities("claude-opus-5"),
        direct.prompt_cache_capabilities("claude-opus-5"),
        "the record builds the vendor's own arm"
    );
}

/// Anthropic's record with a different offer list under it.
///
/// The offer list and the generated table are written by different hands, and
/// what these tests are about is what happens where the two disagree — so the
/// list is the half that moves.
fn offering(models: &'static [Model]) -> Served {
    Served {
        models,
        ..serving("anthropic")
    }
}

#[test]
fn a_models_record_is_the_offer_list_and_the_generated_table_joined() {
    // The two halves a limit is read from used to be read separately: the rungs
    // off the offer list, the window and the ceiling off the table. One record
    // holding both is what lets a reader ask once, and what lets a provider
    // registered at run time answer the same question without a row here.
    let providers = catalogue();
    let described =
        capabilities(&providers, "anthropic", "claude-opus-5").expect("a model this build offers");

    assert_eq!(described.name(), "claude-opus-5");
    assert_eq!(described.window(), 1_000_000, "the table's half");
    assert_eq!(described.output(), 128_000, "the table's half");
    assert!(
        described.accepts().contains(Modality::Pdf),
        "the table's half"
    );

    let offered = serving("anthropic")
        .models
        .iter()
        .find(|model| model.name == "claude-opus-5")
        .expect("the offer list names it");
    assert_eq!(described.rungs(), offered.rungs, "the offer list's half");
    assert_eq!(described.shown(), offered.shown, "the offer list's half");
}

#[test]
fn a_model_no_arm_offers_is_nothing_known_rather_than_a_record_of_zeroes() {
    // A name one word from an offered one, and a provider nobody registered.
    // Both answer nothing, which is what leaves the caller free to fall back to
    // a configured figure — where a record of zeroes would be a session that
    // threw itself away on the first turn.
    let providers = catalogue();

    assert!(capabilities(&providers, "anthropic", "claude-opus-9").is_none());
    assert!(capabilities(&providers, "nobody", "claude-opus-5").is_none());
}

#[test]
fn a_model_offered_with_no_row_in_the_table_stops_the_run() {
    // The offer list is written by hand and the table is generated, so a model
    // added to one and never read into the other is the way these two come
    // apart. Caught where the arm is built, because the alternative is a name
    // on the panel that answers "nothing known" to every question about it.
    const UNLISTED: &[Model] = &[Model::new("claude-from-the-future", &[])];

    let refused = Arm::builtin(offering(UNLISTED)).expect_err("a model with no row");

    let said = refused.to_string();
    assert!(said.contains("claude-from-the-future"), "{said}");
    assert!(said.contains("generate-models"), "{said}");
}

#[test]
fn an_offer_list_that_describes_a_model_wrongly_stops_the_run() {
    // The rungs travel with the offer list, and the panel draws them faster on
    // the left and smarter on the right. A set written down backwards draws a
    // track whose ends are wrong and says so nowhere, so the arm refuses to
    // hold the record rather than the panel refusing to draw it.
    const BACKWARDS: &[Model] = &[Model::new("claude-opus-5", &[Effort::Max, Effort::Low])];

    let refused = Arm::builtin(offering(BACKWARDS)).expect_err("a backwards ladder");

    let said = refused.to_string();
    assert!(said.contains("claude-opus-5"), "{said}");
    assert!(said.contains("max, low"), "{said}");
}
