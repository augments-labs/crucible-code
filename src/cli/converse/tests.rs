//! What a whole loop does: a turn, a question, and a window that changed.

use std::cell::Cell;
use std::io;
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crucible_auth::Store;
use crucible_core::{Delta, Mode, Permission, Revealed, Rules, StopReason, ToolId};
use crucible_runner::{Model, Session, Tools};
use crucible_tui::{Picture, Recording, Size, Terminal, TerminalError};

use super::*;
use crate::cli::fake::{Fixed, Script, Stalling, changing, running};
use crate::cli::sample::Sample;

/// The opening a session starts with.
///
/// What it says is not what any of these tests is about, but the loop takes one
/// and draws it, so they hand it a real one rather than a shape that only
/// exists here.
pub(super) fn opening() -> draw::opening::Standing {
    let workspace =
        crucible_core::Workspace::open(std::env::temp_dir()).expect("a temporary directory");

    draw::opening::Standing::new(
        &draw::Opening {
            credential: None,
            model: Some("script"),
            provider: None,
            unasked: crate::cli::NOTHING_TO_ASK,
            trouble: None,
            workspace: &workspace,
            sessions: &[],
            update: None,
            style: Style::plain(),
        },
        std::time::SystemTime::now(),
    )
}

/// An editor holding `text`, arrived at the way the box would have.
fn typed(text: &str) -> Editor {
    let mut editor = Editor::new();
    for key in text.chars() {
        editor.press(Key::Char(key));
    }
    editor
}

/// The terms a test runs under when neither the style nor cancelling is what
/// it is watching.
///
pub(super) fn plain() -> Terms {
    let unwritten = std::env::temp_dir().join(format!("crucible-unwritten-{}", std::process::id()));

    Terms {
        style: Cell::new(Style::plain()),
        chosen: Cell::new(None),
        reading: std::cell::RefCell::default(),
        cancel: Cancel::new(),
        steer: crucible_core::Steer::new(),
        ledger: Ledger::new(),
        revealed: Revealed::new(),
        plan: Plan::new(),
        putting: crate::cli::seen::Putting::new(),
        leaving: crucible_tools::Background::new(),
        // A provider, so `/model` has a name to write its answer under, and a
        // file inside the same absent tree so nothing a test types reaches a
        // configuration anybody keeps.
        provider: Cell::new(Some("anthropic")),
        settings: crucible_config::Settings::default(),
        choosing: unwritten.join("config.json"),

        // The same again: `/login` is driven where a store of its own is
        // watched, and a loop these terms drive must not write a key into
        // whatever home the machine running the suite has.
        logins: Store::in_home(&unwritten),
        subscriptions: crate::cli::subscription::Subscriptions::production(),

        // Unreachable from here and truthful about it: `/login` asks for a key
        // from a keyboard, and a loop driven off a pipe has none. What a key
        // given at one sets a session up with is proved where there is a
        // terminal to type it into.
        serving: Box::new(|named, _| {
            Err(Fatal::Provider {
                named: named.name.into(),
            })
        }),

        // The same tree, equally absent: a loop these terms drive has no
        // sessions to list and none to pick up. What `/resume` does with ones
        // that are there is proved where they are recorded.
        sessions: unwritten.join("sessions"),
        workspace: Workspace::open(std::env::temp_dir()).expect("a temporary directory"),
        sending: crucible_tui::Sending::default(),
    }
}

/// Runs the whole loop over a scripted provider and typed-ahead input.
///
/// Returns what the terminal ended up with. A test that hangs here has
/// found the deadlock this file exists to avoid, so every one of them is
/// also a liveness check.
fn conversing(rounds: Vec<Vec<Delta>>, offered: Tools, typed: &str) -> String {
    over(Script::new(rounds), offered, typed).0
}

/// A runner that answers from `script` and records nothing.
fn scripted(script: Script, offered: Tools) -> Runner {
    Runner::new(
        Box::new(script),
        offered,
        Model {
            name: "script".into(),
            max_tokens: 64,
            window: None,
            system: None,
            effort: None,
        },
        Session::nowhere(),
    )
}

/// The whole loop over one script: what the terminal ended up with, and how
/// many requests the script was given.
fn over(script: Script, offered: Tools, typed: &str) -> (String, usize) {
    let asked = script.asked();
    let runner = scripted(script, offered);

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(typed.as_bytes().to_vec());

    converse(runner, &mut renderer, &plain(), &opening(), &mut input).expect("the loop to finish");

    (
        renderer.terminal().written().to_string(),
        asked.load(std::sync::atomic::Ordering::Relaxed),
    )
}

fn saying(text: &str) -> Vec<Delta> {
    vec![
        Delta::Text(text.into()),
        Delta::Stopped(StopReason::Yielded),
    ]
}

