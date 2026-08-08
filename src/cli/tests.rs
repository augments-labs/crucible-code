//! What the wiring does before the loop starts.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

use super::*;

/// A sessions directory and a workspace, both real, both temporary.
struct Sample {
    base: PathBuf,
}

impl Sample {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("crucible-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("work")).expect("a temporary directory");

        Self { base }
    }

    /// Where sessions would go. Deliberately not created: whether a startup
    /// makes it is the thing being watched.
    fn logs(&self) -> PathBuf {
        self.base.join("logs")
    }

    fn workspace(&self) -> Workspace {
        Workspace::open(self.base.join("work")).expect("the directory exists")
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn asking(model: &str) -> Cli {
    Cli {
        r#continue: false,
        model: model.to_owned(),
    }
}

fn choice(model: &str) -> Choice {
    Choice::parse(model).expect("a provider and a model")
}

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

    let anthropic = provider(&choice("anthropic/claude-sonnet-4-5"), &from).expect("a provider");
    let openai = provider(&choice("openai/gpt-5.2"), &from).expect("a provider");

    assert_eq!(anthropic.name(), "anthropic");
    assert_eq!(openai.name(), "openai");
    assert_eq!(read.into_inner(), [ANTHROPIC_KEY, OPENAI_KEY]);
}

#[test]
fn a_missing_key_names_the_variable_to_set_and_not_its_value() {
    // The name is configuration; the value is the secret. Only one of them is
    // allowed to reach a terminal.
    let problem = provider(&choice("openai/gpt-5.2"), &|_| None).expect_err("no key was set");

    assert_eq!(problem.to_string(), "OPENAI_API_KEY is not set");
}

#[test]
fn a_provider_this_build_does_not_serve_reads_no_key_at_all() {
    // Reading one first would report a missing key for a provider that could
    // not have been used even with the key in place.
    let read = RefCell::new(Vec::new());

    let problem = provider(&choice("ollama/llama-4"), &|name: &str| {
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

    let Err(problem) = assemble(
        &asking("nowhere/gpt-5.2"),
        &sample.logs(),
        &sample.workspace(),
        &Cancel::new(),
        &|_| Some("a-key".to_owned()),
    ) else {
        panic!("a provider this build does not have was accepted");
    };

    assert!(matches!(problem, Fatal::Provider { .. }), "{problem:?}");
    assert!(
        !sample.logs().exists(),
        "a session was written for a startup that failed"
    );
}

#[test]
fn a_startup_with_nothing_to_authenticate_with_leaves_no_session_behind() {
    // The same invariant, through the other way a startup fails: an unserved
    // provider is caught by a match, a missing key by a lookup, and the two
    // return from different places.
    let sample = Sample::new("no-key");

    // A provider this build does serve, so the only thing left to fail is the
    // key — and the lookup says there is none regardless of what the shell
    // running this test happens to export.
    let Err(problem) = assemble(
        &asking("openai/gpt-5.2"),
        &sample.logs(),
        &sample.workspace(),
        &Cancel::new(),
        &|_| None,
    ) else {
        panic!("a startup with no key was accepted");
    };

    assert!(matches!(problem, Fatal::Credential(_)), "{problem:?}");
    assert!(
        !sample.logs().exists(),
        "a session was written for a startup that failed"
    );
}
