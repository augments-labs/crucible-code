//! Which key is read, and what a startup that fails leaves behind.

use std::cell::RefCell;
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use crucible_credentials::Outgoing;
use crucible_runtime::{Aside, Cancel, Steer};
use crucible_tools::{Ask, Put, Remember, Sensitivity, Verdict};
use crucible_types::{Answered, Message, Question, ToolCall};

use crucible_config::Settings;
use crucible_context::SystemPrompt;

use super::*;
use crate::providers::{Missing, NO_MODEL_CHOSEN, NOTHING_TO_ASK};
use crate::sample::{Sample, WRITTEN};

/// Drives a future to its answer on a current-thread runtime of its own, the
/// way a test takes a turn on a runner it holds.
trait Awaited: std::future::Future + Sized {
    fn awaited(self) -> Self::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a test runtime")
            .block_on(self)
    }
}

impl<F: std::future::Future> Awaited for F {}

struct Nobody;

impl Ask for Nobody {
    fn ask<'a>(
        &'a mut self,
        _call: &'a ToolCall,
        _sensitivity: &'a Sensitivity,
    ) -> crucible_runtime::BoxFuture<'a, (Verdict, Remember)> {
        Box::pin(async { (Verdict::Deny, Remember::Never) })
    }
}

impl Put for Nobody {
    fn put<'a>(
        &'a self,
        _questions: &'a [Question],
    ) -> crucible_runtime::BoxFuture<'a, Option<Vec<Answered>>> {
        Box::pin(async { None })
    }
}

/// The built-in providers, as one generation to resolve a name against.
fn catalogue() -> Providers {
    crate::providers::providers()
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
) -> Result<Box<dyn Provider>, AppError> {
    let stored = StoredCredentials::default();
    let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());
    let http = HttpTurns::unavailable();
    provider(
        serving,
        NOTHING_TO_ASK,
        ProviderAuth {
            settings,
            from,
            stored: &stored,
            subscriptions: &subscriptions,
        },
        &http,
    )
}

#[test]
fn wiring_keeps_the_services_http_reference() {
    let stored = StoredCredentials::default();
    let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());
    let settings = Settings::default();
    let from = |_: &str| None;
    let http = HttpTurns::unavailable();
    let auth = ProviderAuth {
        settings: &settings,
        from: &from,
        stored: &stored,
        subscriptions: &subscriptions,
    };

    let provider = wiring(serving("anthropic"), auth, &http).unwrap();
    let web = wiring(serving("openai"), auth, &http).unwrap();

    assert!(std::ptr::eq(provider.http, &raw const http));
    assert!(std::ptr::eq(web.http, &raw const http));
}

/// Polls `authorizing` once and panics if it was not ready: every credential
/// built by this crate's own wiring answers at its first poll.
fn authorized<T>(authorizing: impl Future<Output = T>) -> T {
    let mut authorizing = pin!(authorizing);
    match authorizing
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(answer) => answer,
        Poll::Pending => panic!("a credential built here would have had to wait"),
    }
}

/// What a credential writes into the header it signs with.
///
/// The one place a key is legitimately read back, and the only way to tell two
/// of them apart: the value is applied to a request and never returned, which
/// is the property, so the request is where an assertion about *which* key was
/// used has to look.
fn signing(credential: &dyn Credential) -> String {
    let mut request = Outgoing::new();
    authorized(credential.authorize(&mut request)).expect("a key is applied rather than renewed");

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

    let keys: Vec<&str> = crate::providers::offered(&catalogue())
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
    let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());

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
    let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());

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
    let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());

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
    let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());

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
    let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());

    let http = HttpTurns::unavailable();
    let problem = provider(
        Some(serving("openai")),
        NO_MODEL_CHOSEN,
        ProviderAuth {
            settings: &settings,
            from: &|_| None,
            stored: &keys,
            subscriptions: &subscriptions,
        },
        &http,
    )
    .expect_err("a subscription sent to an API-key gateway");

    assert!(matches!(problem, AppError::SubscriptionAddress { .. }));
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
    assert!(matches!(problem, AppError::Address { .. }), "{problem:?}");

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
    for one in crate::providers::offered(&catalogue()) {
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
    for one in crate::providers::offered(&catalogue()) {
        assert!(said.contains(one.name), "{said} omits {}", one.name);
    }
}

#[test]
fn a_provider_taken_out_of_the_registry_stops_being_a_name_this_build_serves() {
    // The generation a name is read against is the one in force, not the list
    // this build was compiled with: a provider deregistered is a provider gone,
    // including from the sentence that says what is left.
    let registry = crate::providers::providers().expect("the built-in providers register");
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
        leaving: &crucible_builtins::Background::new(),
        services: &Services::new(),
        settings: &Settings::default(),
        sessions: &logs,
        workspace: &workspace,
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        asking: Arc::new(Nobody),
        hosting: &[],
        terminal: true,
        from: &|_| None,
        stored: &StoredCredentials::default(),
        subscriptions: &Subscriptions::production(&crucible_auth::Renewals::new()),
    }) else {
        panic!("a startup with no key was accepted");
    };

    assert!(matches!(problem, AppError::Credential(_)), "{problem:?}");
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

    let conversation = assemble(&Startup {
        providers: &catalogue(),
        provider: None,
        unasked: NOTHING_TO_ASK,
        model: None,
        effort: None,
        resuming: Resuming::No,
        mode: Mode::Ask,
        leaving: &crucible_builtins::Background::new(),
        services: &Services::new(),
        settings: &Settings::default(),
        sessions: &logs,
        workspace: &workspace,
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        asking: Arc::new(Nobody),
        hosting: &[],
        terminal: true,
        from: &|_| None,
        stored: &StoredCredentials::default(),
        subscriptions: &Subscriptions::production(&crucible_auth::Renewals::new()),
    })
    .expect("a session with nothing set up still starts");

    assert_eq!(
        conversation.runner().model(),
        "",
        "an unnamed model is the empty name"
    );
}

