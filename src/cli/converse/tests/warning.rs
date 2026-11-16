//! A send on a route whose vendor says it uses what is sent, in a run with
//! no terminal to put the question on.

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crucible_app::catalog_fetch::trust_in;
use crucible_app::content_use::{Consent, LAUNCHING, Routes, Serving, WARNED, agent_route};
use crucible_client_api::Author;
use crucible_runner::Tools;
use crucible_session::Session;
use crucible_tui::{Recording, Renderer};
use crucible_types::Compacting;

use crate::cli::Fatal;
use crate::cli::converse::{First, Terms, converse};
use crate::cli::fake::Script;

use crate::cli::converse::answering::Answers;
use crate::cli::converse::docking::Submission;
use crate::cli::converse::queueing::{self, Retained};
use crate::cli::converse::warning::{
    CRAMPED_NOTE, CRAMPED_QUEUE, CRAMPED_ROOM, CRAMPED_SEND, DROPPED, KEPT, Put, QUEUE_KEPT,
    UNSENT, left, named, returned, unanswered,
};
use crate::cli::converse::{Held, Work, ran, refreshed, typing, work};

use crate::cli::style::Style;

use super::{opening, paired, plain, saying, scripted, typed};

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
        Work::Turn("go".to_owned(), Box::default(), Author::Person),
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

/// What the question before `work` offers for going back.
fn going_back(work: &Work) -> &'static str {
    let offered = Put::before(work).offered().expect("fixed answers");
    offered
        .iter()
        .find(|offer| offer.name == "Go back")
        .expect("a way back")
        .says
}

#[test]
fn the_question_before_a_queued_turn_says_the_lines_stay_queued() {
    let queue = Work::Queued("first".to_owned(), Box::default());

    let answered = going_back(&queue);
    assert!(answered.contains("in the queue"), "{answered}");
    assert!(!answered.contains("prompt box"), "{answered}");
}

#[test]
fn the_question_before_a_dock_answer_says_the_answers_stay_in_the_dock() {
    let answer = Work::Answer(Submission {
        era: crucible_client_api::Era::Stint(1),
        id: crucible_types::QuestionId::numbered(1),
        replies: Vec::new(),
    });
    let turn = Work::Turn("go".to_owned(), Box::default(), Author::Person);

    let answered = going_back(&answer);
    assert!(answered.contains("in the dock"), "{answered}");
    assert!(!answered.contains("prompt box"), "{answered}");
    assert!(
        going_back(&turn).contains("prompt box"),
        "{}",
        going_back(&turn)
    );
}

#[test]
fn a_woken_note_held_back_leaves_the_box_as_the_person_left_it() {
    // The note that wakes the model when a background command ends is the
    // application's words. Held back, it is not handed to the person as their
    // message: what they had half-typed stays, and nothing claims otherwise.
    let script = Script::new(vec![saying("answered")]);
    let asked = script.asked();
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        scripted(script, Tools::new(), session)
    });
    let warned = Terms {
        consent: serving_google(),
        ..plain()
    };
    let opening = opening();
    let mut input = Cursor::new(Vec::new());
    let mut held = Held::new(
        warned.plan.clone(),
        warned.sending.get(),
        Answers {
            input: &mut input,
            keys: true,
        },
        &opening,
    );
    held.editor.put("half a thought");
    // Redirected and too short for the panel, so the warning ends unasked.
    let mut cramped = Renderer::new(Recording::redirected(80, 3));
    let note = "Background command finished: exit status 0".to_owned();

    let ended = ran(
        conversation,
        &mut cramped,
        &warned,
        Work::Turn(note, Box::default(), Author::Application),
        &mut held,
    )
    .map(|(_, leaving)| leaving);

    assert!(matches!(ended, Ok(false)), "{ended:?}");
    assert_eq!(asked.load(Ordering::SeqCst), 0, "the note was sent");
    assert_eq!(held.editor.text(), "half a thought");
    let written = cramped.terminal().written().to_string();
    assert!(written.contains(CRAMPED_NOTE), "{written}");
    assert!(!written.contains(CRAMPED_SEND), "{written}");
}

