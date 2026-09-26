//! What each key does to the list of commands still running.
//!
//! The loop that reads them cannot be driven from here — the keyboard it reads is
//! the process's own — so what is tested is the function of a key, which is where
//! the decision actually lives.

use crucible_builtins::{Background, Bash};
use crucible_core::{
    Ancestry, Ask, Calibration, CallResultKey, CallResultReceipt, CallResultStoreError, Cancel,
    Compacted, ContextError, ContextPatch, ContextSnapshot, DescribeTool, InvocationId,
    JournalStore, Message, Mode, Permission, Remember, Rules, RunItem, Sensitivity, SessionId,
    SessionOwner, SessionStore, Settled, Tool, ToolArgs, ToolCall, ToolContext, ToolId, ToolResult,
    Unwatched, Verdict,
};
use crucible_runtime::BoxFuture;
use crucible_sandbox_local::LocalSandbox;
use sha2::{Digest, Sha256};

use crate::cli::sample::Sample;

use super::*;

/// A registry with `count` commands running in it.
///
/// `case` has to be this test's own and nobody else's: the tree it names is
/// removed and remade, so two tests in one process sharing a name delete each
/// other's workspace part way through.
///
/// Real ones, started through the real path: the registry only holds what it was
/// handed, so a list of processes with no processes behind it would be a test
/// about nothing. The verdict comes from the engine in the mode that asks about
/// nothing, which is the only way anything outside it can obtain one.
fn running(case: &str, count: usize) -> (Background, Sample) {
    running_with(case, count, std::sync::Arc::new(local()))
}

/// A runtime of this test binary's own, whose threads run a command's status
/// task, read its output and own a command left running, while a test waits
/// on the command. Built with the I/O driver a command's pipes are waited on
/// with on Unix, as the application's is.
fn runtime() -> tokio::runtime::Handle {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_io()
                .enable_time()
                .build()
                .expect("a runtime to watch commands on")
        })
        .handle()
        .clone()
}

/// This machine's confinement, watching each command it starts on the test
/// binary's runtime.
fn local() -> LocalSandbox {
    LocalSandbox::new().watching_on(runtime())
}

fn running_with(
    case: &str,
    count: usize,
    sandbox: std::sync::Arc<dyn crucible_core::SandboxService>,
) -> (Background, Sample) {
    let left = Background::new();
    left.watching_on(runtime());
    let here = started(&left, case, count, sandbox);
    (left, here)
}