#[test]
fn a_session_with_nothing_chosen_says_whether_a_provider_or_a_credential_is_missing() {
    // The same credentials the welcome's sentence is chosen from decide what a
    // turn is missing, so a client is told what the reader at a screen is.
    for (key, missing) in [
        (Some("a-key"), Missing::Provider),
        (None, Missing::Credential),
    ] {
        let from = |name: &str| {
            key.filter(|_| name == "ANTHROPIC_API_KEY")
                .map(str::to_owned)
        };
        let sample = Sample::new("nothing-chosen-missing");
        let (logs, workspace) = (sample.logs(), sample.workspace());

        let conversation = assemble(&Startup {
            providers: &catalogue(),
            provider: None,
            unasked: missing.sentence(),
            model: None,
            effort: None,
            resuming: Resuming::No,
            mode: Mode::Ask,
            leaving: &crucible_builtins::Background::new(),
            services: &Services::new(),
            settings: &Settings::default(),
            sessions: &logs,
            workspace: &workspace,
            ledger: &Ledger::new(),
            revealed: &Revealed::new(),
            plan: &Plan::new(),
            asking: Arc::new(Nobody),
            hosting: &[],
            terminal: true,
            from: &from,
            stored: &StoredCredentials::default(),
            subscriptions: &Subscriptions::production(&crucible_auth::Renewals::new()),
        })
        .expect("a session with nothing chosen still starts");

        assert_eq!(conversation.missing(), Some(missing));
    }
}

/// The specification one startup resolves to, for a model of `anthropic`.
///
/// The startup is what `coding` reads its answer off, so the tests about the
/// answer build one. Everything the specification does not touch — the
/// session, the workspace, the credentials — belongs to `assemble`, which
/// these tests reach through separately.
fn specified(model: &str, effort: Option<Effort>, settings: &Settings, told: &str) -> Agent {
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
        leaving: &crucible_builtins::Background::new(),
        services: &Services::new(),
        settings,
        sessions: &logs,
        workspace: &workspace,
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        asking: Arc::new(Nobody),
        hosting: &[],
        terminal: true,
        from: &|_| None,
        stored: &StoredCredentials::default(),
        subscriptions: &Subscriptions::production(&crucible_auth::Renewals::new()),
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
            .model()
            .effort,
        Some(Effort::Xhigh)
    );

    // And nothing where nothing said, which is the field left off rather than
    // a rung this program chose on the vendor's behalf.
    assert_eq!(
        specified("claude-opus-5", None, &settings, "")
            .model()
            .effort,
        None
    );
}

