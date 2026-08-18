//! What the list says, and what picking one off it changes.
//!
//! The sessions are recorded through the runner's own API rather than planted
//! as text: what `/resume` picks up has to be what a session leaves behind, and
//! a fixture written by hand is a second opinion about that.

use std::time::Duration;

use crucible_auth::Store;
use crucible_core::{Cancel, Message, Revealed, StopReason};
use crucible_runner::{Model, Runner, Tools};
use crucible_tools::{Ledger, Plan};
use crucible_tui::{Recording, Renderer};

use crate::cli::fake::Script;
use crate::cli::sample::Sample;
use crate::cli::style::Style;

use super::*;

/// A session recorded in `sample` and closed again, holding one exchange.
fn recorded(sample: &Sample, asked: &str) -> Session {
    let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");

    session.append(&Message::User(asked.into()));
    session.append(&Message::Agent {
        text: "an answer".into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
    });

    session
}

/// A runner that answers nothing, recording to `session`.
fn over(session: Session) -> Runner {
    Runner::new(
        Box::new(Script::new(Vec::new())),
        Tools::new(),
        Model {
            name: "script".into(),
            max_tokens: 64,
            system: None,
            effort: None,
        },
        session,
    )
}

fn terms(sample: &Sample) -> Terms {
    Terms {
        style: Style::plain(),
        cancel: Cancel::new(),
        ledger: Ledger::new(),
        revealed: Revealed::new(),
        plan: Plan::new(),
        leaving: crucible_tools::Background::new(),
        provider: std::cell::Cell::new(Some("anthropic")),
        choosing: sample.root().join("unwritten-home.json"),
        logins: Store::in_home(&sample.root()),
        subscriptions: crate::cli::subscription::Subscriptions::production(),

        // `/resume` never reaches it, and these terms have no provider to build
        // one from either — the loop they drive answers from a script.
        serving: Box::new(|named, _| {
            Err(Fatal::Provider {
                named: named.name.into(),
            })
        }),
        sessions: sample.logs(),
        workspace: sample.workspace(),
    }
}

/// How long the list is given to hold every session that belongs on it.
///
/// Generous, because it is only ever waited out by a failure: what is being
/// waited for is a queue draining, which takes no time at all on a machine that
/// is working, and the wait is what turns "took a moment longer than the test
/// expected" into a pass rather than into a report about `/resume`.
const SETTLING: Duration = Duration::from_secs(5);

