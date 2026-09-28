//! What saying something to a confined process has to guarantee.

use std::io::{self, ErrorKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crucible_runtime::BoxFuture;
use crucible_sandbox::SandboxInput;

use crate::testing::runtime;
use crate::{FRAME_BYTES, FrameError, Written};

use super::*;

/// How long a test whose subject is a peer that stopped reading lets one
/// frame sit before calling the peer gone.
const PATIENCE: Duration = Duration::from_millis(50);

/// How long a test whose subject is not a peer that stopped reading lets one
/// frame sit.
///
/// Every frame is written by a task on the runtime, which reports back to the
/// test's thread, and a loaded machine can put far more than [`PATIENCE`]
/// between a task being woken and it running. Long past that, so a case that
/// is not about the patience fails on the code rather than on the machine; a
/// case that passes never waits it out.
const WILLING: Duration = Duration::from_secs(5);

/// How long a test waits for a quiet channel before deciding it is finished.
const SETTLE: Duration = Duration::from_millis(20);

/// How long a test waits for a task it does not control to get somewhere.
const LATEST: Duration = Duration::from_secs(2);

/// A peer that reads everything crucible says.
struct Kept {
    /// Where the bytes are handed for the test to read.
    to: Sender<Vec<u8>>,
}

impl SandboxInput for Kept {
    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> BoxFuture<'a, io::Result<usize>> {
        Box::pin(async move {
            let _ = self.to.send(bytes.to_vec());
            Ok(bytes.len())
        })
    }
}

/// A peer that reads, slowly, and never stops.
struct Slow {
    /// How long each write takes to be taken.
    pause: Duration,
    /// Where the bytes are handed for the test to read.
    to: Sender<Vec<u8>>,
}

impl SandboxInput for Slow {
    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> BoxFuture<'a, io::Result<usize>> {
        Box::pin(async move {
            tokio::time::sleep(self.pause).await;
            let _ = self.to.send(bytes.to_vec());
            Ok(bytes.len())
        })
    }
}

/// What a peer that stopped reading saw of whoever wrote to it.
#[derive(Default)]
struct Seen {
    /// Writes asked of it on a thread that is not the runtime's.
    elsewhere: AtomicUsize,
    /// Whether the pipe has been let go of.
    released: AtomicBool,
}

/// A peer that has stopped reading: a write to it never finishes.
struct Deaf(Arc<Seen>);

impl SandboxInput for Deaf {
    fn write<'a>(&'a mut self, _bytes: &'a [u8]) -> BoxFuture<'a, io::Result<usize>> {
        if !crate::testing::on_runtime() {
            self.0.elsewhere.fetch_add(1, Ordering::Relaxed);
        }
        Box::pin(std::future::pending())
    }
}

impl Drop for Deaf {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::Relaxed);
    }
}

/// A peer that has gone.
struct Gone;

impl SandboxInput for Gone {
    fn write<'a>(&'a mut self, _bytes: &'a [u8]) -> BoxFuture<'a, io::Result<usize>> {
        Box::pin(async {
            Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "the far end has gone",
            ))
        })
    }
}

/// A frame writer over `to`.
fn said(to: impl SandboxInput + 'static, patience: Duration) -> Said {
    Said::new(Box::new(to), patience, runtime())
}

/// A frame writer over a peer that has stopped reading, and what it saw.
fn deaf() -> (Written<Said>, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    (Written::new(said(Deaf(Arc::clone(&seen)), PATIENCE)), seen)
}