#[test]
fn how_long_an_answer_may_be_is_the_model_own_limit_held_under_the_ceiling() {
    // A model this build has the limits of: its own output limit is far above
    // the ceiling, so the ceiling is what is asked for.
    let known = specified("claude-opus-5", None, &Settings::default(), "");
    assert_eq!(known.model().max_tokens, CEILING);

    // And one it has never heard of, where nothing is known and the lower
    // figure is what keeps a request from being refused outright.
    let unknown = specified("claude-from-the-future", None, &Settings::default(), "");
    assert_eq!(unknown.model().max_tokens, UNKNOWN_CEILING);
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
            leaving: &crucible_builtins::Background::new(),
            services: &Services::new(),
            settings: &Settings::default(),
            sessions: &logs,
            workspace: &workspace,
            ledger: &Ledger::new(),
            revealed: &Revealed::new(),
            plan: &Plan::new(),
            asking: Arc::new(Nobody),
            hosting: &[],
            terminal: true,
            from: &|_| Some("sk-test".to_owned()),
            stored: &StoredCredentials::default(),
            subscriptions: &Subscriptions::production(&crucible_auth::Renewals::new()),
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
fn google_serves_both_halves_from_interactions() {
    let reaching = reaching_for("google", Some("gemini-3.8-flash"));

    assert!(
        reaching.searching.is_some(),
        "Google Search is exposed with Search Suggestions"
    );
    assert!(reaching.fetching.is_some(), "URL context remains available");
}

#[test]
fn google_web_authority_is_api_key_only_and_uses_the_checked_recipient() {
    let sample = Sample::new("google-web-authority");
    let stored = sample.subscribed("google");
    let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());
    let defaults = Settings::default();
    let absent = |_: &str| None;
    let auth = ProviderAuth {
        settings: &defaults,
        from: &absent,
        stored: &stored,
        subscriptions: &subscriptions,
    };
    let http = HttpTurns::unavailable();
    let reaching = google_web(
        wiring(serving("google"), auth, &http).unwrap(),
        "gemini-3.8-flash",
    );
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
    let reaching = google_web(
        wiring(serving("google"), auth, &http).unwrap(),
        "gemini-3.8-flash",
    );
    assert_eq!(
        reaching.searching.unwrap().reaches(),
        crucible_tools::Host::Named {
            sent: "https://gateway.example/interactions?alt=sse".into(),
            host: "gateway.example".into()
        }
    );
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
            },
            &http,
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
fn each_provider_is_given_only_the_web_tools_its_vendor_serves_on_its_wire() {
    // (provider, a model it offers, search, fetch). Meta's and xAI's
    // Responses serve a search and nothing that opens one page; the five on
    // Chat Completions are served neither there.
    for (named, model, searching, fetching) in [
        ("anthropic", "claude-opus-5", true, true),
        ("google", "gemini-3.8-flash", true, true),
        ("moonshot", "k3", true, true),
        ("openai", "gpt-5.6-sol", true, true),
        ("meta", "muse-spark-1.3", true, false),
        ("xai", "grok-4.7", true, false),
        ("deepseek", "deepseek-flash", false, false),
        ("zai", "glm-5.3", false, false),
        ("qwen", "qwen3.8-max", false, false),
        ("mimo", "mimo-v2.6-pro", false, false),
        ("minimax", "MiniMax-M3", false, false),
    ] {
        let reaching = reaching_for(named, Some(model));

        assert_eq!(reaching.searching.is_some(), searching, "{named} search");
        assert_eq!(reaching.fetching.is_some(), fetching, "{named} fetch");
    }
    assert_eq!(
        crate::providers::offered(&catalogue()).count(),
        11,
        "a provider this test does not name"
    );
}

#[test]
fn meta_and_xai_search_where_the_provider_sends_its_turns() {
    for (named, model, host) in [
        ("meta", "muse-spark-1.3", "api.meta.ai"),
        ("xai", "grok-4.7", "api.x.ai"),
    ] {
        let searching = reaching_for(named, Some(model))
            .searching
            .expect("a search");
        assert_eq!(searching.name(), named);
        assert!(
            matches!(searching.reaches(), crucible_tools::Host::Named { host: reached, .. } if reached.as_ref() == host),
            "{named}"
        );
    }
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
            leaving: &crucible_builtins::Background::new(),
            services: &Services::new(),
            settings: &Settings::default(),
            sessions: &logs,
            workspace: &workspace,
            ledger: &Ledger::new(),
            revealed: &Revealed::new(),
            plan: &Plan::new(),
            asking: Arc::new(Nobody),
            hosting: &[],
            terminal,
            from: &|_| None,
            stored: &StoredCredentials::default(),
            subscriptions: &Subscriptions::production(&crucible_auth::Renewals::new()),
        },
        &Settings::default(),
        &crate::following::Following::default(),
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
            "bash_output",
            "ask_user",
            "tool_search"
        ]
    );
}