#[test]
fn a_turn_streams_what_the_model_said_and_the_loop_comes_back_for_more() {
    // The drain ends when the worker drops its senders. If it did not, this
    // test would hang instead of failing, which is the point of running the
    // real loop rather than asserting on a mock.
    let written = conversing(vec![saying("hello")], Tools::new(), "hi\n");

    assert!(written.contains("hello"), "{written}");
}

#[test]
fn a_theme_taken_mid_session_is_what_the_rows_after_it_are_drawn_in() {
    // `/theme` changes the table the whole interface is drawn in. A loop that
    // read the style once, before the first prompt, would go on drawing in
    // whatever was in force when the session opened: the transcript would
    // follow the new theme and everything the loop itself draws — the box, the
    // expanded view, a queued prompt — would not.
    //
    // The box is what a reader actually looks at while they choose, and it is
    // the one thing this suite cannot reach: drawing it needs raw mode, which
    // reaches the controlling terminal. So the property is pinned on a row the
    // same captured style fed — the one that says no model has been chosen.
    let terms = Terms {
        style: Cell::new(Style::coloured()),
        ..plain()
    };
    let was = terms.style();

    // No model, so a line that is not a command reaches `draw::unconfigured`,
    // which is drawn from the style the loop is holding.
    let runner = Runner::new(
        Box::new(Script::new(vec![])),
        Tools::new(),
        Model {
            name: "".into(),
            max_tokens: 64,
            window: None,
            system: None,
            effort: None,
        },
        Session::nowhere(),
    );

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"/theme colourblind-dark\nhello\n".to_vec());

    converse(runner, &mut renderer, &terms, &opening(), &mut input).expect("the loop to finish");

    let worn = |style: Style| {
        style
            .palette()
            .open(crucible_tui::Slot::Strong)
            .as_str()
            .to_owned()
    };
    let now = terms.style();

    assert_ne!(worn(now), worn(was), "the theme did not change");

    let written = renderer.terminal().written();
    let after = written
        .rsplit_once("colourblind-dark")
        .map_or("", |(_, after)| after);

    assert!(
        after.contains(&worn(now)),
        "the loop kept the theme the session opened in:\n{after:?}"
    );
}

#[test]
fn a_window_the_user_resized_wraps_the_turns_that_follow_it() {
    // Catching the signal a resize sends needs `unsafe`, which this
    // workspace forbids, so a prompt is the only moment the loop can notice
    // one. Unnoticed, the width read at startup is the width every turn is
    // wrapped to for the rest of the session.
    let runner = scripted(Script::new(vec![saying("abcdefghijkl")]), Tools::new());
    let mut renderer = Renderer::new(Narrowing::new());
    let mut input = Cursor::new(b"go\n".to_vec());

    converse(runner, &mut renderer, &plain(), &opening(), &mut input).expect("the loop to finish");

    let shown = Picture::of(renderer.terminal().written(), NARROW, 24);
    let said = shown.said();
    assert!(
        said.windows(2)
            .any(|pair| pair.first().is_some_and(|row| row == "abcdefghij")
                && pair.get(1).is_some_and(|row| row == "kl")),
        "expected a wrap at the new width, got {said:?}"
    );
}

#[test]
fn two_prompts_are_two_turns() {
    // The runner has to survive being handed to a thread and back, or the
    // second turn has no transcript to continue from.
    let written = conversing(
        vec![saying("first"), saying("second")],
        Tools::new(),
        "one\ntwo\n",
    );

    assert!(written.contains("first"), "{written}");
    assert!(written.contains("second"), "{written}");
}

#[test]
fn every_line_finished_during_a_turn_is_kept_in_the_order_it_was_typed() {
    // Return pressed twice while a long turn ran. The box takes the second
    // line as readily as the first and clears itself both times, so a queue
    // that kept only one of them loses a prompt the user watched it accept --
    // and it never runs, and nothing says so.
    let mut waiting = Prompts::default();
    let mut editor = typed("run the tests");
    assert_eq!(waiting.accept(&mut editor), Retained::Accepted);
    let mut editor = typed("now fix what failed");
    assert_eq!(waiting.accept(&mut editor), Retained::Accepted);

    assert_eq!(waiting.pop().as_deref(), Some("run the tests"));
    assert_eq!(waiting.pop().as_deref(), Some("now fix what failed"));
    assert!(waiting.pop().is_none());
}

#[test]
fn the_whole_queue_is_named_and_a_line_taken_back_leaves_the_rest_in_order() {
    // The panel names them all, so the queue has to hand them all over, oldest
    // first. And a line taken back from the middle is the reader's until its
    // turn, without disturbing the ones still waiting on either side of it.
    let mut waiting = Prompts::default();
    for line in ["first", "second", "third"] {
        let mut editor = typed(line);
        assert_eq!(waiting.accept(&mut editor), Retained::Accepted);
    }

    assert_eq!(
        waiting.waiting_all().collect::<Vec<_>>(),
        vec!["first", "second", "third"]
    );
    assert_eq!(waiting.waiting_count(), 3);

    // The middle one, taken back: what is left is the two around it, still in
    // the order they were typed.
    assert_eq!(waiting.drop(1).as_deref(), Some("second"));
    assert_eq!(
        waiting.waiting_all().collect::<Vec<_>>(),
        vec!["first", "third"]
    );
    assert_eq!(waiting.waiting_count(), 2);

    // Past the end is nothing, and costs nothing.
    assert!(waiting.drop(9).is_none());
}

