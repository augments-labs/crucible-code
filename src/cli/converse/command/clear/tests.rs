//! What `/clear` leaves behind, and what it starts over.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crucible_app::Conversation;
use crucible_auth::Store;
use crucible_builtins::{Ledger, Plan};
use crucible_core::{AgentId, Cancel, Message, Revealed, StopReason, ToolArgs, Transcript};
use crucible_runner::{Agent, Model, Runner, Tools};
use crucible_session::{Session, recent};
use crucible_tui::{Recording, Renderer};

use crate::cli::converse::tests::paired;
use crate::cli::converse::{Answers, Held};
use crate::cli::draw::opening::{Opening, Standing};
use crate::cli::fake::Script;
use crate::cli::sample::Sample;
use crate::cli::style::Style;

use super::super::Terms;
use super::run;

/// What a session holds, for a session holding nothing — the same holder
/// `/resume`'s tests lend their runs.
fn lent<'a>(input: &'a mut dyn std::io::BufRead, opening: &'a Standing) -> Held<'a> {
    Held::new(
        crucible_builtins::Plan::new(),
        crucible_tui::Sending::default(),
        Answers { input, keys: false },
        opening,
    )
}

/// The opening card a clear puts back, read off `sample`'s workspace.
fn standing(sample: &Sample) -> Standing {
    Standing::new(
        &Opening {
            model: Some("script"),
            unasked: "",
            trouble: None,
            workspace: &sample.workspace(),
            sessions: &[],
            update: None,
            style: Style::plain(),
        },
        std::time::SystemTime::now(),
    )
}

/// The terms a clear is run under: a session directory of the sample's own,
/// and the two things the tools of such a run would have been built with.
fn terms(sample: &Sample, ledger: &Ledger, plan: &Plan) -> Terms {
    Terms {
        style: std::cell::Cell::new(Style::plain()),
        chosen: std::cell::Cell::new(None),
        reading: std::cell::RefCell::default(),
        cancel: Cancel::new(),
        ending: crate::cli::ending::Ending::deaf(),
        steer: crucible_core::Steer::new(),
        aside: crucible_core::Aside::new(),
        ledger: ledger.clone(),
        revealed: Revealed::new(),
        plan: plan.clone(),
        putting: crate::cli::seen::Putting::new(),
        client: crate::cli::client::Client::new(),
        leaving: crucible_builtins::Background::new(),
        pending_model: std::cell::Cell::new(None),
        pending_mode: std::cell::Cell::new(None),
        settings: crucible_config::Settings::default(),
        choosing: sample.root().join("unwritten-home.json"),
        logins: Store::in_home(&sample.root()),
        subscriptions: crucible_app::subscription::Subscriptions::production(
            &crucible_auth::Renewals::new(),
        ),

        // `/clear` never reaches it, and these terms have no provider to
        // build one from either — the loop they drive answers from a
        // script.
        serving: Box::new(|named, _| {
            Err(crucible_app::AppError::Provider {
                named: named.name.into(),
                has: named.name.into(),
            })
        }),
        sessions: sample.logs(),
        workspace: sample.workspace(),
        sending: crucible_tui::Sending::default(),
        commands: crate::cli::converse::command::builtins(&std::sync::Arc::default())
            .expect("the built-in commands register"),
        providers: crucible_app::providers::providers().expect("the built-in providers register"),
    }
}

/// A runner recording to a session of `sample`'s, holding one exchange.
///
/// The log and the transcript hold the same two messages, because that is
/// what a session that took a turn looks like: what `/clear` leaves behind
/// is read back off the disk, and a log the transcript disagrees with would
/// prove nothing about either.
fn talking(sample: &Sample, asked: &str) -> Conversation {
    let answered = Message::Agent {
        continuation: None,
        text: "an answer".into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
    };

    let session =
        Arc::new(Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session"));
    session.append(&Message::said(asked));
    session.append(&answered);

    let mut transcript = Transcript::new();
    transcript
        .push(Message::said(asked))
        .expect("valid fixture transcript");
    transcript.push(answered).expect("valid fixture transcript");

    paired(Arc::clone(&session), |session| {
        Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            Agent::new(
                AgentId::new("test"),
                Model {
                    name: "script".into(),
                    max_tokens: 64,
                    window: None,
                    accepts: None,
                    effort: None,
                },
            ),
            crucible_context::ContextInputs::new(std::env::temp_dir()),
            session,
        )
        .resuming(transcript)
    })
}