#[test]
fn a_session_carries_a_tool_for_reading_a_command_it_left_running() {
    // Advertised rather than deferred, for the reason `ask_user` is: the moment
    // a model needs this is the moment a command it left running has said
    // nothing yet, and the note it is told about it in is the same result it is
    // being asked not to poll. A tool it has to look up first is one it will
    // not look up then.
    let tools = offered(true);

    assert!(
        tools
            .advertised()
            .iter()
            .any(|schema| schema.name == "bash_output"),
        "the tool was registered without being offered"
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

/// What the published schema says `compaction.<key>` falls back to.
fn published_compaction_default(key: &str) -> Option<u64> {
    let schema: serde_json::Value =
        serde_json::from_str(&crucible_config::schema()).expect("the schema is JSON");
    schema
        .pointer(&format!("/properties/compaction/properties/{key}/default"))
        .and_then(serde_json::Value::as_u64)
}

#[test]
fn the_schema_offers_the_keep_and_recap_a_run_falls_back_to() {
    // The figures live in the runner and the schema is declared in the
    // configuration crate, so neither can own both. An editor writes the
    // schema's default into somebody's file; it has to be the one a run uses.
    let defaults = policy(&Settings::default()).compaction;

    assert_eq!(
        published_compaction_default("keep"),
        Some(defaults.keep_tokens)
    );
    assert_eq!(
        published_compaction_default("recap"),
        Some(u64::from(defaults.recap_tokens))
    );
}

#[test]
fn stable_instructions_hold_no_session_fact() {
    let said = under(&Settings::default());

    assert_eq!(said, SystemPrompt::default().instructions_text());
    assert!(said.contains("operating inside crucible"), "{said}");
    assert!(!said.contains("# This session"), "{said}");
    assert!(!said.contains("workspace root"), "{said}");
    assert!(!said.contains("Toolset generation"), "{said}");
}

#[test]
fn the_agent_is_named_coding_and_stands_under_what_the_wiring_asked() {
    // Two fields the wiring decides and a later registry needs. This pins the
    // stable operator instructions `coding` puts in the definition; the
    // assembly test below follows them through the runner and separately
    // proves the workspace fact reaches typed context.
    let asked = "read the workspace before changing it";
    let built = specified("claude-opus-5", None, &Settings::default(), asked);

    assert_eq!(built.id().as_str(), "coding");
    assert_eq!(built.instructions(), Some(asked));
}

#[test]
fn context_counts_what_settings_append_to_the_system_field_as_project_instructions() {
    // `/context` shows `systemPrompt.append` apart from crucible's own part of
    // the field. The definition is what carries the split to the runner.
    let sample = Sample::new("startup-context-appended");
    let settings = sample
        .settings(r#"{"systemPrompt":{"append":"Run the project checks before you finish."}}"#);
    let told = under(&settings);
    let appended = told
        .strip_prefix(&under(&Settings::default()))
        .expect("what is appended follows crucible's own part");

    let built = specified("claude-opus-5", None, &settings, &told);

    assert!(appended.contains("Run the project checks"), "{appended}");
    assert_eq!(built.appended(), appended.len());
}

#[test]
fn context_leaves_a_replaced_system_field_under_the_system_prompt() {
    // `systemPrompt.custom` replaces crucible's own part rather than adding to
    // it, so only what is appended after it is counted apart.
    let sample = Sample::new("startup-context-custom");
    let replaced = sample.user(r#"{"systemPrompt":{"custom":"Answer in haiku."}}"#);
    let both = sample.user(
        r#"{"systemPrompt":{"custom":"Answer in haiku.","append":"Run the project checks."}}"#,
    );
    let told = under(&both);
    let appended = told
        .strip_prefix(&under(&replaced))
        .expect("what is appended follows the replaced part");

    let built = specified("claude-opus-5", None, &both, &told);

    assert!(appended.contains("Run the project checks"), "{appended}");
    assert_eq!(built.appended(), appended.len());
}

#[test]
fn a_definition_the_wiring_had_nothing_to_say_under_is_told_nothing() {
    // The rule `Agent::telling` enforces, at the one site outside the runner
    // that writes the field: no instructions and empty instructions are two
    // different requests, and a prompt nobody wrote is the first.
    //
    // Unreachable through the shipped wiring, because `under` always
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

    let mut conversation = assemble(&Startup {
        providers: &catalogue(),
        provider: None,
        unasked: NOTHING_TO_ASK,
        model: None,
        effort: None,
        resuming: Resuming::No,
        mode: Mode::Ask,
        leaving: &crucible_builtins::Background::new(),
        services: &Services::new(),
        settings: &configured,
        sessions: &logs,
        workspace: &workspace,
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        asking: Arc::new(Nobody),
        hosting: &[],
        terminal: true,
        from: &|_| None,
        stored: &StoredCredentials::default(),
        subscriptions: &Subscriptions::production(&crucible_auth::Renewals::new()),
    })
    .expect("a session to assemble");
    let runner = &mut conversation.runner;

    let asked = runner
        .instructions()
        .expect("a turn is asked under something");
    assert_eq!(asked, under(&configured));
    assert!(!asked.contains(&workspace.root().display().to_string()));

    let (events, _seen) = std::sync::mpsc::channel();
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
    let run = runner.starting(&events, &cancel, &steer, &aside);
    let _ = runner
        .turn("probe", Box::new([]), &mut Nobody, &run)
        .awaited();
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
        leaving: &crucible_builtins::Background::new(),
        services: &Services::new(),
        settings: &Settings::default(),
        sessions: &logs,
        workspace: &workspace,
        ledger: &Ledger::new(),
        revealed: &Revealed::new(),
        plan: &Plan::new(),
        asking: Arc::new(Nobody),
        hosting: &["docs".to_owned()],
        terminal: true,
        from: &|_| None,
        stored: &StoredCredentials::default(),
        subscriptions: &Subscriptions::production(&crucible_auth::Renewals::new()),
    }) else {
        panic!("a run naming a server nothing wrote down was accepted");
    };

    assert!(matches!(problem, AppError::NoServer { .. }), "{problem:?}");
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
    let at = directory.join(crucible_builtins::program::spelled("docs-mcp"));
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
            leaving: &crucible_builtins::Background::new(),
            services: &Services::new(),
            settings: &settings,
            sessions: &logs,
            workspace: &workspace,
            ledger: &Ledger::new(),
            revealed: &Revealed::new(),
            plan: &Plan::new(),
            asking: Arc::new(Nobody),
            hosting,
            terminal: true,
            from: &|name| (name == "PATH").then(|| path.clone()),
            stored: &StoredCredentials::default(),
            subscriptions: &Subscriptions::production(&crucible_auth::Renewals::new()),
        })
        .expect("a run this test wrote the record for")
    };

    let hosted = starting(&["docs".to_owned()]);
    assert!(
        hosted.runner().offering().is_empty(),
        "a live toolset has no generation until the turn that prepares it"
    );

    let alone = starting(&[]);
    assert!(
        alone.runner().offering().contains(&"bash".to_owned()),
        "a run that named no server is the built-in roster itself: {:?}",
        alone.runner().offering()
    );
}