#[test]
fn what_is_said_after_holding_back_names_what_became_of_it() {
    // The person's message is back in the box; the note is dropped, and says
    // the model was not told; room and the dock's answers had nothing to give
    // back, and the queue stays queued. Neither of the note's lines promises a
    // send to try again.
    let message = Work::Turn("hello".to_owned(), Box::default(), Author::Person);
    let queue = Work::Queued("first".to_owned(), Box::default());
    let note = Work::Turn(
        "Background command finished: exit status 0".to_owned(),
        Box::default(),
        Author::Application,
    );
    let room = Work::Room(Compacting::Full);
    let answers = Work::Answer(Submission {
        era: crucible_client_api::Era::Stint(1),
        id: crucible_types::QuestionId::numbered(1),
        replies: Vec::new(),
    });

    assert_eq!(left(&message, false), KEPT);
    assert_eq!(left(&message, true), CRAMPED_SEND);
    assert_eq!(left(&note, false), DROPPED);
    assert_eq!(left(&note, true), CRAMPED_NOTE);
    assert_eq!(left(&room, false), UNSENT);
    assert_eq!(left(&room, true), CRAMPED_ROOM);
    assert_eq!(left(&answers, false), UNSENT);
    assert_eq!(left(&answers, true), CRAMPED_ROOM);
    assert_eq!(left(&queue, false), QUEUE_KEPT);
    assert_eq!(left(&queue, true), CRAMPED_QUEUE);
    // The line says the message is back in the box exactly where it is put
    // back, and only the person's own message is.
    for work in [&message, &note, &room, &answers, &queue] {
        assert_eq!(left(work, false) == KEPT, returned(work).is_some());
    }
    assert_eq!(returned(&message), Some("hello"));
    for said in [DROPPED, CRAMPED_NOTE] {
        assert!(said.contains("not told the command ended"), "{said}");
        assert!(!said.contains("again"), "{said}");
    }
}

#[test]
fn the_question_before_a_woken_note_does_not_promise_it_to_the_prompt_box() {
    let note = Work::Turn(
        "Background command finished: exit status 0".to_owned(),
        Box::default(),
        Author::Application,
    );

    let back = going_back(&note);
    assert!(!back.contains("Keeps your message"), "{back}");
    assert!(back.contains("nothing is sent"), "{back}");
}

#[test]
fn the_question_before_room_is_made_offers_no_message_to_send_or_keep() {
    // Making room sends the conversation for its recap; there is no message
    // in the box to send, or to keep there. `/compact`, and "Carry on from
    // summary" on picking a session up, are the two ways it reaches this panel.
    for why in [Compacting::Asked, Compacting::Resumed] {
        let room = Work::Room(why);
        let offered = Put::before(&room).offered().expect("fixed answers");
        for offer in offered {
            assert!(!offer.says.contains("message"), "{why:?}: {}", offer.says);
        }
        let back = going_back(&room);
        assert!(back.contains("nothing is sent"), "{back}");
        assert!(back.contains("conversation"), "{back}");
    }
}

#[test]
fn a_queue_held_back_keeps_every_line_and_the_box_as_the_reader_left_it() {
    // The lines queued behind a turn are the next turn. Held back on a route
    // with no yes, every one of them stays in the queue: none is lost, none
    // takes the place of what the reader has typed in the box, and they are
    // not asked about again until the reader sends the prompt they go with.
    let script = Script::new(vec![saying("answered")]);
    let asked = script.asked();
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        scripted(script, Tools::new(), session)
    });
    let warned = Terms {
        consent: serving_google(),
        ..plain()
    };
    let opening = opening();
    let mut input = Cursor::new(Vec::new());
    let mut held = Held::new(
        warned.plan.clone(),
        warned.sending.get(),
        Answers {
            input: &mut input,
            keys: true,
        },
        &opening,
    );
    for line in ["first", "second", "third"] {
        assert_eq!(held.queued.accept(&mut typed(line)), Retained::Accepted);
    }
    held.editor.put("half a thought");
    // Redirected and too short for the panel, so the warning ends unasked.
    let mut cramped = Renderer::new(Recording::redirected(80, 3));

    let (conversation, taken) = queueing::taken(
        conversation,
        &mut cramped,
        &warned,
        &mut held,
        Style::plain(),
    )
    .expect("the queue to be taken");

    assert_eq!(
        taken,
        Some(false),
        "the queue was not taken, or the session left"
    );
    assert_eq!(asked.load(Ordering::SeqCst), 0, "the queue was sent");
    assert_eq!(held.editor.text(), "half a thought");
    let waiting = held.queued.waiting_all().collect::<Vec<_>>();
    assert_eq!(waiting, ["first", "second", "third"]);
    let written = cramped.terminal().written().to_string();
    assert!(written.contains(CRAMPED_QUEUE), "{written}");
    assert!(!written.contains(CRAMPED_SEND), "{written}");

    // Nothing the held-back turn offered is left for a turn that runs first.
    assert!(!warned.steer.any(), "a held-back line is still on offer");
    // Back at the top of the loop, before the reader has sent anything.
    let (_, again) = queueing::taken(
        conversation,
        &mut cramped,
        &warned,
        &mut held,
        Style::plain(),
    )
    .expect("the queue to be asked");
    assert_eq!(
        again, None,
        "the queue was taken again before the reader sent"
    );
    assert_eq!(held.queued.waiting_count(), 3);
    assert_eq!(
        cramped.terminal().written().matches(CRAMPED_QUEUE).count(),
        1
    );
}