/// Starts `count` commands left running in `left`, through the real tool.
fn started(
    left: &Background,
    case: &str,
    count: usize,
    sandbox: std::sync::Arc<dyn crucible_core::SandboxService>,
) -> Sample {
    let here = Sample::new(case);
    let cancel = Cancel::new();
    // This fixture exercises the real process/background path, but not Linux
    // namespace availability. Selecting the compatibility backend explicitly
    // keeps that boundary visible instead of depending on the host running the
    // test to permit nested user namespaces.
    let tool = Bash::new(here.workspace(), sandbox)
        .sandboxing(false)
        .leaving(left.clone());
    let mut engine = Permission::with(Mode::FullAccess, Rules::default());

    for at in 0..count {
        let call = ToolCall {
            id: ToolId::new(format!("{case}-{at}")),
            name: tool.name().into(),
            args: ToolArgs::new(r#"{"command":"sleep 30","background":true}"#),
        };

        let Settled::Approved(approved) = crucible_runtime::answered!(engine.decide(
            &call,
            &tool.sensitivity(&call.args),
            &mut Nobody
        )) else {
            panic!("full access asked about a command");
        };

        let context = ToolContext::new(Ancestry::new(), call.id.clone(), &cancel, None, &Unwatched)
            .with_invocation(InvocationId::new());
        // Awaited on the test's runtime, as a turn awaits a call: the tool
        // starts the readers of a command's output on the runtime awaiting it.
        let output = runtime()
            .block_on(tool.run(approved, &context))
            .expect("the command started");
        assert!(
            !output.is_failed(),
            "a command this test needs running was refused: {}",
            output.text()
        );

        // A detached command stays in the registry only once its start result is
        // durably accepted; an abandoned acceptance ends the process instead.
        let pending = context
            .take_call_result()
            .expect("the pending result slot")
            .expect("a detached command leaves a result to accept");
        let result = ToolResult {
            id: call.id.clone(),
            output: output.into_recorded(),
        };
        let receipt = crucible_runtime::answered!(JOURNAL.put_call_result(pending.key(), &result))
            .expect("the test journal stores the result");
        crucible_runtime::answered!(pending.accept(receipt))
            .expect("the detached command accepts its receipt");
    }

    // Asserted rather than assumed. What these tests are about is what a key does
    // to a list, and a list one command short would fail them somewhere the reason
    // is not written down.
    assert_eq!(
        left.count(),
        count,
        "the registry did not take every command this test started"
    );

    here
}

mod cleanup;

#[test]
fn failed_stop_keeps_the_last_row_and_its_retry_notice() {
    let (sandbox, denied) = cleanup::sandbox();
    let (left, _here) = running_with("failed-stop", 1, sandbox);
    let mut leaving = Leaving::default();
    let number = left.running().first().expect("running command").number;
    drop(leaving.rows(&left, 80, 24, Glyphs::Unicode));

    assert_eq!(
        leaving.against(Pressed::Key(Key::Char('x')), &left),
        Moved::Redraw,
        "failed cleanup closed the panel"
    );
    waiting_until("the refused stop", || {
        left.running().first().is_some_and(|one| one.refused)
    });
    assert_eq!(
        leaving.watched(&left),
        Moved::Redraw,
        "the refusal was not drawn on the next beat"
    );
    assert_eq!(left.running().first().map(|one| one.number), Some(number));
    for (columns, room, glyphs) in [(80, 24, Glyphs::Unicode), (24, 8, Glyphs::Ascii)] {
        let rows = leaving.rows(&left, columns, room, glyphs);
        let text = rows.iter().map(Row::text).collect::<Vec<_>>().join("\n");
        assert!(text.contains("Stop failed; x retries"), "{text}");
        assert!(
            !text.contains(cleanup::PRIVATE_ERROR),
            "opaque error reached the panel"
        );
        assert!(rows.len() <= room);
        assert!(
            rows.iter()
                .all(|row| crucible_tui::columns(&row.text()) <= columns)
        );
    }

    denied.store(false, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        leaving.against(Pressed::Key(Key::Char('x')), &left),
        Moved::Redraw
    );
    waiting_until("the stopped command's row going", || left.count() == 0);
    assert_eq!(leaving.watched(&left), Moved::Left);
}

/// Waits, up to a ceiling no passing run comes near, until `until` holds.
fn waiting_until(what: &str, until: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !until() {
        assert!(
            std::time::Instant::now() < deadline,
            "{what} never happened"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// The command line lets go of the registry of commands left running before it
/// shuts the runtime down, so a command still running at exit is ended by its
/// owner on a runtime that is still running, rather than on the thread
/// letting go, or on one already draining where the stop would never run.
#[test]
fn a_command_left_running_at_exit_is_ended_on_the_runtime_before_it_is_shut_down() {
    let (started_one, stopped) = crate::cli::leaving_first(Background::new, |services, left| {
        let runtime = services.runtime().handle()?;
        left.watching_on(runtime.clone());
        let (sandbox, on_runtime) = cleanup::counting_on(runtime);
        let here = started(left, "ended-at-exit", 1, sandbox);
        Ok::<_, crucible_app::runtime::Unstarted>((here, on_runtime))
    });

    let (_here, on_runtime) = started_one.expect("the runtime could not be started");
    assert_eq!(stopped, Ok(()), "the runtime was left with work on it");
    assert_eq!(
        on_runtime.load(std::sync::atomic::Ordering::Acquire),
        1,
        "the command left running was not ended on the runtime before it shut down"
    );
}

/// The beat a standing panel is looked at again on without a key: the frame
/// a key's own outcome is shown on.
const FRAME: std::time::Duration = std::time::Duration::from_millis(250);

#[test]
fn the_key_that_stops_one_returns_at_once_while_its_stop_stalls() {
    let (sandbox, stalled) = cleanup::stalling();
    let (left, _here) = running_with("stalled-stop", 1, sandbox);
    let mut leaving = Leaving::default();
    stalled.store(true, std::sync::atomic::Ordering::Release);

    let began = std::time::Instant::now();
    let moved = leaving.against(Pressed::Key(Key::Char('x')), &left);
    let took = began.elapsed();
    stalled.store(false, std::sync::atomic::Ordering::Release);

    assert!(
        took < FRAME,
        "the key waited {took:?} on a stop that stalled"
    );
    assert_eq!(moved, Moved::Redraw);
}

/// Answers nothing, because in this mode nothing is asked.
struct Nobody;

/// A journal that keeps nothing but can still receipt a start result, which is
/// all a detached command needs before the registry may keep it.
struct Journal;

static JOURNAL: Journal = Journal;

/// Nothing model-visible is recorded here: what a detached command needs is a
/// receipt for its start result, and a journal that answered a context back
/// would be describing a session these tests never had.
impl SessionStore for Journal {
    fn session_id(&self) -> Option<SessionId> {
        None
    }

    fn owner(&self) -> Option<SessionOwner> {
        None
    }

    fn append_message<'a>(&'a self, _message: &'a Message) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn context_snapshot(&self) -> Option<ContextSnapshot> {
        None
    }

    fn contextual<'a>(
        &'a self,
        _patch: &'a ContextPatch,
    ) -> BoxFuture<'a, Result<(), ContextError>> {
        Box::pin(async move { Ok(()) })
    }

    fn compacted<'a>(&'a self, _replaced: usize, _recap: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn display_compacted(&self, _compacted: Compacted, _pruned: bool) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }

    fn pruned<'a>(&'a self, _freed: usize, _results: &'a [ToolId]) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn restricted<'a>(
        &'a self,
        _freed: usize,
        _results: &'a [ToolId],
        _notice: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn measured<'a>(&'a self, _calibration: &'a Calibration) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn calibrated(&self) -> Option<Calibration> {
        None
    }
}

