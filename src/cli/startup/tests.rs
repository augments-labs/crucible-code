//! Which key is read, and what a startup that fails leaves behind.

use std::cell::RefCell;

use crucible_core::{
    Aside, Ask, Cancel, Message, Outgoing, Remember, Sensitivity, Steer, ToolCall, Verdict,
};

use crucible_config::Settings;

use super::*;
use crate::cli::sample::{Sample, WRITTEN};
use crate::cli::{NO_MODEL_CHOSEN, NOTHING_TO_ASK};

struct Nobody;

impl Ask for Nobody {
    fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
        (Verdict::Deny, Remember::Never)
    }
}

/// The built-in providers, as one generation to resolve a name against.
fn catalogue() -> Providers {
    crate::cli::providers()
        .expect("the built-in providers register")
        .snapshot()
}

/// The record the wiring resolves before it builds anything.
fn serving(named: &str) -> Served {
    served(&catalogue(), named).expect("a provider this build has")
}

/// What gets built on a machine nobody has logged in from. The store is the
/// other half of the same question and the tests about it call [`provider`]
/// themselves; every test about a variable is asking this one.
fn built(
    serving: Option<Served>,
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
) -> Result<Box<dyn Provider>, Fatal> {
    let stored = StoredCredentials::default();
    let subscriptions = Subscriptions::production();
    provider(
        serving,
        NOTHING_TO_ASK,
        ProviderAuth {
            settings,
            from,
            stored: &stored,
            subscriptions: &subscriptions,
        },
    )
}

/// What a credential writes into the header it signs with.
///
/// The one place a key is legitimately read back, and the only way to tell two
/// of them apart: the value is applied to a request and never returned, which
/// is the property, so the request is where an assertion about *which* key was
/// used has to look.
fn signing(credential: &dyn Credential) -> String {
    let mut request = Outgoing::new();
    credential
        .authorize(&mut request)
        .expect("a key is applied rather than renewed");

    request
        .headers()
        .iter()
        .find(|(name, _)| &**name == "authorization")
        .map(|(_, value)| value.to_string())
        .expect("the header it was built for")
}

#[test]
fn a_key_written_down_signs_the_request_where_no_variable_holds_one() {
    // What `/login` is for: given once, and the shell says nothing about it
    // ever again.
    let sample = Sample::new("key-stored");
    let keys = sample.stored("openai");

    let signed = key(
        "OPENAI_API_KEY",
        Header::bearer(),
        &|_| None,
        keys.get("openai"),
    )
    .expect("a key written down");

    assert_eq!(signing(&*signed), format!("Bearer {WRITTEN}"));
}

#[test]
fn a_variable_signs_the_request_over_the_key_written_down_for_it() {
    // A second account, a work key, or one that was rotated an hour ago. What
    // is exported is chosen for this run and lasts as long as the shell it was
    // exported in, so it is the one that wins over what is on the disk.
    let sample = Sample::new("key-exported-over-stored");
    let keys = sample.stored("openai");

    let signed = key(
        "OPENAI_API_KEY",
        Header::bearer(),
        &|_| Some("an-exported-key".to_owned()),
        keys.get("openai"),
    )
    .expect("a key exported");

    assert_eq!(signing(&*signed), "Bearer an-exported-key");
}

#[test]
fn a_variable_exported_blank_leaves_the_key_written_down_standing() {
    // `OPENAI_API_KEY=` turns off the variable, which is all it can say
    // anything about. Somebody who ran `/login` said so once and for every run
    // after it, and their shell profile is not where they unsay it.
    let sample = Sample::new("key-blanked");
    let keys = sample.stored("openai");

    let signed = key(
        "OPENAI_API_KEY",
        Header::bearer(),
        &|_| Some(String::new()),
        keys.get("openai"),
    )
    .expect("a key written down");

    assert_eq!(signing(&*signed), format!("Bearer {WRITTEN}"));
}

#[test]
fn each_provider_reads_the_key_belonging_to_it() {
    // The pairing is the whole of this function, and every arm builds whichever
    // way round it is wired: swapping two bodies would send one vendor's key to
    // the other vendor's endpoint with everything else still green.
    let read = RefCell::new(Vec::new());
    let from = |name: &str| {
        read.borrow_mut().push(name.to_owned());
        Some("a-key".to_owned())
    };
    let nothing = Settings::default();

    let keys: Vec<&str> = crate::cli::offered(&catalogue())
        .map(|one| {
            let made = built(Some(one), &nothing, &from).expect("a provider");

            assert_eq!(made.name(), one.name);
            one.key
        })
        .collect();

    assert_eq!(read.into_inner(), keys);
}