#[test]
fn a_line_sent_alone_from_the_queue_and_held_back_goes_back_to_the_queue() {
    // Ctrl+S between turns sent the highlighted line alone, and the route it
    // would go on has no yes. Held back, it is the queue's line still: it goes
    // back to the front of the queue, the box keeps what the reader had typed,
    // and the lines wait for the reader as the whole queue does.
    let script = Script::new(vec![saying("answered")]);
    let asked = script.asked();
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        scripted(script, Tools::new(), session)
    });
    let warned = Terms {
        consent: serving_google(),
        ..plain()
    };
    let opening = opening();
    let mut input = Cursor::new(Vec::new());
    let mut held = Held::new(
        warned.plan.clone(),
        warned.sending.get(),
        Answers {
            input: &mut input,
            keys: true,
        },
        &opening,
    );
    for line in ["first", "second", "third"] {
        assert_eq!(held.queued.accept(&mut typed(line)), Retained::Accepted);
    }
    held.editor.put("half a thought");
    let alone = held.queued.sent_alone().expect("the highlighted line");
    let mut cramped = Renderer::new(Recording::redirected(80, 3));

    let (conversation, leaving) = ran(
        conversation,
        &mut cramped,
        &warned,
        Work::Queued(alone, Box::default()),
        &mut held,
    )
    .expect("the line to be held back");

    assert!(!leaving, "the session left");
    assert_eq!(asked.load(Ordering::SeqCst), 0, "the line was sent");
    assert_eq!(held.editor.text(), "half a thought");
    let waiting = held.queued.waiting_all().collect::<Vec<_>>();
    assert_eq!(waiting, ["first", "second", "third"]);
    let (_, again) = queueing::taken(
        conversation,
        &mut cramped,
        &warned,
        &mut held,
        Style::plain(),
    )
    .expect("the queue to be asked");
    assert_eq!(again, None, "the queue was taken before the reader sent");
}

#[test]
fn a_line_is_the_work_of_where_it_was_sent_from() {
    // Held back, the work a line started decides where the line goes: one from
    // the box back into the box, one Ctrl+S sent from the queue back to the
    // queue. So the loop must start the queue's work for the queue's line.
    let queue = work(
        "first".to_owned(),
        Box::default(),
        Author::Person,
        typing::Origin::Queue,
    );
    let typed = work(
        "first".to_owned(),
        Box::default(),
        Author::Person,
        typing::Origin::Box,
    );

    assert!(
        matches!(&queue, Work::Queued(line, _) if line == "first"),
        "the queue's line did not start the queue's work"
    );
    assert!(
        matches!(&typed, Work::Turn(line, _, Author::Person) if line == "first"),
        "a typed line did not start a turn of the reader's"
    );
}

#[test]
fn the_panel_names_the_route_with_the_glyph_sets_own_dot() {
    // The registry spells some routes with a dot between the plan and its
    // site. The panel's title and its "Starts" line are drawn, so a terminal
    // set to ASCII glyphs is not sent U+00B7 in either.
    let routes = Routes::production();
    let warned = routes
        .warned("subscription:moonshot@kimi.ai")
        .expect("the Kimi Code row is warned");

    assert_eq!(
        named(warned, crucible_tui::Glyphs::Ascii),
        "Kimi Code - kimi.ai"
    );
    assert_eq!(
        named(warned, crucible_tui::Glyphs::Unicode),
        "Kimi Code · kimi.ai"
    );
}
