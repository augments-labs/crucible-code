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
use std::time::Duration;

use crucible_auth::Store;
use crucible_core::{
    AgentId, Cancel, Message, Revealed, SessionId, StopReason, ToolArgs, ToolCall, ToolId,
    ToolOutput, ToolResult,
};
use crucible_runner::{AgentSpec, Model, Runner, Tools};
use crucible_tools::{Ledger, Plan};
use crucible_tui::{Recording, Renderer, Row};

use crate::cli::converse::{Answers, Held};
use crate::cli::draw::opening::{Opening, Standing};
use crate::cli::fake::Script;
use crate::cli::sample::Sample;
use crate::cli::style::Style;

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

/// A runner that answers nothing, recording to `session`.
fn over(session: Session) -> Runner {
    Runner::new(
        Box::new(Script::new(Vec::new())),
        Tools::new(),
        AgentSpec::new(
            AgentId::new("test"),
            Model {
                name: "script".into(),
                max_tokens: 64,
                window: None,
                accepts: None,
                effort: None,
            },
        ),
        crucible_runner::ContextInputs::new(std::env::temp_dir()),
        session,
    )
}

fn terms(sample: &Sample) -> Terms {
    Terms {
        style: Cell::new(Style::plain()),
        chosen: Cell::new(None),
        reading: std::cell::RefCell::default(),
        cancel: Cancel::new(),
        steer: crucible_core::Steer::new(),
        aside: crucible_core::Aside::new(),
        ledger: Ledger::new(),
        revealed: Revealed::new(),
        plan: Plan::new(),
        putting: crate::cli::seen::Putting::new(),
        leaving: crucible_tools::Background::new(),
        provider: std::cell::Cell::new(Some("anthropic")),
        pending_model: std::cell::Cell::new(None),
        pending_mode: std::cell::Cell::new(None),
        settings: crucible_config::Settings::default(),
        choosing: sample.root().join("unwritten-home.json"),
        logins: Store::in_home(&sample.root()),
        subscriptions: crate::cli::subscription::Subscriptions::production(),

        // `/resume` never reaches it, and these terms have no provider to build
        // one from either — the loop they drive answers from a script.
        serving: Box::new(|named, _| {
            Err(Fatal::Provider {
                named: named.name.into(),
                has: named.name.into(),
            })
        }),
        sessions: sample.logs(),
        workspace: sample.workspace(),
        sending: crucible_tui::Sending::default(),
        commands: crate::cli::converse::command::builtins()
            .expect("the built-in commands register"),
        providers: crate::cli::providers().expect("the built-in providers register"),
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
        if let Some(found) = recent(&sample.logs(), &sample.workspace(), SHOWN)
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

/// Runs `/resume {said}` against `runner`, and says what the window ends up
/// showing — one row a line, the blank ones left out.
fn resuming(said: &str, sample: &Sample, runner: &mut Runner) -> String {
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = std::io::empty();
    let opening = standing(sample);

    run(
        said,
        &mut renderer,
        runner,
        &mut lent(&mut input, &opening),
        &terms(sample),
    )
    .expect("the terminal to be written");

    renderer.terminal().picture().said().join("\n")
}

#[test]
fn a_directory_nothing_was_recorded_in_says_so() {
    let sample = Sample::new("resume-empty");
    let mut runner = over(Session::nowhere());

    let written = resuming("", &sample, &mut runner);

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
    let mut runner = over(Session::nowhere());

    let written = resuming("", &sample, &mut runner);
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
    let mut runner = over(Session::nowhere());

    let absent = SessionId::new();
    for said in ["the second one", absent.as_str()] {
        let written = resuming(said, &sample, &mut runner);

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

    let mut runner = over(Session::nowhere());
    let written = resuming(&id, &sample, &mut runner);

    assert_eq!(runner.session().path(), path);
    assert_eq!(
        runner.transcript().len(),
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
    let mut runner = over(open).resuming(transcript);

    let written = resuming(&id, &sample, &mut runner);

    assert!(
        written.contains("this is the session you are in"),
        "{written}"
    );
    assert!(!written.contains("another crucible"), "{written}");
    assert_eq!(runner.session().path(), path, "{written}");
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

    let leaving = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let left = leaving.path().to_owned();
    let mut runner = over(leaving);

    runner.session().append(&Message::said("said in passing"));

    let written = resuming(&id, &sample, &mut runner);

    assert_ne!(runner.session().path(), left, "{written}");

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

    let mut runner = over(Session::nowhere());
    let mut renderer = Renderer::new(Recording::new(80, 24));
    renderer
        .present(&[Row::new().then(Slot::Plain, "said in the session being left")])
        .expect("a recording cannot fail");

    let mut input = std::io::empty();
    let opening = standing(&sample);
    run(
        &id,
        &mut renderer,
        &mut runner,
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

    let mut runner = over(Session::nowhere());
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = std::io::empty();
    let opening = standing(&sample);
    let mut held = lent(&mut input, &opening);
    held.images.push("a-picture.png".into());

    run(&id, &mut renderer, &mut runner, &mut held, &terms(&sample))
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
        text: "".into(),
        calls: vec![crucible_core::ToolCall {
            id: ToolId::new("call-1"),
            name: "todo_write".into(),
            args: crucible_core::ToolArgs::new(
                r#"{"tasks":[{"task":"Write the contributor guide","state":"doing"}]}"#,
            ),
        }],
        stop: Some(StopReason::WantsTools),
    });
    // Answered, the way a log a session actually left holds it: a trailing
    // call nothing answered is a turn that broke off, and the replay drops it.
    planned.append(&Message::ToolResults(vec![crucible_core::ToolResult {
        id: ToolId::new("call-1"),
        output: crucible_core::ToolOutput::ok("1 task planned"),
    }]));
    drop(planned);

    let mut runner = over(Session::nowhere());
    let terms = terms(&sample);
    terms.plan.replay(&crucible_core::ToolArgs::new(
        r#"{"tasks":[{"task":"Work of the session being left","state":"doing"}]}"#,
    ));

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = std::io::empty();
    let opening = standing(&sample);
    run(
        &id,
        &mut renderer,
        &mut runner,
        &mut lent(&mut input, &opening),
        &terms,
    )
    .expect("the terminal to be written");

    let tasks = terms.plan.tasks();
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(
        tasks.first().map(crucible_tools::Task::said),
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

    let mut runner = over(Session::nowhere());
    let terms = terms(&sample);
    terms.revealed.reveal("web_search");
    assert!(terms.revealed.holds("web_search"));

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = std::io::empty();
    let opening = standing(&sample);
    run(
        &id,
        &mut renderer,
        &mut runner,
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

    let mut runner = over(Session::nowhere());
    let mut renderer = Renderer::new(Recording::new(80, 24));

    let call = ToolId::new("a call of the session being left");
    let mut input = std::io::empty();
    let opening = standing(&sample);
    let mut held = lent(&mut input, &opening);
    held.kept.calling(call.clone(), "read a file".into());
    held.kept.finished(&call, "line\nline\nline".into(), 0);
    assert!(!held.kept.is_empty());

    run(&id, &mut renderer, &mut runner, &mut held, &terms(&sample))
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

    let listed = saved("a better name", &id, &sample.logs(), &sample.workspace());
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
        output: ToolOutput::ok("theme = midnight"),
    }]));
    drop(session);

    let held = glimpse(&sample.logs(), &sample.workspace(), &id).expect("a finished log");
    let runner = over(recorded(&sample, "another session entirely"));
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
    let runner = over(recorded(&sample, "another session entirely"));

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
    let shows = Picker::previews(room);
    assert!(shows > 0, "a window this tall keeps the pane");

    let behind = furthest(shows + 12, room);
    assert_eq!(
        shows + 12 - behind,
        shows,
        "the pane stands short of full at its furthest back"
    );

    // A tail no longer than the pane has nothing to wheel back through.
    assert_eq!(furthest(shows, room), 0);
    assert_eq!(furthest(shows / 2, room), 0);
}

#[test]
fn the_picker_says_the_words_it_was_drawn_to_say() {
    // The component draws whatever words it is handed, and its own tests hand
    // it the design's. These are the ones a reader gets, so they are asserted
    // where they are written down rather than where they are drawn.
    let glyphs = Glyphs::Unicode;

    assert_eq!(HINT, "a session, or a branch");
    assert_eq!(NOVIEW, "nothing to show");
    assert_eq!(TAKES, "Enter to resume · Esc to cancel");
    assert_eq!(NEVER, "no earlier session for this workspace");
    assert_eq!(CUT, "the rest could not be read");

    assert_eq!(nothing("deploy"), "no session holds \"deploy\"");
    assert_eq!(
        heading(5, 5, "/w", glyphs),
        "Resume a session · 5 of 5 · /w"
    );

    let (walking, _) = keys(glyphs, true);
    assert_eq!(
        walking,
        "↑↓ to walk · ctrl+r to rename · type to search · esc to cancel"
    );

    // With nothing on the list there is nothing to walk to and nothing to
    // rename: what is left to do is narrow the query, or leave.
    let (narrowing, short) = keys(glyphs, false);
    assert_eq!(narrowing, "type to narrow · esc to cancel");
    assert_eq!(short, narrowing);
}

#[test]
fn a_rename_says_what_its_own_keys_do_and_not_the_list_s() {
    // The one row on screen that says what the keys do, while the keys have
    // all changed underneath it: none of walking, renaming or searching is
    // what a key does with a title open, and a row that went on offering them
    // is the picker disagreeing with itself about the mode the reader is in.
    let glyphs = Glyphs::Unicode;

    let (saving, short) = renaming(glyphs);
    assert_eq!(saving, "enter to save · esc to cancel");
    assert_eq!(short, "enter · esc");
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
