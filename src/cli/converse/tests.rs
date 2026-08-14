//! What a whole loop does: a turn, a question, and a window that changed.

use std::cell::Cell;
use std::io;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use crucible_auth::Store;
use crucible_core::{
    Delta, Mode, Permission, Rules, Settled, StopReason, ToolArgs, ToolCall, ToolId,
};
use crucible_runner::{Model, Session, Tools};
use crucible_tui::{Recording, Size, Terminal, TerminalError};

use super::*;
use crate::cli::fake::{Fixed, Script, changing, running};
use crate::cli::sample::Sample;

/// The terms a test runs under when neither the style nor cancelling is what
/// it is watching.
///
/// The file `always` would write to is inside a tree of this process's own and
/// is never created: a test that watches the writing points these terms at a
/// sample of its own instead.
fn plain() -> Terms {
    let unwritten = std::env::temp_dir().join(format!("crucible-unwritten-{}", std::process::id()));

    Terms {
        style: Style::plain(),
        cancel: Cancel::new(),
        remembering: crucible_config::local(&unwritten),

        // A provider, so `/model` has a name to write its answer under, and a
        // file inside the same absent tree so nothing a test types reaches a
        // configuration anybody keeps.
        provider: Cell::new(Some("anthropic")),
        choosing: unwritten.join("config.json"),

        // The same again: `/login` is driven where a store of its own is
        // watched, and a loop these terms drive must not write a key into
        // whatever home the machine running the suite has.
        logins: Store::in_home(&unwritten),

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
fn every_line_finished_during_a_turn_is_kept_in_the_order_it_was_typed() {
    // Return pressed twice while a long turn ran. The box takes the second
    // line as readily as the first and clears itself both times, so a queue
    // that kept only one of them loses a prompt the user watched it accept --
    // and it never runs, and nothing says so.
    let mut waiting = VecDeque::new();

    queue(&mut waiting, Some("run the tests".to_owned()));
    queue(&mut waiting, None);
    queue(&mut waiting, Some("now fix what failed".to_owned()));

    assert_eq!(waiting, ["run the tests", "now fix what failed"]);
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
            effort: None,
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
            effort: None,
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
    let runner = scripted(Script::new(vec![]), Tools::new())
        .permitting(Permission::with(Mode::FullAccess, Rules::new()));
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(Vec::new());

    converse(runner, &mut renderer, &plain(), &mut input).expect("the loop to finish");

    let written = renderer.terminal().written();
    assert!(written.contains("fullAccess › "), "{written}");
}

#[test]
fn the_box_and_the_mode_stand_under_a_turn_that_is_still_being_written() {
    // A turn is the longest a session goes without a prompt on screen, and it
    // is the stretch the mode is deciding things over -- both used to leave
    // with the box and come back only once there was nothing left to decide.
    // Pinned to the escape that parks the cursor as well as to the words: rows
    // drawn under the tail and not counted are rows the next frame rewinds
    // over, which would corrupt the turn rather than merely mislead about it.
    // The cursor comes back into the box rather than onto the answer, because
    // the box is what takes typing while the turn runs — two rows up from the
    // last of the four, and at the column the line starts on.
    let runner = scripted(Script::new(vec![saying("hello")]), Tools::new())
        .permitting(Permission::with(Mode::FullAccess, Rules::new()));
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"go\n".to_vec());

    converse(runner, &mut renderer, &plain(), &mut input).expect("the loop to finish");

    let written = renderer.terminal().written();
    assert!(written.contains("full access mode on"), "{written}");
    assert!(
        written.contains("hello\r\n\u{256d}"),
        "the box did not stand under the answer: {written}"
    );
    assert!(
        written.contains("\x1b[2A\x1b[5G"),
        "the cursor was not parked in the box: {written}"
    );
}

/// The moment in the middle of a turn, where the loop draws and waits at once.
/// The whole loop under terms of the test's own: what an answer leaves behind
/// depends on where those terms point.
fn answering(terms: &Terms, rounds: Vec<Vec<Delta>>, offered: Tools, typed: &str) -> String {
    let runner = scripted(Script::new(rounds), offered);

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(typed.as_bytes().to_vec());

    converse(runner, &mut renderer, terms, &mut input).expect("the loop to finish");

    renderer.terminal().written().to_string()
}
fn tools(tool: Fixed) -> Tools {
    let mut offered = Tools::new();
    offered.add(Box::new(tool));
    offered
}
/// The call the script below made, as the engine will be asked about it.
fn asking(name: &str) -> ToolCall {
    ToolCall {
        id: ToolId::new("a"),
        name: name.into(),
        args: ToolArgs::new("{}"),
    }
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

    converse(runner, &mut renderer, &plain(), &mut input).expect("the loop to finish");

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
            system: None,
            effort: None,
        },
        Session::nowhere(),
    );

    let mut renderer = Renderer::new(Recording::redirected(80, 24));
    let mut input = Cursor::new(b"what is 2+2\n".to_vec());

    let problem = converse(runner, &mut renderer, &plain(), &mut input)
        .expect_err("a run that answered nothing to fail");

    assert!(matches!(problem, Fatal::Unanswerable(_)), "{problem:?}");
}