#[cfg(unix)]
#[test]
fn existing_user_configuration_is_private_before_settings_can_read_it() {
    use std::ffi::OsString;
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

    protected(&home).expect("the private boundary");

    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

mod conformance;
mod following;
mod lending;
mod unserved;
mod windows;

/// Where `stored` sends a Moonshot request, with `exported` in its variable
/// and `sending` configured, if anything.
fn moonshot_address(
    stored: &StoredCredentials,
    exported: Option<&str>,
    sending: Option<Endpoint>,
) -> Result<Endpoint, AppError> {
    let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());
    let from = |name: &str| {
        (name == "MOONSHOT_API_KEY")
            .then(|| exported.map(str::to_owned))
            .flatten()
    };
    credential(
        ApiAudience {
            provider: "moonshot",
            variable: "MOONSHOT_API_KEY",
            vendor: Moonshot::CODING,
        },
        sending,
        ProviderAuth {
            settings: &Settings::default(),
            from: &from,
            stored,
            subscriptions: &subscriptions,
        },
    )
    .map(|(endpoint, _)| endpoint)
}

#[test]
fn each_kimi_row_sends_its_credential_to_its_own_site_and_to_no_other() {
    for (name, signed_in, wanted) in [
        ("moonshot@kimi.ai", true, Moonshot::CODING_AI),
        ("moonshot", true, Moonshot::CODING),
        ("moonshot@kimi.ai", false, Moonshot::CODING_AI),
        ("moonshot", false, Moonshot::CODING),
    ] {
        let sample = Sample::new("kimi-row-address");
        let stored = if signed_in {
            sample.subscribed(name)
        } else {
            sample.stored(name)
        };

        let address = moonshot_address(&stored, None, None).expect("a stored credential");
        assert_eq!(address, wanted, "{name} signed in {signed_in}");
    }
}

#[test]
fn a_configured_address_takes_a_kimi_ai_key_and_never_its_sign_in() {
    let custom = Endpoint::parse("https://proxy.invalid/v1/chat/completions").unwrap();

    let sample = Sample::new("kimi-ai-key-custom");
    let stored = sample.stored("moonshot@kimi.ai");
    assert_eq!(
        moonshot_address(&stored, None, Some(custom.clone())).expect("a stored key"),
        custom
    );

    let sample = Sample::new("kimi-ai-sign-in-custom");
    let stored = sample.subscribed("moonshot@kimi.ai");
    assert!(matches!(
        moonshot_address(&stored, None, Some(custom)),
        Err(AppError::SubscriptionAddress { .. })
    ));
}

#[test]
fn a_key_from_the_variable_goes_to_its_own_row_whatever_the_store_holds() {
    // The variable's key belongs to kimi.com; a kimi.ai key in the store is
    // not sent to kimi.com, and the variable's key is not sent to kimi.ai.
    let sample = Sample::new("kimi-variable-row");
    let stored = sample.stored("moonshot@kimi.ai");

    assert_eq!(
        moonshot_address(&stored, Some("exported-key"), None).expect("an exported key"),
        Moonshot::CODING
    );
}

#[test]
fn a_store_0_43_3_wrote_signs_in_where_it_did() {
    let sample = Sample::new("store-of-0-43-3");
    let stored = sample.holding(
        r#"{"version":2,"keys":{"moonshot":"fabricated-moonshot-key","anthropic":"fabricated-anthropic-key"},"subscriptions":{"openai":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{"account_id":"test-account"},"expires_at":18446744073709551615,"refreshed_at":1}},"identities":{}}"#,
    );
    let rows = crate::providers::Rows::production();

    assert_eq!(
        rows.held("moonshot", &stored).map(|row| row.shown),
        Some("MoonshotAI · kimi.com")
    );
    assert_eq!(
        rows.held("openai", &stored).map(|row| row.list),
        Some(crate::providers::List::Subscription)
    );
    assert_eq!(
        rows.held("anthropic", &stored).map(|row| row.shown),
        Some("Anthropic")
    );
    assert_eq!(
        moonshot_address(&stored, None, None).expect("the kimi.com key"),
        Moonshot::CODING
    );
}

/// A store as 0.43.3 leaves one after a roll back: its own `moonshot` key
/// beside the kimi.ai sign-in the new release wrote.
const TWO_HELD: &str = r#"{"version":2,"keys":{"moonshot":"fabricated-moonshot-key"},"subscriptions":{"moonshot@kimi.ai":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{"device_id":"01234567-89ab-4cde-8fab-0123456789ab","expires_in":"3600"},"expires_at":18446744073709551615,"refreshed_at":1}},"identities":{}}"#;

#[test]
fn after_a_roll_back_the_credential_0_43_3_wrote_is_the_one_sent() {
    let sample = Sample::new("rolled-back");
    let stored = sample.holding(TWO_HELD);

    assert_eq!(
        moonshot_address(&stored, None, None).expect("the kimi.com key"),
        Moonshot::CODING
    );
}

