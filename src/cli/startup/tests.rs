//! Which key is read, and what a startup that fails leaves behind.

use std::cell::RefCell;

use super::*;
use crate::cli::sample::Sample;

/// The entry the wiring resolves before it builds anything.
fn serving(named: &str) -> Served {
    served(named).expect("a provider this build has")
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
        let built = provider(Some(serving(one.name)), &nothing, &from).expect("a provider");

        assert_eq!(built.name(), one.name);
    }

    assert_eq!(read.into_inner(), PROVIDERS.map(|one| one.key));
}

#[test]
fn a_provider_reads_the_variable_its_configuration_names() {
    // Somebody with a work key and a personal key has two variables, and only
    // one of them can be the vendor's usual name.
    let sample = Sample::new("key-variable");
    let settings =
        sample.settings(r#"{"providers": {"anthropic": {"apiKeyEnv": "WORK_ANTHROPIC_KEY"}}}"#);

    let read = RefCell::new(Vec::new());
    provider(Some(serving("anthropic")), &settings, &|name: &str| {
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
        sample.local(r#"{"providers": {"anthropic": {"baseUrl": "https://gateway.example/v1"}}}"#);

    let built = provider(Some(serving("anthropic")), &settings, &|_| {
        Some("a-key".to_owned())
    })
    .expect("a provider pointed at a gateway");

    assert_eq!(built.name(), "anthropic");
}

#[test]
fn an_address_that_would_put_the_key_on_the_wire_stops_the_run() {
    // Not a warning that carries on at the vendor's address: somebody who set
    // this has a reason not to reach the vendor, and going there anyway would
    // send the key somewhere they did not ask for.
    let sample = Sample::new("base-url-insecure");
    let settings =
        sample.local(r#"{"providers": {"anthropic": {"baseUrl": "http://gateway.example"}}}"#);

    let problem = provider(Some(serving("anthropic")), &settings, &|_| {
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
    let problem = provider(Some(serving("openai")), &Settings::default(), &|_| None)
        .expect_err("no key was set");

    assert_eq!(problem.to_string(), "OPENAI_API_KEY is not set");
}

#[test]
fn a_machine_with_no_key_at_all_gets_the_provider_that_answers_nothing() {
    // Not a refusal to start. The session is the place the key gets set up, and
    // ending the process takes away the screen that is done on.
    let read = RefCell::new(Vec::new());

    let nowhere = provider(None, &Settings::default(), &|name: &str| {
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
    let unarmed = Served {
        name: "ollama",
        key: "OLLAMA_API_KEY",
    };

    let problem = provider(Some(unarmed), &Settings::default(), &|_| {
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
        provider(Some(one), &Settings::default(), &|_| {
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
        model: Some("gpt-5.6-terra"),
        resuming: false,
        mode: Mode::Ask,
        settings: &Settings::default(),
        sessions: &logs,
        workspace: &workspace,
        cancel: &Cancel::new(),
        from: &|_| None,
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
        model: None,
        resuming: false,
        mode: Mode::Ask,
        settings: &Settings::default(),
        sessions: &logs,
        workspace: &workspace,
        cancel: &Cancel::new(),
        from: &|_| None,
    })
    .expect("a session with nothing set up still starts");

    assert_eq!(runner.model(), "", "an unnamed model is the empty name");
}
