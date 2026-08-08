//! What a whole loop does: a turn, a question, and a window that changed.

use std::cell::Cell;
use std::io::Cursor;

use crucible_core::{Delta, Sensitivity, StopReason, ToolId};
use crucible_runner::{Model, Session, Tools};
use crucible_tui::{Recording, Size, TerminalError};

use super::*;
use crate::cli::fake::{Fixed, Script};

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

    converse(runner, &mut renderer, &Cancel::new(), &mut input).expect("the loop to finish");

    (
        renderer.terminal().written().to_string(),
        asked.load(std::sync::atomic::Ordering::Relaxed),
    )
}

fn tools(tool: Fixed) -> Tools {
    let mut offered = Tools::new();
    offered.add(Box::new(tool));
    offered
}

fn saying(text: &str) -> Vec<Delta> {
    vec![
        Delta::Text(text.into()),
        Delta::Stopped(StopReason::Yielded),
    ]
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

    converse(runner, &mut renderer, &Cancel::new(), &mut input).expect("the loop to finish");

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
fn a_question_asked_mid_turn_is_answered_from_the_same_input() {
    // The turn blocks on the answer while the loop is drawing its events.
    // Both are on the one channel, so this deadlocks if they are not.
    let written = conversing(
        vec![calling("write"), saying("changed it")],
        tools(Fixed::new("write", Sensitivity::MutatesFile)),
        "edit it\ny\n",
    );

    assert!(written.contains("wants to change a file"), "{written}");
    assert!(written.contains("changed it"), "{written}");
}

#[test]
fn refusing_a_tool_ends_the_turn_where_the_user_can_see_why() {
    let written = conversing(
        vec![calling("write")],
        tools(Fixed::new("write", Sensitivity::MutatesFile)),
        "edit it\nn\n",
    );

    assert!(written.contains("write was not allowed"), "{written}");
}

#[test]
fn a_question_left_unanswered_at_end_of_input_is_refused() {
    // The input ends mid-question. Nothing consented, so nothing runs, and
    // the loop still returns instead of waiting on a pipe that is closed.
    let written = conversing(
        vec![calling("write")],
        tools(Fixed::new("write", Sensitivity::MutatesFile)),
        "edit it\n",
    );

    assert!(written.contains("was not allowed"), "{written}");
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
fn yes_allows_this_call_only() {
    assert_eq!(verdict(Some("y\n")), Verdict::AllowOnce);
    assert_eq!(verdict(Some("yes")), Verdict::AllowOnce);
}

#[test]
fn always_allows_calls_like_it_for_the_rest_of_the_session() {
    assert_eq!(verdict(Some("a\n")), Verdict::AllowSession);
    assert_eq!(verdict(Some("always")), Verdict::AllowSession);
}

#[test]
fn anything_else_is_a_refusal() {
    // Including the empty line, which is what someone types when they meant
    // to read the question first.
    for answer in ["n", "no", "", "\n", "yeah", "Y E S", "1"] {
        assert_eq!(verdict(Some(answer)), Verdict::Deny, "{answer:?}");
    }
}

#[test]
fn end_of_input_is_a_refusal() {
    // A pipe that closed mid-question cannot consent to anything.
    assert_eq!(verdict(None), Verdict::Deny);
}