impl JournalStore for Journal {
    fn append_run_item<'a>(&'a self, _item: &'a RunItem) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn put_call_result<'a>(
        &'a self,
        key: CallResultKey,
        result: &'a ToolResult,
    ) -> BoxFuture<'a, Result<CallResultReceipt, CallResultStoreError>> {
        Box::pin(async move {
            let mut digest = Sha256::new();
            digest.update(b"crucible:leaving-test-call-result:v1\0");
            digest.update(key.bytes());
            digest.update(result.id.as_str().as_bytes());
            digest.update(result.output.text().as_bytes());
            digest.update([u8::from(result.output.is_failed())]);
            Ok(CallResultReceipt::from_digest(digest.finalize().into()))
        })
    }
}

impl Ask for Nobody {
    fn ask<'a>(
        &'a mut self,
        _call: &'a ToolCall,
        _sensitivity: &'a Sensitivity,
    ) -> BoxFuture<'a, (Verdict, Remember)> {
        Box::pin(async { (Verdict::Deny, Remember::Never) })
    }
}

#[test]
fn the_arrows_walk_the_list_and_stop_at_its_ends() {
    let (left, _here) = running("arrows", 2);
    let mut leaving = Leaving::default();

    assert_eq!(leaving.against(Pressed::Up, &left), Moved::Still);
    assert_eq!(leaving.against(Pressed::Down, &left), Moved::Redraw);
    assert_eq!(leaving.at, 1);
    assert_eq!(
        leaving.against(Pressed::Down, &left),
        Moved::Still,
        "the mark walked off the end of the list"
    );
}