#[test]
fn a_start_that_finds_two_says_which_went_in_one_line() {
    let sample = Sample::new("settle-line");
    let _ = sample.holding(TWO_HELD);
    let rows = crate::providers::Rows::production();

    let said = settle(&sample.store(), &rows, "·").expect("a line");

    assert_eq!(
        said,
        "two credentials were stored for moonshot; the Kimi Code · kimi.ai sign-in was removed, \
         and the MoonshotAI · kimi.com key is used"
    );
    assert!(!sample.store().read().has_subscription("moonshot@kimi.ai"));
    assert_eq!(settle(&sample.store(), &rows, "·"), None);
}

#[test]
fn a_start_that_cannot_remove_the_second_says_so_and_claims_nothing_went() {
    let sample = Sample::new("settle-unwritten");
    let _ = sample.holding(TWO_HELD);
    std::fs::create_dir(sample.home().join("auth.json.new")).expect("a directory this test made");
    let rows = crate::providers::Rows::production();

    let said = settle(&sample.store(), &rows, "·").expect("a line");

    assert!(
        said.starts_with(
            "two credentials are stored for moonshot; the MoonshotAI · kimi.com key is used"
        ),
        "{said}"
    );
    assert!(!said.contains("was removed"), "{said}");
    // It comes back at every start, so it says why.
    assert!(
        said.contains("stays in the store until a start can remove it: "),
        "{said}"
    );
    assert!(sample.store().read().has_subscription("moonshot@kimi.ai"));
}

#[test]
fn each_kimi_site_answers_the_web_tools_of_the_credential_sent_to_it_and_no_other() {
    let lookup = |_: &str| Some("fabricated".to_owned());
    let key = || -> Box<dyn crucible_credentials::Credential> {
        Box::new(crucible_credentials::HeaderKey::new(
            crucible_credentials::ApiKey::from_lookup("K", lookup).expect("a key"),
            crucible_credentials::Header::bearer(),
        ))
    };
    let transport =
        || -> Box<dyn crucible_provider::Transport> { Box::new(HttpTurns::unavailable()) };

    for (endpoint, site, other) in [
        (Moonshot::CODING_AI, "api.kimi.ai", "api.kimi.com"),
        (Moonshot::CODING, "api.kimi.com", "api.kimi.ai"),
    ] {
        let source = moonshot_site(&endpoint, key(), transport()).expect("that site's services");
        let said = format!("{source:?}");
        // The paths are redacted from what is printed; the provider's own
        // tests hold them. Both services stand on the site's host.
        assert_eq!(
            said.matches(&format!("https://{site}/")).count(),
            2,
            "{said}"
        );
        assert!(!said.contains(other), "{said}");
    }
    let custom = Endpoint::parse("https://proxy.invalid/v1/chat/completions").unwrap();
    assert!(moonshot_site(&custom, key(), transport()).is_none());
}

/// A kimi.ai sign-in, a kimi.com sign-in, and keys, fabricated, as a store
/// holds them.
const KIMI_AI_SIGN_IN: &str = r#"{"version":2,"keys":{},"subscriptions":{"moonshot@kimi.ai":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}}}"#;
const KIMI_COM_SIGN_IN: &str = r#"{"version":2,"keys":{},"subscriptions":{"moonshot":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}}}"#;
const KIMI_AI_KEY: &str =
    r#"{"version":2,"keys":{"moonshot@kimi.ai":"fabricated-kimi-ai-key"},"subscriptions":{}}"#;
const GOOGLE_KEY: &str =
    r#"{"version":2,"keys":{"google":"fabricated-google-key"},"subscriptions":{}}"#;
const OPENAI_SIGN_IN: &str = r#"{"version":2,"keys":{},"subscriptions":{"openai":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}}}"#;
const NOTHING: &str = r#"{"version":2,"keys":{},"subscriptions":{}}"#;