#[test]
fn a_provider_reads_the_variable_its_configuration_names() {
    // Somebody with a work key and a personal key has two variables, and only
    // one of them can be the vendor's usual name.
    let sample = Sample::new("key-variable");
    let settings =
        sample.user(r#"{"providers": {"anthropic": {"apiKeyEnv": "WORK_ANTHROPIC_KEY"}}}"#);

    let read = RefCell::new(Vec::new());
    built(Some(serving("anthropic")), &settings, &|name: &str| {
        read.borrow_mut().push(name.to_owned());
        Some("a-key".to_owned())
    })
    .expect("a provider");

    // That name and no other. Reading the usual variable as well would pick up
    // a key the user pointed crucible away from.
    assert_eq!(read.into_inner(), ["WORK_ANTHROPIC_KEY"]);
}

#[test]
fn a_provider_is_built_at_the_address_its_configuration_names() {
    // A gateway or a proxy speaking the vendor's protocol. What the address
    // then does with a request is the provider's own test; what this one
    // watches is that a configured address is read and accepted rather than
    // refused on the way past.
    let sample = Sample::new("base-url");
    let settings =
        sample.user(r#"{"providers": {"anthropic": {"baseUrl": "https://gateway.example/v1"}}}"#);

    let made = built(Some(serving("anthropic")), &settings, &|_| {
        Some("a-key".to_owned())
    })
    .expect("a provider pointed at a gateway");

    assert_eq!(made.name(), "anthropic");
}

#[test]
fn an_openai_subscription_uses_its_fixed_audience() {
    // A plan's token is issued against one address, and the registry is what
    // keeps the two paired: the credential and its audience come back as one
    // answer rather than as halves a call site could recombine.
    let sample = Sample::new("subscription-endpoint");
    let keys = sample.subscribed("openai");
    let subscriptions = Subscriptions::production();

    let (endpoint, _) = credential(
        ApiAudience {
            provider: "openai",
            variable: "OPENAI_API_KEY",
            vendor: OpenAi::VENDOR,
        },
        None,
        ProviderAuth {
            settings: &Settings::default(),
            from: &|_| None,
            stored: &keys,
            subscriptions: &subscriptions,
        },
    )
    .expect("a stored subscription");

    assert_eq!(endpoint, OpenAi::SUBSCRIPTION);
}

#[test]
fn a_deliberate_subscription_login_wins_over_an_inherited_api_key() {
    // A variable is inherited from whichever shell launched this run; an
    // account authorized through `/login` was chosen on purpose, after the
    // shell was what it was. The deliberate credential signs the request.
    let sample = Sample::new("subscription-over-environment");
    let keys = sample.subscribed("openai");
    let subscriptions = Subscriptions::production();

    let (endpoint, _) = credential(
        ApiAudience {
            provider: "openai",
            variable: "OPENAI_API_KEY",
            vendor: OpenAi::VENDOR,
        },
        None,
        ProviderAuth {
            settings: &Settings::default(),
            from: &|_| Some("inherited-key".to_owned()),
            stored: &keys,
            subscriptions: &subscriptions,
        },
    )
    .expect("the explicitly stored account login");

    assert_eq!(endpoint, OpenAi::SUBSCRIPTION);
}

#[test]
fn a_kimi_subscription_uses_the_managed_coding_audience() {
    let sample = Sample::new("kimi-subscription-endpoint");
    let keys = sample.subscribed("moonshot");
    let subscriptions = Subscriptions::production();

    let (endpoint, _) = credential(
        ApiAudience {
            provider: "moonshot",
            variable: "MOONSHOT_API_KEY",
            vendor: Moonshot::CODING,
        },
        None,
        ProviderAuth {
            settings: &Settings::default(),
            from: &|_| None,
            stored: &keys,
            subscriptions: &subscriptions,
        },
    )
    .expect("a stored Kimi account");

    assert_eq!(endpoint, Moonshot::CODING);
}

#[test]
fn an_exported_api_key_still_selects_a_configured_address_over_a_subscription() {
    // An exported key is chosen for this run, and the configured gateway is
    // where its owner pointed it. A stored subscription outranks neither.
    let sample = Sample::new("api-key-over-subscription");
    let keys = sample.subscribed("openai");
    let settings =
        sample.user(r#"{"providers": {"openai": {"baseUrl": "https://gateway.example/v1"}}}"#);
    let subscriptions = Subscriptions::production();

    let (endpoint, _) = credential(
        ApiAudience {
            provider: "openai",
            variable: "OPENAI_API_KEY",
            vendor: OpenAi::VENDOR,
        },
        sending_to(&settings, "openai").expect("an address the check accepted"),
        ProviderAuth {
            settings: &settings,
            from: &|_| Some("an-exported-key".to_owned()),
            stored: &keys,
            subscriptions: &subscriptions,
        },
    )
    .expect("the explicit API key for this run");

    assert_eq!(endpoint.as_str(), "https://gateway.example/v1");
}

#[test]
fn a_subscription_token_never_follows_a_configured_api_key_address() {
    // `baseUrl` is somebody's reason not to reach the vendor, and a plan's
    // token is the vendor's: with nothing else to sign with, the run is told
    // the two settings cannot stand together rather than sending the token to
    // a gateway.
    let sample = Sample::new("subscription-custom-endpoint");
    let keys = sample.subscribed("openai");
    let settings =
        sample.user(r#"{"providers": {"openai": {"baseUrl": "https://gateway.example/v1"}}}"#);
    let subscriptions = Subscriptions::production();

    let problem = provider(
        Some(serving("openai")),
        NO_MODEL_CHOSEN,
        ProviderAuth {
            settings: &settings,
            from: &|_| None,
            stored: &keys,
            subscriptions: &subscriptions,
        },
    )
    .expect_err("a subscription sent to an API-key gateway");

    assert!(matches!(problem, Fatal::SubscriptionAddress { .. }));
}

#[test]
fn an_address_that_would_put_the_key_on_the_wire_stops_the_run() {
    // Not a warning that carries on at the vendor's address: somebody who set
    // this has a reason not to reach the vendor, and going there anyway would
    // send the key somewhere they did not ask for.
    let sample = Sample::new("base-url-insecure");
    let settings =
        sample.user(r#"{"providers": {"anthropic": {"baseUrl": "http://gateway.example"}}}"#);

    let problem = built(Some(serving("anthropic")), &settings, &|_| {
        Some("a-key".to_owned())
    })
    .expect_err("plain http to somewhere else to be refused");

    let said = problem.to_string();
    assert!(matches!(problem, Fatal::Address { .. }), "{problem:?}");

    // The dotted path and the value, because whoever reads this has the file
    // open and needs to find the line.
    assert!(said.contains("providers.anthropic.baseUrl"), "{said}");
    assert!(said.contains("http://gateway.example"), "{said}");
}

#[test]
fn a_missing_key_names_the_variable_to_set_and_not_its_value() {
    // The name is configuration; the value is the secret. Only one of them is
    // allowed to reach a terminal. Reachable because the flag can name a
    // provider outright — a provider chosen from the keys held has one by
    // construction.
    let problem = built(Some(serving("openai")), &Settings::default(), &|_| None)
        .expect_err("no key was set");

    assert_eq!(problem.to_string(), "OPENAI_API_KEY is not set");
}

#[test]
fn a_machine_with_no_key_at_all_gets_the_provider_that_answers_nothing() {
    // Not a refusal to start. The session is the place the key gets set up, and
    // ending the process takes away the screen that is done on.
    let read = RefCell::new(Vec::new());

    let nowhere = built(None, &Settings::default(), &|name: &str| {
        read.borrow_mut().push(name.to_owned());
        Some("a-key".to_owned())
    })
    .expect("a provider that refuses rather than a refusal to build one");

    assert_eq!(nowhere.name(), "none");
    assert!(
        read.into_inner().is_empty(),
        "a key was read for a provider there is none of"
    );
}

#[test]
fn every_name_the_registry_holds_is_one_its_own_record_can_build() {
    // The check runs before the banner and the factory runs after it. A name
    // the first let through and the second had no arm for would be a run that
    // announced its model and then said the provider does not exist. A record
    // carries its own factory, so the two halves cannot be registered apart —
    // this is that walked, name by name.
    for one in crate::cli::offered(&catalogue()) {
        served(&catalogue(), one.name).expect("a check that agrees with the record");
        built(Some(one), &Settings::default(), &|_| {
            Some("a-key".to_owned())
        })
        .expect("an arm for every name the registry holds");
    }
}

#[test]
fn a_name_no_record_was_registered_under_is_refused_and_the_others_are_named() {
    // The sentence is the whole of what somebody who mistyped a provider has to
    // work from, so it names what this build actually holds rather than only
    // what it does not.
    let problem = served(&catalogue(), "ollama").expect_err("this build has no such provider");
    let said = problem.to_string();

    assert!(said.contains("ollama"), "{said}");
    for one in crate::cli::offered(&catalogue()) {
        assert!(said.contains(one.name), "{said} omits {}", one.name);
    }
}

#[test]
fn a_provider_taken_out_of_the_registry_stops_being_a_name_this_build_serves() {
    // The generation a name is read against is the one in force, not the list
    // this build was compiled with: a provider deregistered is a provider gone,
    // including from the sentence that says what is left.
    let registry = crate::cli::providers().expect("the built-in providers register");
    let mut staged = registry.stage();
    staged
        .deregister("anthropic")
        .expect("a registered provider");
    registry.commit(staged).expect("the smaller generation");

    let left = registry.snapshot();
    let problem = served(&left, "anthropic").expect_err("a provider no longer registered");
    let said = problem.to_string();

    assert!(!said.contains("anthropic, "), "{said} still offers it");
    assert!(said.contains("moonshot"), "{said}");
}

#[test]
fn a_startup_with_nothing_to_authenticate_with_leaves_no_session_behind() {
    // An empty session written for a run that never happened is then the newest
    // one for this directory, so --continue would offer it instead of the last
    // real session.
    let sample = Sample::new("no-key");
    let (logs, workspace) = (sample.logs(), sample.workspace());

    // A provider this build does serve, so the only thing left to fail is the
    // key — and the lookup says there is none regardless of what the shell
    // running this test happens to export.
    let Err(problem) = assemble(&Startup {
        providers: &catalogue(),
        provider: Some(serving("openai")),
        unasked: NO_MODEL_CHOSEN,
        model: Some("gpt-5.6-terra"),
        effort: None,
        resuming: Resuming::No,
        mode: Mode::Ask,
        leaving: &crucible_tools::Background::new(),
        settings: &Settings::default(),
        sessions: &logs,
        workspace: &workspace,
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        putting: &Putting::new(),
        hosting: &[],
        terminal: true,
        from: &|_| None,
        stored: &StoredCredentials::default(),
        subscriptions: &Subscriptions::production(),
    }) else {
        panic!("a startup with no key was accepted");
    };

    assert!(matches!(problem, Fatal::Credential(_)), "{problem:?}");
    assert!(
        !logs.exists(),
        "a session was written for a startup that failed"
    );
}

#[test]
fn a_session_with_nothing_chosen_starts_and_asks_for_no_model() {
    // The state the warning under the welcome describes. Everything but the
    // turn works, which is what leaves `/model` somewhere to be typed.
    let sample = Sample::new("no-model");
    let (logs, workspace) = (sample.logs(), sample.workspace());

    let runner = assemble(&Startup {
        providers: &catalogue(),
        provider: None,
        unasked: NOTHING_TO_ASK,
        model: None,
        effort: None,
        resuming: Resuming::No,
        mode: Mode::Ask,
        leaving: &crucible_tools::Background::new(),
        settings: &Settings::default(),
        sessions: &logs,
        workspace: &workspace,
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        putting: &Putting::new(),
        hosting: &[],
        terminal: true,
        from: &|_| None,
        stored: &StoredCredentials::default(),
        subscriptions: &Subscriptions::production(),
    })
    .expect("a session with nothing set up still starts");

    assert_eq!(runner.model(), "", "an unnamed model is the empty name");
}

/// The specification one startup resolves to, for a model of `anthropic`.
///
/// The startup is what `coding` reads its answer off, so the tests about the
/// answer build one. Everything the specification does not touch — the
/// session, the workspace, the credentials — belongs to `assemble`, which
/// these tests reach through separately.
fn specified(model: &str, effort: Option<Effort>, settings: &Settings, told: &str) -> AgentSpec {
    let sample = Sample::new(&format!("specified-{model}"));
    let (logs, workspace) = (sample.logs(), sample.workspace());
    let catalogue = catalogue();
    let startup = Startup {
        providers: &catalogue,
        provider: Some(serving("anthropic")),
        unasked: NOTHING_TO_ASK,
        model: Some(model),
        effort,
        resuming: Resuming::No,
        mode: Mode::Ask,
        leaving: &crucible_tools::Background::new(),
        settings,
        sessions: &logs,
        workspace: &workspace,
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        putting: &Putting::new(),
        hosting: &[],
        terminal: true,
        from: &|_| None,
        stored: &StoredCredentials::default(),
        subscriptions: &Subscriptions::production(),
    };

    coding(&startup, "anthropic", model, told)
}

#[test]
fn a_rung_the_run_resolved_is_on_the_model_every_turn_is_asked_of() {
    // The one thing this function does with it: a rung that stopped here would
    // be shown on the welcome and asked for nowhere.
    let settings = Settings::default();

    assert_eq!(
        specified("claude-opus-5", Some(Effort::Xhigh), &settings, "")
            .model
            .effort,
        Some(Effort::Xhigh)
    );

    // And nothing where nothing said, which is the field left off rather than
    // a rung this program chose on the vendor's behalf.
    assert_eq!(
        specified("claude-opus-5", None, &settings, "").model.effort,
        None
    );
}

#[test]
fn operational_windows_use_conservative_provider_defaults() {
    let settings = Settings::default();

    for (provider, model, held) in [
        ("anthropic", "claude-sonnet-5", 200_000),
        ("anthropic", "claude-haiku-4-5", 200_000),
        ("openai", "gpt-5.6-sol", 272_000),
        ("openai", "gpt-5.5", 272_000),
        ("moonshot", "k3", 262_144),
        ("moonshot", "kimi-for-coding-highspeed", 262_144),
    ] {
        assert_eq!(
            window(&catalogue(), serving(provider), model, &settings),
            held,
            "{provider}/{model}"
        );
    }
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

#[test]
fn how_long_an_answer_may_be_is_the_model_own_limit_held_under_the_ceiling() {
    // A model this build has the limits of: its own output limit is far above
    // the ceiling, so the ceiling is what is asked for.
    let known = specified("claude-opus-5", None, &Settings::default(), "");
    assert_eq!(known.model.max_tokens, CEILING);

    // And one it has never heard of, where nothing is known and the lower
    // figure is what keeps a request from being refused outright.
    let unknown = specified("claude-from-the-future", None, &Settings::default(), "");
    assert_eq!(unknown.model.max_tokens, UNKNOWN_CEILING);
}

/// What `named` gets to reach the web with, given a key and maybe a model.
fn reaching_for(named: &str, model: Option<&'static str>) -> Reaching {
    let sample = Sample::new(&format!("web-source-{named}"));
    let (logs, workspace) = (sample.logs(), sample.workspace());

    web(
        &Startup {
            providers: &catalogue(),
            provider: Some(serving(named)),
            unasked: NO_MODEL_CHOSEN,
            model,
            effort: None,
            resuming: Resuming::No,
            mode: Mode::Ask,
            leaving: &crucible_tools::Background::new(),
            settings: &Settings::default(),
            sessions: &logs,
            workspace: &workspace,
            ledger: &Ledger::new(),
            revealed: &Revealed::new(),
            plan: &Plan::new(),
            putting: &Putting::new(),
            hosting: &[],
            terminal: true,
            from: &|_| Some("sk-test".to_owned()),
            stored: &StoredCredentials::default(),
            subscriptions: &Subscriptions::production(),
        },
        &Settings::default(),
    )
}

#[test]
fn anthropic_serves_both_halves_of_reaching_the_web() {
    let reaching = reaching_for("anthropic", Some("claude-opus-5"));

    assert!(reaching.searching.is_some());
    assert!(reaching.fetching.is_some());
}

#[test]
fn google_exposes_url_fetch_without_a_search_source() {
    let reaching = reaching_for("google", Some("gemini-3.8-flash"));

    assert!(
        reaching.searching.is_none(),
        "Google Search must not be exposed"
    );
    assert!(
        reaching.fetching.is_some(),
        "URL context must remain available"
    );
}

#[test]
fn google_web_authority_is_api_key_only_and_uses_the_checked_recipient() {
    let sample = Sample::new("google-web-authority");
    let stored = sample.subscribed("google");
    let subscriptions = Subscriptions::production();
    let defaults = Settings::default();
    let absent = |_: &str| None;
    let auth = ProviderAuth {
        settings: &defaults,
        from: &absent,
        stored: &stored,
        subscriptions: &subscriptions,
    };
    let reaching = google_web(wiring(serving("google"), auth).unwrap(), "gemini-3.8-flash");
    assert!(reaching.searching.is_none());
    assert!(reaching.fetching.is_none());

    let settings = sample.user(r#"{"providers":{"google":{"baseUrl":"https://gateway.example/interactions?alt=sse","apiKeyEnv":"WORK_GEMINI"}}}"#);
    let from = |name: &str| (name == "WORK_GEMINI").then(|| "synthetic-key".into());
    let auth = ProviderAuth {
        settings: &settings,
        from: &from,
        stored: &stored,
        subscriptions: &subscriptions,
    };
    let reaching = google_web(wiring(serving("google"), auth).unwrap(), "gemini-3.8-flash");
    assert!(reaching.searching.is_none());
    assert!(reaching.fetching.is_some());

    let invalid =
        sample.user(r#"{"providers":{"google":{"baseUrl":"http://remote.example/interactions"}}}"#);
    assert!(
        wiring(
            serving("google"),
            ProviderAuth {
                settings: &invalid,
                from: &from,
                stored: &stored,
                subscriptions: &subscriptions
            }
        )
        .is_err()
    );
}

#[test]
fn openai_serves_both_through_one_tool() {
    // Reading a page is an action inside this vendor's search tool rather than
    // a tool of its own, which is a fact about the wire and not about what the
    // model is offered: both tools appear and one service answers them.
    let reaching = reaching_for("openai", Some("gpt-5.6"));

    assert!(reaching.searching.is_some());
    assert!(reaching.fetching.is_some());
}

#[test]
fn moonshot_serves_both_halves_from_kimi_code() {
    // Its own two services, which is what this vendor's own client reaches.
    let reaching = reaching_for("moonshot", Some("kimi-k2"));

    assert!(reaching.searching.is_some());
    assert!(reaching.fetching.is_some());
}

#[test]
fn a_session_with_no_model_chosen_reaches_nothing() {
    // A side request has to name a model, and the one it names is the session's.
    // Nothing is chosen yet in the state `/model` exists to leave open.
    for named in ["anthropic", "google", "openai"] {
        let reaching = reaching_for(named, None);

        assert!(reaching.searching.is_none(), "{named}");
        assert!(reaching.fetching.is_none(), "{named}");
    }
}

/// The tools a session with these terms would be given.
fn offered(terminal: bool) -> crucible_runner::Tools {
    let workspace = Workspace::open(std::env::temp_dir().as_path()).expect("a directory");
    let logs = std::env::temp_dir().join(format!("crucible-tools-{}", std::process::id()));

    tools(
        &Startup {
            providers: &catalogue(),
            provider: None,
            unasked: NOTHING_TO_ASK,
            model: None,
            effort: None,
            resuming: Resuming::No,
            mode: Mode::Ask,
            leaving: &crucible_tools::Background::new(),
            settings: &Settings::default(),
            sessions: &logs,
            workspace: &workspace,
            ledger: &Ledger::new(),
            revealed: &Revealed::new(),
            plan: &Plan::new(),
            putting: &Putting::new(),
            hosting: &[],
            terminal,
            from: &|_| None,
            stored: &StoredCredentials::default(),
            subscriptions: &Subscriptions::production(),
        },
        &Settings::default(),
        Reaching {
            searching: None,
            fetching: None,
        },
        Arc::new(LocalSandbox::new()),
    )
    .expect("the built-in tool roster is valid")
}

#[test]
fn a_session_with_somebody_at_a_keyboard_can_ask_them() {
    // Advertised rather than deferred: a model that cannot see it will not go
    // looking for it at the moment it realises it should ask, and that moment is
    // the only thing it exists for.
    let tools = offered(true);

    assert!(
        tools
            .advertised()
            .iter()
            .any(|schema| schema.name == "ask_user"),
        "the tool was registered without being offered"
    );
}

#[test]
fn a_session_with_nobody_there_does_not_carry_a_tool_for_asking_them() {
    // Not deferred either, so a search cannot find it: a tool that can only ever
    // answer "there is no one here" is a schema spent saying so.
    let tools = offered(false);

    assert!(tools.find("ask_user").is_none());
    assert!(
        tools
            .deferred()
            .iter()
            .all(|schema| schema.name() != "ask_user")
    );
}

#[test]
fn the_tools_a_session_already_had_are_unchanged_in_name_and_order() {
    let tools = offered(true);
    let named: Vec<String> = tools
        .advertised()
        .iter()
        .map(|schema| schema.name.to_owned())
        .collect();

    assert_eq!(
        named,
        [
            "read",
            "grep",
            "glob",
            "edit",
            "write",
            "bash",
            "ask_user",
            "tool_search"
        ]
    );
}

#[test]
fn recap_room_defaults_to_ten_k_and_accepts_a_configured_ceiling() {
    let defaults = policy(&Settings::default()).compaction;
    assert_eq!(defaults.recap_tokens, 10_240);

    let sample = Sample::new("compaction-recap-ceiling");
    let configured = sample.settings(r#"{"compaction":{"recap":12000}}"#);
    assert_eq!(policy(&configured).compaction.recap_tokens, 12_000);
}

#[test]
fn the_agent_is_named_coding_and_stands_under_what_the_wiring_asked() {
    // Two fields the wiring decides and a later registry needs. This pins the
    // stable operator instructions `coding` puts in the definition; the
    // assembly test below follows them through the runner and separately
    // proves the workspace fact reaches typed context.
    let asked = "read the workspace before changing it";
    let built = specified("claude-opus-5", None, &Settings::default(), asked);

    assert_eq!(built.id.as_str(), "coding");
    assert_eq!(built.instructions(), Some(asked));
}

#[test]
fn a_definition_the_wiring_had_nothing_to_say_under_is_told_nothing() {
    // The rule `AgentSpec::told` enforces, at the one site outside the runner
    // that writes the field: no instructions and empty instructions are two
    // different requests, and a prompt nobody wrote is the first.
    //
    // Unreachable through the shipped wiring, because `standing::under` always
    // names where the work is and so never returns an empty prompt. What this
    // pins is that the composition root reaches the field through the write
    // path that enforces the rule rather than around it — which is what a
    // definition read from a file, where an empty body is an ordinary case,
    // will arrive needing.
    let built = specified("claude-opus-5", None, &Settings::default(), "");

    assert_eq!(
        built.instructions(),
        None,
        "a definition nobody wrote a prompt for carries the empty string"
    );
}

#[test]
fn a_session_is_assembled_with_stable_instructions_and_workspace_context() {
    // Both request surfaces the wiring owns: operator instructions stay in the
    // stable system field, while the workspace root is assembled as typed
    // context on the first pass. Testing only either half would let startup
    // silently drop the other at their new boundary.
    let sample = Sample::new("startup-assembled-under");
    let (logs, workspace) = (sample.logs(), sample.workspace());
    let configured = sample.settings(r#"{"compaction":{"spendCeiling":500000}}"#);

    let mut runner = assemble(&Startup {
        providers: &catalogue(),
        provider: None,
        unasked: NOTHING_TO_ASK,
        model: None,
        effort: None,
        resuming: Resuming::No,
        mode: Mode::Ask,
        leaving: &crucible_tools::Background::new(),
        settings: &configured,
        sessions: &logs,
        workspace: &workspace,
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        putting: &Putting::new(),
        hosting: &[],
        terminal: true,
        from: &|_| None,
        stored: &StoredCredentials::default(),
        subscriptions: &Subscriptions::production(),
    })
    .expect("a session to assemble");

    let asked = runner
        .instructions()
        .expect("a turn is asked under something");
    assert_eq!(asked, standing::under(&configured));
    assert!(!asked.contains(&workspace.root().display().to_string()));

    let (events, _seen) = std::sync::mpsc::channel();
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
    let run = runner.starting(&events, &cancel, &steer, &aside);
    let _ = runner.turn("probe", Box::new([]), &mut Nobody, &run);
    let workspace_fact = runner
        .transcript()
        .messages()
        .iter()
        .find_map(|message| match message {
            Message::Context(fragment) if fragment.section() == "workspace" => {
                Some(fragment.text())
            }
            Message::Context(_)
            | Message::User { .. }
            | Message::Agent { .. }
            | Message::ToolResults(_) => None,
        })
        .expect("the first pass workspace context");
    assert!(
        workspace_fact.contains(&workspace.root().display().to_string()),
        "the session was assembled without its workspace context"
    );

    assert_eq!(
        runner.policy().bounds.spend,
        Some(500_000),
        "the ceiling the document set did not reach the runner"
    );
}

#[test]
fn a_configured_spend_ceiling_is_resolved_off_the_document() {
    // The figure a document sets and the loop enforces, with the composition
    // root the only place the two meet. Nothing said means no ceiling, which is
    // the same absence a document that never mentions it leaves.
    assert_eq!(policy(&Settings::default()).bounds.spend, None);

    let sample = Sample::new("startup-spend-ceiling");
    let configured = sample.settings(r#"{"compaction":{"spendCeiling":500000}}"#);
    assert_eq!(policy(&configured).bounds.spend, Some(500_000));
}

#[test]
fn the_figures_no_document_reaches_are_the_ones_this_program_ships() {
    // The other half of the resolution above: five compaction figures and the
    // spend ceiling come out of a document, and the two byte ceilings and the
    // retry policy deliberately do not. Nothing else in the tree reads them
    // back, so without this a composition root that zeroed either one would
    // leave every test green while the loop lost its memory bound and its
    // patience with a provider.
    let shipped = RunPolicy::default();

    let sample = Sample::new("startup-unreached-figures");
    let configured = sample.settings(
        r#"{"compaction":{"spendCeiling":500000,"keep":1000,"recap":12000,"askOnResume":10}}"#,
    );

    for (named, built) in [
        ("a machine with no document", policy(&Settings::default())),
        (
            "a document that set every figure it can",
            policy(&configured),
        ),
    ] {
        assert_eq!(
            built.bounds.response_bytes, shipped.bounds.response_bytes,
            "{named} moved the response ceiling"
        );
        assert_eq!(
            built.bounds.tool_output_bytes, shipped.bounds.tool_output_bytes,
            "{named} moved the tool-output ceiling"
        );
        assert_eq!(
            built.retry.attempts, shipped.retry.attempts,
            "{named} moved how many times a failed response is asked for again"
        );
        assert_eq!(
            built.retry.first_pause, shipped.retry.first_pause,
            "{named} moved the wait before the first retry"
        );
    }

    assert_eq!(
        policy(&configured).bounds.spend,
        Some(500_000),
        "the document above was not read, so the figures it left alone prove nothing"
    );
    assert_eq!(policy(&configured).compaction.keep_tokens, 1_000);
}

/// A configuration file holding one server, named `docs`, run by `command`.
fn wrote(command: &str) -> String {
    format!(r#"{{"mcp": {{"servers": {{"docs": {{"command": "{command}"}}}}}}}}"#)
}

#[test]
fn naming_a_server_nobody_wrote_down_fails_before_a_session_file_exists() {
    // For the reason the missing-credential startup above leaves none: a run
    // that cannot host what it was asked to host is one that never happened,
    // and an empty session file would be the newest for this directory.
    let sample = Sample::new("unknown-server");
    let (logs, workspace) = (sample.logs(), sample.workspace());

    let Err(problem) = assemble(&Startup {
        providers: &catalogue(),
        provider: None,
        unasked: NOTHING_TO_ASK,
        model: None,
        effort: None,
        resuming: Resuming::No,
        mode: Mode::Ask,
        leaving: &crucible_tools::Background::new(),
        settings: &Settings::default(),
        sessions: &logs,
        workspace: &workspace,
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        putting: &Putting::new(),
        hosting: &["docs".to_owned()],
        terminal: true,
        from: &|_| None,
        stored: &StoredCredentials::default(),
        subscriptions: &Subscriptions::production(),
    }) else {
        panic!("a run naming a server nothing wrote down was accepted");
    };

    assert!(matches!(problem, Fatal::NoServer { .. }), "{problem:?}");
    assert!(
        !logs.exists(),
        "a session was written for a run that could not be hosted"
    );
}

#[test]
fn a_run_that_named_a_server_reaches_the_runner_as_a_live_toolset() {
    // What a hosted run costs, stated where it is paid: the built-in roster is
    // no longer the between-turn view, because the generation the model is
    // offered is not assembled until the turn that starts the servers. A run
    // that named none must therefore keep the roster it always had, which is
    // the other half of this test and the reason there are two constructors.
    let sample = Sample::new("hosted");
    let (logs, workspace) = (sample.logs(), sample.workspace());
    let directory = sample.root().join("bin");
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let at = directory.join(crucible_tools::program::spelled("docs-mcp"));
    std::fs::write(&at, "").expect("a temporary directory");
    let path = directory.display().to_string();
    let settings = sample.user(&wrote("docs-mcp"));

    let starting = |hosting: &[String]| {
        assemble(&Startup {
            providers: &catalogue(),
            provider: None,
            unasked: NOTHING_TO_ASK,
            model: None,
            effort: None,
            resuming: Resuming::No,
            mode: Mode::Ask,
            leaving: &crucible_tools::Background::new(),
            settings: &settings,
            sessions: &logs,
            workspace: &workspace,
            ledger: &Ledger::new(),
            revealed: &Revealed::new(),
            plan: &Plan::new(),
            putting: &Putting::new(),
            hosting,
            terminal: true,
            from: &|name| (name == "PATH").then(|| path.clone()),
            stored: &StoredCredentials::default(),
            subscriptions: &Subscriptions::production(),
        })
        .expect("a run this test wrote the record for")
    };

    let hosted = starting(&["docs".to_owned()]);
    assert!(
        hosted.offering().is_empty(),
        "a live toolset has no generation until the turn that prepares it"
    );

    let alone = starting(&[]);
    assert!(
        alone.offering().contains(&"bash".to_owned()),
        "a run that named no server is the built-in roster itself: {:?}",
        alone.offering()
    );
}