#[test]
fn a_click_on_a_row_marks_it_and_a_second_click_opens_it() {
    // The first click is the arrows' work done at once — it moves the mark to
    // the row pointed at; the second, on the row already marked, is the enter
    // that shows what the command has printed.
    let (left, _here) = running("clicking", 2);
    let mut leaving = Leaving::default();

    // Row HEAD is the first command, HEAD + 1 the second; the rows above them
    // are the heading and its blanks. `Pressed` is not `Copy` — a paste carries
    // a string — so the same click is spelled twice.
    let second = || Pressed::Clicked {
        row: super::HEAD + 1,
        column: 4,
    };
    assert_eq!(leaving.against(second(), &left), Moved::Redraw);
    assert_eq!(leaving.at, 1, "the click marked the second row");
    assert!(leaving.shown.is_none(), "one click only marks");

    assert_eq!(leaving.against(second(), &left), Moved::Redraw);
    assert!(leaving.shown.is_some(), "a second click opened it");
}

#[test]
fn a_click_on_the_chrome_or_past_the_list_is_a_click_on_nothing() {
    let (left, _here) = running("clicking-past", 1);

    // The heading row, and a row below the one command there is.
    for row in [0, super::HEAD - 1, super::HEAD + 1] {
        let mut leaving = Leaving::default();
        assert_eq!(
            leaving.against(Pressed::Clicked { row, column: 2 }, &left),
            Moved::Still,
            "row {row}"
        );
        assert_eq!(leaving.at, 0, "row {row} moved the mark");
        assert!(leaving.shown.is_none(), "row {row} opened something");
    }
}

#[test]
fn the_key_that_opened_it_closes_it() {
    // What every other `ctrl+` key here does, and what makes it a toggle rather
    // than a door.
    let (left, _here) = running("closing", 1);

    for closing in [Pressed::Background, Pressed::Escape] {
        let mut leaving = Leaving::default();
        assert_eq!(leaving.against(closing, &left), Moved::Left);
    }
}

#[test]
fn stopping_the_last_one_takes_the_list_with_it() {
    // Nothing left to stand, and a frame of empty chrome is worse than the row
    // under the box that opened this.
    let (left, _here) = running("stopping-last", 1);
    let mut leaving = Leaving::default();

    assert_eq!(
        leaving.against(Pressed::Key(Key::Char('x')), &left),
        Moved::Redraw
    );
    waiting_until("the command ending", || left.count() == 0);
    assert_eq!(leaving.watched(&left), Moved::Left);
}

#[test]
fn stopping_one_of_several_keeps_the_list_open() {
    let (left, _here) = running("stopping-one", 2);
    let mut leaving = Leaving::default();
    drop(leaving.rows(&left, 80, 24, Glyphs::Unicode));

    assert_eq!(
        leaving.against(Pressed::Key(Key::Char('x')), &left),
        Moved::Redraw
    );
    waiting_until("one command ending", || left.count() == 1);
    assert_eq!(leaving.watched(&left), Moved::Redraw);
}

#[test]
fn enter_shows_what_one_has_printed_and_the_way_back_is_the_list() {
    let (left, _here) = running("showing", 1);
    let mut leaving = Leaving::default();

    assert_eq!(
        leaving.against(Pressed::Key(Key::Enter), &left),
        Moved::Redraw
    );
    assert!(leaving.shown.is_some(), "nothing was opened");

    // And out of it into the list rather than out of both: this was opened from
    // there, and the way back is where the reader came from.
    assert_eq!(leaving.against(Pressed::Escape, &left), Moved::Redraw);
    assert!(leaving.shown.is_none());
}

#[test]
fn a_command_that_ended_while_the_list_was_open_brings_the_mark_back_inside_it() {
    let (left, _here) = running("ended-while-open", 2);
    let mut leaving = Leaving::default();
    leaving.against(Pressed::Down, &left);
    assert_eq!(leaving.at, 1);

    let numbers: Vec<usize> = left.running().iter().map(|one| one.number).collect();
    if let Some(last) = numbers.last() {
        left.stop(*last).expect("background cleanup");
    }
    waiting_until("the stopped command's row going", || left.count() == 1);

    drop(leaving.rows(&left, 80, 24, Glyphs::Unicode));

    assert_eq!(
        leaving.at, 0,
        "the mark was left pointing at a command that had gone"
    );
}