#[test]
fn a_line_the_turn_steered_by_stops_waiting_behind_it() {
    // The defect this pins: a line typed mid-turn was both steered into the
    // running turn and left in the queue, so it reached the transcript and
    // went on being named above the box as though it were still owed -- and
    // then ran a second time as its own turn.
    let mut waiting = Prompts::default();
    for line in ["first", "second", "third"] {
        let mut editor = typed(line);
        assert_eq!(waiting.accept(&mut editor), Retained::Accepted);
    }

    // The turn drains the whole queue at one boundary and says so a line at a
    // time, so the middle of the three is dropped without disturbing its
    // neighbours -- and the bytes it reserved come back with it.
    assert!(waiting.steered("second"));
    assert_eq!(
        waiting.waiting_all().collect::<Vec<_>>(),
        vec!["first", "third"]
    );
    assert_eq!(waiting.bytes, "first".len() + "third".len());

    // A line the queue does not have changes nothing. The turn reports every
    // line it worked in, including one the reader took back into the box
    // between the turn reading the queue and saying what it read.
    assert!(!waiting.steered("second"));
    assert!(!waiting.steered("never typed"));
    assert_eq!(waiting.waiting_count(), 2);
}

#[test]
fn the_whole_queue_goes_into_one_turn_rather_than_one_turn_each() {
    // Three lines typed behind a turn are one thing the reader wanted said, not
    // three conversations. Taken one at a time the first of them was answered
    // before the model had read the second, so the agent worked to a question
    // the reader had already added to -- and the reader watched three turns go
    // by saying what one turn was asked.
    let mut waiting = Prompts::default();
    for line in [
        "run the tests",
        "then fix what failed",
        "and say what you did",
    ] {
        let mut editor = typed(line);
        assert_eq!(waiting.accept(&mut editor), Retained::Accepted);
    }

    // The oldest is the turn's prompt and the rest are offered to that same
    // turn, which records them together at its first boundary. Nothing is left
    // waiting, and the bytes they reserved come back with them.
    let steer = crucible_core::Steer::new();
    assert_eq!(
        batched(&mut waiting, &steer).as_deref(),
        Some("run the tests")
    );
    assert_eq!(waiting.waiting_count(), 0);
    assert_eq!(waiting.bytes, 0);
    assert_eq!(
        steer.take(),
        vec!["then fix what failed", "and say what you did"]
    );
}

#[test]
fn an_empty_queue_is_no_turn_and_offers_nothing() {
    // The loop asks the queue before it asks the keyboard, and almost every
    // time there is nothing there. Nothing is what it must get back: a turn
    // taken on an empty queue is a prompt nobody typed.
    let mut waiting = Prompts::default();
    let steer = crucible_core::Steer::new();

    assert!(batched(&mut waiting, &steer).is_none());
    assert!(steer.take().is_empty());
}

#[test]
fn what_is_waiting_is_the_line_that_goes_next_rather_than_the_one_typed_last() {
    // The row above the box names it, so which of the queue it names is what
    // the reader checks their typing against. The oldest is the one the next
    // turn takes, and naming the newest would say a line is coming that three
    // others are in front of.
    let mut waiting = Prompts::default();
    assert_eq!(waiting.waiting(), None);

    let mut editor = typed("run the tests");
    assert_eq!(waiting.accept(&mut editor), Retained::Accepted);
    let mut editor = typed("now fix what failed");
    assert_eq!(waiting.accept(&mut editor), Retained::Accepted);

    assert_eq!(waiting.waiting(), Some("run the tests"));

    let _ = waiting.pop();
    assert_eq!(waiting.waiting(), Some("now fix what failed"));

    let _ = waiting.pop();
    assert_eq!(waiting.waiting(), None);
}

#[test]
fn typed_ahead_prompt_count_is_bounded_without_losing_the_refused_line() {
    let mut waiting = Prompts::default();

    for index in 0..QUEUED_LINES {
        let mut editor = typed(&format!("prompt-{index}"));
        assert_eq!(waiting.accept(&mut editor), Retained::Accepted);
    }

    let mut editor = typed("still in the box");
    assert_eq!(waiting.accept(&mut editor), Retained::Refused);
    assert_eq!(editor.text(), "still in the box");
    assert_eq!(waiting.lines.len(), QUEUED_LINES);

    let _ = waiting.pop();
    assert_eq!(waiting.accept(&mut editor), Retained::Accepted);
    assert!(editor.is_empty());
}