/// One state a provider's credential can be in, and what it is served on:
/// the provider, the store, whether its variable is set, the `baseUrl`, the
/// route and the configured origin.
type Case = (
    &'static str,
    &'static str,
    bool,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

/// Where each way of holding a credential sends a provider's requests, and
/// the route that is: the stored sign-in first where no address is set, a key
/// from the environment on its provider's fixed row before a stored one, and a
/// configured address answering for the route it is recognised as or none.
/// Where [`credential`] resolves the same state, the address it sends to is
/// one of the route's origins.
#[test]
fn a_provider_is_served_on_the_route_its_credential_goes_to() {
    let routes = content_use::Routes::production();
    let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());
    let cases: [Case; 13] = [
        (
            "moonshot",
            KIMI_AI_SIGN_IN,
            false,
            None,
            Some("subscription:moonshot@kimi.ai"),
            None,
        ),
        (
            "moonshot",
            KIMI_COM_SIGN_IN,
            true,
            None,
            Some("subscription:moonshot"),
            None,
        ),
        (
            "moonshot",
            KIMI_AI_KEY,
            false,
            None,
            Some("key:moonshot@kimi.ai"),
            None,
        ),
        (
            "moonshot",
            KIMI_AI_KEY,
            true,
            None,
            Some("key:moonshot"),
            None,
        ),
        (
            "moonshot",
            NOTHING,
            true,
            Some("https://api.moonshot.ai/v1"),
            Some("api.moonshot.ai"),
            Some("https://api.moonshot.ai"),
        ),
        (
            "moonshot",
            KIMI_COM_SIGN_IN,
            true,
            Some("https://gateway.example/v1"),
            None,
            Some("https://gateway.example"),
        ),
        ("google", NOTHING, true, None, Some("key:google"), None),
        ("google", GOOGLE_KEY, false, None, Some("key:google"), None),
        (
            "openai",
            OPENAI_SIGN_IN,
            true,
            None,
            Some("subscription:openai"),
            None,
        ),
        ("openai", NOTHING, true, None, Some("key:openai"), None),
        (
            "anthropic",
            NOTHING,
            true,
            None,
            Some("key:anthropic"),
            None,
        ),
        ("moonshot", NOTHING, false, None, None, None),
        (
            "moonshot",
            KIMI_COM_SIGN_IN,
            false,
            Some("https://gateway.example/v1"),
            None,
            None,
        ),
    ];
    for (at, (named, held, exported, base, route, origin)) in cases.into_iter().enumerate() {
        let sample = Sample::new(&format!("served-on-{at}"));
        let stored = sample.holding(held);
        let settings = match base {
            Some(base) => sample.user(&format!(
                r#"{{"providers": {{"{named}": {{"baseUrl": "{base}"}}}}}}"#
            )),
            None => Settings::default(),
        };
        let from = move |_: &str| exported.then(|| "fabricated-exported-key".to_owned());
        let auth = ProviderAuth {
            settings: &settings,
            from: &from,
            stored: &stored,
            subscriptions: &subscriptions,
        };

        let serving = served_on(named, "VARIABLE", auth);
        let case = format!("case {at}: {named} {held} {exported} {base:?}");
        if route.is_none() && origin.is_none() {
            assert_eq!(serving, None, "{case}");
            continue;
        }
        let serving = serving.unwrap_or_else(|| panic!("{case}: served on nothing"));
        assert_eq!(serving.route.as_deref(), route, "{case}");
        assert_eq!(
            serving.at.as_ref().map(ToString::to_string).as_deref(),
            origin,
            "{case}"
        );

        let vendor = match named {
            "moonshot" => Moonshot::CODING,
            "openai" => OpenAi::VENDOR,
            _ => continue,
        };
        let sending = base.map(|base| Endpoint::parse(base).unwrap());
        let (endpoint, _) = credential(
            ApiAudience {
                provider: named,
                variable: "VARIABLE",
                vendor,
            },
            sending,
            auth,
        )
        .unwrap_or_else(|_| panic!("{case}: no credential"));
        let sent = crucible_http::Origin::of(endpoint.as_str()).unwrap();
        let reached = serving.at.as_ref() == Some(&sent)
            || serving
                .route
                .as_deref()
                .and_then(|route| routes.warned(route))
                .is_none_or(|warned| warned.origins.contains(&sent.to_string().as_str()));
        assert!(reached, "{case}: {sent} is not where {serving:?} goes");
    }
}

/// A start reads the yes from the user's file and what each provider is
/// served on, before anything is sent: a key from the environment on a warned
/// route holds that route's origin until its yes, and a yes in the file lets
/// it through.
#[test]
fn a_start_holds_a_warned_route_until_the_users_file_says_yes() {
    use crucible_http::Hold as _;

    let google = crucible_http::Origin::of(Google::VENDOR.as_str()).unwrap();
    for (tree, document, held) in [
        ("unsaid", "{}", Some("key:google")),
        (
            "said",
            r#"{"contentUse": {"accepted": ["key:google"]}}"#,
            None,
        ),
    ] {
        let sample = Sample::new(&format!("start-consent-{tree}"));
        let (logs, workspace) = (sample.logs(), sample.workspace());
        let services = Services::new();
        let settings = sample.user(document);

        assemble(&Startup {
            providers: &catalogue(),
            provider: Some(serving("google")),
            unasked: NOTHING_TO_ASK,
            model: Some("gemini-3.8-flash"),
            effort: None,
            resuming: Resuming::No,
            mode: Mode::Ask,
            leaving: &crucible_builtins::Background::new(),
            services: &services,
            settings: &settings,
            sessions: &logs,
            workspace: &workspace,
            ledger: &Ledger::new(),
            revealed: &Revealed::new(),
            plan: &Plan::new(),
            asking: Arc::new(Nobody),
            hosting: &[],
            terminal: true,
            from: &|_| Some("fabricated-exported-key".to_owned()),
            stored: &StoredCredentials::default(),
            subscriptions: &Subscriptions::production(&crucible_auth::Renewals::new()),
        })
        .expect("a start with a key");

        assert_eq!(services.consent().held(&google).as_deref(), held, "{tree}");
        // A model address shared by two rows follows the one served.
        let kimi = crucible_http::Origin::of(Moonshot::CODING.as_str()).unwrap();
        assert_eq!(
            services.consent().held(&kimi).as_deref(),
            Some("key:moonshot"),
            "{tree}"
        );
    }
}

