//! What a write made through the store contract waits for.
//!
//! The runner writes through [`SessionStore`], and each of those writes
//! answers only once the thread that owns the file has taken its line: the
//! bytes are with the operating system, or the failure that stopped them is
//! already the session's trouble. The writes a session makes through its own
//! methods do not wait, and are the other half of what these tests set the
//! awaited ones beside.

use std::future::Future;
use std::path::Path;
use std::pin::pin;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use crucible_core::{SessionStore, Workspace};

use super::*;
use crate::session::QUEUE;

/// Waits for `future` on a runtime of its own, as a turn is waited for.
fn waited<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a test runtime")
        .block_on(future)
}

/// Asks `future` once, with a waker that wakes nothing.
fn asked_once<F: Future>(future: std::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// A log that takes each write only once the test lets it, and keeps what it
/// took. Dropping the sender lets every write through.
struct Gated {
    gate: Receiver<()>,
    written: Written,
    /// Set when the writer thread lets go of the sink, which is when it ends.
    gone: Arc<AtomicBool>,
}

/// A session writing onto a [`Gated`] log, the sender that opens its gate,
/// what the log took, and whether the writer has let go of it.
///
/// The session comes first so that it is dropped last: a test that fails with
/// the gate still shut opens it on the way out, rather than leaving the
/// session's drop waiting on a writer that can never finish.
fn gated(
    name: &str,
) -> (
    Session,
    std::sync::mpsc::Sender<()>,
    Written,
    Arc<AtomicBool>,
) {
    let (open, gate) = channel();
    let written = Written::default();
    let gone = Arc::new(AtomicBool::new(false));
    let sink = Gated {
        gate,
        written: Arc::clone(&written),
        gone: Arc::clone(&gone),
    };
    (
        Session::writing(PathBuf::from(name), sink),
        open,
        written,
        gone,
    )
}

impl std::io::Write for Gated {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let _ = self.gate.recv();
        self.written
            .lock()
            .expect("the test is holding it")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for Gated {
    fn drop(&mut self) {
        self.gone.store(true, Ordering::Release);
    }
}

/// How many lines `written` holds.
fn lines(written: &Written) -> usize {
    String::from_utf8_lossy(&written.lock().expect("the writer is not holding it"))
        .lines()
        .count()
}

#[test]
fn a_write_through_the_store_answers_only_once_the_log_has_its_line() {
    let (session, open, written, _) = gated("gated.jsonl");
    let message = said("kept before the turn goes on");

    let mut appending = pin!(SessionStore::append_message(&session, &message));

    assert!(
        asked_once(appending.as_mut()).is_pending(),
        "the write answered while the log had not taken its line"
    );
    // Long enough for the writer to have taken the line off the queue and be
    // held at the gate, so a writer that answered on taking the line rather
    // than on writing it has answered by now.
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        asked_once(appending.as_mut()).is_pending(),
        "the write answered while its line was held at the gate"
    );
    drop(open);
    waited(appending);
    assert_eq!(
        lines(&written),
        1,
        "the write answered before its line reached the log"
    );
}

#[test]
fn a_write_the_log_refused_is_trouble_by_the_time_it_answers() {
    // Released a moment after the write was asked for, so a write that did not
    // wait would answer before the log had failed and find nothing to report.
    let (release, held) = channel();
    let session = Session::writing(PathBuf::from("refused.jsonl"), Blocked { held });
    let releasing = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        release.send(()).expect("the writer is waiting");
    });

    waited(SessionStore::append_message(
        &session,
        &said("lost to a full disk"),
    ));

    assert!(
        session.trouble().is_some(),
        "the write answered before the failure it met was recorded"
    );
    releasing.join().expect("the release");
}

/// A log whose writer comes apart at its first line, without the hook that
/// prints a panic: what is being shown is what the session says, not what the
/// test harness does.
struct ComingApart;