/// Waits for `settled` to hold, so a test never races a task it started.
fn until(settled: impl Fn() -> bool) -> bool {
    let began = Instant::now();
    while began.elapsed() < LATEST {
        if settled() {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    settled()
}

/// Everything the peer has been handed, once it has gone quiet.
fn collected(arrived: &Receiver<Vec<u8>>) -> String {
    let mut bytes = Vec::new();
    while let Ok(chunk) = arrived.recv_timeout(SETTLE) {
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A frame writer over a peer that reads everything, and its far end.
fn saying(patience: Duration) -> (Written<Said>, Receiver<Vec<u8>>) {
    let (to, arrived) = mpsc::channel();
    (Written::new(said(Kept { to }, patience)), arrived)
}

/// Which ending a frame reached, where the guard cares that it is the pipe's.
fn because(failure: &FrameError) -> String {
    failure.to_string()
}

/// The everyday case. A frame crucible sends has to arrive as one line, or the
/// far end joins it to whatever comes next and reads neither.
#[test]
fn what_crucible_says_reaches_the_program() {
    let (mut writing, arrived) = saying(WILLING);

    let sent = writing.send(r#"{"method":"tools/list"}"#);

    assert!(
        sent.is_ok(),
        "a peer that reads must hear crucible: {sent:?}"
    );
    assert_eq!(collected(&arrived), "{\"method\":\"tools/list\"}\n");
}

/// A frame is handed over whole or not at all. Half a frame on the wire is
/// worse than none: the far end reads it joined to the next one.
#[test]
fn a_frame_is_handed_over_whole_or_not_at_all() {
    let (to, arrived) = mpsc::channel();
    let mut said = said(Kept { to }, WILLING);

    let began = said.write_all(br#"{"method":"ready"}"#);
    thread::sleep(SETTLE);
    let early = arrived.try_recv();
    let ended = said.write_all(b"\n").and_then(|()| said.flush());

    assert!(began.is_ok() && ended.is_ok(), "{began:?} {ended:?}");
    assert!(
        early.is_err(),
        "no part of a frame may reach the peer before it is whole: {early:?}"
    );
    assert_eq!(collected(&arrived), "{\"method\":\"ready\"}\n");
}

/// The reason this exists. A peer that stopped reading is indistinguishable
/// from one that is slow, so crucible spends a patience on it and then says
/// so, rather than parking the host on a pipe nobody is draining.
#[test]
fn a_peer_that_stopped_reading_is_given_up_on() {
    let (mut writing, _seen) = deaf();

    let began = Instant::now();
    let sent = writing.send(r#"{"method":"tools/list"}"#);
    let waited = began.elapsed();

    assert!(
        matches!(sent, Err(FrameError::Unreadable { .. })),
        "a peer that never reads must end the frame: {sent:?}"
    );
    if let Err(failure) = sent {
        assert!(
            because(&failure).contains("stopped reading"),
            "the ending must name what went wrong: {}",
            because(&failure)
        );
    }
    assert!(
        waited >= PATIENCE,
        "giving up before the patience is spent is not patience: {waited:?}"
    );
}

/// A peer that has gone is not a peer crucible waits on. Spending the patience
/// on a pipe that already answered would delay every real ending.
#[test]
fn a_peer_that_has_gone_is_reported_rather_than_waited_out() {
    let mut writing = Written::new(said(Gone, WILLING));

    let began = Instant::now();
    let sent = writing.send(r#"{"method":"tools/list"}"#);
    let waited = began.elapsed();

    assert!(
        matches!(&sent, Err(failure) if because(failure).contains("the far end has gone")),
        "the pipe's own words must survive: {sent:?}"
    );
    assert!(
        waited < WILLING,
        "an answer already given must not be waited out: {waited:?}"
    );
}

/// The patience is spent on one frame and handed back. A budget for the whole
/// conversation would cut off a program for having been talked to for
/// longer than crucible guessed it would be.
#[test]
fn patience_is_for_one_frame_and_not_for_the_conversation() {
    const FRAMES: usize = 40;

    // Each frame a small part of the patience, so a machine that stretches
    // one does not stretch it past; forty of them well past the patience.
    let patience = Duration::from_millis(400);
    let (to, arrived) = mpsc::channel();
    let pause = Duration::from_millis(12);
    let mut writing = Written::new(said(Slow { pause, to }, patience));

    let began = Instant::now();
    let mut ended = None;
    for _ in 0..FRAMES {
        if let Err(failure) = writing.send(r#"{"method":"ping"}"#) {
            ended = Some(format!("{failure:?}"));
            break;
        }
    }
    let waited = began.elapsed();

    assert!(
        ended.is_none(),
        "a peer that keeps reading must not be given up on: {ended:?}"
    );
    assert!(
        waited > patience,
        "the conversation has to outlast one patience for this to prove anything: {waited:?}"
    );
    assert_eq!(collected(&arrived).lines().count(), FRAMES);
}

/// Nothing further is said once a frame was given up on. The bytes are still
/// in the peer's pipe, so a later frame would arrive joined to the one
/// crucible already reported as never sent.
#[test]
fn nothing_further_is_said_once_a_frame_was_given_up_on() {
    let (mut writing, _seen) = deaf();

    let first = writing.send(r#"{"method":"tools/list"}"#);
    let began = Instant::now();
    let second = writing.send(r#"{"method":"ping"}"#);
    let waited = began.elapsed();

    assert!(first.is_err(), "a peer that never reads ends the frame");
    assert!(
        second.is_err(),
        "a conversation crucible gave up on must not be spoken to again: {second:?}"
    );
    assert!(
        waited < PATIENCE,
        "an ending already reached must not be waited out again: {waited:?}"
    );
}

/// A frame crucible never ends is a frame that grows without limit. The
/// ceiling stands where the bytes are retained, not only where a frame writer
/// checks, so nothing can fill this buffer by never sending a newline.
#[test]
fn a_frame_that_never_ends_is_refused_rather_than_retained() {
    const PIECE: usize = 64 * 1024;

    let (to, arrived) = mpsc::channel();
    let mut said = said(Kept { to }, WILLING);

    let mut refused = None;
    for _ in 0..=(FRAME_BYTES / PIECE) {
        if let Err(failure) = said.write_all(&vec![b'x'; PIECE]) {
            refused = Some(failure.to_string());
            break;
        }
    }

    assert!(
        matches!(&refused, Some(said) if said.contains("without ending a frame")),
        "a frame with no end must be refused rather than held: {refused:?}"
    );
    assert_eq!(
        collected(&arrived),
        "",
        "nothing unfinished may reach the peer"
    );
}

/// A peer that stopped reading keeps the frame crucible gave up on, and the
/// work writing it is still parked in a write that will never finish. That
/// work has to end with the conversation: a writer nobody can reach, holding a
/// pipe into a program that is being stopped, is the hang this is here to
/// prevent. It has to be work on the runtime the transport was handed, too,
/// because a thread parked in that write belongs to nobody.
#[test]
fn a_peer_that_stopped_reading_is_given_up_on_by_owned_work_that_ends_with_it() {
    let (mut writing, seen) = deaf();

    let sent = writing.send(r#"{"method":"tools/list"}"#);

    assert!(
        matches!(&sent, Err(failure @ FrameError::Unreadable { .. })
            if failure.to_string().contains("stopped reading") && !failure.never_left()),
        "the ending a peer that stopped reading has always had: {sent:?}"
    );
    drop(writing);
    assert!(
        until(|| seen.released.load(Ordering::Relaxed)),
        "dropping the conversation has to end the write it gave up on and let the pipe go"
    );
    assert_eq!(
        seen.elsewhere.load(Ordering::Relaxed),
        0,
        "every write has to be made by work on the runtime the transport was handed, \
         never by a thread of the transport's own"
    );
}

/// A frame writer over `to`, its task on the runtime the test runs on.
fn awaited(to: impl SandboxInput + 'static, patience: Duration) -> Written<Said> {
    Written::new(Said::new(
        Box::new(to),
        patience,
        &tokio::runtime::Handle::current(),
    ))
}

/// A frame awaited reaches the program as one line, as a frame waited for does.
#[tokio::test]
async fn an_awaited_frame_reaches_the_program() {
    let (to, arrived) = mpsc::channel();
    let mut writing = awaited(Kept { to }, WILLING);

    let sent = writing.send_async(r#"{"method":"tools/list"}"#).await;

    assert!(
        sent.is_ok(),
        "a peer that reads must hear crucible: {sent:?}"
    );
    assert_eq!(collected(&arrived), "{\"method\":\"tools/list\"}\n");
}

/// A peer that stopped reading is given up on at the patience whether the
/// frame is awaited or waited for; the frame may yet land, so it is not called
/// undelivered, and nothing further is said.
#[tokio::test(start_paused = true)]
async fn an_awaited_frame_to_a_peer_that_stopped_reading_is_given_up_on() {
    let seen = Arc::new(Seen::default());
    let mut writing = awaited(Deaf(Arc::clone(&seen)), PATIENCE);

    let began = tokio::time::Instant::now();
    let sent = writing.send_async(r#"{"method":"tools/list"}"#).await;
    let waited = began.elapsed();
    let again = writing.send_async(r#"{"method":"ping"}"#).await;

    assert!(
        matches!(&sent, Err(failure @ FrameError::Unreadable { .. })
            if failure.to_string().contains("stopped reading") && !failure.never_left()),
        "the ending a peer that stopped reading has always had: {sent:?}"
    );
    assert!(
        waited >= PATIENCE,
        "giving up early is not patience: {waited:?}"
    );
    assert!(
        matches!(&again, Err(failure) if failure.to_string().contains("already stopped")),
        "a conversation crucible gave up on is not spoken to again: {again:?}"
    );
}

/// A peer that has gone is reported as the pipe says, at once.
#[tokio::test]
async fn an_awaited_frame_to_a_peer_that_has_gone_is_refused_in_its_words() {
    let mut writing = awaited(Gone, WILLING);

    let sent = writing.send_async(r#"{"method":"tools/list"}"#).await;

    assert!(
        matches!(&sent, Err(failure @ FrameError::Unreadable { source })
            if source.kind() == ErrorKind::BrokenPipe
                && failure.to_string().contains("the far end has gone")
                && failure.never_left()),
        "the pipe's own words, and a frame that never left: {sent:?}"
    );
}

/// An awaited send given up on after the frame was handed over leaves the
/// frame with the task: the next flush waits for that frame's answer and sends
/// nothing twice.
#[tokio::test]
async fn an_awaited_send_given_up_on_sends_nothing_twice() {
    let (to, arrived) = mpsc::channel();
    let pause = Duration::from_millis(50);
    let mut said = Said::new(
        Box::new(Slow { pause, to }),
        WILLING,
        &tokio::runtime::Handle::current(),
    );
    tokio::io::AsyncWriteExt::write_all(&mut said, b"{\"method\":\"once\"}\n")
        .await
        .expect("held");

    let early = tokio::time::timeout(
        Duration::from_millis(1),
        tokio::io::AsyncWriteExt::flush(&mut said),
    )
    .await;
    let later = tokio::io::AsyncWriteExt::flush(&mut said).await;

    assert!(early.is_err(), "the pipe had not taken it yet: {early:?}");
    assert!(
        later.is_ok(),
        "the frame handed over is answered: {later:?}"
    );
    assert_eq!(collected(&arrived), "{\"method\":\"once\"}\n");
}

/// Two frames and whatever each was answered, for a flush given up on after
/// its frame was handed over and a second frame sent after it.
async fn after_a_flush_given_up_on(
    to: impl SandboxInput + 'static,
    patience: Duration,
    then_waiting: bool,
) -> Result<(), FrameError> {
    let mut writing = awaited(to, patience);
    // Handed over in the first poll; given up on before the pipe answers.
    let given_up = tokio::time::timeout(Duration::ZERO, writing.send_async(r#"{"n":1}"#)).await;
    assert!(
        given_up.is_err(),
        "the pipe had not answered yet: {given_up:?}"
    );
    if then_waiting {
        tokio::task::spawn_blocking(move || writing.send(r#"{"n":2}"#))
            .await
            .expect("the waiting send ran")
    } else {
        writing.send_async(r#"{"n":2}"#).await
    }
}

/// A flush given up on after its frame was handed over leaves that frame with
/// the task. The frame sent after it has to reach the peer too, and be answered
/// for itself: an answer is the answer of the frame it is reported for,
/// whether the second send is awaited or waited for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_sent_after_a_flush_given_up_on_reaches_the_peer() {
    for then_waiting in [false, true] {
        let (to, arrived) = mpsc::channel();
        let pause = Duration::from_millis(20);

        let second = after_a_flush_given_up_on(Slow { pause, to }, WILLING, then_waiting).await;

        assert!(
            second.is_ok(),
            "the second frame was taken (waiting: {then_waiting}): {second:?}"
        );
        assert_eq!(
            collected(&arrived),
            "{\"n\":1}\n{\"n\":2}\n",
            "both frames reached the peer, once each, in order (waiting: {then_waiting})"
        );
    }
}

/// A peer that goes after a moment, taking nothing.
struct Leaves(Duration);

impl SandboxInput for Leaves {
    fn write<'a>(&'a mut self, _bytes: &'a [u8]) -> BoxFuture<'a, io::Result<usize>> {
        Box::pin(async move {
            tokio::time::sleep(self.0).await;
            Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "the far end has gone",
            ))
        })
    }
}

/// And where the frame given up on was not taken, the frame after it was never
/// handed over, and is reported as never having left rather than as the
/// earlier frame's failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_after_one_that_was_not_taken_is_reported_unsent() {
    for then_waiting in [false, true] {
        let second =
            after_a_flush_given_up_on(Leaves(Duration::from_millis(20)), WILLING, then_waiting)
                .await;

        assert!(
            matches!(&second, Err(failure)
                if failure.never_left() && failure.to_string().contains("was not sent")),
            "the second frame is reported as never sent, not with the first frame's \
             failure (waiting: {then_waiting}): {second:?}"
        );
    }
}