#[test]
fn a_run_starts_at_the_speed_the_users_file_keeps_for_the_model_in_force() {
    // A restart holds a chosen speed: the file is read as the run starts.
    // A speed kept for another model than the one a run starts on was never
    // shown at that model's price, and is not its speed.
    for (kept, asked) in [
        (Some("gpt-5.6-sol"), crucible_models::Speed::Fast),
        (Some("gpt-5.5"), crucible_models::Speed::Standard),
        (None, crucible_models::Speed::Standard),
    ] {
        let sample = Sample::new(&format!("start-fast-{}", kept.unwrap_or("none")));
        let (logs, workspace) = (sample.logs(), sample.workspace());
        let file = sample.user_file();
        if let Some(model) = kept {
            crate::remember::hastening(&file, "openai", model).expect("a speed written down");
        }
        let services = Services::new();
        services.consent().keeps_in(file);

        let conversation = assemble(&Startup {
            providers: &catalogue(),
            provider: Some(serving("openai")),
            unasked: NO_MODEL_CHOSEN,
            model: Some("gpt-5.6-sol"),
            effort: None,
            resuming: Resuming::No,
            mode: Mode::Ask,
            leaving: &crucible_builtins::Background::new(),
            services: &services,
            settings: &Settings::default(),
            sessions: &logs,
            workspace: &workspace,
            ledger: &Ledger::new(),
            revealed: &Revealed::new(),
            plan: &Plan::new(),
            asking: Arc::new(Nobody),
            hosting: &[],
            terminal: true,
            from: &|_| Some("sk-test".to_owned()),
            stored: &StoredCredentials::default(),
            subscriptions: &Subscriptions::production(&crucible_auth::Renewals::new()),
        })
        .expect("a session over a key starts");

        assert_eq!(conversation.runner().speed(), asked, "kept {kept:?}");
    }
}

/// A web search on a model whose own route is warned waits for that route's
/// yes as a turn on it does.
#[test]
fn a_search_on_a_warned_model_sends_nothing_until_its_yes() {
    let (url, heard) = crate::sample::recording();
    let sample = Sample::new("web-source-warned-model");
    let (logs, workspace) = (sample.logs(), sample.workspace());
    let settings = sample.user(&format!(
        r#"{{"providers": {{"meta": {{"baseUrl": "{url}"}}}}}}"#
    ));
    let services = Services::new();
    let model = "muse-spark-1.3-contributor";

    let reaching = web(
        &Startup {
            providers: &catalogue(),
            provider: Some(serving("meta")),
            unasked: NO_MODEL_CHOSEN,
            model: Some(model),
            effort: None,
            resuming: Resuming::No,
            mode: Mode::Ask,
            leaving: &crucible_builtins::Background::new(),
            services: &services,
            settings: &settings,
            sessions: &logs,
            workspace: &workspace,
            ledger: &Ledger::new(),
            revealed: &Revealed::new(),
            plan: &Plan::new(),
            asking: Arc::new(Nobody),
            hosting: &[],
            terminal: true,
            from: &|_| Some("fabricated-meta-key".to_owned()),
            stored: &StoredCredentials::default(),
            subscriptions: &Subscriptions::production(&crucible_auth::Renewals::new()),
        },
        &settings,
    );
    let searching = reaching.searching.expect("Meta searches");
    let runtime = services.runtime().handle().unwrap();
    let search =
        || runtime.block_on(searching.search("what the user is working on", &Cancel::new()));

    let held = search().err().map(|problem| problem.to_string());
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(
        heard.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "{held:?}"
    );
    let route = content_use::model_route("meta", model);
    assert!(held.is_some_and(|said| said.contains(&route)));

    services.consent().record(&route);
    let _ = search();
    assert!(heard.load(std::sync::atomic::Ordering::SeqCst) > 0);
    drop(services);
}

/// A `MiniMax` key asks after its plan's limits only where it was given on a
/// Token Plan row: a pay-as-you-go key, stored or exported, has no plan to ask
/// after, and the key itself never says which it is.
#[test]
fn only_a_minimax_token_plan_key_asks_after_its_limits() {
    let subscriptions = Subscriptions::production(&crucible_auth::Renewals::new());
    let settings = Settings::default();
    let http = HttpTurns::unavailable();
    let cases = [
        ("minimax@token-plan.minimax.io", false, true),
        ("minimax@token-plan.minimaxi.com", false, true),
        ("minimax@minimax.io", false, false),
        ("minimax@minimaxi.com", false, false),
        // The variable answers before the store, on the pay-as-you-go row.
        ("minimax@token-plan.minimax.io", true, false),
    ];
    for (at, (stored, exported, asks)) in cases.into_iter().enumerate() {
        let sample = Sample::new(&format!("minimax-plan-{at}"));
        let stored = sample.holding(&format!(
            r#"{{"version":2,"keys":{{"{stored}":"sk-cp-fabricated-key"}},"subscriptions":{{}}}}"#
        ));
        let from = move |_: &str| exported.then(|| "fabricated-exported-key".to_owned());
        let auth = ProviderAuth {
            settings: &settings,
            from: &from,
            stored: &stored,
            subscriptions: &subscriptions,
        };

        let provider = provider(Some(serving("minimax")), NOTHING_TO_ASK, auth, &http)
            .unwrap_or_else(|error| panic!("case {at}: {error}"));

        assert_eq!(provider.ask_limits().is_some(), asks, "case {at}");
    }
}
