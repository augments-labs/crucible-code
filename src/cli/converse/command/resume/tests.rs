//! What the listing says, and what picking a session up by its id changes.
//!
//! The sessions are recorded through the runner's own API rather than planted
//! as text: what `/resume` picks up has to be what a session leaves behind, and
//! a fixture written by hand is a second opinion about that.
//!
//! The picker's keys are not driven from here: a [`Recording`] takes writes and
//! answers no key, so what a key does is proven where the key tables live, in
//! `finding`'s own tests. What these prove is everything around the keys — the
//! id-bearing listing a keyboardless run prints, what an id picks up, what a
//! rename writes down, and what the marked row's meta line says.

use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

use crucible_auth::Store;
use crucible_builtins::{Ledger, Plan};
use crucible_runner::{Agent, Model, Runner, Tools};
use crucible_runtime::Cancel;
use crucible_session::Session;
use crucible_tools::Revealed;
use crucible_tui::{Recording, Renderer, Row};
use crucible_types::{
    AgentId, Message, RecordedToolOutput, SessionId, StopReason, ToolArgs, ToolCall, ToolId,
    ToolResult,
};

use crate::cli::converse::tests::paired;
use crate::cli::converse::{Answers, Held};
use crate::cli::draw::opening::{Opening, Standing};
use crate::cli::fake::Script;
use crate::cli::sample::Sample;
use crate::cli::style::Style;
use crucible_workspace::Workspace;

use super::*;

/// The card a session opens with, as a launch would have built it.
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
        SystemTime::now(),
    )
}

/// A session recorded in `sample` and closed again, holding one exchange.
fn recorded(sample: &Sample, asked: &str) -> Session {
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");

    session.append(&Message::said(asked));
    session.append(&Message::Agent {
        continuation: None,
        text: "an answer".into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
    });

    session
}

/// The id a recorded session answers to, as `/resume` is handed it.
fn named(session: &Session) -> String {
    session
        .id()
        .expect("a recorded session has a name")
        .as_str()
        .to_owned()
}

/// A conversation that answers nothing, recording to `session`.
fn over(session: &Arc<Session>) -> Conversation {
    paired(Arc::clone(session), |session| runner(&session))
}

/// A runner that answers nothing, recording to `session`.
fn runner(session: &Arc<Session>) -> Runner {
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
        session.clone(),
    )
}

fn terms(sample: &Sample) -> Terms {
    Terms {
        consent: crucible_app::content_use::Consent::new(
            crucible_app::content_use::Routes::production(),
        ),
        style: Cell::new(Style::plain()),
        chosen: Cell::new(None),
        reading: std::cell::RefCell::default(),
        settled: std::cell::RefCell::default(),
        cancel: Cancel::new(),
        runtime: crate::cli::fake::runtime(),
        ending: crate::cli::ending::Ending::deaf(),
        steer: crucible_runtime::Steer::new(),
        aside: crucible_runtime::Aside::new(),
        ledger: Ledger::new(),
        revealed: Revealed::new(),
        plan: Plan::new(),
        putting: crate::cli::seen::Putting::new(),
        client: crate::cli::client::Client::new(),
        leaving: crucible_builtins::Background::new(),
        pending_model: std::cell::Cell::new(None),
        pending_mode: std::cell::Cell::new(None),
        pending_speed: std::cell::Cell::new(None),
        settings: crucible_config::Settings::default(),
        choosing: sample.root().join("unwritten-home.json"),
        logins: Store::in_home(&sample.root()),
        subscriptions: crucible_app::subscription::Subscriptions::production(
            &crucible_auth::Renewals::new(),
        ),

        // `/resume` never reaches it, and these terms have no provider to build
        // one from either — the loop they drive answers from a script.
        serving: Box::new(|named, _| {
            Err(crucible_app::AppError::Provider {
                named: named.name.into(),
                has: named.name.into(),
            })
        }),
        sourcing: Box::new(|_, _, _| crucible_app::startup::Reaching::nothing()),
        environment: Box::new(|_| None),
        sessions: sample.logs(),
        workspace: sample.workspace(),
        sending: std::cell::Cell::default(),
        pinning: std::cell::Cell::default(),
        commands: crate::cli::converse::command::builtins(&std::sync::Arc::default())
            .expect("the built-in commands register"),
        providers: crucible_app::providers::providers().expect("the built-in providers register"),
    }
}

/// How long the list is given to hold a session that belongs on it.
///
/// Generous, because it is only ever waited out by a failure: what is being
/// waited for is a queue draining, which takes no time at all on a machine that
/// is working, and the wait is what turns "took a moment longer than the test
/// expected" into a pass rather than into a report about `/resume`.
const SETTLING: Duration = Duration::from_secs(5);

/// The session `id` names, once the list holds it.
///
/// A session reaches the list when its first prompt reaches its log, and the
/// log is written by the thread that owns its queue — so a read racing that
/// thread would find the list one row short.
fn on_the_list(sample: &Sample, id: &SessionId) -> Recorded {
    let since = std::time::Instant::now();

    loop {
        if let Some(found) = recent(
            &sample.logs(),
            Roots::These(&[sample.workspace().root()]),
            Reach::FirstFrame,
            SHOWN,
        )
        .into_iter()
        .find(|session| session.id() == id)
        {
            return found;
        }

        assert!(
            since.elapsed() < SETTLING,
            "the session never reached the list"
        );

        std::thread::sleep(Duration::from_millis(1));
    }
}

/// What a session holds, for a session holding nothing.
///
/// No keys, because these drive a recording rather than a terminal somebody is
/// at, and the question a large session asks has nobody to answer it — which is
/// also why the reader it reads answers from is empty.
fn lent<'a>(input: &'a mut dyn std::io::BufRead, opening: &'a Standing) -> Held<'a> {
    Held::new(
        Plan::new(),
        crucible_tui::Sending::default(),
        Answers { input, keys: false },
        opening,
    )
}

/// Runs `/resume {said}` against `conversation`, and says what the window ends up
/// showing — one row a line, the blank ones left out — beside the session the
/// loop holds afterwards.
fn resuming(
    said: &str,
    sample: &Sample,
    conversation: &mut Conversation,
) -> (String, Arc<Session>) {
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = std::io::empty();
    let opening = standing(sample);
    let mut held = lent(&mut input, &opening);

    run(said, &mut renderer, conversation, &mut held, &terms(sample))
        .expect("the terminal to be written");

    (
        renderer.terminal().picture().said().join("\n"),
        Arc::clone(conversation.session()),
    )
}

#[test]
fn a_directory_nothing_was_recorded_in_says_so() {
    let sample = Sample::new("resume-empty");
    let session = Arc::new(Session::nowhere());
    let mut conversation = over(&session);

    let (written, _) = resuming("", &sample, &mut conversation);

    assert!(written.contains(NEVER), "{written}");
}

#[test]
fn the_list_names_each_session_by_its_id() {
    // The id is the only handle a keyboardless run leaves: there is no picker
    // to walk, so the row has to carry the exact word `--resume` and
    // `/resume` take. The order belongs to `recent` and is proven there.
    let sample = Sample::new("resume-list");
    let one = recorded(&sample, "one question");
    let first = named(&one);
    drop(one);
    let two = recorded(&sample, "another question");
    let second = named(&two);
    drop(two);
    let session = Arc::new(Session::nowhere());
    let mut conversation = over(&session);

    let (written, _) = resuming("", &sample, &mut conversation);
    let rows: Vec<&str> = written
        .lines()
        .filter(|row| row.contains("question"))
        .collect();

    assert_eq!(rows.len(), 2, "{written}");
    for (id, asked) in [(&first, "one question"), (&second, "another question")] {
        assert!(
            rows.iter()
                .any(|row| row.starts_with(id.as_str()) && row.contains(asked)),
            "{written}"
        );
    }
    assert!(written.contains("just now"), "{written}");
}

