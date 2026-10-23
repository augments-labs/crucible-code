//! A send on a route whose vendor says it uses what is sent, in a run with
//! no terminal to put the question on.

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crucible_app::content_use::{Consent, LAUNCHING, Routes, Serving, WARNED, agent_route};
use crucible_runner::Tools;
use crucible_session::Session;
use crucible_tui::{Recording, Renderer};

use crate::cli::Fatal;
use crate::cli::converse::{First, Terms, converse};
use crate::cli::fake::Script;

use crate::cli::converse::warning::unanswered;

use super::{opening, paired, plain, saying, scripted};

/// Consent that serves the fake provider on the Google key row's route.
fn serving_google() -> Consent {
    let consent = Consent::new(Routes::production());
    consent.served(
        "anthropic",
        Some(Serving {
            route: Some("key:google".to_owned()),
            at: None,
        }),
    );
    consent
}

/// What typing `typed` down a pipe came to, and how many requests the
/// provider was asked.
fn piped(consent: Consent, typed: &str) -> (Result<(), Fatal>, usize, String) {
    let terms = Terms { consent, ..plain() };
    let script = Script::new(vec![saying("answered")]);
    let asked = script.asked();
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        scripted(script, Tools::new(), session)
    });
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(typed.as_bytes().to_vec());

    let ended = converse(
        conversation,
        &mut renderer,
        &terms,
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .map(|_| ());
    let written = renderer.terminal().written().to_string();
    (ended, asked.load(Ordering::SeqCst), written)
}

#[test]
fn down_a_pipe_a_warned_route_with_no_yes_ends_the_run_and_sends_nothing() {
    let (ended, asked, _) = piped(serving_google(), "hello\n");

    assert_eq!(asked, 0, "a request was sent");
    let Err(Fatal::Unanswered(said)) = ended else {
        panic!("the run went on: {ended:?}");
    };
    let google = WARNED
        .iter()
        .find(|warned| warned.route == "key:google")
        .expect("the Google key row is warned");
    assert!(said.contains(google.warning.sentence), "{said}");
    assert!(said.ends_with("answer it once in a terminal."), "{said}");
    assert!(
        crate::cli::Fatal::Unanswered(said.clone())
            .to_string()
            .contains("Nothing was sent")
    );
}

#[test]
fn a_route_already_said_yes_to_is_sent_on_without_a_question() {
    let consent = serving_google();
    consent.recorded(["key:google".to_owned()]);

    let (ended, asked, written) = piped(consent, "hello\n");

    assert!(ended.is_ok(), "{ended:?}");
    assert_eq!(asked, 1);
    assert!(written.contains("answered"), "{written}");
}

#[test]
fn a_route_its_vendor_says_nothing_about_is_never_asked_about() {
    let consent = Consent::new(Routes::production());
    consent.served(
        "anthropic",
        Some(Serving {
            route: Some("key:anthropic".to_owned()),
            at: None,
        }),
    );

    let (ended, asked, _) = piped(consent, "hello\n");

    assert!(ended.is_ok(), "{ended:?}");
    assert_eq!(asked, 1);
}

#[test]
fn compact_with_no_model_to_ask_is_answered_before_the_route_is_asked_about() {
    // Room is made by asking the model for a recap, so with no model chosen
    // there is nothing to send and no vendor to ask about: the answer is the
    // warning a prompt gets, not the question about what the vendor keeps.
    let terms = Terms {
        consent: serving_google(),
        ..plain()
    };
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        crucible_runner::Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            crucible_runner::Agent::new(
                crucible_types::AgentId::new("test"),
                crucible_runner::Model {
                    name: String::new().into(),
                    max_tokens: 64,
                    window: None,
                    accepts: None,
                    effort: None,
                },
            ),
            crucible_context::ContextInputs::new(std::env::temp_dir()),
            session,
        )
    });
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"/compact\n".to_vec());

    let ended = converse(
        conversation,
        &mut renderer,
        &terms,
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    );

    assert!(ended.is_ok(), "{ended:?}");
    let written = renderer.terminal().written();
    assert!(written.contains("No model selected"), "{written}");
}