#[test]
fn typed_ahead_prompt_bytes_are_bounded_without_losing_the_refused_line() {
    let mut waiting = Prompts::default();

    let mut first = typed("x");
    assert_eq!(waiting.accept(&mut first), Retained::Accepted);
    assert_eq!(waiting.bytes, 1);

    // The accounting is what is being pinned, so the queue is stood one byte
    // short of the ceiling directly rather than by retaining a real MiB.
    waiting.bytes = QUEUED_BYTES - 1;

    let mut editor = typed("still in the box");
    assert_eq!(waiting.accept(&mut editor), Retained::Refused);
    assert_eq!(editor.text(), "still in the box");
    assert_eq!(waiting.bytes, QUEUED_BYTES - 1);

    waiting.bytes = QUEUED_BYTES - 2;
    let mut editor = typed("xy");
    assert_eq!(waiting.accept(&mut editor), Retained::Accepted);
    assert_eq!(waiting.bytes, QUEUED_BYTES);
}

#[test]
fn a_blank_line_is_not_a_turn() {
    // Otherwise the return key alone sends an empty prompt and costs a
    // request. Counted at the provider rather than in what was drawn: a line on
    // screen is written again in every frame it is on screen for, so counting
    // appearances would count frames.
    let (written, asked) = over(
        Script::new(vec![saying("answered")]),
        Tools::new(),
        "\n   \nreal\n",
    );

    assert_eq!(asked, 1, "{written}");
    assert!(written.contains("answered"), "{written}");
}

#[test]
fn an_answer_that_was_cut_short_does_not_come_back_looking_complete() {
    // The partial answer stays in the transcript and the prompt returns, so
    // with nothing said the user reads a sentence that stops mid-thought as
    // the whole of it — and the model reads its own truncation as a
    // finished thought on the next turn.
    let written = conversing(
        vec![vec![
            Delta::Text("as I was say".into()),
            Delta::Stopped(StopReason::OutOfTokens),
        ]],
        Tools::new(),
        "go\n",
    );

    assert!(written.contains("as I was say"), "{written}");
    assert!(written.contains("unfinished"), "{written}");
}

#[test]
fn a_turn_that_finished_properly_leaves_nothing_extra_behind() {
    let written = conversing(vec![saying("all done")], Tools::new(), "go\n");

    assert!(written.contains("all done"), "{written}");
    assert!(!written.contains("unfinished"), "{written}");
}

#[test]
fn a_provider_that_fails_says_so_instead_of_ending_the_session() {
    // Nothing else posts the turn's own failure, so if the wiring drops it
    // the user gets a prompt back with no explanation and retypes the
    // thing that just failed.
    let (written, asked) = over(Script::refusing(), Tools::new(), "go\nagain\n");

    assert!(written.contains("HTTP 401"), "{written}");
    assert_eq!(asked, 2, "a failed turn does not end the session");
}

#[test]
fn a_log_that_failed_with_the_last_line_still_queued_is_reported_before_the_prompt_goes_away() {
    // The writer thread runs behind the loop, so the poll after a turn sees
    // only what has already reached the disk. Whatever is still queued when
    // input ends is nobody's news until the queue is drained — and the turn
    // most likely to be in it is the last one, which is the one worth knowing
    // about.
    //
    // Nothing is typed here on purpose. The loop breaks before it takes a
    // turn, so the in-loop poll never runs at all, and the only path that can
    // still say anything is the drain after it. A test that let the poll run
    // would pass with the report after the loop deleted.
    let session = Session::onto("/nowhere".into(), Failing);
    session.append(&crucible_core::Message::User("queued".into()));

    let runner = Runner::new(
        Box::new(Script::new(vec![])),
        Tools::new(),
        Model {
            name: "script".into(),
            max_tokens: 64,
            window: None,
            system: None,
            effort: None,
        },
        session,
    );

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(Vec::new());

    converse(runner, &mut renderer, &plain(), &opening(), &mut input).expect("the loop to finish");

    let written = renderer.terminal().written();
    assert!(
        written.contains("stopped being recorded"),
        "the drained failure never reached the terminal: {written}"
    );
}

#[test]
fn a_terminal_that_fails_mid_turn_leaves_the_turn_recorded_all_the_same() {
    // The worker owns the runner, the runner owns the session, and the log is
    // finished by a thread the session's `Drop` waits for — on whichever thread
    // holds it. Returning the moment a write failed would drop the join handle
    // and leave that thread running with the process on its way out, so the
    // turn on screen when the window closed is the turn missing from the log.
    let kept = Arc::new(Mutex::new(Vec::new()));
    let session = Session::onto("/nowhere".into(), Kept(Arc::clone(&kept)));

    let provider = Script::new(vec![saying("what the model said")]);
    let started = provider.asked();
    let runner = Runner::new(
        Box::new(provider),
        Tools::new(),
        Model {
            name: "script".into(),
            max_tokens: 64,
            window: None,
            system: None,
            effort: None,
        },
        session,
    );

    // Three writes come before the turn: the row at the top of the window, the
    // opening written down once the first prompt has been read, and the prompt
    // mark, colour and all. That leaves the turn's first frame as what finds
    // the terminal gone -- after the worker has been handed the runner.
    let mut renderer = Renderer::new(BreakingWhenStarted {
        inner: Recording::new(80, 24),
        left: 3,
        started: Arc::clone(&started),
    });
    let mut input = Cursor::new(b"go\n".to_vec());

    let problem = converse(runner, &mut renderer, &plain(), &opening(), &mut input)
        .expect_err("the terminal to fail");

    assert!(matches!(problem, Fatal::Terminal(_)), "{problem:?}");
    assert_eq!(started.load(Ordering::Acquire), 1, "the turn never began");

    let written = String::from_utf8(kept.lock().expect("a lock").clone()).expect("a log of text");
    assert!(
        written.contains("what the model said"),
        "the turn never reached the log: {written:?}"
    );
}