#[test]
fn an_id_that_names_nothing_says_so_and_shows_the_list_again() {
    // Both halves matter. The refusal is the same sentence `--resume` refuses
    // with, and the listing after it is something to try instead. The two
    // shapes fail the same way because they are the same fact: neither names a
    // session recorded here, and whether that is spelling or absence is
    // nothing the reader can act on differently.
    let sample = Sample::new("resume-unknown");
    drop(recorded(&sample, "the only question"));
    let session = Arc::new(Session::nowhere());
    let mut conversation = over(&session);

    let absent = SessionId::new();
    for said in ["the second one", absent.as_str()] {
        let (written, _) = resuming(said, &sample, &mut conversation);

        assert!(
            written.contains(&format!("! no session {said} in this workspace")),
            "{written}"
        );
        assert!(written.contains("the only question"), "{written}");
    }
}

#[test]
fn picking_one_up_makes_it_the_session_being_recorded_to() {
    let sample = Sample::new("resume-picked");
    let earlier = recorded(&sample, "what was asked before");
    let id = named(&earlier);
    let path = earlier.path().to_owned();
    drop(earlier);

    let session = Arc::new(Session::nowhere());
    let mut conversation = over(&session);
    let (written, now) = resuming(&id, &sample, &mut conversation);

    assert_eq!(now.path(), path);
    assert_eq!(
        conversation.runner().transcript().len(),
        2,
        "the prompt and the answer came back: {written}"
    );
    assert!(written.contains("what was asked before"), "{written}");
    assert!(
        written.contains("Tips"),
        "the card stands above the replay, where a launch would have drawn it: {written}"
    );
}

