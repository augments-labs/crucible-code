//! What a whole loop does: a turn, a question, and a window that changed.

use std::cell::Cell;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use crucible_core::{Delta, Mode, StopReason};
use crucible_runner::{Model, Session, Tools};
use crucible_tui::{Recording, Size, TerminalError};

use super::*;
use crate::cli::fake::Script;

mod asked;

/// The terms a test runs under when neither the style nor cancelling is what
/// it is watching.
///
/// The file `always` would write to is inside a tree of this process's own and
/// is never created: a test that watches the writing points these terms at a
/// sample of its own instead.
fn plain() -> Terms {
    Terms {
        style: Style::plain(),
        mode: Mode::Ask,
        cancel: Cancel::new(),
        remembering: crucible_config::local(
            &std::env::temp_dir().join(format!("crucible-unwritten-{}", std::process::id())),
        ),
    }
}

/// A terminal that narrows to ten columns once the renderer has read the
/// size it starts with.
///
/// The loop owns the renderer for its whole run, so nothing outside can
/// resize between turns the way a user does. This one resizes itself.
struct Narrowing {
    inner: Recording,
    asked: Cell<usize>,
}

impl Narrowing {
    fn new() -> Self {
        Self {
            inner: Recording::new(80, 24),
            asked: Cell::new(0),
        }
    }

    fn written(&self) -> &str {
        self.inner.written()
    }
}

impl Terminal for Narrowing {
    fn size(&self) -> Result<Size, TerminalError> {
        let asked = self.asked.get();
        self.asked.set(asked + 1);

        Ok(Size {
            columns: if asked == 0 { 80 } else { 10 },
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
            system: None,
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

    converse(runner, &mut renderer, &plain(), &mut input).expect("the loop to finish");

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
fn a_window_the_user_resized_wraps_the_turns_that_follow_it() {
    // Catching the signal a resize sends needs `unsafe`, which this
    // workspace forbids, so a prompt is the only moment the loop can notice
    // one. Unnoticed, the width read at startup is the width every turn is
    // wrapped to for the rest of the session.
    let runner = scripted(Script::new(vec![saying("abcdefghijkl")]), Tools::new());
    let mut renderer = Renderer::new(Narrowing::new());
    let mut input = Cursor::new(b"go\n".to_vec());

    converse(runner, &mut renderer, &plain(), &mut input).expect("the loop to finish");

    let written = renderer.terminal().written();
    assert!(
        written.contains("abcdefghij\r\nkl"),
        "expected a wrap at the new width, got {written:?}"
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
fn a_blank_line_is_not_a_turn() {
    // Otherwise the return key alone sends an empty prompt and costs a
    // request. Counted at the provider rather than in what was drawn: the
    // renderer writes a line once live and again on its way to scrollback,
    // so counting appearances would count frames.
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

/// A log that fails every write, the way a full disk does.
struct Failing;

impl std::io::Write for Failing {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            "no space left on device",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
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
            system: None,
        },
        session,
    );

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(Vec::new());

    converse(runner, &mut renderer, &plain(), &mut input).expect("the loop to finish");

    let written = renderer.terminal().written();
    assert!(
        written.contains("stopped being recorded"),
        "the drained failure never reached the terminal: {written}"
    );
}

/// A terminal that takes `left` writes and refuses everything after them, the
/// way one whose window has been closed does.
struct Breaking {
    inner: Recording,
    left: usize,
}

impl Terminal for Breaking {
    fn size(&self) -> Result<Size, TerminalError> {
        self.inner.size()
    }

    fn write(&mut self, text: &str) -> Result<(), TerminalError> {
        if self.left == 0 {
            return Err(TerminalError::Io(std::io::ErrorKind::BrokenPipe.into()));
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

/// A log the test can read back once the session has finished writing it.
#[derive(Debug)]
struct Kept(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Kept {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("a lock nothing panicked in")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
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

    let runner = Runner::new(
        Box::new(Script::new(vec![saying("what the model said")])),
        Tools::new(),
        Model {
            name: "script".into(),
            max_tokens: 64,
            system: None,
        },
        session,
    );

    // One write is the prompt mark, colour and all, which the loop makes before
    // it reads. That leaves the first frame of the turn as what finds the
    // terminal gone -- after the worker has been handed the runner.
    let mut renderer = Renderer::new(Breaking {
        inner: Recording::new(80, 24),
        left: 1,
    });
    let mut input = Cursor::new(b"go\n".to_vec());

    let problem =
        converse(runner, &mut renderer, &plain(), &mut input).expect_err("the terminal to fail");

    assert!(matches!(problem, Fatal::Terminal(_)), "{problem:?}");

    let written = String::from_utf8(kept.lock().expect("a lock").clone()).expect("a log of text");
    assert!(
        written.contains("what the model said"),
        "the turn never reached the log: {written:?}"
    );
}

#[test]
fn the_prompt_the_loop_ended_at_is_a_row_of_its_own() {
    // The mark is written straight through the terminal so it can be left
    // without a line ending while the user types after it, which leaves the
    // renderer no row to settle. Unless the loop ends that row, the next thing
    // drawn lands on top of it — a report below, or the shell's own prompt
    // once crucible is gone.
    let written = conversing(vec![], Tools::new(), "");

    assert!(written.ends_with('\n'), "{written:?}");
}

#[test]
fn the_prompt_line_names_the_mode_in_force() {
    // fullAccess is the mode worth pinning: in `ask` every sensitive call
    // announces itself with a question, so the prompt line is the only place a
    // session that never asks says what it is. Written before the read, which
    // is why empty input still shows it once.
    let runner = scripted(Script::new(vec![]), Tools::new());
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(Vec::new());

    let terms = Terms {
        mode: Mode::FullAccess,
        ..plain()
    };
    converse(runner, &mut renderer, &terms, &mut input).expect("the loop to finish");

    let written = renderer.terminal().written();
    assert!(written.contains("fullAccess › "), "{written}");
}
