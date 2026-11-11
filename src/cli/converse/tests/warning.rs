//! A send on a route whose vendor says it uses what is sent, in a run with
//! no terminal to put the question on.

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crucible_app::catalog_fetch::trust_in;
use crucible_app::content_use::{Consent, LAUNCHING, Routes, Serving, WARNED, agent_route};
use crucible_runner::Tools;
use crucible_session::Session;
use crucible_tui::{Recording, Renderer};

use crate::cli::Fatal;
use crate::cli::converse::{First, Terms, converse};
use crate::cli::fake::Script;

use crate::cli::converse::answering::Answers;
use crate::cli::converse::docking::Submission;
use crate::cli::converse::warning::{CRAMPED_ROOM, CRAMPED_SEND, UNSENT, unanswered};
use crate::cli::converse::{Held, Work, ran, refreshed};

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
    assert!(said.contains(google.warning.sentence.as_ref()), "{said}");
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
    use crucible_config::{Driver, Serves};
    use crucible_sandbox::{SandboxEnablement, SandboxPolicy, SandboxService};
    use crucible_sandbox_local::LocalSandbox;
    use crucible_session::RouteBinding;
    use crucible_types::Transcript;
    use crucible_workspace::Workspace;

    let consent = Consent::new(Routes::production());
    let route = Route::External {
        serves: Serves::Provider("anthropic"),
        instance: "work".into(),
        driver: Driver::Claude,
        executable: std::env::temp_dir()
            .join("crucible-no-such-claude-program")
            .to_string_lossy()
            .into(),
        profile: None,
        model: None,
        effort: None,
        location: None,
        enabled: true,
    };
    let agent = Agent::routed(&route, &REGISTRY, |_| None).expect("Claude Code's route");
    let state = std::env::temp_dir().join("crucible-no-such-state");
    let reason = agent
        .program(&consent, &trust_in(&state))
        .expect_err("the program is not there")
        .to_string();
    let runtime = crate::cli::fake::runtime();
    let workspace = Workspace::open(std::env::temp_dir()).expect("a temporary directory");
    let policy = SandboxPolicy::standard(&workspace)
        .expect("a policy")
        .with_enabled(false);
    let sandbox: Arc<dyn SandboxService> = Arc::new(LocalSandbox::new());
    let runs = Runs::new(
        Launcher::new(
            sandbox,
            runtime.clone(),
            &policy,
            &consent,
            &state,
            &std::env::temp_dir().join("crucible-no-such-sessions"),
        ),
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
    // Wide enough that the sentence is drawn on one row.
    let mut renderer = Renderer::new(Recording::new(240, 24));
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
    assert!(
        written.contains(&reason),
        "{reason:?} is not in {written:?}"
    );
    assert!(
        consent.asks(&agent_route("claude")).is_some(),
        "the route was answered"
    );
    assert_eq!(asked.load(Ordering::SeqCst), 0, "a request was sent");
}

/// Nobody at a keyboard: a question the model waits on goes unanswered.
struct Unseen;

impl crucible_tools::Put for Unseen {
    fn put<'a>(
        &'a self,
        _questions: &'a [crucible_types::Question],
    ) -> crucible_runtime::BoxFuture<'a, Option<Vec<crucible_types::Answered>>> {
        Box::pin(async { None })
    }
}

/// The model asking which language without waiting for the answer.
fn deferring() -> Vec<crucible_models::Delta> {
    vec![
        crucible_models::Delta::ToolStarted {
            id: crucible_types::ToolId::new("q"),
            name: "ask_user".into(),
        },
        crucible_models::Delta::ToolArgs(
            r#"{"mode":"deferred","questions":[{"heading":"Language","question":"Which language?","answers":[{"answer":"Rust"},{"answer":"Python"}]}]}"#.into(),
        ),
        crucible_models::Delta::Stopped(crucible_types::StopReason::WantsTools),
    ]
}

/// The dock's answer to what waits in `questions`: Rust, for its one
/// question.
fn rust(questions: &crucible_app::client::Questions) -> Submission {
    let (era, batches) = questions.waiting();
    let id = batches.first().expect("a batch waits").id();
    let said = |text: &str| crucible_client_api::Said::new(text).expect("short enough");
    Submission {
        era,
        id,
        replies: vec![crucible_client_api::Replied::Answered(
            crucible_client_api::Picked {
                chosen: vec![said("Rust")],
                note: said(""),
            },
        )],
    }
}