#[test]
fn a_terminal_failure_cancels_a_provider_that_would_otherwise_stay_live() {
    let (provider, escaped) = Stalling::new();
    let runner = Runner::new(
        Box::new(provider),
        Tools::new(),
        Model {
            name: "stalling".into(),
            max_tokens: 64,
            window: None,
            system: None,
            effort: None,
        },
        Session::nowhere(),
    );
    let terms = plain();
    let cancellation = terms.cancel.clone();
    let mut renderer = Renderer::new(Breaking {
        inner: Recording::new(80, 24),
        left: 3,
    });
    let mut input = Cursor::new(b"go\n".to_vec());

    let problem = converse(runner, &mut renderer, &terms, &opening(), &mut input)
        .expect_err("the terminal to fail");

    assert!(matches!(problem, Fatal::Terminal(_)), "{problem:?}");
    assert!(cancellation.requested(), "the provider was never cancelled");
    assert!(
        !escaped.load(std::sync::atomic::Ordering::Acquire),
        "the provider reached its test escape instead of observing cancellation"
    );
}

#[test]
fn a_piped_run_ends_the_row_its_prompt_was_left_on() {
    // The mark carries no line ending while a line is being typed after it. On
    // a screen this process owns that costs nothing — the frame after it draws
    // the whole window again. Down a pipe there is no frame and no screen to
    // give back: what crucible wrote is the last thing in the file, and a row
    // left unended is one the next thing written lands on.
    let runner = scripted(Script::new(vec![]), Tools::new());
    let mut renderer = Renderer::new(Recording::redirected(80, 24));
    let mut input = Cursor::new(Vec::new());

    converse(runner, &mut renderer, &plain(), &opening(), &mut input).expect("the loop to finish");

    let written = renderer.terminal().written();
    assert!(written.ends_with('\n'), "{written:?}");
}

#[test]
fn the_prompt_line_names_the_mode_in_force() {
    // fullAccess is the mode worth pinning: in `ask` every sensitive call
    // announces itself with a question, so the prompt line is the only place a
    // session that never asks says what it is. Written before the read, which
    // is why empty input still shows it once.
    let runner = scripted(Script::new(vec![]), Tools::new())
        .permitting(Permission::with(Mode::FullAccess, Rules::new()));
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(Vec::new());

    converse(runner, &mut renderer, &plain(), &opening(), &mut input).expect("the loop to finish");

    let written = renderer.terminal().written();
    assert!(written.contains("fullAccess › "), "{written}");
}

#[test]
fn the_mark_a_piped_line_is_typed_after_comes_out_of_the_glyph_set() {
    // Down a pipe there is no box, so the mode and this mark are the whole of
    // the prompt. A piped run is also the one most likely to have the setting
    // turned down, and a hollow square is a poor thing to leave somebody
    // waiting behind.
    for (glyphs, said) in [
        (crucible_tui::Glyphs::Unicode, "fullAccess › "),
        (crucible_tui::Glyphs::Ascii, "fullAccess > "),
    ] {
        let runner = scripted(Script::new(vec![]), Tools::new())
            .permitting(Permission::with(Mode::FullAccess, Rules::new()));
        let mut renderer = Renderer::new(Recording::new(80, 24));
        let mut input = Cursor::new(Vec::new());
        let terms = Terms {
            style: std::cell::Cell::new(Style::drawn(glyphs)),
            ..plain()
        };

        converse(runner, &mut renderer, &terms, &opening(), &mut input)
            .expect("the loop to finish");

        let written = renderer.terminal().written();
        assert!(written.contains(said), "{glyphs:?}: {written}");
    }
}

