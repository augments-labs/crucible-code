//! What saying something to a confined process has to guarantee.

use std::io::{self, ErrorKind};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::{FRAME_BYTES, FrameError, Written};

use super::*;

/// How long a test lets one frame sit before calling the peer gone.
const PATIENCE: Duration = Duration::from_millis(50);

/// How long a test waits for a quiet channel before deciding it is finished.
const SETTLE: Duration = Duration::from_millis(20);

/// A peer that reads everything crucible says.
struct Kept {
    /// Where the bytes are handed for the test to read.
    to: Sender<Vec<u8>>,
}

impl Write for Kept {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let _ = self.to.send(bytes.to_vec());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A peer that reads, slowly, and never stops.
struct Slow {
    /// How long each write takes to be taken.
    pause: Duration,
    /// Where the bytes are handed for the test to read.
    to: Sender<Vec<u8>>,
}

impl Write for Slow {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        thread::sleep(self.pause);
        let _ = self.to.send(bytes.to_vec());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A peer that has stopped reading, holding whoever writes to it.
///
/// It is released when the test drops the other end, so a worker parked in
/// here goes away with the test rather than outliving it.
struct Deaf {
    /// Held until the test lets go.
    until: Receiver<()>,
}

impl Write for Deaf {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let _ = self.until.recv();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A peer that has gone.
struct Gone;

impl Write for Gone {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            ErrorKind::BrokenPipe,
            "the far end has gone",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
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
    (Written::new(Said::new(Kept { to }, patience)), arrived)
}

/// Which ending a frame reached, where the guard cares that it is the pipe's.
fn because(failure: &FrameError) -> String {
    failure.to_string()
}

/// The everyday case. A frame crucible sends has to arrive as one line, or the
/// far end joins it to whatever comes next and reads neither.
#[test]
fn what_crucible_says_reaches_the_program() {
    let (mut writing, arrived) = saying(PATIENCE);

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
    let mut said = Said::new(Kept { to }, PATIENCE);

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
    let (release, until) = mpsc::channel();
    let mut writing = Written::new(Said::new(Deaf { until }, PATIENCE));

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
    drop(release);
}

/// A peer that has gone is not a peer crucible waits on. Spending the patience
/// on a pipe that already answered would delay every real ending.
#[test]
fn a_peer_that_has_gone_is_reported_rather_than_waited_out() {
    let mut writing = Written::new(Said::new(Gone, PATIENCE));

    let began = Instant::now();
    let sent = writing.send(r#"{"method":"tools/list"}"#);
    let waited = began.elapsed();

    assert!(
        matches!(&sent, Err(failure) if because(failure).contains("the far end has gone")),
        "the pipe's own words must survive: {sent:?}"
    );
    assert!(
        waited < PATIENCE,
        "an answer already given must not be waited out: {waited:?}"
    );
}

/// The patience is spent on one frame and handed back. A budget for the whole
/// conversation would cut off a program for having been talked to for
/// longer than crucible guessed it would be.
#[test]
fn patience_is_for_one_frame_and_not_for_the_conversation() {
    const FRAMES: usize = 40;

    let (to, arrived) = mpsc::channel();
    let pause = Duration::from_millis(2);
    let mut writing = Written::new(Said::new(Slow { pause, to }, PATIENCE));

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
        waited > PATIENCE,
        "the conversation has to outlast one patience for this to prove anything: {waited:?}"
    );
    assert_eq!(collected(&arrived).lines().count(), FRAMES);
}

/// Nothing further is said once a frame was given up on. The bytes are still
/// in the peer's pipe, so a later frame would arrive joined to the one
/// crucible already reported as never sent.
#[test]
fn nothing_further_is_said_once_a_frame_was_given_up_on() {
    let (release, until) = mpsc::channel();
    let mut writing = Written::new(Said::new(Deaf { until }, PATIENCE));

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
    drop(release);
}

/// A frame crucible never ends is a frame that grows without limit. The
/// ceiling stands where the bytes are retained, not only where a frame writer
/// checks, so nothing can fill this buffer by never sending a newline.
#[test]
fn a_frame_that_never_ends_is_refused_rather_than_retained() {
    const PIECE: usize = 64 * 1024;

    let (to, arrived) = mpsc::channel();
    let mut said = Said::new(Kept { to }, PATIENCE);

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
