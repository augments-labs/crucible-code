//! Which key is read, and what a startup that fails leaves behind.

use std::cell::RefCell;

use crucible_core::Outgoing;

use crucible_config::Settings;

use super::*;
use crate::cli::sample::{Sample, WRITTEN};
use crate::cli::{NO_MODEL_CHOSEN, NOTHING_TO_ASK};

/// The entry the wiring resolves before it builds anything.
fn serving(named: &str) -> Served {
    served(named).expect("a provider this build has")
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

    for one in PROVIDERS {
        let made = built(Some(serving(one.name)), &nothing, &from).expect("a provider");

        assert_eq!(made.name(), one.name);
    }

    assert_eq!(read.into_inner(), PROVIDERS.map(|one| one.key));
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
fn a_name_in_the_list_with_no_arm_behind_it_is_refused_rather_than_built() {
    // The list and the match are two halves of one change. A provider added to
    // the list alone reaches here as a name nothing can build, and this is the
    // arm that says so instead of returning a provider for the wrong vendor.
    // Qualified: `Model` here is the one a turn is asked of, and this is the one
    // a panel offers.
    const OFFERED: &[crate::cli::Model] = &[crate::cli::Model::new("llama-4", &[])];

    let unarmed = Served {
        name: "ollama",
        shown: "Ollama",
        key: "OLLAMA_API_KEY",
        models: OFFERED,
    };

    let problem = built(Some(unarmed), &Settings::default(), &|_| {
        Some("a-key".to_owned())
    })
    .expect_err("this build has no such provider");

    assert!(problem.to_string().contains("ollama"), "{problem}");
}

#[test]
fn every_name_the_check_accepts_is_one_an_arm_can_build() {
    // The check runs before the banner and the match runs after it. A name the
    // first let through and the second had no arm for would be a run that
    // announced its model and then said the provider does not exist.
    for one in PROVIDERS {
        served(one.name).expect("a check that agrees with the arm");
        built(Some(one), &Settings::default(), &|_| {
            Some("a-key".to_owned())
        })
        .expect("an arm for every name the check accepts");
    }

    let problem = served("ollama").expect_err("this build has no such provider");
    assert!(problem.to_string().contains("ollama"), "{problem}");
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
        cancel: &Cancel::new(),
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        putting: &Putting::new(),
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
        cancel: &Cancel::new(),
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        putting: &Putting::new(),
        terminal: true,
        from: &|_| None,
        stored: &StoredCredentials::default(),
        subscriptions: &Subscriptions::production(),
    })
    .expect("a session with nothing set up still starts");

    assert_eq!(runner.model(), "", "an unnamed model is the empty name");
}

#[test]
fn a_rung_the_run_resolved_is_on_the_model_every_turn_is_asked_of() {
    // The one thing this function does with it: a rung that stopped here would
    // be shown on the welcome and asked for nowhere.
    let asking = coding(
        "anthropic",
        "claude-opus-5",
        Some(Effort::Xhigh),
        &Settings::default(),
        "",
    );

    assert_eq!(asking.model.effort, Some(Effort::Xhigh));

    // And nothing where nothing said, which is the field left off rather than
    // a rung this program chose on the vendor's behalf.
    assert_eq!(
        coding("anthropic", "claude-opus-5", None, &Settings::default(), "")
            .model
            .effort,
        None
    );
}

#[test]
fn operational_windows_use_conservative_provider_defaults() {
    let settings = Settings::default();

    assert_eq!(
        window("anthropic", "claude-sonnet-5", &settings),
        Some(200_000)
    );
    assert_eq!(
        window("anthropic", "claude-haiku-4-5", &settings),
        Some(200_000)
    );
    assert_eq!(window("openai", "gpt-5.6-sol", &settings), Some(272_000));
    assert_eq!(window("openai", "gpt-5.5", &settings), Some(272_000));
    assert_eq!(window("moonshot", "k3", &settings), Some(262_144));
    assert_eq!(
        window("moonshot", "kimi-for-coding-highspeed", &settings),
        Some(262_144)
    );
}

#[test]
fn unknown_models_do_not_bypass_the_providers_default_operational_window() {
    let settings = Settings::default();

    assert_eq!(
        window("anthropic", "claude-future", &settings),
        Some(200_000)
    );
    assert_eq!(window("openai", "gpt-future", &settings), Some(272_000));
    assert_eq!(window("moonshot", "kimi-future", &settings), Some(262_144));
    assert_eq!(window("unheard-of", "model", &settings), None);
}

#[test]
fn an_explicit_context_window_can_opt_back_into_a_larger_window() {
    let sample = Sample::new("context-window-opt-in");
    let settings = sample.settings(
        r#"{"providers":{"anthropic":{"contextWindow":{"claude-sonnet-5":1000000}},"openai":{"defaultContextWindow":872000},"moonshot":{"contextWindow":{"k3":1048576}}}}"#,
    );

    assert_eq!(
        window("anthropic", "claude-sonnet-5", &settings),
        Some(1_000_000)
    );
    assert_eq!(window("openai", "gpt-5.6-sol", &settings), Some(872_000));
    assert_eq!(window("openai", "gpt-future", &settings), Some(872_000));
    assert_eq!(window("moonshot", "k3", &settings), Some(1_048_576));
}