#[test]
fn the_box_and_the_mode_stand_under_a_turn_that_is_still_being_written() {
    // A turn is the longest a session goes without a prompt on screen, and it
    // is the stretch the mode is deciding things over -- both used to leave
    // with the box and come back only once there was nothing left to decide.
    // Pinned to the escape that parks the cursor as well as to the words: where
    // the cursor is left is what a reader takes for the place their typing goes,
    // so a turn that parked it on the answer would mislead about the box as
    // much as an absent box would.
    // The cursor comes back into the box rather than onto the answer, because
    // the box is what takes typing while the turn runs — two rows up from the
    // last of the four, and at the column the line starts on.
    let runner = scripted(Script::new(vec![saying("hello")]), Tools::new())
        .permitting(Permission::with(Mode::FullAccess, Rules::new()));
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"go\n".to_vec());

    converse(runner, &mut renderer, &plain(), &opening(), &mut input).expect("the loop to finish");

    let shown = moment(renderer.terminal().written(), "thinking");
    let rows = shown.rows();
    let at = rows
        .iter()
        .position(|row| row.contains("thinking"))
        .unwrap_or_else(|| panic!("the row did not say what the turn was doing: {rows:?}"));

    // The rhythm around the row: a blank above it and a blank below. The blanks
    // are the whole of what stops it reading as the last line of the answer or
    // the first line of the box, and they are asserted here rather than in the
    // component because neither of the two things they separate is the
    // component's to know about.
    assert!(
        rows.get(at - 1).is_some_and(String::is_empty),
        "the row did not stand clear of the transcript: {rows:?}"
    );
    assert!(
        rows.get(at + 1).is_some_and(String::is_empty),
        "the row did not stand clear of the box: {rows:?}"
    );
    assert!(
        rows.get(at + 2)
            .is_some_and(|row| row.starts_with('\u{256d}')),
        "the box was not under the row: {rows:?}"
    );

    // The mode has the last row to itself, for the whole of the stretch it is
    // deciding things over -- which used to leave with the box and come back
    // only once there was nothing left to decide.
    assert!(
        rows.last()
            .is_some_and(|row| row.contains("full access mode on")),
        "the foot did not say the mode: {rows:?}"
    );

    // And the cursor comes back into the box rather than onto the answer,
    // because the box is what takes typing while the turn runs.
    assert_eq!(shown.caret(), (at + 3, 4), "{rows:?}");
}

/// The window as it stood at the end of the first frame that said `said`.
///
/// A frame draws over the one before it rather than replacing it, so the
/// picture at a moment is the whole log up to that moment: rows that frame did
/// not name are still showing whatever put them there.
fn moment(written: &str, said: &str) -> Picture {
    const OPENS: &str = "\x1b[?2026h";

    let at = written.find(said).expect("a frame that said it");
    let ends = written[at..]
        .find(OPENS)
        .map_or(written.len(), |from| at + from);

    Picture::of(&written[..ends], 80, 24)
}

/// The moment in the middle of a turn, where the loop draws and waits at once.
/// The whole loop under terms of the test's own: what an answer leaves behind
/// depends on where those terms point.
fn answering(terms: &Terms, rounds: Vec<Vec<Delta>>, offered: Tools, typed: &str) -> String {
    let runner = scripted(Script::new(rounds), offered);

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(typed.as_bytes().to_vec());

    converse(runner, &mut renderer, terms, &opening(), &mut input).expect("the loop to finish");

    renderer.terminal().written().to_string()
}
fn tools(tool: Fixed) -> Tools {
    let mut offered = Tools::new();
    offered.add(Box::new(tool));
    offered
}
fn calling(name: &str) -> Vec<Delta> {
    vec![
        Delta::ToolStarted {
            id: ToolId::new("a"),
            name: name.into(),
        },
        Delta::ToolArgs("{}".into()),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

#[test]
fn a_turn_that_asks_a_loop_with_nobody_at_it_is_told_so_and_carries_on() {
    // The liveness half matters more than the words: a panel standing where no
    // key will ever arrive is a turn that never ends, and this loop reads lines
    // rather than keys. A test that hangs here has found exactly that.
    // The *same* handle the loop lends its ends to. A tool built with a handle
    // of its own would answer "nobody there" without the questions ever leaving
    // the worker, and this test would prove nothing about the seam it is for.
    let putting = crate::cli::seen::Putting::new();
    let terms = Terms {
        putting: putting.clone(),
        ..plain()
    };

    let mut offered = Tools::new();
    offered.add(Box::new(crucible_tools::AskUser::new(std::sync::Arc::new(
        putting,
    ))));

    let asking = vec![
        Delta::ToolStarted {
            id: ToolId::new("a"),
            name: "ask_user".into(),
        },
        Delta::ToolArgs(
            r#"{"questions":[{"heading":"Language","question":"Which language?",
                "answers":[{"answer":"Rust"},{"answer":"Python"}]}]}"#
                .into(),
        ),
        Delta::Stopped(StopReason::WantsTools),
    ];

    let runner = scripted(Script::new(vec![asking, saying("carried on")]), offered);
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"go\n".to_vec());

    converse(runner, &mut renderer, &terms, &opening(), &mut input).expect("the loop to finish");

    let written = renderer.terminal().written().to_string();
    assert!(written.contains("carried on"), "{written}");
}

mod command;
mod question;