/// Runs `/clear` against `conversation`, and says what reached the terminal and
/// which session the loop holds afterwards.
fn clearing(
    sample: &Sample,
    terms: &Terms,
    conversation: &mut Conversation,
) -> (String, Arc<Session>) {
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = std::io::empty();
    let opening = standing(sample);
    let mut held = lent(&mut input, &opening);

    run(&mut renderer, conversation, &mut held, terms).expect("the terminal to be written");

    (
        renderer.terminal().written().to_string(),
        Arc::clone(conversation.session()),
    )
}

/// How long the list is given to hold the session that was left.
///
/// The same wait `/resume`'s tests take, and for the same reason: a session
/// reaches the list when its first prompt reaches its log, and that log is
/// written by the thread that owns its queue.
const SETTLING: Duration = Duration::from_secs(5);

/// What `/resume` would list for `sample`, once `of` sessions are on it.
fn listed(sample: &Sample, of: usize) -> Vec<String> {
    let since = Instant::now();

    loop {
        let found = recent(&sample.logs(), &sample.workspace(), 9);

        if found.len() == of {
            return found
                .iter()
                .map(|session| session.asked().to_owned())
                .collect();
        }

        assert!(
            since.elapsed() < SETTLING,
            "{} of {of} sessions reached the list",
            found.len()
        );

        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn clearing_starts_a_new_session_and_leaves_the_old_one_where_it_can_be_found() {
    // The whole of what `/clear` is now: the session in hand is not the one
    // it was, what the model is told is nothing, and what was said is still
    // somewhere — which is the one outcome this command must never lose.
    let sample = Sample::new("clear-starts-a-session");
    let mut conversation = talking(&sample, "what was said before");
    let left = conversation
        .session()
        .id()
        .cloned()
        .expect("a recorded session");

    let (_, now) = clearing(
        &sample,
        &terms(&sample, &Ledger::new(), &Plan::new()),
        &mut conversation,
    );

    let now = now.id().cloned().expect("a recorded session");
    assert_ne!(now, left, "the session in hand is the one that was left");
    assert_eq!(
        conversation.runner().transcript().len(),
        0,
        "the transcript came with it"
    );
    assert_eq!(listed(&sample, 1), ["what was said before"]);
}

#[test]
fn clearing_takes_the_screen_and_what_was_held_behind_it() {
    // An empty context is what `/clear` promises, and the screen is part of
    // it: rows left standing were said by a session the agent has no memory
    // of, and what was held behind those rows goes with them — a key
    // opening what is behind a row nobody can see is worse than no offer.
    let sample = Sample::new("clear-empties-the-screen");
    let mut conversation = talking(&sample, "what was said before");
    let terms = terms(&sample, &Ledger::new(), &Plan::new());

    let mut renderer = Renderer::new(Recording::new(80, 24));
    renderer
        .commit("a row of the session being left")
        .expect("the terminal to be written");

    let mut input = std::io::empty();
    let opening = standing(&sample);
    let mut held = lent(&mut input, &opening);
    let call = crucible_core::ToolId::new("call-1");
    held.kept.calling(call.clone(), "read".into());
    held.kept
        .finished(&call, "what the row had no room for".into(), 3);
    assert_eq!(held.kept.newest().count(), 1);

    run(&mut renderer, &mut conversation, &mut held, &terms).expect("the terminal to be written");

    let picture = renderer.terminal().picture().rows().join("\n");
    assert!(
        !picture.contains("a row of the session being left"),
        "{picture}"
    );
    assert!(
        !picture.contains("started a new session"),
        "the screen after a clear is a fresh start, not an announcement: {picture}"
    );
    assert!(
        picture.contains("Tips"),
        "the opening card stands where the transcript was: {picture}"
    );
    assert_eq!(held.kept.newest().count(), 0);
}

#[test]
fn a_plan_written_before_a_clear_is_not_standing_over_the_session_after_it() {
    // The panel above the box is drawn from this, and the tasks in it were
    // written by a session that is now over. Left standing, it would list
    // work above a prompt whose agent has never heard of any of it.
    let sample = Sample::new("clear-forgets-the-plan");
    let mut conversation = talking(&sample, "what was said before");
    let plan = Plan::new();

    plan.replay(&ToolArgs::new(
        r#"{"tasks":[{"task":"Write the contributor guide","state":"doing"}]}"#,
    ));
    assert_eq!(plan.tasks().len(), 1);

    clearing(
        &sample,
        &terms(&sample, &Ledger::new(), &plan),
        &mut conversation,
    );

    assert!(plan.tasks().is_empty());
}

#[test]
fn an_image_pasted_before_a_clear_is_not_attached_after_it() {
    // The paste put `[Image #1]` in a prompt of the session being left, and
    // the numbering starts over with the session. An image still held here
    // would be attached to the first prompt after the clear that says the
    // marker — a picture the agent was never shown and the user never sent
    // it.
    let sample = Sample::new("clear-forgets-the-images");
    let mut conversation = talking(&sample, "what was said before");
    let terms = terms(&sample, &Ledger::new(), &Plan::new());

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = std::io::empty();
    let opening = standing(&sample);
    let mut held = lent(&mut input, &opening);
    held.images.push("a-picture.png".into());

    run(&mut renderer, &mut conversation, &mut held, &terms).expect("the terminal to be written");

    assert!(held.images.is_empty());
}

#[test]
fn what_was_said_before_a_clear_comes_back_when_that_session_is_picked_up() {
    // The log the clear left is closed rather than abandoned: an unfinished
    // one still holds a claim, and `/resume` would refuse it as open in
    // another crucible — which names this crucible.
    let sample = Sample::new("clear-then-resume");
    let mut conversation = talking(&sample, "what was said before");
    let terms = terms(&sample, &Ledger::new(), &Plan::new());

    clearing(&sample, &terms, &mut conversation);
    assert_eq!(listed(&sample, 1), ["what was said before"]);
    let picked = recent(&sample.logs(), &sample.workspace(), 1)
        .first()
        .map(|session| session.id().as_str().to_owned())
        .expect("the cleared session is on the list");

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = std::io::empty();
    let opening = standing(&sample);
    let mut held = crate::cli::converse::Held::new(
        Plan::new(),
        crucible_tui::Sending::default(),
        crate::cli::converse::Answers {
            input: &mut input,
            keys: false,
        },
        &opening,
    );
    super::super::resume::run(&picked, &mut renderer, &mut conversation, &mut held, &terms)
        .expect("the terminal to be written");

    let written = renderer.terminal().written().to_string();
    assert!(written.contains("what was said before"), "{written}");
    assert_eq!(conversation.runner().transcript().len(), 2, "{written}");
}

#[test]
fn a_session_that_said_nothing_is_left_where_it_is() {
    // Starting a second log here would leave the first one empty and the
    // second one about to be, which is two files for a session that never
    // happened -- and `--continue` picks the newest of them.
    let sample = Sample::new("clear-said-nothing");
    let session =
        Arc::new(Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session"));
    let held = session.id().cloned().expect("a recorded session");
    let mut conversation = paired(Arc::clone(&session), |session| {
        Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            Agent::new(
                AgentId::new("test"),
                Model {
                    name: "script".into(),
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

    let (written, now) = clearing(
        &sample,
        &terms(&sample, &Ledger::new(), &Plan::new()),
        &mut conversation,
    );

    assert!(written.contains("nothing had been said"), "{written}");
    assert_eq!(now.id(), Some(&held), "{written}");
}

#[test]
fn a_new_session_that_cannot_be_started_leaves_the_one_in_hand_running() {
    // The session being recorded is the one thing a failure here must not
    // cost. Reported the way every other path with a filename in it is, and
    // the loop carries on with the session it had.
    let sample = Sample::new("clear-cannot-start");
    let mut conversation = talking(&sample, "what was said before");
    let held = conversation
        .session()
        .id()
        .cloned()
        .expect("a recorded session");

    let blocked = sample.root().join("not-a-directory");
    std::fs::write(&blocked, "").expect("a file where a directory is wanted");
    let terms = Terms {
        sessions: blocked,
        ..terms(&sample, &Ledger::new(), &Plan::new())
    };

    let (written, now) = clearing(&sample, &terms, &mut conversation);

    assert!(written.contains("! "), "{written}");
    assert!(
        !written.contains("Tips"),
        "a failed clear leaves the screen exactly as it was: {written}"
    );
    assert_eq!(now.id(), Some(&held), "{written}");
}

#[test]
fn clearing_forgets_the_tools_that_were_looked_up() {
    // They belong to the conversation that looked them up. Left standing they
    // would be advertised to a session that never asked — the schema cost this
    // whole mechanism exists to avoid, paid for a model with no memory of why.
    let sample = Sample::new("clear-forgets-the-lookups");
    let mut conversation = talking(&sample, "what was said before");
    let terms = terms(&sample, &Ledger::new(), &Plan::new());

    terms.revealed.reveal("web_search");
    assert!(terms.revealed.holds("web_search"));

    clearing(&sample, &terms, &mut conversation);

    assert!(!terms.revealed.holds("web_search"));
}