#[test]
fn how_long_an_answer_may_be_is_the_model_own_limit_held_under_the_ceiling() {
    // A model this build has the limits of: its own output limit is far above
    // the ceiling, so the ceiling is what is asked for.
    let known = coding("anthropic", "claude-opus-5", None, &Settings::default(), "");
    assert_eq!(known.model.max_tokens, CEILING);

    // And one it has never heard of, where nothing is known and the lower
    // figure is what keeps a request from being refused outright.
    let unknown = coding(
        "anthropic",
        "claude-from-the-future",
        None,
        &Settings::default(),
        "",
    );
    assert_eq!(unknown.model.max_tokens, UNKNOWN_CEILING);
}

/// What `named` gets to reach the web with, given a key and maybe a model.
fn reaching_for(named: &str, model: Option<&'static str>) -> Reaching {
    let sample = Sample::new(&format!("web-source-{named}"));
    let (logs, workspace) = (sample.logs(), sample.workspace());

    web(
        &Startup {
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
            cancel: &Cancel::new(),
            ledger: &Ledger::new(),
            revealed: &Revealed::new(),
            plan: &Plan::new(),
            putting: &Putting::new(),
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
    for named in ["anthropic", "openai"] {
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
            cancel: &Cancel::new(),
            ledger: &Ledger::new(),
            revealed: &Revealed::new(),
            plan: &Plan::new(),
            putting: &Putting::new(),
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
    )
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
            .all(|schema| schema.name != "ask_user")
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
    // Two of the five fields the wiring decides, and the two a later reader
    // arrives needing. Nothing outside a test reads `id` yet — a later registry will select on it — and
    // `instructions` is the definition's opening value: what the window
    // reading and `Runner::instructions` answer with before a turn is taken.
    // `Runner::telling` replaces it before every turn, the first included, so
    // no turn goes out under this exact text.
    //
    // So this pins what `coding` puts in the definition, and not that a prompt
    // reaches the runner at all: it is handed `asked` and asserts it comes back
    // out. `a_session_is_assembled_asking_under_the_prompt_the_wiring_built`
    // is the one that follows it through `assemble`.
    let asked = "read the workspace before changing it";
    let built = coding(
        "anthropic",
        "claude-opus-5",
        None,
        &Settings::default(),
        asked,
    );

    assert_eq!(built.id.as_str(), "coding");
    assert_eq!(built.instructions(), Some(asked));
}

#[test]
fn a_definition_the_wiring_had_nothing_to_say_under_is_told_nothing() {
    // The rule `AgentSpec::instructions` states, at the one site outside the
    // runner that writes the field: no instructions and empty instructions are
    // two different requests, and a prompt nobody wrote is the first.
    //
    // Unreachable through the shipped wiring, because `standing::under` always
    // names where the work is and so never returns an empty prompt. What this
    // pins is that the composition root reaches the field through the write
    // path that enforces the rule rather than around it — which is what a
    // definition read from a file, where an empty body is an ordinary case,
    // will arrive needing.
    let built = coding("anthropic", "claude-opus-5", None, &Settings::default(), "");

    assert_eq!(
        built.instructions(),
        None,
        "a definition nobody wrote a prompt for carries the empty string"
    );
}

#[test]
fn a_session_is_assembled_asking_under_the_prompt_the_wiring_built() {
    // What `coding` is handed, rather than what it does with it. Everything
    // above tests the two ends — `standing::under` builds a prompt, `coding`
    // keeps the one it is given, `policy` resolves a document — and nothing
    // tested that either arrives, so `assemble` could pass an empty string or
    // drop the resolved ceiling on the way through and leave the whole package
    // green.
    let sample = Sample::new("startup-assembled-under");
    let (logs, workspace) = (sample.logs(), sample.workspace());
    let configured = sample.settings(r#"{"compaction":{"spendCeiling":500000}}"#);

    let runner = assemble(&Startup {
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
        cancel: &Cancel::new(),
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        putting: &Putting::new(),
        terminal: true,
        from: &|_| None,
        stored: &StoredCredentials::default(),
        subscriptions: &Subscriptions::production(),
    })
    .expect("a session to assemble");

    // The workspace root, because `standing::under` is the only thing that puts
    // it there. A substring rather than the whole prompt: this is about the
    // prompt having been built and handed over, and the wording of it belongs
    // to the tests that own the wording.
    let asked = runner
        .instructions()
        .expect("a turn is asked under something");
    assert!(
        asked.contains(&workspace.root().display().to_string()),
        "the session was assembled without the prompt the wiring built"
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