// Which questions a mode leaves to be drawn.
//
// What each mode answers is settled where the engine is, over sensitivities
// written down by hand. What needs the loop running is that the answer reaches
// the screen: the engine crosses to the worker with the runner and comes back,
// and a mode is the one thing about a session that changes after it started —
// so a session drawing one mode while deciding by another would be wrong in the
// place nobody can check by reading.
//
// Every test here types nothing an unexpected question could be answered with.
// A question drawn where none was expected meets the end of input, which is a
// refusal, so the turn ends and the sentence after it is never said — the
// assertions below fail rather than hang.

/// The whole loop in one mode, over the tools given.
fn deciding(mode: Mode, offered: Tools, rounds: Vec<Vec<Delta>>, typed: &str) -> String {
    let runner =
        scripted(Script::new(rounds), offered).permitting(Permission::with(mode, Rules::new()));

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(typed.as_bytes().to_vec());

    converse(runner, &mut renderer, &plain(), &opening(), &mut input).expect("the loop to finish");

    renderer.terminal().written().to_string()
}

#[test]
fn allow_edits_changes_a_file_with_no_question_drawn() {
    let written = deciding(
        Mode::AllowEdits,
        tools(Fixed::new("write", changing())),
        vec![calling("write"), saying("changed it")],
        "edit it\n",
    );

    assert!(!written.contains("wants to change"), "{written}");
    assert!(written.contains("changed it"), "{written}");
}

#[test]
fn allow_edits_asks_before_a_command_nothing_proved_anything_about() {
    // The mode is a sentence about reach, and a command line nobody read
    // closely enough reaches whatever the user can. Answered `n`, so the
    // question is one the loop waited on rather than one it drew and walked
    // past.
    let written = deciding(
        Mode::AllowEdits,
        tools(Fixed::new("bash", running("cargo test"))),
        vec![calling("bash"), saying("ran it")],
        "go\nn\n",
    );

    assert!(
        written.contains("bash wants to run: cargo test"),
        "{written}"
    );
    assert!(written.contains("bash was not allowed"), "{written}");
}

#[test]
fn allow_edits_draws_the_question_for_a_command_that_only_changes_the_workspace() {
    // The line this mode used to run unasked, because every path in it was
    // found inside the working directory. A shell reopens those paths by name
    // after the reading, so the reading was never a guarantee about what ran —
    // and the mode now says what its name says: files yes, processes ask.
    let written = deciding(
        Mode::AllowEdits,
        tools(Fixed::new("bash", running("mkdir src/net"))),
        vec![calling("bash"), saying("made it")],
        "go\ny\n",
    );

    assert!(
        written.contains("bash wants to run: mkdir src/net"),
        "{written}"
    );
    assert!(written.contains("made it"), "{written}");
}

#[test]
fn full_access_draws_neither_question() {
    // Both in one round, which is the shape that would catch a mode read once
    // per turn rather than asked of every call.
    let mut offered = tools(Fixed::new("write", changing()));
    offered.add(Box::new(Fixed::new("bash", running("cargo test"))));

    let round = vec![
        Delta::ToolStarted {
            id: ToolId::new("a"),
            name: "write".into(),
        },
        Delta::ToolArgs("{}".into()),
        Delta::ToolStarted {
            id: ToolId::new("b"),
            name: "bash".into(),
        },
        Delta::ToolArgs("{}".into()),
        Delta::Stopped(StopReason::WantsTools),
    ];

    let written = deciding(
        Mode::FullAccess,
        offered,
        vec![round, saying("both done")],
        "go\n",
    );

    assert!(!written.contains("wants to"), "{written}");
    assert!(written.contains("both done"), "{written}");
}

#[test]
fn the_mode_a_command_named_is_the_mode_the_next_turn_is_decided_under() {
    // `/mode` is answered on this thread, and the turn after it decides on
    // another. The row under the box is drawn from the mode read here on the
    // way in, and the call is decided by the engine that went with the runner:
    // one value, or the screen would be describing a session that is not the
    // one running. The turn starts in `ask`, which is the mode that would have
    // drawn the question this asserts is absent.
    let written = deciding(
        Mode::Ask,
        tools(Fixed::new("write", changing())),
        vec![calling("write"), saying("changed it")],
        "/mode allowEdits\nedit it\n",
    );

    assert!(written.contains("allow edits on"), "{written}");
    assert!(!written.contains("wants to change"), "{written}");
    assert!(written.contains("changed it"), "{written}");
}

// A terminal and a log that do what a real one will not do on request.
//
// What the loop writes to is where several of its promises are kept: the
// window is read again at every prompt, a write that fails must not take the
// turn's own record with it, and a log has to be finished by whichever thread
// is holding it. None of that can be driven from a real terminal on a real
// disk, so each of these is one thing going wrong, on purpose, at a moment a
// test chose.

/// How wide [`Narrowing`] leaves the window once it has been read once.
pub(super) const NARROW: usize = 10;