impl std::io::Write for ComingApart {
    fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
        std::panic::resume_unwind(Box::new("the writer came apart"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_write_no_writer_acknowledged_is_trouble() {
    let session = Session::writing(PathBuf::from("apart.jsonl"), ComingApart);

    waited(SessionStore::append_message(&session, &said("never taken")));
    let first = session.trouble();
    waited(SessionStore::measured(&session, &reading(10, 1)));

    assert!(
        first.is_some(),
        "a line the writer never acknowledged went unreported"
    );
    assert_eq!(
        session.finish(),
        first,
        "the next write, and the end, report the same first failure"
    );
}

#[test]
fn a_write_meeting_a_full_queue_waits_for_room_without_holding_its_thread() {
    let (session, open, written, _) = gated("full.jsonl");
    // One line the writer holds while the gate is shut, and a queue's worth
    // behind it.
    for nth in 0..=QUEUE {
        session.append(&said(&format!("queued {nth}")));
    }

    let last = said("the one that waited for room");
    let mut appending = pin!(SessionStore::append_message(&session, &last));

    assert!(
        asked_once(appending.as_mut()).is_pending(),
        "a write into a full queue answered without its line being taken"
    );
    drop(open);
    waited(appending);
    let log = String::from_utf8(written.lock().expect("the writer is idle").clone())
        .expect("a log of text");
    assert_eq!(log.lines().count(), QUEUE + 2, "{log}");
    assert!(
        log.lines()
            .last()
            .is_some_and(|line| line.contains("the one that waited for room")),
        "the write that waited was not the last line: {log}"
    );
}

/// A log whose writer comes apart at the first line the gate lets through.
struct ComingApartOnceOpened {
    gate: Receiver<()>,
}

impl std::io::Write for ComingApartOnceOpened {
    fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
        let _ = self.gate.recv();
        std::panic::resume_unwind(Box::new("the writer came apart"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_write_waiting_for_room_answers_once_the_writer_is_gone() {
    // The writer holds one line at the gate and a queue's worth waits behind
    // it, so the awaited write waits for room; then the writer comes apart.
    // No line will ever be taken off the queue again, and a write still
    // waiting for one would never answer: it is waited for on a thread of its
    // own, so that failing here is a failure rather than a hang.
    let (open, gate) = channel();
    let session = Arc::new(Session::writing(
        PathBuf::from("apart.jsonl"),
        ComingApartOnceOpened { gate },
    ));
    for nth in 0..=QUEUE {
        session.append(&said(&format!("queued {nth}")));
    }
    let (answered, answer) = channel();
    let writing = Arc::clone(&session);
    std::thread::spawn(move || {
        waited(SessionStore::append_message(
            &*writing,
            &said("waiting for room"),
        ));
        let _ = answered.send(writing.trouble());
    });
    // Long enough for the write to find the queue full and wait for room.
    std::thread::sleep(Duration::from_millis(50));

    drop(open);

    let trouble = answer
        .recv_timeout(Duration::from_secs(10))
        .expect("the write waiting for room never answered once the writer was gone");
    assert!(
        trouble.is_some(),
        "the write the writer never took went unreported"
    );
}

/// A log that takes a while over every write, and says when the writer
/// thread lets go of it.
struct Slowed {
    written: Written,
    gone: Arc<AtomicBool>,
}

impl std::io::Write for Slowed {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        std::thread::sleep(Duration::from_millis(20));
        self.written
            .lock()
            .expect("the test is holding it")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for Slowed {
    fn drop(&mut self) {
        self.gone.store(true, Ordering::Release);
    }
}

#[test]
fn dropping_a_session_ends_the_thread_that_wrote_it() {
    // The log is slow, so a line queued behind the awaited one is still being
    // written when the session is dropped: a drop that did not wait for the
    // writer would return with the thread still holding the log.
    let written = Written::default();
    let gone = Arc::new(AtomicBool::new(false));
    let session = Session::writing(
        PathBuf::from("ended.jsonl"),
        Slowed {
            written: Arc::clone(&written),
            gone: Arc::clone(&gone),
        },
    );
    waited(SessionStore::append_message(
        &session,
        &said("the last line"),
    ));
    session.append(&said("queued behind it and never awaited"));

    drop(session);

    assert!(
        gone.load(Ordering::Acquire),
        "the writer thread was still holding the log after the session was dropped"
    );
    assert_eq!(lines(&written), 2, "the queue was not drained first");
}

/// Where the process [`the_process_the_next_test_kills`] writes its session.
const KILLED_LOGS: &str = "CRUCIBLE_TEST_KILLED_SESSION_LOGS";
/// The workspace that session is about.
const KILLED_WORKSPACE: &str = "CRUCIBLE_TEST_KILLED_SESSION_WORKSPACE";
/// The file it makes once every one of its writes has been acknowledged.
const KILLED_READY: &str = "CRUCIBLE_TEST_KILLED_SESSION_READY";

/// What the killed process wrote, in order.
const SPOKEN: [&str; 3] = ["the first thing said", "the second", "the third"];

/// Records [`SPOKEN`] through the store, says so, and waits to be killed.
///
/// A test in its own right only so that the test binary can be started as
/// this process; run any other way, it finds nothing to do.
#[test]
fn the_process_the_next_test_kills() {
    let (Some(logs), Some(workspace), Some(ready)) = (
        std::env::var_os(KILLED_LOGS),
        std::env::var_os(KILLED_WORKSPACE),
        std::env::var_os(KILLED_READY),
    ) else {
        return;
    };
    let workspace = Workspace::open(PathBuf::from(workspace)).expect("the workspace");
    let session = Session::start(Path::new(&logs), &workspace, None).expect("a new session");

    for spoken in SPOKEN {
        waited(SessionStore::append_message(&session, &said(spoken)));
    }
    fs::write(ready, b"acknowledged\n").expect("the ready file");

    loop {
        std::thread::park();
    }
}

#[test]
fn acknowledged_lines_survive_the_process_being_killed() {
    let sample = Sample::new("session-killed");
    let ready = sample.logs().join("ready");
    let mut killed = Command::new(std::env::current_exe().expect("the test binary"))
        .args([
            "--exact",
            "session::tests::acknowledged::the_process_the_next_test_kills",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(KILLED_LOGS, sample.logs())
        .env(KILLED_WORKSPACE, sample.workspace().root())
        .env(KILLED_READY, &ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("the process to kill");

    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready.exists() && Instant::now() < deadline {
        if killed.try_wait().expect("its status").is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    killed.kill().expect("the kill");
    killed.wait().expect("the killed process reaped");
    assert!(
        ready.exists(),
        "the process never said its writes were taken"
    );

    let (_, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the killed session");

    assert_eq!(
        transcript.messages(),
        SPOKEN.map(said).as_slice(),
        "a write acknowledged before the kill is missing from the log"
    );
}
