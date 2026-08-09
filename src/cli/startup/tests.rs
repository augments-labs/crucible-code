//! Which key is read, and what a startup that fails leaves behind.

use std::cell::RefCell;

use super::*;
use crate::cli::sample::Sample;

#[test]
fn each_provider_reads_the_key_belonging_to_it() {
    // The pairing is the whole of this function, and both arms build whichever
    // way round they are wired: swapping the two bodies would send one vendor's
    // key to the other vendor's endpoint with everything else still green.
    let read = RefCell::new(Vec::new());
    let from = |name: &str| {
        read.borrow_mut().push(name.to_owned());
        Some("a-key".to_owned())
    };
    let nothing = Settings::default();

    let anthropic = provider("anthropic", &nothing, &from).expect("a provider");
    let openai = provider("openai", &nothing, &from).expect("a provider");

    assert_eq!(anthropic.name(), "anthropic");
    assert_eq!(openai.name(), "openai");
    assert_eq!(read.into_inner(), [ANTHROPIC_KEY, OPENAI_KEY]);
}

#[test]
fn a_provider_reads_the_variable_its_configuration_names() {
    // Somebody with a work key and a personal key has two variables, and only
    // one of them can be the vendor's usual name.
    let sample = Sample::new("key-variable");
    let settings =
        sample.settings(r#"{"providers": {"anthropic": {"apiKeyEnv": "WORK_ANTHROPIC_KEY"}}}"#);

    let read = RefCell::new(Vec::new());
    provider("anthropic", &settings, &|name: &str| {
        read.borrow_mut().push(name.to_owned());
        Some("a-key".to_owned())
    })
    .expect("a provider");

    // That name and no other. Reading the usual variable as well would pick up
    // a key the user pointed crucible away from.
    assert_eq!(read.into_inner(), ["WORK_ANTHROPIC_KEY"]);
}

#[test]
fn a_missing_key_names_the_variable_to_set_and_not_its_value() {
    // The name is configuration; the value is the secret. Only one of them is
    // allowed to reach a terminal.
    let problem = provider("openai", &Settings::default(), &|_| None).expect_err("no key was set");

    assert_eq!(problem.to_string(), "OPENAI_API_KEY is not set");
}

#[test]
fn a_provider_this_build_does_not_serve_reads_no_key_at_all() {
    // Reading one first would report a missing key for a provider that could
    // not have been used even with the key in place.
    let read = RefCell::new(Vec::new());

    let problem = provider("ollama", &Settings::default(), &|name: &str| {
        read.borrow_mut().push(name.to_owned());
        Some("a-key".to_owned())
    })
    .expect_err("this build has no such provider");

    assert!(problem.to_string().contains("ollama"), "{problem}");
    assert!(read.into_inner().is_empty(), "a key was read anyway");
}

#[test]
fn a_startup_that_cannot_reach_a_provider_leaves_no_session_behind() {
    // An empty session written for a run that never happened is then the
    // newest one for this directory, so --continue would offer it instead
    // of the last real session.
    let sample = Sample::new("no-provider");
    let (logs, workspace) = (sample.logs(), sample.workspace());

    let Err(problem) = assemble(&Startup {
        provider: "nowhere",
        model: "gpt-5.2",
        resuming: false,
        mode: Mode::Ask,
        settings: &Settings::default(),
        sessions: &logs,
        workspace: &workspace,
        cancel: &Cancel::new(),
        from: &|_| Some("a-key".to_owned()),
    }) else {
        panic!("a provider this build does not have was accepted");
    };

    assert!(matches!(problem, Fatal::Provider { .. }), "{problem:?}");
    assert!(
        !logs.exists(),
        "a session was written for a startup that failed"
    );
}

#[test]
fn a_startup_with_nothing_to_authenticate_with_leaves_no_session_behind() {
    // The same invariant, through the other way a startup fails: an unserved
    // provider is caught by a match, a missing key by a lookup, and the two
    // return from different places.
    let sample = Sample::new("no-key");
    let (logs, workspace) = (sample.logs(), sample.workspace());

    // A provider this build does serve, so the only thing left to fail is the
    // key — and the lookup says there is none regardless of what the shell
    // running this test happens to export.
    let Err(problem) = assemble(&Startup {
        provider: "openai",
        model: "gpt-5.2",
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