/// A terminal that narrows to ten columns once the renderer has read the size
/// it starts with.
///
/// The loop owns the renderer for its whole run, so nothing outside can resize
/// between turns the way a user does. This one resizes itself.
pub(super) struct Narrowing {
    inner: Recording,
    asked: Cell<usize>,
}

impl Narrowing {
    pub(super) fn new() -> Self {
        Self {
            inner: Recording::new(80, 24),
            asked: Cell::new(0),
        }
    }

    pub(super) fn written(&self) -> &str {
        self.inner.written()
    }
}

impl Terminal for Narrowing {
    fn size(&self) -> Result<Size, TerminalError> {
        let asked = self.asked.get();
        self.asked.set(asked + 1);

        Ok(Size {
            columns: if asked == 0 { 80 } else { NARROW },
            rows: 24,
        })
    }

    fn write(&mut self, text: &str) -> Result<(), TerminalError> {
        self.inner.write(text)
    }

    fn flush(&mut self) -> Result<(), TerminalError> {
        self.inner.flush()
    }

    fn is_terminal(&self) -> bool {
        self.inner.is_terminal()
    }
}

/// A terminal that takes `left` writes and refuses everything after them, the
/// way one whose window has been closed does.
pub(super) struct Breaking {
    pub(super) inner: Recording,
    pub(super) left: usize,
}

impl Terminal for Breaking {
    fn size(&self) -> Result<Size, TerminalError> {
        self.inner.size()
    }

    fn write(&mut self, text: &str) -> Result<(), TerminalError> {
        if self.left == 0 {
            return Err(TerminalError::Io(io::ErrorKind::BrokenPipe.into()));
        }

        self.left -= 1;
        self.inner.write(text)
    }

    fn flush(&mut self) -> Result<(), TerminalError> {
        self.inner.flush()
    }

    fn is_terminal(&self) -> bool {
        self.inner.is_terminal()
    }
}

/// A closing window whose failure waits until the provider has the request.
///
/// The worker records the prompt before it asks the provider. Waiting on that
/// boundary removes a scheduler race from the test above without making the
/// drawing side wait in production.
struct BreakingWhenStarted {
    inner: Recording,
    left: usize,
    started: Arc<AtomicUsize>,
}

impl Terminal for BreakingWhenStarted {
    fn size(&self) -> Result<Size, TerminalError> {
        self.inner.size()
    }

    fn write(&mut self, text: &str) -> Result<(), TerminalError> {
        if self.left == 0 {
            let until = Instant::now() + Duration::from_secs(2);
            while self.started.load(Ordering::Acquire) == 0 && Instant::now() < until {
                std::thread::park_timeout(Duration::from_millis(1));
            }
            return Err(TerminalError::Io(io::ErrorKind::BrokenPipe.into()));
        }

        self.left -= 1;
        self.inner.write(text)
    }

    fn flush(&mut self) -> Result<(), TerminalError> {
        self.inner.flush()
    }

    fn is_terminal(&self) -> bool {
        self.inner.is_terminal()
    }
}

/// A log that fails every write, the way a full disk does.
pub(super) struct Failing;

impl io::Write for Failing {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::StorageFull,
            "no space left on device",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A log the test can read back once the session has finished writing it.
#[derive(Debug)]
pub(super) struct Kept(pub(super) Arc<Mutex<Vec<u8>>>);

impl io::Write for Kept {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("a lock nothing panicked in")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_huge_line_without_a_newline_is_refused_before_it_is_retained() {
    let bytes = vec![b'x'; QUEUED_BYTES + 1];
    let mut input = io::BufReader::with_capacity(4096, Cursor::new(bytes));

    let problem = read(&mut input).expect_err("an oversized input line to be refused");

    assert!(matches!(problem, Fatal::InputTooLong), "{problem:?}");
    assert!(problem.to_string().contains("1 MiB"), "{problem}");
}

#[test]
fn a_prompt_that_cannot_be_answered_down_a_pipe_fails_rather_than_ending_quietly() {
    // Interactively this is a warning and the session carries on, because
    // `/model` is a key away. Down a pipe nobody can type it, so every line
    // after this one would be read and none of them answered — and the run
    // would end `Ok`, which is the one thing a script looks at. `echo ... |
    // crucible` reporting success while answering nothing is the "it does
    // nothing" report arriving as a zero exit.
    let runner = Runner::new(
        Box::new(Script::new(Vec::new())),
        Tools::new(),
        Model {
            name: String::new().into(),
            max_tokens: 64,
            window: None,
            system: None,
            effort: None,
        },
        Session::nowhere(),
    );

    let mut renderer = Renderer::new(Recording::redirected(80, 24));
    let mut input = Cursor::new(b"what is 2+2\n".to_vec());

    let problem = converse(runner, &mut renderer, &plain(), &opening(), &mut input)
        .expect_err("a run that answered nothing to fail");

    assert!(matches!(problem, Fatal::Unanswerable(_)), "{problem:?}");
}