/// Where on the list the session that was asked `wanted` sits, as `/resume`
/// would be told it — once the list holds all `of` of them.
///
/// A session reaches the list when its first prompt reaches its log, and the
/// session in hand is written to by the thread that owns its queue. So a
/// position read while one of them is still missing is a position that names a
/// different row by the time `/resume` reads the list for itself: the one still
/// arriving is the newest, and it arrives at the top.
fn at(sample: &Sample, wanted: &str, of: usize) -> String {
    let since = std::time::Instant::now();

    loop {
        let listed = recent(&sample.logs(), &sample.workspace(), SHOWN);

        if listed.len() == of {
            let found = listed
                .iter()
                .position(|session| session.asked() == wanted)
                .expect("the session is on the list");

            return (found + 1).to_string();
        }

        assert!(
            since.elapsed() < SETTLING,
            "{} of {of} sessions reached the list",
            listed.len()
        );

        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Runs `/resume {said}` against `runner`, and says what reached the terminal.
fn resuming(said: &str, sample: &Sample, runner: &mut Runner) -> String {
    let mut renderer = Renderer::new(Recording::new(80, 24));

    run(said, &mut renderer, runner, &terms(sample)).expect("the terminal to be written");

    renderer.terminal().written().to_string()
}

#[test]
fn a_directory_nothing_was_recorded_in_says_so() {
    let sample = Sample::new("resume-empty");
    let mut runner = over(Session::nowhere());

    let written = resuming("", &sample, &mut runner);

    assert!(
        written.contains("nothing has been worked on here yet"),
        "{written}"
    );
}

#[test]
fn the_list_is_numbered_from_one_in_the_order_it_came_back_in() {
    // Which order that is belongs to `recent` and is proven there. What is
    // proven here is that the numbers follow it and start at one: they are the
    // only way to pick a session, so a number beside the wrong row picks the
    // wrong session.
    let sample = Sample::new("resume-list");
    drop(recorded(&sample, "one question"));
    drop(recorded(&sample, "another question"));
    let mut runner = over(Session::nowhere());

    let listed = recent(&sample.logs(), &sample.workspace(), SHOWN);
    let written = resuming("", &sample, &mut runner);
    let rows: Vec<&str> = written
        .lines()
        .filter(|row| row.contains("question"))
        .collect();

    assert_eq!(rows.len(), listed.len(), "{written}");
    for (at, session) in listed.iter().enumerate() {
        let wanted = format!("{}  ", at + 1);
        assert!(
            rows.get(at)
                .is_some_and(|row| row.starts_with(&wanted) && row.contains(session.asked())),
            "{written}"
        );
    }
    assert!(written.contains("just now"), "{written}");
}

#[test]
fn a_number_that_names_nothing_says_so_and_shows_the_list_again() {
    // Both halves matter. The list is what the numbers mean, so a refusal
    // without it leaves nothing to try instead.
    let sample = Sample::new("resume-unknown");
    drop(recorded(&sample, "the only question"));
    let mut runner = over(Session::nowhere());

    for said in ["4", "0", "-1", "the second one"] {
        let written = resuming(said, &sample, &mut runner);

        assert!(
            written.contains(&format!("! {said} is not on the list")),
            "{written}"
        );
        assert!(written.contains("the only question"), "{written}");
    }
}

#[test]
fn picking_one_up_makes_it_the_session_being_recorded_to() {
    let sample = Sample::new("resume-picked");
    let earlier = recorded(&sample, "what was asked before");
    let path = earlier.path().to_owned();
    drop(earlier);

    let mut runner = over(Session::nowhere());
    let written = resuming("1", &sample, &mut runner);

    assert_eq!(runner.session().path(), path);
    assert_eq!(
        runner.transcript().len(),
        2,
        "the prompt and the answer came back: {written}"
    );
    assert!(written.contains("what was asked before"), "{written}");
    assert!(written.contains("2 messages"), "{written}");
}

#[test]
fn the_session_already_open_is_refused_as_the_one_being_used() {
    // Not as "open in another crucible", which is what the log itself would
    // say: the claim on that file is this process's own, and being sent to
    // close a crucible that is this one is worse than not being answered.
    // Continued the way `--continue` continues one, so the session in hand is
    // both on the list and claimed by this process — which is the arrangement
    // the answer is about.
    let sample = Sample::new("resume-itself");
    drop(recorded(&sample, "the session in hand"));
    let (open, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    let path = open.path().to_owned();
    let mut runner = over(open).resuming(transcript);

    let written = resuming("1", &sample, &mut runner);

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
    drop(recorded(&sample, "the one picked up"));

    let leaving = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
    let left = leaving.path().to_owned();
    let mut runner = over(leaving);

    runner
        .session()
        .append(&Message::User("said in passing".into()));

    // Asked for by where it is on the list rather than by a number written
    // here, and asked once both sessions are on it. The session being left is
    // being written to as this runs, so a number written here would name
    // whichever row that log had reached by then — and the row it reaches is
    // the first one, which is this session asking to resume itself.
    let written = resuming(&at(&sample, "the one picked up", 2), &sample, &mut runner);

    assert_ne!(runner.session().path(), left, "{written}");

    let recovered = std::fs::read_to_string(&left).expect("the log it was recording to");
    assert!(recovered.contains("said in passing"), "{recovered}");
}

#[test]
fn what_stands_between_the_count_and_the_age_comes_out_of_the_glyph_set() {
    // The row is two facts about the session picked up, and what says they are
    // two is the mark between them. A terminal that cannot draw that mark gets
    // the one the setting names rather than a question mark standing where the
    // sentence divides.
    let sample = Sample::new("resume-mark");
    drop(recorded(&sample, "the one picked up"));

    let listed = recent(&sample.logs(), &sample.workspace(), SHOWN);
    let picked = listed.first().expect("the session just recorded");
    let now = SystemTime::now();

    let said = |glyphs| {
        picked_up(picked, 2, now, 70, glyphs)
            .get(1)
            .map(Row::text)
            .unwrap_or_default()
    };

    assert!(said(Glyphs::Unicode).starts_with("2 messages · started"));
    assert!(said(Glyphs::Ascii).starts_with("2 messages - started"));
}