/// What a dock answer on the route `consent` serves came to, once a turn on
/// a route with nothing to ask about left a batch waiting: what `ran`
/// returned, how many requests the provider was asked in all, whether the
/// batch still waits, what the terminal was told and what the open dock
/// shows. `keys` is whether the loop reads keys, in a window too short for
/// the question's panel to be drawn in.
fn answered_on(consent: Consent, keys: bool) -> (Result<bool, Fatal>, usize, bool, String, String) {
    let questions = crucible_app::client::Questions::new();
    let mut offered = Tools::new();
    offered
        .add_builtin(questions.asking_for_proof(crucible_builtins::AskUser::new(Arc::new(Unseen))))
        .expect("the question tool");
    let script = Script::new(vec![deferring(), saying("Asked."), saying("answered")]);
    let asked = script.asked();
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        scripted(script, offered, session)
    })
    .holding_for_proof(questions.clone())
    .expect("the handle");
    let unwarned = plain();
    let opening = opening();
    let mut input = Cursor::new(Vec::new());
    let mut held = Held::new(
        unwarned.plan.clone(),
        unwarned.sending.get(),
        Answers {
            input: &mut input,
            keys: false,
        },
        &opening,
    );
    held.questions = Some(questions.clone());
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let (conversation, _) = ran(
        conversation,
        &mut renderer,
        &unwarned,
        Work::Turn("go".to_owned(), Box::default()),
        &mut held,
    )
    .expect("the turn ran");
    assert_eq!(asked.load(Ordering::SeqCst), 2, "the asking turn");
    let submission = rust(&questions);
    let id = submission.id;
    refreshed(&mut held);
    held.answers.keys = keys;

    let warned = Terms { consent, ..plain() };
    // Redirected, so nothing polls the process's own input for a key.
    let mut cramped = Renderer::new(Recording::redirected(80, 3));
    let ended = ran(
        conversation,
        &mut cramped,
        &warned,
        Work::Answer(submission),
        &mut held,
    )
    .map(|(_, leaving)| leaving);

    let waits = questions.waiting().1.iter().any(|batch| batch.id() == id);
    let _ = held.docking.toggled();
    let shown = held
        .docking
        .rows(80, 40, crucible_tui::Glyphs::default())
        .0
        .iter()
        .map(crucible_tui::Row::text)
        .collect::<Vec<_>>()
        .join("\n");
    (
        ended,
        asked.load(Ordering::SeqCst),
        waits,
        cramped.terminal().written().to_string(),
        shown,
    )
}

#[test]
fn down_a_pipe_an_answer_on_a_warned_route_with_no_yes_ends_the_run_and_sends_nothing() {
    let (ended, asked, waits, _, _) = answered_on(serving_google(), false);

    let Err(Fatal::Unanswered(said)) = ended else {
        panic!("the answer was not held: {ended:?}");
    };
    assert!(said.ends_with("answer it once in a terminal."), "{said}");
    assert_eq!(asked, 2, "the answer was sent");
    assert!(waits, "the batch was settled");
}

#[test]
fn an_answer_held_back_at_a_terminal_stays_in_the_dock_refused() {
    // The window is too short for the warning panel to stand, which ends it
    // as `Cramped` without a key being read: what the reader is told is the
    // dock's wording, not the prompt box's, and the dock leaves its
    // submission for a refusal it can be retried from.
    let (ended, asked, waits, written, shown) = answered_on(serving_google(), true);

    assert!(matches!(ended, Ok(false)), "{ended:?}");
    assert_eq!(asked, 2, "the answer was sent");
    assert!(waits, "the batch was settled");
    assert!(written.contains(CRAMPED_ROOM), "{written}");
    assert!(!written.contains(CRAMPED_SEND), "{written}");
    assert!(shown.contains(UNSENT), "{shown}");
    assert!(shown.contains("Retry submission"), "{shown}");
}

#[test]
fn an_answer_on_a_route_already_said_yes_to_is_sent_without_a_question() {
    let consent = serving_google();
    consent.recorded(["key:google".to_owned()]);

    let (ended, asked, waits, written, _) = answered_on(consent, false);

    assert!(matches!(ended, Ok(false)), "{ended:?}");
    assert_eq!(asked, 3, "the answer was not sent");
    assert!(!waits, "the batch still waits");
    assert!(!written.contains(UNSENT), "{written}");
}