#[test]
fn the_session_already_open_is_refused_as_the_one_being_used() {
    // Not as "open in another crucible", which is what the log itself would
    // say: the claim on that file is this process's own, and being sent to
    // close a crucible that is this one is worse than not being answered.
    // Continued the way `--continue` continues one, so the session in hand is
    // both recorded and claimed by this process — which is the arrangement the
    // answer is about.
    let sample = Sample::new("resume-itself");
    drop(recorded(&sample, "the session in hand"));
    let (open, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    let id = named(&open);
    let path = open.path().to_owned();
    let open = Arc::new(open);
    let mut conversation = paired(open, |open| runner(&open).resuming(transcript));

    let (written, now) = resuming(&id, &sample, &mut conversation);

    assert!(
        written.contains("this is the session you are in"),
        "{written}"
    );
    assert!(!written.contains("another crucible"), "{written}");
    assert_eq!(now.path(), path, "{written}");
}

#[test]
fn what_was_being_recorded_to_is_closed_and_stays_readable() {
    // The session left behind is finished rather than dropped, so its log is
    // complete before this process moves on — and complete means a later
    // crucible can continue it.
    let sample = Sample::new("resume-leaving");
    let wanted = recorded(&sample, "the one picked up");
    let id = named(&wanted);
    drop(wanted);

    let leaving =
        Arc::new(Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session"));
    let left = leaving.path().to_owned();
    let mut conversation = over(&leaving);

    leaving.append(&Message::said("said in passing"));

    let (written, now) = resuming(&id, &sample, &mut conversation);

    assert_ne!(now.path(), left, "{written}");

    let recovered = std::fs::read_to_string(&left).expect("the log it was recording to");
    assert!(recovered.contains("said in passing"), "{recovered}");
}

#[test]
fn the_transcript_a_session_replaces_is_not_left_standing_above_it() {
    // Two conversations in one band would be joined at a point nothing marks,
    // and a reader scrolling back would walk out of the session they picked up
    // and into the one they left without being told.
    let sample = Sample::new("resume-replaces");
    let earlier = recorded(&sample, "what was asked before");
    let id = named(&earlier);
    drop(earlier);

    let session = Arc::new(Session::nowhere());
    let mut conversation = over(&session);
    let mut renderer = Renderer::new(Recording::new(80, 24));
    renderer
        .present(&[Row::new().then(Slot::Plain, "said in the session being left")])
        .expect("a recording cannot fail");

    let mut input = std::io::empty();
    let opening = standing(&sample);
    run(
        &id,
        &mut renderer,
        &mut conversation,
        &mut lent(&mut input, &opening),
        &terms(&sample),
    )
    .expect("the terminal to be written");

    let written = renderer.terminal().picture().said().join("\n");

    assert!(written.contains("what was asked before"), "{written}");
    assert!(
        !written.contains("said in the session being left"),
        "{written}"
    );
}

#[test]
fn an_image_pasted_in_the_session_being_left_is_not_attached_after_it() {
    // The paste put `[Image #1]` in a prompt of the session being left, and the
    // numbering starts over with the session. An image still held here would be
    // attached to the first prompt after the resume that says the marker.
    let sample = Sample::new("resume-forgets-the-images");
    let earlier = recorded(&sample, "what was asked before");
    let id = named(&earlier);
    drop(earlier);

    let session = Arc::new(Session::nowhere());
    let mut conversation = over(&session);
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = std::io::empty();
    let opening = standing(&sample);
    let mut held = lent(&mut input, &opening);
    held.images.push("a-picture.png".into());

    run(
        &id,
        &mut renderer,
        &mut conversation,
        &mut held,
        &terms(&sample),
    )
    .expect("the terminal to be written");

    assert!(held.images.is_empty());
}

#[test]
fn the_plan_that_comes_back_is_the_one_the_session_picked_up_wrote() {
    // "Resume" means the exact state of the session picked up: the plan its
    // last `todo_write` left is standing over the box again, and the plan of
    // the session being left — work this agent now has no memory of — is not.
    let sample = Sample::new("resume-replays-the-plan");
    let planned = recorded(&sample, "plan the work");
    let id = named(&planned);
    planned.append(&Message::Agent {
        continuation: None,
        text: "".into(),
        calls: vec![crucible_types::ToolCall {
            id: ToolId::new("call-1"),
            name: "todo_write".into(),
            args: crucible_types::ToolArgs::new(
                r#"{"tasks":[{"task":"Write the contributor guide","state":"doing"}]}"#,
            ),
        }],
        stop: Some(StopReason::WantsTools),
    });
    // Answered, the way a log a session actually left holds it: a trailing
    // call nothing answered is a turn that broke off, and the replay drops it.
    planned.append(&Message::ToolResults(vec![crucible_types::ToolResult {
        id: ToolId::new("call-1"),
        output: crucible_types::RecordedToolOutput::ok("1 task planned"),
    }]));
    drop(planned);

    let session = Arc::new(Session::nowhere());
    let mut conversation = over(&session);
    let terms = terms(&sample);
    terms.plan.replay(&crucible_types::ToolArgs::new(
        r#"{"tasks":[{"task":"Work of the session being left","state":"doing"}]}"#,
    ));

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = std::io::empty();
    let opening = standing(&sample);
    run(
        &id,
        &mut renderer,
        &mut conversation,
        &mut lent(&mut input, &opening),
        &terms,
    )
    .expect("the terminal to be written");

    let tasks = terms.plan.tasks();
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(
        tasks.first().map(crucible_builtins::Task::said),
        Some("Write the contributor guide")
    );
}

#[test]
fn the_tools_looked_up_by_the_session_being_left_are_forgotten() {
    // They belong to the conversation that looked them up — the same reason
    // `/clear` forgets them: left standing they would be advertised to a
    // session that never asked.
    let sample = Sample::new("resume-forgets-the-lookups");
    let earlier = recorded(&sample, "what was asked before");
    let id = named(&earlier);
    drop(earlier);

    let session = Arc::new(Session::nowhere());
    let mut conversation = over(&session);
    let terms = terms(&sample);
    terms.revealed.reveal("web_search");
    assert!(terms.revealed.holds("web_search"));

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = std::io::empty();
    let opening = standing(&sample);
    run(
        &id,
        &mut renderer,
        &mut conversation,
        &mut lent(&mut input, &opening),
        &terms,
    )
    .expect("the terminal to be written");

    assert!(!terms.revealed.holds("web_search"));
}

#[test]
fn what_was_held_behind_rows_that_have_gone_is_dropped_with_them() {
    // A key opening what is behind a row nobody can see is the one thing worse
    // than not offering at all.
    let sample = Sample::new("resume-forgets");
    let earlier = recorded(&sample, "what was asked before");
    let id = named(&earlier);
    drop(earlier);

    let session = Arc::new(Session::nowhere());
    let mut conversation = over(&session);
    let mut renderer = Renderer::new(Recording::new(80, 24));

    let call = ToolId::new("a call of the session being left");
    let mut input = std::io::empty();
    let opening = standing(&sample);
    let mut held = lent(&mut input, &opening);
    held.kept.calling(call.clone(), "read a file".into());
    held.kept.finished(&call, "line\nline\nline".into(), 0);
    assert!(!held.kept.is_empty());

    run(
        &id,
        &mut renderer,
        &mut conversation,
        &mut held,
        &terms(&sample),
    )
    .expect("the terminal to be written");

    // The session picked up made no calls of its own, so anything left here is
    // the old session's.
    assert!(held.kept.is_empty());
}

#[test]
fn a_saved_title_outlives_the_picker_and_the_session() {
    // A rename is written into the index rather than held on the frame, so it
    // has to still be there after the picker is gone — and after the session
    // has been continued and finished, which rewrites the index entry.
    let sample = Sample::new("resume-retitle");
    let session = recorded(&sample, "the first question");
    let id = session.id().expect("a recorded session has a name").clone();
    drop(session);
    drop(on_the_list(&sample, &id));

    let listed = saved("a better name", &id, &sample.logs());
    let found = listed
        .iter()
        .find(|session| session.id() == &id)
        .expect("the renamed session stays on the list");
    assert_eq!(found.title(), "a better name");

    let (reopened, _) = Session::reopen(&sample.logs(), &sample.workspace(), &id)
        .expect("the session the id names");
    reopened.append(&Message::said("carried on"));
    drop(reopened);

    let found = on_the_list(&sample, &id);
    assert_eq!(found.title(), "a better name");
}

#[test]
fn the_preview_holds_the_work_a_session_did_and_not_only_what_was_said() {
    // A conversation is its tool work as much as its answers, and the pane is
    // showing what Enter would leave the reader looking at — so the call line
    // and the row its result came back on are in it, drawn by whatever draws
    // them live.
    let sample = Sample::new("resume-preview-work");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let id = session.id().expect("a recorded session has a name").clone();
    let call = ToolId::new("c-1");

    session.append(&Message::said("read the config"));
    session.append(&Message::Agent {
        continuation: None,
        text: "I will look at it.".into(),
        calls: vec![ToolCall {
            id: call.clone(),
            name: "read".into(),
            args: ToolArgs::new(r#"{"path":"crucible.json"}"#),
        }],
        stop: Some(StopReason::WantsTools),
    });
    session.append(&Message::ToolResults(vec![ToolResult {
        id: call,
        output: RecordedToolOutput::ok("theme = midnight"),
    }]));
    drop(session);

    let held = glimpse(&sample.logs(), &sample.workspace(), &id).expect("a finished log");
    let runner = runner(&Arc::new(recorded(&sample, "another session entirely")));
    let against = replaying::Replay {
        runner: &runner,
        pruned: &Pruned::default(),
        style: Style::plain(),
    };
    let rows = previewed(
        &held,
        &against,
        Picker::previewing(100).expect("a window this wide keeps the pane"),
    );

    let drawn = rows.iter().map(Row::text).collect::<Vec<_>>().join("\n");
    assert!(drawn.contains("read the config"), "{drawn}");
    assert!(drawn.contains("I will look at it."), "{drawn}");
    assert!(
        drawn.contains("Read"),
        "no call line in the preview: {drawn}"
    );
    assert!(drawn.contains("theme = midnight"), "{drawn}");
}

#[test]
fn a_preview_is_drawn_for_the_pane_the_window_leaves_it() {
    // The pane's width is the reader's to change under it, so the rows are
    // drawn against whatever it is now rather than against whatever it was
    // when the session was first looked at.
    let sample = Sample::new("resume-preview-width");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let id = session.id().expect("a recorded session has a name").clone();
    session.append(&Message::said(
        "a question long enough that no narrow pane holds it on one row at all",
    ));
    drop(session);

    let held = glimpse(&sample.logs(), &sample.workspace(), &id).expect("a finished log");
    let runner = runner(&Arc::new(recorded(&sample, "another session entirely")));

    for columns in [Picker::FOLDS_AT, 100, 160] {
        let room = Picker::previewing(columns).expect("a window this wide keeps the pane");
        let against = replaying::Replay {
            runner: &runner,
            pruned: &Pruned::default(),
            style: Style::plain(),
        };
        let rows = previewed(&held, &against, room);
        assert!(!rows.is_empty(), "nothing drawn at {columns} columns");
        for row in &rows {
            assert!(
                crucible_tui::columns(&row.text()) <= room,
                "a row wider than the pane at {columns} columns: {:?}",
                row.text()
            );
        }
    }
}

#[test]
fn wheeling_the_preview_back_never_empties_the_pane() {
    // The pane shows the end of the slice it is handed, so a window allowed
    // to shrink past the pane is a pane going blank under a reader who is
    // only wheeling back through a tail that has more.
    let room = 30;
    let shows = Picker::previews(room, 0);
    assert!(shows > 0, "a window this tall keeps the pane");

    let behind = furthest(shows + 12, room, 0);
    assert_eq!(
        shows + 12 - behind,
        shows,
        "the pane stands short of full at its furthest back"
    );

    // A tail no longer than the pane has nothing to wheel back through.
    assert_eq!(furthest(shows, room, 0), 0);
    assert_eq!(furthest(shows / 2, room, 0), 0);

    // A notice of two rows takes one from the pane, and the pane's floor
    // moves with it.
    assert_eq!(furthest(shows + 12, room, 2), 13);
}

#[test]
fn the_picker_says_the_words_it_was_drawn_to_say() {
    // The component draws whatever words it is handed, and its own tests hand
    // it the design's. These are the ones a reader gets, so they are asserted
    // where they are written down rather than where they are drawn. The
    // heading and the keys row change with the keys, and have tests of their
    // own below.
    assert_eq!(HINT, "a session, or a branch");
    assert_eq!(NOVIEW, "nothing to show");
    assert_eq!(TAKES, "Enter to resume · Esc to cancel");
    assert_eq!(NEVER, "no earlier session for this workspace");
    assert_eq!(CUT, "the rest could not be read");

    assert_eq!(nothing("deploy", None), "no session holds \"deploy\"");
    assert_eq!(
        nothing("deploy", Some("main")),
        "no session holds \"deploy\""
    );

    // With nothing typed, what emptied the list is the branch Ctrl+B keeps.
    assert_eq!(nothing("", Some("main")), "no session on main");
    // Spelled as the heading spells it: a branch is the checkout's to name,
    // and a control character in it is drawn as the space the heading shows.
    assert_eq!(nothing("", Some("fix\tlogin")), "no session on fix login");

    // Or nothing was ever recorded here, while something was elsewhere.
    assert_eq!(nothing("", None), NEVER);
}

#[test]
fn a_rename_says_what_its_own_keys_do_and_not_the_list_s() {
    // The one row on screen that says what the keys do, while the keys have
    // all changed underneath it: none of walking, renaming or searching is
    // what a key does with a title open, and a row that went on offering them
    // is the picker disagreeing with itself about the mode the reader is in.
    let glyphs = Glyphs::Unicode;

    assert_eq!(
        renaming(glyphs),
        ["enter to save · esc to cancel", "enter · esc"]
    );
}

#[test]
fn the_meta_line_counts_the_messages_and_names_the_branch() {
    // One line under the preview: age, count and branch. The count is spelled
    // singular where it is one, because "1 messages" is the kind of line that
    // says nobody read it.
    let sample = Sample::new("resume-meta");
    let session = Session::start(&sample.logs(), &sample.workspace(), Some("feature/x"))
        .expect("a new session");
    let id = session.id().expect("a recorded session has a name").clone();
    session.append(&Message::said("the only thing said"));
    drop(session);

    let listed = on_the_list(&sample, &id);
    let held = glimpse(&sample.logs(), &sample.workspace(), &id).expect("a finished log");
    assert!(!held.busy());

    let said = meta(
        &listed,
        Some(&held),
        SystemTime::now(),
        Style::plain().glyphs(),
    );
    assert!(said.contains("just now"), "{said}");
    assert!(said.contains("1 message"), "{said}");
    assert!(!said.contains("1 messages"), "{said}");
    assert!(said.contains("feature/x"), "{said}");
    assert!(!said.contains("in use elsewhere"), "{said}");
}

#[test]
fn a_session_the_index_holds_no_count_for_says_nothing_about_one() {
    // The count is written down when a session ends, so a session recorded
    // before there were counts — or one still being written — has none in the
    // index. "0 messages" under a preview full of them is a lie about the
    // session, where the rest of the line is not.
    let sample = Sample::new("resume-uncounted");
    let session =
        Session::start(&sample.logs(), &sample.workspace(), Some("main")).expect("a new session");
    let id = session.id().expect("a recorded session has a name").clone();
    session.append(&Message::said("something was said"));

    let listed = on_the_list(&sample, &id);
    assert_eq!(listed.messages(), 0, "the count is written at the end");

    let said = meta(&listed, None, SystemTime::now(), Style::plain().glyphs());
    assert!(!said.contains("message"), "{said}");
    assert!(said.contains("just now"), "{said}");
    assert!(said.contains("main"), "{said}");
}

#[test]
fn a_session_another_crucible_holds_open_is_said_to_be_in_use() {
    // Answered inline on the meta line rather than as a refusal: the reader
    // finds out while they are looking at the row, before Enter has closed the
    // picker over a session that would refuse to open.
    let sample = Sample::new("resume-busy");
    let open = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let id = open.id().expect("a recorded session has a name").clone();
    open.append(&Message::said("held open elsewhere"));

    let listed = on_the_list(&sample, &id);
    let held = glimpse(&sample.logs(), &sample.workspace(), &id).expect("a claimed log");
    assert!(held.busy());

    let said = meta(
        &listed,
        Some(&held),
        SystemTime::now(),
        Style::plain().glyphs(),
    );
    assert!(said.contains("in use elsewhere"), "{said}");

    drop(open);
    let held = glimpse(&sample.logs(), &sample.workspace(), &id).expect("a finished log");
    assert!(!held.busy());

    let said = meta(
        &on_the_list(&sample, &id),
        Some(&held),
        SystemTime::now(),
        Style::plain().glyphs(),
    );
    assert!(!said.contains("in use elsewhere"), "{said}");
}

/// A session recorded in `workspace` on `branch` and closed again: asked
/// `asked`, or, with nothing asked, opened and left — a header and nothing
/// under it, which is what starting crucible and quitting leaves behind.
fn recorded_in(
    sample: &Sample,
    workspace: &Workspace,
    branch: Option<&str>,
    asked: Option<&str>,
) -> SessionId {
    let session = Session::start(&sample.logs(), workspace, branch).expect("a new session");
    if let Some(asked) = asked {
        session.append(&Message::said(asked));
    }
    let id = session.id().expect("a recorded session has a name").clone();
    drop(session);
    id
}

/// A directory beside the sample's workspace, opened as one.
fn beside(sample: &Sample, name: &str) -> Workspace {
    let root = sample.root().with_file_name(name);
    std::fs::create_dir_all(&root).expect("a directory beside the workspace");
    Workspace::open(root).expect("the directory exists")
}

/// The sample's workspace as the picker stands in it, with `others` as its
/// repository's other checkouts and `branch` checked out.
fn standing_in(sample: &Sample, others: &[&Workspace], branch: Option<&str>) -> Here {
    Here {
        root: sample.workspace().root().to_path_buf(),
        others: others
            .iter()
            .map(|other| other.root().to_path_buf())
            .collect(),
        branch: branch.map(str::to_owned),
        home: None,
    }
}

/// What the sessions `scope` leaves were asked, sorted: which are shown is
/// the question, and the order is `recent`'s, proven there.
fn shown_under(listed: &[Recorded], scope: Scope, here: &Here) -> Vec<String> {
    let mut titles: Vec<String> = chosen(listed, &scoped(listed, scope, here))
        .into_iter()
        .map(|session| session.title().to_owned())
        .collect();
    titles.sort();
    titles
}

/// A path under the root of this platform's paths, a part at a time.
fn under(parts: &[&str]) -> PathBuf {
    parts.iter().fold(
        PathBuf::from(std::path::MAIN_SEPARATOR_STR),
        |path, part| path.join(part),
    )
}

#[test]
fn the_picker_looks_through_the_whole_index_and_offers_a_hundred() {
    // The case the list was too short for: a directory whose newest logs are
    // other directories' sessions and sessions nothing was asked in. Looking
    // only as far as the welcome screen does, this directory's would be cut
    // to the few left among them.
    let sample = Sample::new("resume-reach");
    let elsewhere = beside(&sample, "elsewhere");
    for at in 0..110 {
        recorded_in(
            &sample,
            &sample.workspace(),
            None,
            Some(&format!("here {at}")),
        );
    }
    for at in 0..30 {
        recorded_in(&sample, &elsewhere, None, Some(&format!("elsewhere {at}")));
    }
    for _ in 0..30 {
        recorded_in(&sample, &sample.workspace(), None, None);
    }

    let listed = scanned(&sample.logs());
    let here = standing_in(&sample, &[], None);
    let offered = chosen(&listed, &scoped(&listed, Scope::default(), &here));

    assert_eq!(offered.len(), OFFERED);
    assert_eq!(OFFERED, 100);
    assert!(
        offered
            .iter()
            .all(|session| session.title().starts_with("here ")),
        "{:?}",
        offered
            .iter()
            .map(|session| session.title())
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_session_nothing_was_asked_in_is_listed_under_no_key() {
    // Somebody who started crucible and left asked nothing to come back to,
    // in this directory or any other, on this branch or any other.
    let sample = Sample::new("resume-unasked");
    let checkout = beside(&sample, "checkout");
    let elsewhere = beside(&sample, "elsewhere");
    for workspace in [&sample.workspace(), &checkout, &elsewhere] {
        recorded_in(&sample, workspace, Some("main"), Some("asked"));
        recorded_in(&sample, workspace, Some("main"), None);
    }

    let listed = scanned(&sample.logs());
    let here = standing_in(&sample, &[&checkout], Some("main"));
    for all in [false, true] {
        for worktrees in [false, true] {
            for branch in [false, true] {
                let scope = Scope {
                    all,
                    worktrees,
                    branch,
                };
                let shown = shown_under(&listed, scope, &here);
                assert!(!shown.is_empty(), "{scope:?}");
                assert!(shown.iter().all(|title| title == "asked"), "{scope:?}");
            }
        }
    }
}

#[test]
fn another_directory_is_listed_only_while_ctrl_a_shows_every_project() {
    let sample = Sample::new("resume-all");
    let elsewhere = beside(&sample, "elsewhere");
    recorded_in(&sample, &sample.workspace(), None, Some("here"));
    recorded_in(&sample, &elsewhere, None, Some("elsewhere"));

    let listed = scanned(&sample.logs());
    let here = standing_in(&sample, &[], None);

    assert_eq!(shown_under(&listed, Scope::default(), &here), ["here"]);
    let all = Scope {
        all: true,
        ..Scope::default()
    };
    assert_eq!(shown_under(&listed, all, &here), ["elsewhere", "here"]);
}

#[test]
fn another_checkout_of_this_repository_is_listed_while_ctrl_w_adds_them() {
    let sample = Sample::new("resume-worktrees");
    let checkout = beside(&sample, "checkout");
    let elsewhere = beside(&sample, "elsewhere");
    recorded_in(&sample, &sample.workspace(), None, Some("here"));
    recorded_in(&sample, &checkout, None, Some("a checkout"));
    recorded_in(&sample, &elsewhere, None, Some("elsewhere"));

    let listed = scanned(&sample.logs());
    let here = standing_in(&sample, &[&checkout], None);

    assert_eq!(shown_under(&listed, Scope::default(), &here), ["here"]);
    let worktrees = Scope {
        worktrees: true,
        ..Scope::default()
    };
    assert_eq!(
        shown_under(&listed, worktrees, &here),
        ["a checkout", "here"]
    );
}

#[test]
fn ctrl_b_keeps_only_the_branch_checked_out_here() {
    let sample = Sample::new("resume-branch");
    let elsewhere = beside(&sample, "elsewhere");
    recorded_in(&sample, &sample.workspace(), Some("main"), Some("on main"));
    recorded_in(
        &sample,
        &sample.workspace(),
        Some("feature"),
        Some("on feature"),
    );
    recorded_in(&sample, &sample.workspace(), None, Some("on no branch"));
    recorded_in(&sample, &elsewhere, Some("main"), Some("main elsewhere"));

    let listed = scanned(&sample.logs());
    let here = standing_in(&sample, &[], Some("main"));

    let branch = Scope {
        branch: true,
        ..Scope::default()
    };
    assert_eq!(shown_under(&listed, branch, &here), ["on main"]);
    let everywhere = Scope {
        all: true,
        branch: true,
        ..Scope::default()
    };
    assert_eq!(
        shown_under(&listed, everywhere, &here),
        ["main elsewhere", "on main"]
    );
}

#[test]
fn each_key_flips_its_own_scope_and_ctrl_b_waits_for_a_branch() {
    let sample = Sample::new("resume-keys");
    let mut stood = Stood::opened(Vec::new());
    let unbranched = standing_in(&sample, &[], None);

    // Nothing checked out: Ctrl+B has nothing to keep, so it does nothing.
    assert_eq!(
        pressed(Pressed::Background, &mut stood, &unbranched),
        Moved::Still
    );
    assert_eq!(stood.scope, Scope::default());

    assert_eq!(
        pressed(Pressed::All, &mut stood, &unbranched),
        Moved::Redraw
    );
    assert!(stood.scope.all);
    assert_eq!(
        pressed(Pressed::Key(Key::WordErase), &mut stood, &unbranched),
        Moved::Redraw
    );
    assert!(stood.scope.worktrees);

    // Ctrl+A again goes back to what was shown before it: the other
    // checkouts, which Ctrl+W added under it.
    pressed(Pressed::All, &mut stood, &unbranched);
    assert_eq!(
        stood.scope,
        Scope {
            worktrees: true,
            ..Scope::default()
        }
    );

    let branched = standing_in(&sample, &[], Some("main"));
    assert_eq!(
        pressed(Pressed::Background, &mut stood, &branched),
        Moved::Redraw
    );
    assert!(stood.scope.branch);

    // Backspace held rubs a word out of the search line, as it always did.
    stood.standing.query.put("two words");
    pressed(Pressed::Key(Key::RubWord), &mut stood, &branched);
    assert_eq!(stood.standing.query.text(), "two ");
    assert!(stood.scope.worktrees);
}

#[test]
fn the_heading_names_what_the_keys_show() {
    let glyphs = Glyphs::Unicode;
    let here = Here {
        root: under(&["home", "ada", "code", "crucible"]),
        others: Vec::new(),
        branch: Some("main".to_owned()),
        home: Some(under(&["home", "ada"])),
    };
    let sep = std::path::MAIN_SEPARATOR;
    let all = Scope {
        all: true,
        ..Scope::default()
    };
    let worktrees = Scope {
        worktrees: true,
        ..Scope::default()
    };
    let branch = Scope {
        branch: true,
        ..Scope::default()
    };

    assert_eq!(
        heading(3, 5, Scope::default(), &here, glyphs),
        format!("Resume a session · 3 of 5 · ~{sep}code{sep}crucible")
    );
    assert_eq!(
        heading(3, 5, all, &here, glyphs),
        "Resume a session · 3 of 5 · all projects"
    );
    assert_eq!(
        heading(3, 5, worktrees, &here, glyphs),
        "Resume a session · 3 of 5 · this repository's worktrees"
    );
    assert_eq!(
        heading(3, 5, branch, &here, glyphs),
        format!("Resume a session · 3 of 5 · main · ~{sep}code{sep}crucible")
    );
    assert_eq!(
        heading(
            3,
            5,
            Scope {
                all: true,
                ..branch
            },
            &here,
            glyphs
        ),
        "Resume a session · 3 of 5 · main · all projects"
    );

    // The branch goes before the directory, so a directory too long for the
    // row is what the window cuts, and the branch Ctrl+B keeps stays read.
    let deep = Here {
        root: under(&[
            "home",
            "ada",
            "a-directory-whose-name-is-long",
            "and-longer-below-it",
        ]),
        ..here
    };
    let said = heading(1, 1, branch, &deep, glyphs);
    let drawn = crucible_tui::clip(&said, 40);
    assert!(
        drawn.starts_with("Resume a session · 1 of 1 · main · ~"),
        "{drawn}"
    );
}

#[test]
fn the_keys_row_names_what_each_key_does_next() {
    // Longest first: every key in words, then each toggle by its next effect
    // with the keys that explain themselves dropped, then the same again
    // without Ctrl+R, then the bare keys a narrow window is left with.
    let glyphs = Glyphs::Unicode;
    let all = Scope {
        all: true,
        worktrees: true,
        branch: true,
    };

    assert_eq!(
        keys(glyphs, true, Scope::default(), true),
        [
            "↑↓ to walk · ctrl+r to rename · ctrl+a to show all projects · \
             ctrl+b to only show this branch · ctrl+w to show all worktrees · \
             type to search · esc to cancel",
            "ctrl+r rename · ctrl+a all projects · ctrl+b this branch · \
             ctrl+w worktrees · esc",
            "ctrl+a all projects · ctrl+b this branch · ctrl+w worktrees · esc",
            "↑↓ · enter · ctrl+r · ctrl+a · ctrl+b · ctrl+w · esc",
        ]
    );
    assert_eq!(
        keys(glyphs, true, all, true),
        [
            "↑↓ to walk · ctrl+r to rename · ctrl+a to show this project · \
             ctrl+b to show all branches · ctrl+w to hide other worktrees · \
             type to search · esc to cancel",
            "ctrl+r rename · ctrl+a this project · ctrl+b all branches · \
             ctrl+w hide worktrees · esc",
            "ctrl+a this project · ctrl+b all branches · ctrl+w hide worktrees · esc",
            "↑↓ · enter · ctrl+r · ctrl+a · ctrl+b · ctrl+w · esc",
        ]
    );

    // No branch checked out: Ctrl+B would do nothing, so it is not offered.
    assert_eq!(
        keys(glyphs, true, Scope::default(), false),
        [
            "↑↓ to walk · ctrl+r to rename · ctrl+a to show all projects · \
             ctrl+w to show all worktrees · type to search · esc to cancel",
            "ctrl+r rename · ctrl+a all projects · ctrl+w worktrees · esc",
            "ctrl+a all projects · ctrl+w worktrees · esc",
            "↑↓ · enter · ctrl+r · ctrl+a · ctrl+w · esc",
        ]
    );

    // With nothing on the list there is nothing to walk to and nothing to
    // rename: what is left to do is narrow the query, change what the keys
    // show, or leave.
    assert_eq!(
        keys(glyphs, false, Scope::default(), false),
        [
            "type to narrow · ctrl+a to show all projects · \
             ctrl+w to show all worktrees · esc to cancel",
            "ctrl+a all projects · ctrl+w worktrees · esc",
            "type to narrow · ctrl+a · ctrl+w · esc",
        ]
    );
}

/// The keys row a picker handed `forms` draws across `columns`.
fn drawn_keys(forms: &[String], columns: usize) -> String {
    let forms: Vec<&str> = forms.iter().map(String::as_str).collect();
    let picker = crucible_tui::Picker {
        heading: "Resume a session",
        query: "",
        typed: 0,
        hint: HINT,
        sessions: &[],
        marked: 0,
        renaming: None,
        refused: None,
        preview: &[],
        preview_meta: "",
        takes: TAKES,
        nothing: NEVER,
        noview: NOVIEW,
        keys: &forms,
        notice: &[],
        pointer: None,
    };
    let rows = picker.within(columns, 30, Glyphs::Unicode);
    rows.last()
        .map(Row::text)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[test]
fn at_eighty_columns_each_toggle_is_named_by_what_it_does_next() {
    // Eighty columns is the window most people open, and a keys row there that
    // names only keys leaves the reader to press each one to find out. With a
    // branch checked out the row is at its longest, and in every scope each
    // toggle still says what it does next.
    for all in [false, true] {
        for worktrees in [false, true] {
            for branch in [false, true] {
                let scope = Scope {
                    all,
                    worktrees,
                    branch,
                };
                let a = if all {
                    "ctrl+a this project"
                } else {
                    "ctrl+a all projects"
                };
                let b = if branch {
                    "ctrl+b all branches"
                } else {
                    "ctrl+b this branch"
                };
                let w = if worktrees {
                    "ctrl+w hide worktrees"
                } else {
                    "ctrl+w worktrees"
                };
                assert_eq!(
                    drawn_keys(&keys(Glyphs::Unicode, true, scope, true), 80),
                    format!("{a} · {b} · {w} · esc"),
                    "{scope:?}"
                );
            }
        }
    }

    // Without a branch there is room for Ctrl+R as well.
    assert_eq!(
        drawn_keys(&keys(Glyphs::Unicode, true, Scope::default(), false), 80),
        "ctrl+r rename · ctrl+a all projects · ctrl+w worktrees · esc"
    );

    // A narrower window still gets every key, by name alone.
    assert_eq!(
        drawn_keys(&keys(Glyphs::Unicode, true, Scope::default(), true), 60),
        "↑↓ · enter · ctrl+r · ctrl+a · ctrl+b · ctrl+w · esc"
    );
}

#[test]
fn enter_on_a_session_recorded_elsewhere_says_how_to_resume_it_there() {
    let sample = Sample::new("resume-elsewhere");
    let yonder = beside(&sample, "elsewhere");
    let away = recorded_in(&sample, &yonder, None, Some("away"));
    let home = recorded_in(&sample, &sample.workspace(), None, Some("home"));

    let here = standing_in(&sample, &[], None);
    let mut stood = Stood::opened(scanned(&sample.logs()));
    stood.scope.all = true;
    stood.standing.found = scoped(&stood.listed, stood.scope, &here);
    let at = |id: &SessionId, stood: &Stood| {
        stood
            .standing
            .found
            .iter()
            .position(|&at| stood.listed.get(at).map(Recorded::id) == Some(id))
            .expect("listed")
    };

    // Taken nowhere: the picker stays on the same row, and the row under the
    // list says the command that picks it up in its own directory.
    stood.standing.marked = at(&away, &stood);
    let marked = stood.standing.marked;
    assert_eq!(
        pressed(Pressed::Key(Key::Enter), &mut stood, &here),
        Moved::Redraw
    );
    assert_eq!(stood.standing.marked, marked);
    assert_eq!(stood.told.as_ref(), Some(&away));
    // Written as the command writes it: as typed, without the `\\?\` that
    // resolving put on the root on Windows.
    let root = crucible_workspace::typed(yonder.root())
        .display()
        .to_string();
    let resume = format!("crucible --resume {}", away.as_str());
    #[cfg(not(windows))]
    let (columns, said) = {
        let command = format!("cd {root} && {resume}");
        (wide(&command) + 2, vec![command])
    };
    #[cfg(windows)]
    let (columns, said) = (400, windows_said(&root, &resume, 400, Glyphs::Unicode));
    assert_eq!(
        elsewhere(
            stood.marked().expect("marked"),
            None,
            columns,
            Glyphs::Unicode
        ),
        said
    );

    // The foot under its preview says what Enter does there, which is not
    // what it does on a session of this directory.
    let marked = stood.marked().expect("marked");
    assert_eq!(
        taken(marked, sample.workspace().root()),
        "Enter to see how to resume · Esc to cancel"
    );

    // The next key takes it down.
    pressed(Pressed::Key(Key::End), &mut stood, &here);
    assert_eq!(stood.told, None);

    // A session of this directory is taken as it always was.
    stood.standing.marked = at(&home, &stood);
    assert_eq!(
        taken(stood.marked().expect("marked"), sample.workspace().root()),
        TAKES
    );
    assert_eq!(
        pressed(Pressed::Key(Key::Enter), &mut stood, &here),
        Moved::Took
    );
    assert_eq!(stood.told, None);
}

#[cfg(not(windows))]
#[test]
fn the_command_to_resume_elsewhere_breaks_after_its_and_and_never_cuts_the_id() {
    // A real id, as long as one ever is, and a directory longer than the
    // window: at eighty columns the command takes two rows, broken where a
    // shell carries on reading, and only the directory is cut.
    let sample = Sample::new("resume-broken");
    let long = "a-directory-whose-name-runs-past-half-the-window";
    let yonder = beside(&sample, long);
    let away = recorded_in(&sample, &yonder, None, Some("away"));
    let listed = scanned(&sample.logs());
    let session = listed
        .iter()
        .find(|session| session.id() == &away)
        .expect("listed");
    assert_eq!(away.as_str().len(), 36, "{away:?}");

    let rows = elsewhere(session, None, 80, Glyphs::Unicode);
    let root = crucible_workspace::typed(yonder.root());
    assert_eq!(rows.len(), 2, "{rows:?}");
    let (first, second) = (rows.first().expect("one"), rows.get(1).expect("two"));
    assert!(
        wide(&format!("cd {} &&", root.display())) > 78,
        "the directory alone is wider than the window"
    );
    assert!(first.starts_with("cd "), "{first:?}");
    assert!(first.ends_with(" &&"), "{first:?}");
    assert!(first.contains('…'), "the cut is said: {first:?}");
    assert!(first.ends_with(&format!("{long} &&")), "{first:?}");
    assert_eq!(second, &format!("crucible --resume {}", away.as_str()));

    // Drawn, every row is whole on an eighty-column window.
    for row in &rows {
        assert!(wide(row) <= 78, "{row:?}");
    }
    let notice: Vec<&str> = rows.iter().map(String::as_str).collect();
    let picker = crucible_tui::Picker {
        heading: "Resume a session",
        query: "",
        typed: 0,
        hint: HINT,
        sessions: &[],
        marked: 0,
        renaming: None,
        refused: None,
        preview: &[],
        preview_meta: "",
        takes: TAKES,
        nothing: NEVER,
        noview: NOVIEW,
        keys: &["esc"],
        notice: &notice,
        pointer: None,
    };
    let drawn: Vec<String> = picker
        .within(80, 24, Glyphs::Unicode)
        .iter()
        .map(Row::text)
        .collect();
    assert!(
        drawn
            .iter()
            .any(|row| row.trim() == format!("crucible --resume {}", away.as_str())),
        "{drawn:#?}"
    );

    // Where the whole command fits one row, it is one row.
    let short = elsewhere(session, None, 400, Glyphs::Unicode);
    assert_eq!(
        short,
        [format!(
            "cd {} && crucible --resume {}",
            root.display(),
            away.as_str()
        )]
    );
}

#[test]
fn a_directory_is_written_under_home_and_quoted_where_a_shell_needs_it() {
    let home = under(&["home", "ada"]);
    let sep = std::path::MAIN_SEPARATOR;

    assert_eq!(
        homed(&home.join("code"), Some(&home)),
        format!("~{sep}code")
    );
    assert_eq!(homed(&home, Some(&home)), "~");
    assert_eq!(
        homed(&under(&["srv", "code"]), Some(&home)),
        format!("{sep}srv{sep}code")
    );
    assert_eq!(
        homed(&home.join("code"), None),
        format!("{sep}home{sep}ada{sep}code")
    );
    // What a terminal would act on is not drawn.
    assert_eq!(
        homed(&home.join("a\u{1b}b"), Some(&home)),
        format!("~{sep}a b")
    );
}

#[test]
fn a_posix_shell_reads_the_directory_back_whole() {
    assert_eq!(posix_quoted("~/code/crucible-code"), "~/code/crucible-code");
    assert_eq!(
        posix_quoted("/srv/x_y@1.2+3=4:5,6%7"),
        "/srv/x_y@1.2+3=4:5,6%7"
    );
    assert_eq!(posix_quoted("~/my code"), "~/'my code'");
    assert_eq!(posix_quoted("/srv/it's"), r"'/srv/it'\''s'");
    assert_eq!(posix_quoted("~"), "~");
    assert_eq!(posix_quoted("/a;rm -rf b"), "'/a;rm -rf b'");
}

#[cfg(not(windows))]
#[test]
fn a_posix_shell_is_handed_a_directory_under_home() {
    // Under home, where a POSIX shell expands `~` back.
    let home = under(&["home", "ada"]);
    assert_eq!(commanded(&home.join("my code"), Some(&home)), "~/'my code'");
}

const AWAY: &str = "019854c2-9a1e-73f1-b0d6-2f1c4e7a58d1";

/// What Windows is told for a session recorded in `place`, across `columns`.
fn on_windows(place: &str, columns: usize) -> Vec<String> {
    windows_said(
        place,
        &format!("crucible --resume {AWAY}"),
        columns,
        Glyphs::Unicode,
    )
}

#[test]
fn windows_is_told_a_row_for_cmd_and_a_row_for_powershell() {
    assert_eq!(
        on_windows(r"D:\code\website", 120),
        [
            "cmd".to_owned(),
            r"pushd D:\code\website".to_owned(),
            "PowerShell".to_owned(),
            r"Set-Location -LiteralPath D:\code\website".to_owned(),
            "then".to_owned(),
            format!("crucible --resume {AWAY}"),
        ]
    );
}

#[test]
fn no_windows_row_joins_two_commands() {
    // `&&` is a parse error in Windows PowerShell 5.1, the one Windows ships,
    // and `;` separates nothing in cmd: each row is one command on its own.
    for place in [r"D:\code\website", r"D:\a b", r"D:\a&b", r"D:\a[1]"] {
        for columns in [200, 80, 56, 30] {
            let rows = on_windows(place, columns);
            for row in &rows {
                assert!(!row.contains("&&"), "{row:?}");
                assert!(!row.contains(';'), "{row:?}");
            }
            // A label is a row of its own, so a command row copied whole
            // carries nothing but the command into the shell.
            for pair in rows.chunks(2) {
                let [label, command] = pair else {
                    panic!("a label without its command: {rows:?}");
                };
                assert!(
                    ["cmd", "PowerShell", "then"].contains(&label.as_str()),
                    "{rows:?}"
                );
                assert!(
                    ["pushd ", "Set-Location -LiteralPath ", "crucible --resume "]
                        .iter()
                        .any(|verb| command.starts_with(verb)),
                    "{rows:?}"
                );
            }
        }
    }
}

#[test]
fn cmd_changes_drive_and_powershell_reads_no_wildcard() {
    // cmd's `cd` stays on the current drive; `pushd` moves to the
    // directory's. PowerShell's `cd` reads `[` and `]` as a wildcard, which
    // `-LiteralPath` does not.
    assert_eq!(
        on_windows(r"D:\a b[1]", 120),
        [
            "cmd".to_owned(),
            r#"pushd "D:\a b[1]""#.to_owned(),
            "PowerShell".to_owned(),
            r"Set-Location -LiteralPath 'D:\a b[1]'".to_owned(),
            "then".to_owned(),
            format!("crucible --resume {AWAY}"),
        ]
    );
}

#[test]
fn neither_shell_expands_anything_in_the_directory() {
    // Inside cmd's double quotes `$` and a backtick are plain; inside
    // PowerShell's single quotes nothing is special but the quote.
    assert_eq!(
        on_windows(r"D:\a$HOME`n", 120),
        [
            "cmd".to_owned(),
            r#"pushd "D:\a$HOME`n""#.to_owned(),
            "PowerShell".to_owned(),
            r"Set-Location -LiteralPath 'D:\a$HOME`n'".to_owned(),
            "then".to_owned(),
            format!("crucible --resume {AWAY}"),
        ]
    );
    // cmd expands `%NAME%` even inside double quotes, so it is given no row
    // for a directory that holds a `%`.
    assert_eq!(
        on_windows(r"D:\a%PATH%b", 120),
        [
            "PowerShell".to_owned(),
            r"Set-Location -LiteralPath 'D:\a%PATH%b'".to_owned(),
            "then".to_owned(),
            format!("crucible --resume {AWAY}"),
        ]
    );
    // PowerShell ends its single quotes at a typographic one too; each is
    // doubled, which it reads back as one.
    assert_eq!(
        on_windows("D:\\it's \u{2019}x\u{2018}", 120),
        [
            "cmd".to_owned(),
            "pushd \"D:\\it's \u{2019}x\u{2018}\"".to_owned(),
            "PowerShell".to_owned(),
            "Set-Location -LiteralPath 'D:\\it''s \u{2019}\u{2019}x\u{2018}\u{2018}'".to_owned(),
            "then".to_owned(),
            format!("crucible --resume {AWAY}"),
        ]
    );
    // A double quote cannot be in a Windows name, and would end cmd's
    // quoting: cmd is not given it. PowerShell's single quotes hold it as it is.
    assert_eq!(
        on_windows(r#"D:\a" & del b"#, 120),
        [
            "cmd".to_owned(),
            r#"pushd "D:\a & del b""#.to_owned(),
            "PowerShell".to_owned(),
            r#"Set-Location -LiteralPath 'D:\a" & del b'"#.to_owned(),
            "then".to_owned(),
            format!("crucible --resume {AWAY}"),
        ]
    );
}

#[test]
fn every_label_has_a_row_of_its_own_and_the_id_stays_whole() {
    assert_eq!(
        on_windows(r"D:\code\website", 56),
        [
            "cmd".to_owned(),
            r"pushd D:\code\website".to_owned(),
            "PowerShell".to_owned(),
            r"Set-Location -LiteralPath D:\code\website".to_owned(),
            "then".to_owned(),
            format!("crucible --resume {AWAY}"),
        ]
    );

    // A directory too long for its row loses its front, and says so.
    let long = r"D:\a-directory-whose-name-runs-past-half-the-window\website";
    let rows = on_windows(long, 56);
    assert!(rows.iter().all(|row| wide(row) <= 54), "{rows:#?}");
    assert!(
        rows.iter()
            .any(|row| row.starts_with("pushd …") && row.ends_with(r"\website")),
        "{rows:#?}"
    );
    assert!(
        rows.iter().any(|row| {
            row.starts_with("Set-Location -LiteralPath …") && row.ends_with(r"\website")
        }),
        "{rows:#?}"
    );
    assert!(
        rows.contains(&format!("crucible --resume {AWAY}")),
        "{rows:#?}"
    );
}

#[cfg(windows)]
#[test]
fn cmd_and_powershell_are_handed_the_directory_whole() {
    // Whole, never under `~`, which cmd reads as a directory of that name.
    let home = under(&["Users", "ada"]);
    assert_eq!(
        commanded(&home.join("my code"), Some(&home)),
        home.join("my code").display().to_string()
    );
}

#[cfg(windows)]
#[test]
fn a_resolved_directory_is_written_as_someone_types_it() {
    // A session's header keeps the spelling resolving gave, `\\?\C:\...`,
    // which cmd will not `cd` into and nobody recognises their project in.
    let home = Path::new(r"C:\Users\ada");
    let website = Path::new(r"\\?\C:\Users\ada\projects\website");

    assert_eq!(
        commanded(website, Some(home)),
        r"C:\Users\ada\projects\website"
    );
    assert_eq!(
        commanded(Path::new(r"\\?\UNC\server\share\x"), Some(home)),
        r"\\server\share\x"
    );
    // The row is written under home, which the resolved spelling never is.
    assert_eq!(homed(website, Some(home)), r"~\projects\website");
}

#[test]
fn a_search_finds_a_session_by_the_directory_its_row_shows() {
    let sample = Sample::new("resume-sought");
    let elsewhere = beside(&sample, "elsewhere");
    recorded_in(&sample, &elsewhere, None, Some("a question"));
    let listed = scanned(&sample.logs());
    let session = listed.first().expect("the session");

    assert!(sought(session, "~/elsewhere", "elsew"));
    // Its own directory's row shows none, so there is none to match.
    assert!(!sought(session, "", "elsew"));
    assert!(sought(session, "", "question"));
}

#[test]
fn a_rename_reads_back_every_directory_for_the_keys_to_narrow_again() {
    let sample = Sample::new("resume-rename-scope");
    let elsewhere = beside(&sample, "elsewhere");
    recorded_in(&sample, &elsewhere, None, Some("away"));
    let id = recorded_in(&sample, &sample.workspace(), None, Some("home"));

    let listed = saved("renamed", &id, &sample.logs());
    let here = standing_in(&sample, &[], None);
    let all = Scope {
        all: true,
        ..Scope::default()
    };

    assert_eq!(shown_under(&listed, all, &here), ["away", "renamed"]);
    assert_eq!(shown_under(&listed, Scope::default(), &here), ["renamed"]);
}

#[test]
fn the_card_a_resume_puts_back_is_drawn_with_the_glyphs_in_force_now() {
    // The card's facts were read at launch, but the characters it is drawn
    // with are the session's: somebody who switched to ascii because their
    // font has no box drawing would otherwise find the session they picked up
    // headed by the one set they cannot read.
    let sample = Sample::new("resume-draws-with-the-glyphs-now");
    let earlier = recorded(&sample, "what was asked before");
    let id = named(&earlier);
    drop(earlier);

    let terms = terms(&sample);
    // What the settings panel's Glyphs row does to a running session.
    terms
        .style
        .set(terms.style().drawing(crucible_tui::Glyphs::Ascii));

    let session = Arc::new(Session::nowhere());
    let mut conversation = over(&session);
    let mut renderer = Renderer::new(Recording::new(80, 24));
    renderer.draws(crucible_tui::Glyphs::Ascii);
    let mut input = std::io::empty();
    // Read at launch, when the session drew in unicode.
    let opening = standing(&sample);

    run(
        &id,
        &mut renderer,
        &mut conversation,
        &mut lent(&mut input, &opening),
        &terms,
    )
    .expect("the terminal to be written");

    let picture = renderer.terminal().picture().rows().join("\n");
    assert!(picture.contains("Tips"), "{picture}");
    assert!(picture.contains("what was asked before"), "{picture}");
    assert!(picture.is_ascii(), "{picture}");
}