#[test]
fn an_installed_agent_with_no_yes_is_unanswered_with_what_starting_it_runs() {
    // The question shown on a terminal says what starting the agent runs
    // after the vendor's sentence; a run with no terminal to ask on says it
    // in the same place, and still ends on where to answer.
    let route = agent_route("claude");
    let claude = WARNED
        .iter()
        .find(|warned| warned.route == route)
        .expect("Claude Code's route is warned");

    let said = unanswered(claude);

    let launched = format!("{} {LAUNCHING}", claude.warning.sentence);
    assert!(said.contains(&launched), "{said}");
    assert!(said.ends_with("answer it once in a terminal."), "{said}");
}

#[test]
fn a_route_that_launches_nothing_is_unanswered_without_that_sentence() {
    let google = WARNED
        .iter()
        .find(|warned| warned.route == "key:google")
        .expect("the Google key row is warned");

    assert!(!unanswered(google).contains(LAUNCHING));
}

#[test]
fn a_missing_agent_program_is_refused_before_the_warning_is_put() {
    // A prompt on an installed agent's route, whose turn would start an agent
    // whose program is nowhere, is refused with what the launch would say,
    // drawn as a failed turn is, rather than put to anybody first: nobody is
    // asked to agree to a launch that cannot happen, and the route is left
    // unanswered. Down a pipe, where the question ends the run, the run goes
    // on past it.
    use crucible_app::access::Route;
    use crucible_app::backends::REGISTRY;
    use crucible_app::launching::{Agent, Controls, Launcher, Runs};
    use crucible_app::outside::Installed;
    use crucible_config::Driver;
    use crucible_sandbox::{SandboxEnablement, SandboxPolicy, SandboxService};
    use crucible_sandbox_local::LocalSandbox;
    use crucible_session::RouteBinding;
    use crucible_types::Transcript;
    use crucible_workspace::Workspace;

    let consent = Consent::new(Routes::production());
    let route = Route::External {
        provider: "anthropic",
        instance: "work".into(),
        driver: Driver::Claude,
        executable: std::env::temp_dir()
            .join("crucible-no-such-claude-program")
            .to_string_lossy()
            .into(),
        profile: None,
        model: None,
        effort: None,
        enabled: true,
    };
    let agent = Agent::routed(&route, &REGISTRY, |_| None).expect("Claude Code's route");
    let reason = agent
        .program(&consent)
        .err()
        .expect("the program is not there")
        .to_string();
    let runtime = crate::cli::fake::runtime();
    let workspace = Workspace::open(std::env::temp_dir()).expect("a temporary directory");
    let policy = SandboxPolicy::standard(&workspace)
        .expect("a policy")
        .with_enabled(false);
    let sandbox: Arc<dyn SandboxService> = Arc::new(LocalSandbox::new());
    let runs = Runs::new(
        Launcher::new(sandbox, runtime.clone(), &policy, &consent),
        Arc::new(SandboxEnablement::new(false, false)),
        agent,
        "work",
        &Controls::default(),
    );
    let script = Script::new(vec![saying("answered")]);
    let asked = script.asked();
    let mut conversation = paired(Arc::new(Session::nowhere()), |session| {
        scripted(script, Tools::new(), session)
    })
    .consenting(consent.clone());
    let installed = Installed {
        backend: None,
        relaunch: Box::new(runs),
        route: RouteBinding::new("claude", "work", None).expect("a route"),
    };
    drop(
        runtime
            .block_on(conversation.external(
                Arc::new(Session::nowhere()),
                Transcript::new(),
                installed,
            ))
            .expect("moved onto the agent's route"),
    );
    let terms = Terms {
        consent: consent.clone(),
        ..plain()
    };
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"hello\n".to_vec());

    let ended = converse(
        conversation,
        &mut renderer,
        &terms,
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .map(|_| ());

    assert!(ended.is_ok(), "the route was put first: {ended:?}");
    let written = renderer.terminal().written().to_string();
    let flat = written.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains(&reason), "{written}");
    assert!(
        consent.asks(&agent_route("claude")).is_some(),
        "the route was answered"
    );
    assert_eq!(asked.load(Ordering::SeqCst), 0, "a request was sent");
}
