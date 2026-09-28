//! What reading a confined stream has to guarantee.

use std::collections::VecDeque;
use std::error::Error as _;
use std::fmt::Write as _;
use std::io::{self, BufRead as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crucible_runtime::{BoxFuture, Cancel};
use crucible_sandbox::{SandboxOutput, SandboxRead};

use crate::testing::runtime;
use crate::{FrameError, Frames};

use super::{CHUNK, Heard, QUEUED};

/// How long a test whose subject is a silence sits through one.
const PATIENCE: Duration = Duration::from_millis(50);

/// How long a test whose subject is not a silence lets one last.
///
/// Every read is made by a task on the runtime and handed across to the test's
/// thread, and a loaded machine can put far more than [`PATIENCE`] between a
/// task being woken and it running. Long past that, so a case that is not
/// about the patience fails on the code rather than on the machine; a case
/// that passes never waits it out.
const WILLING: Duration = Duration::from_secs(5);

/// How long a stream waits for each of its pauses, where a test gives it one.
/// Short, because a test that waits in real milliseconds should wait as few of
/// them as it can.
const PAUSE: Duration = Duration::from_millis(1);

/// How long a test waits for a task it does not control to get somewhere.
const LATEST: Duration = Duration::from_secs(2);

/// One thing a confined stream does when it is asked what it has.
enum Step {
    /// It has a frame, and the newline that ends one.
    Says(&'static str),
    /// It has nothing yet, and its writer is still there.
    Waits,
    /// It has a piece of what it said, and the rest went past the ceiling.
    Loses {
        /// What is still in the buffer.
        retained: &'static str,
        /// What was consumed and dropped.
        discarded: usize,
    },
    /// Its writer has gone.
    Closes,
}

/// A confined stream that does what it was told to, then goes quiet forever.
///
/// Running out of script is silence rather than an ending, because a hung
/// program is exactly a process that is still there and has stopped talking.
struct Says {
    /// What is left to do.
    steps: VecDeque<Step>,
    /// How long each [`Step::Waits`] lasts when the stream is waited on; none
    /// at all is a wait that gives the runtime its turn and no more.
    pause: Duration,
}

impl Says {
    fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self::pausing(steps, Duration::ZERO)
    }

    fn pausing(steps: impl IntoIterator<Item = Step>, pause: Duration) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            pause,
        }
    }
}

impl SandboxOutput for Says {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        match self.steps.pop_front() {
            None | Some(Step::Waits) => Ok(SandboxRead::Pending),
            Some(Step::Closes) => Ok(SandboxRead::End),
            Some(Step::Says(frame)) => Ok(SandboxRead::Bytes(copied(frame, buffer))),
            Some(Step::Loses {
                retained,
                discarded,
            }) => Ok(SandboxRead::Limited {
                retained: copied(retained, buffer),
                discarded,
            }),
        }
    }

    /// Each wait is as long as the test chose rather than the default
    /// waiting read's own pause, and running out of script waits for ever.
    fn read<'a>(&'a mut self, buffer: &'a mut [u8]) -> BoxFuture<'a, io::Result<SandboxRead>> {
        Box::pin(async move {
            loop {
                if self.steps.is_empty() {
                    return std::future::pending().await;
                }
                match self.read_ready(buffer)? {
                    SandboxRead::Pending if self.pause.is_zero() => tokio::task::yield_now().await,
                    SandboxRead::Pending => tokio::time::sleep(self.pause).await,
                    read => return Ok(read),
                }
            }
        })
    }
}

/// Puts one line into the caller's buffer, as a pipe would.
fn copied(frame: &str, buffer: &mut [u8]) -> usize {
    let said = format!("{frame}\n");
    let bytes = said.as_bytes();
    let taken = bytes.len().min(buffer.len());
    if let Some((into, from)) = buffer.get_mut(..taken).zip(bytes.get(..taken)) {
        into.copy_from_slice(from);
    }
    taken
}

/// Frames read off a stream that says these things.
///
/// Read through this crate's own frame reader rather than through either
/// protocol's parser: what a line means is decided above, and a test of the
/// stream underneath that needed one of them would be testing both.
fn hearing(steps: impl IntoIterator<Item = Step>) -> Frames<Heard<Says>> {
    Frames::new(Heard::new(Says::new(steps), WILLING, runtime()))
}

/// The same, each wait of the stream's lasting `pause`, and a silence given
/// up on after `patience`.
fn patiently(
    steps: impl IntoIterator<Item = Step>,
    pause: Duration,
    patience: Duration,
) -> Frames<Heard<Says>> {
    Frames::new(Heard::new(Says::pausing(steps, pause), patience, runtime()))
}

/// What a refusal said, all the way down to the operating system where there
/// is one.
fn because(why: &FrameError) -> String {
    let mut said = why.to_string();
    let mut next: Option<&dyn std::error::Error> = why.source();
    while let Some(source) = next {
        let _ = write!(said, "; {source}");
        next = source.source();
    }
    said
}

#[test]
fn what_a_program_says_arrives_whole() {
    let said = r#"{"id":1,"method":"tools/list","params":{}}"#;
    let mut frames = hearing([Step::Says(said), Step::Closes]);

    let frame = frames.next_frame();

    assert!(
        matches!(frame, Some(Ok(ref arrived)) if arrived == said),
        "a confined process's own words must reach the host whole: {frame:?}"
    );
}

/// The reason this type exists. A stream that answers "nothing yet" is not a
/// stream that has ended, and treating the two alike would end a conversation
/// every time a program took a breath.
///
/// How long a wait lasts is not what is being asked here, so each wait lasts
/// no time at all, under a patience long past anything a loaded machine adds:
/// the case fails on the code rather than on the machine.
#[test]
fn a_pause_is_not_an_ending() {
    let mut frames = patiently(
        [
            Step::Waits,
            Step::Waits,
            Step::Waits,
            Step::Says(r#"{"method":"ready"}"#),
            Step::Closes,
        ],
        Duration::ZERO,
        WILLING,
    );

    let frame = frames.next_frame();

    assert!(
        matches!(frame, Some(Ok(ref arrived)) if arrived.contains("ready")),
        "waiting must not lose what was said afterwards: {frame:?}"
    );
}

/// Patience is spent on one silence and handed back whenever something is
/// said. A budget for the whole conversation would kill a program for the
/// crime of being useful for longer than crucible guessed.
#[test]
fn patience_is_for_one_silence_and_not_for_the_conversation() {
    // Each silence a small part of the patience, so a machine that stretches
    // one does not stretch it past; a hundred of them well past the patience.
    let patience = Duration::from_millis(400);
    let pause = Duration::from_millis(5);
    let mut steps = Vec::new();
    for _ in 0..100 {
        steps.push(Step::Waits);
        steps.push(Step::Says(r#"{"method":"ready"}"#));
    }
    steps.push(Step::Closes);
    let mut frames = patiently(steps, pause, patience);

    let began = Instant::now();
    let mut ended = None;
    for said in 0..100 {
        match frames.next_frame() {
            Some(Ok(_)) => {}
            other => {
                ended = Some(format!(
                    "frame {said} of 100: {}",
                    match other {
                        Some(Err(why)) => because(&why),
                        _ => "the stream finished".to_owned(),
                    }
                ));
                break;
            }
        }
    }
    let waited = began.elapsed();

    assert!(
        ended.is_none(),
        "a conversation must not run out of patience while it is being had: {ended:?}"
    );
    assert!(
        waited > patience,
        "the test proves nothing unless the whole conversation outlasts one \
         silence: waited {waited:?}, patience {patience:?}"
    );
}

/// A peer that has stopped reading and a peer that is merely slow look the
/// same from here, so the only honest answer is a deadline crucible chose.
#[test]
fn a_program_that_says_nothing_for_long_enough_is_given_up_on() {
    let mut frames = patiently([Step::Waits], PAUSE, PATIENCE);

    let began = Instant::now();
    let why = frames
        .next_frame()
        .expect("silence is not the stream finishing")
        .expect_err("silence must end the conversation");
    let waited = began.elapsed();

    assert!(
        because(&why).contains("said nothing"),
        "the ending must say it was silence rather than a failure: {}",
        because(&why)
    );
    assert!(
        waited >= PATIENCE,
        "giving up before the patience is spent is not patience: {waited:?}"
    );
}

/// Bytes past the output ceiling are gone, and one of them was a newline. The
/// reader has lost the boundary the program was stating, so what is left is
/// not a shorter conversation but a stream that cannot be framed at all.
#[test]
fn a_stream_with_a_hole_in_it_cannot_be_framed() {
    let mut frames = hearing([
        Step::Loses {
            retained: r#"{"method":"rea"#,
            discarded: 4096,
        },
        Step::Says(r#"{"method":"ready"}"#),
        Step::Closes,
    ]);

    let why = frames
        .next_frame()
        .expect("a hole is not the stream finishing")
        .expect_err("a hole must end the conversation");

    assert!(
        because(&why).contains("4096"),
        "the ending must say how much was lost: {}",
        because(&why)
    );
}

#[test]
fn a_closed_stream_is_the_ordinary_ending() {
    let mut frames = hearing([Step::Closes]);

    let frame = frames.next_frame();

    assert!(
        frame.is_none(),
        "a writer that has gone is nobody's fault: {frame:?}"
    );
}

/// The whole reason the waiting looks up between waits rather than blocking. A
/// patience answers "how long may a quiet program stay quiet"; an interrupt
/// answers "somebody wants this to stop", and those are different questions.
/// A reader that could only give the first answer would make escape mean *at
/// the deadline*, which for a request patience measured in minutes is not a
/// reader anybody can interrupt.
#[test]
fn a_reader_asked_to_stop_gives_up_on_the_silence_rather_than_waiting_it_out() {
    let cancel = Cancel::new();
    // Far longer than this case is willing to take, so that ending at the
    // patience and ending because somebody asked cannot be confused: only one
    // of the two can produce an answer inside the assertion below.
    let mut heard = Heard::new(Says::new([Step::Waits]), Duration::from_secs(2), runtime());
    heard.abandoned_when(Some(cancel.clone()));
    cancel.request();

    let began = Instant::now();
    let stopped = heard.fill_buf().expect_err("the wait was interrupted");
    let waited = began.elapsed();

    assert_eq!(
        stopped.kind(),
        io::ErrorKind::ConnectionAborted,
        "an abandoned wait is not a program that timed out, and a caller that \
         has to tell them apart reads the kind: {stopped}"
    );
    assert_ne!(
        stopped.kind(),
        io::ErrorKind::Interrupted,
        "and it must not be the kind every std reader retries, or the caller \
         that gave up would be put straight back into the wait"
    );
    assert!(
        waited < Duration::from_millis(500),
        "a wait somebody asked to end must end there and then, not at the \
         patience: {waited:?}"
    );
}

/// The token is set around one exchange, so the reader has to put it down
/// again. A stream that stayed abandoned would refuse the next call for a
/// press that was spent on the last one.
#[test]
fn a_reader_handed_no_token_waits_out_a_silence_it_was_told_to_abandon_before() {
    let cancel = Cancel::new();
    let mut heard = Heard::new(
        Says::new([Step::Waits, Step::Says(r#"{"method":"ready"}"#)]),
        WILLING,
        runtime(),
    );
    heard.abandoned_when(Some(cancel.clone()));
    heard.abandoned_when(None);
    cancel.request();

    let arrived = heard.fill_buf().expect("no token is no interruption");

    assert!(
        std::str::from_utf8(arrived)
            .unwrap_or_default()
            .contains("ready"),
        "a spent press must not end the exchange after it"
    );
}

/// A stream that always has one more byte and never ends a frame.
///
/// The shape a patience cannot answer: it is never quiet for long enough to be
/// given up on, so a reader measuring silences alone waits on it for as long as
/// it cares to keep typing.
struct Dribbles;

impl SandboxOutput for Dribbles {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        let Some(byte) = buffer.first_mut() else {
            return Ok(SandboxRead::Pending);
        };
        // Not a newline: this is a frame that goes on forever, not a slow one.
        *byte = b'x';
        Ok(SandboxRead::Bytes(1))
    }

    /// One byte a millisecond, waited out on the runtime's clock as a pipe is
    /// waited on, rather than by holding the worker that reads it.
    fn read<'a>(&'a mut self, buffer: &'a mut [u8]) -> BoxFuture<'a, io::Result<SandboxRead>> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            self.read_ready(buffer)
        })
    }
}

/// Reads one line from `reader` on a thread of its own, and gives up on the
/// whole test after `waiting`.
///
/// A reader that will not stop cannot be asked whether it stopped, so the
/// waiting happens here rather than in the reader: what comes back is either
/// the ending or the absence of one.
fn ended(mut reader: Heard<Dribbles>, waiting: Duration) -> Option<(io::Result<usize>, Duration)> {
    let (done, ending) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let began = Instant::now();
        let mut line = String::new();
        let read = reader.read_line(&mut line);
        drop(done.send((read, began.elapsed())));
    });
    ending.recv_timeout(waiting).ok()
}

#[test]
fn a_reader_given_a_deadline_gives_up_on_a_peer_that_never_stops_typing() {
    let mut reader = Heard::new(Dribbles, Duration::from_secs(30), runtime());
    reader.bounded_until(Instant::now().checked_add(Duration::from_millis(100)));

    let (read, waited) = ended(reader, Duration::from_secs(5)).expect(
        "a deadline is a deadline however busy the far end is; without one this reader \
         is still going",
    );

    let ending = read.expect_err("a frame that never ended is not a line");
    assert_eq!(
        ending.kind(),
        io::ErrorKind::TimedOut,
        "it is the far end that failed to answer in the time it was given, which is \
         what a reader says about a peer that ran out of it: {ending}"
    );
    assert!(
        waited < Duration::from_secs(5),
        "and it ended at the deadline rather than at the patience thirty seconds away: \
         {waited:?}"
    );
}

#[test]
fn a_reader_asked_to_stop_while_bytes_keep_arriving_stops_anyway() {
    let cancel = Cancel::new();
    cancel.request();
    let mut reader = Heard::new(Dribbles, Duration::from_secs(30), runtime());
    reader.abandoned_when(Some(cancel));

    let (read, waited) = ended(reader, Duration::from_secs(5))
        .expect("a press is answered whether or not the far end is saying anything");

    let ending = read.expect_err("a reader that was asked to stop did not finish the line");
    assert_eq!(
        ending.kind(),
        io::ErrorKind::ConnectionAborted,
        "a press ends the wait as the near end letting go, not as a slow peer: {ending}"
    );
    assert!(
        waited < Duration::from_secs(5),
        "and it is answered at the press rather than once the far end pauses for \
         breath: {waited:?}"
    );
}

/// A stream whose own read fails the way an abandonment is spelled.
struct Aborts;

impl SandboxOutput for Aborts {
    fn read_ready(&mut self, _buffer: &mut [u8]) -> io::Result<SandboxRead> {
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "the connection was aborted",
        ))
    }
}

#[test]
fn a_connection_the_backend_lost_is_not_reported_as_a_press_nobody_made() {
    // The kind that says crucible let go is crucible's to set, and a backend
    // that happens to fail with it would otherwise be indistinguishable from
    // the reader being asked to stop. What is decided on that difference is
    // whether a call in flight was abandoned by the near end or lost with the
    // far one.
    let mut reader = Heard::new(Aborts, WILLING, runtime());
    let mut line = String::new();

    let ending = reader
        .read_line(&mut line)
        .expect_err("a stream that will not read is not a line");

    assert_eq!(
        ending.kind(),
        io::ErrorKind::BrokenPipe,
        "the far end went, and nobody pressed anything"
    );
    assert!(
        ending.to_string().contains("the connection was aborted"),
        "the backend's own account of it survives: {ending}"
    );
}

/// What a stream saw of whoever read it.
#[derive(Default)]
struct Seen {
    /// Reads made on a thread that is not the runtime's.
    elsewhere: AtomicUsize,
    /// How many bytes it has handed over.
    handed: AtomicUsize,
    /// Whether it has been let go of.
    released: AtomicBool,
}

impl Seen {
    /// Counts one read, and where it was made.
    fn asked(&self) {
        if !crate::testing::on_runtime() {
            self.elsewhere.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Whether everything that read the stream was work on the runtime.
    fn read_by_owned_work(&self) -> bool {
        self.elsewhere.load(Ordering::Relaxed) == 0
    }
}

/// A peer that has gone quiet and stays so, whose output is still open.
struct Quiet(Arc<Seen>);

impl SandboxOutput for Quiet {
    fn read_ready(&mut self, _buffer: &mut [u8]) -> io::Result<SandboxRead> {
        self.0.asked();
        Ok(SandboxRead::Pending)
    }

    fn read<'a>(&'a mut self, _buffer: &'a mut [u8]) -> BoxFuture<'a, io::Result<SandboxRead>> {
        self.0.asked();
        Box::pin(std::future::pending())
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::Relaxed);
    }
}

/// A peer that never stops talking: every read is a buffer full of frames.
struct Chatty(Arc<Seen>);

impl SandboxOutput for Chatty {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        self.0.asked();
        for (at, byte) in buffer.iter_mut().enumerate() {
            *byte = if at % 3 == 2 { b'\n' } else { b'1' };
        }
        self.0.handed.fetch_add(buffer.len(), Ordering::Relaxed);
        Ok(SandboxRead::Bytes(buffer.len()))
    }
}

impl Drop for Chatty {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::Relaxed);
    }
}

/// Waits for `settled` to hold, so a test never races a task it started.
fn until(settled: impl Fn() -> bool) -> bool {
    let began = Instant::now();
    while began.elapsed() < LATEST {
        if settled() {
            return true;
        }
        std::thread::sleep(PAUSE);
    }
    settled()
}

/// A slow peer on the reading side: one that has stopped saying anything. The
/// host gives up on it at the patience, as it always has, and the reading it
/// gave up on ends with the conversation rather than waiting on the pipe for
/// ever — which only work the runtime owns can promise, since a read parked on
/// a thread of the transport's own is one nobody can reach.
#[test]
fn a_peer_gone_quiet_is_given_up_on_by_owned_work_that_ends_with_it() {
    let seen = Arc::new(Seen::default());
    let mut heard = Heard::new(Quiet(Arc::clone(&seen)), PATIENCE, runtime());

    let silence = heard
        .fill_buf()
        .map(<[u8]>::len)
        .expect_err("a peer gone quiet is given up on");

    assert_eq!(silence.kind(), io::ErrorKind::TimedOut, "{silence}");
    assert!(
        silence.to_string().contains("said nothing"),
        "the ending a quiet peer has always had: {silence}"
    );
    drop(heard);
    assert!(
        until(|| seen.released.load(Ordering::Relaxed)),
        "dropping the conversation has to end the read and let the stream go"
    );
    assert!(
        seen.read_by_owned_work(),
        "every read has to be made by work on the runtime the transport was handed, \
         never by a thread of the transport's own nor by the host's"
    );
}

/// A slow reader: a host that stops asking while the peer keeps talking.
/// What has been read ahead of it is bounded, so a peer cannot fill crucible by
/// out-talking it; the rest waits in the peer's own pipe, and the reading ends
/// with the conversation.
#[test]
fn a_host_that_stops_reading_holds_back_a_bounded_read_ahead_and_ends_it() {
    let seen = Arc::new(Seen::default());
    let mut frames = Frames::new(Heard::new(Chatty(Arc::clone(&seen)), WILLING, runtime()));

    let first = frames.next_frame();
    assert!(
        matches!(first, Some(Ok(ref frame)) if frame == "11"),
        "{first:?}"
    );
    let settled = until(|| {
        let before = seen.handed.load(Ordering::Relaxed);
        std::thread::sleep(PAUSE * 20);
        seen.handed.load(Ordering::Relaxed) == before
    });

    let ahead = (QUEUED + 2) * CHUNK;
    assert!(
        settled && seen.handed.load(Ordering::Relaxed) <= ahead,
        "a host that stopped reading must leave the rest in the peer's pipe, not in \
         crucible: {} bytes read ahead, at most {ahead}",
        seen.handed.load(Ordering::Relaxed)
    );
    drop(frames);
    assert!(
        until(|| seen.released.load(Ordering::Relaxed)),
        "dropping the conversation has to end the read ahead and let the stream go"
    );
    assert!(
        seen.read_by_owned_work(),
        "every read has to be made by work on the runtime the transport was handed"
    );
}

/// Frames read off a stream that says these things, awaited on the runtime the
/// test runs on rather than waited for by a thread.
fn awaiting(steps: impl IntoIterator<Item = Step>, patience: Duration) -> Frames<Heard<Says>> {
    Frames::new(Heard::new(
        Says::new(steps),
        patience,
        &tokio::runtime::Handle::current(),
    ))
}

/// The awaitable half hands over what the waiting half does: a frame whole,
/// and then the ordinary ending.
#[tokio::test]
async fn awaited_frames_arrive_whole_and_then_the_stream_ends() {
    let said = r#"{"id":1,"method":"tools/list"}"#;
    let mut frames = awaiting([Step::Says(said), Step::Closes], WILLING);

    let first = frames.next_frame_async().await;
    let after = frames.next_frame_async().await;

    assert!(
        matches!(first, Some(Ok(ref arrived)) if arrived == said),
        "a frame awaited arrives whole: {first:?}"
    );
    assert!(after.is_none(), "and then the stream ends: {after:?}");
}

/// A silence awaited is given up on at the patience, as a silence waited for
/// is, and says it was a silence.
#[tokio::test(start_paused = true)]
async fn an_awaited_silence_is_given_up_on_at_the_patience() {
    let mut frames = awaiting([Step::Waits], PATIENCE);

    let began = tokio::time::Instant::now();
    let why = frames
        .next_frame_async()
        .await
        .expect("silence is not the stream finishing")
        .expect_err("silence must end the conversation");

    assert!(
        because(&why).contains("said nothing"),
        "the ending must say it was silence: {}",
        because(&why)
    );
    assert!(
        began.elapsed() >= PATIENCE,
        "giving up before the patience is spent is not patience: {:?}",
        began.elapsed()
    );
}

/// A press raised while a read is awaited ends it as the near end letting go,
/// long before a patience far away.
#[tokio::test]
async fn an_awaited_read_somebody_abandons_ends_at_the_press() {
    let cancel = Cancel::new();
    let mut heard = Heard::new(
        Says::new([Step::Waits]),
        Duration::from_secs(30),
        &tokio::runtime::Handle::current(),
    );
    heard.abandoned_when(Some(cancel.clone()));
    let pressing = cancel.clone();
    let _press = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        pressing.request();
    });

    let began = Instant::now();
    let stopped = tokio::io::AsyncBufReadExt::fill_buf(&mut heard)
        .await
        .map(<[u8]>::len)
        .expect_err("the wait was abandoned");

    assert_eq!(
        stopped.kind(),
        io::ErrorKind::ConnectionAborted,
        "{stopped}"
    );
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "a wait somebody asked to end must end at the press: {:?}",
        began.elapsed()
    );
}

/// A peer that never stops typing is ended at the deadline the exchange was
/// given, however busy it keeps the reader.
#[tokio::test]
async fn an_awaited_read_past_its_deadline_is_overdue() {
    let mut reader = Heard::new(
        Dribbles,
        Duration::from_secs(30),
        &tokio::runtime::Handle::current(),
    );
    reader.bounded_until(Instant::now().checked_add(Duration::from_millis(100)));
    let mut line = String::new();

    let ending = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line),
    )
    .await
    .expect("a deadline is a deadline however busy the far end is")
    .expect_err("a frame that never ended is not a line");

    assert_eq!(ending.kind(), io::ErrorKind::TimedOut, "{ending}");
    assert!(
        ending.to_string().contains("did not finish within"),
        "it is the exchange's length that ran out, not a silence: {ending}"
    );
}

/// A stream that lost bytes to its ceiling is refused whether its frames are
/// awaited or waited for, and says how much it lost.
#[tokio::test]
async fn an_awaited_stream_with_a_hole_in_it_is_refused() {
    let mut frames = awaiting(
        [
            Step::Loses {
                retained: r#"{"method":"rea"#,
                discarded: 4096,
            },
            Step::Closes,
        ],
        WILLING,
    );

    let why = frames
        .next_frame_async()
        .await
        .expect("a hole is not the stream finishing")
        .expect_err("a hole must end the conversation");

    assert!(because(&why).contains("4096"), "{}", because(&why));
}

/// Giving up on awaiting a frame gives up nothing the stream said: the next
/// wait carries on from where the dropped one was.
#[tokio::test]
async fn an_awaited_frame_given_up_on_is_still_there_for_the_next_wait() {
    let mut frames = Frames::new(Heard::new(
        Says::pausing(
            [
                Step::Waits,
                Step::Says(r#"{"method":"ready"}"#),
                Step::Closes,
            ],
            Duration::from_millis(50),
        ),
        WILLING,
        &tokio::runtime::Handle::current(),
    ));

    let early = tokio::time::timeout(Duration::from_millis(1), frames.next_frame_async()).await;
    let later = frames.next_frame_async().await;

    assert!(early.is_err(), "nothing had arrived yet: {early:?}");
    assert!(
        matches!(later, Some(Ok(ref frame)) if frame.contains("ready")),
        "the frame arrives for the wait after the one given up on: {later:?}"
    );
}

/// Marks the start of an exchange on `heard` the way a host does: the token
/// and the deadline the exchange runs under.
fn exchange<O>(heard: &mut Heard<O>) {
    heard.abandoned_when(None);
    heard.bounded_until(Instant::now().checked_add(WILLING));
}

/// A wait given up on is not the next exchange's silence. A read awaited
/// under a timeout that ran out, an idle stretch longer than the patience, and
/// then a new exchange whose answer comes well inside the patience: the answer
/// arrives, as it would to a host waiting on its own thread, rather than the
/// new exchange being told at once that the program said nothing.
#[tokio::test(start_paused = true)]
async fn a_new_exchange_after_a_wait_given_up_on_sits_through_a_silence_of_its_own() {
    let patience = Duration::from_millis(100);
    let mut frames = Frames::new(Heard::new(
        // Quiet until 20 ms into the second exchange, which begins at 310 ms.
        Says::pausing(
            [Step::Waits, Step::Says(r#"{"method":"ready"}"#)],
            Duration::from_millis(330),
        ),
        patience,
        &tokio::runtime::Handle::current(),
    ));

    exchange(frames.stream_mut());
    let given_up = tokio::time::timeout(Duration::from_millis(10), frames.next_frame_async()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    exchange(frames.stream_mut());
    let began = tokio::time::Instant::now();
    let answered = frames.next_frame_async().await;

    assert!(given_up.is_err(), "nothing had arrived yet: {given_up:?}");
    assert!(
        matches!(answered, Some(Ok(ref frame)) if frame.contains("ready")),
        "a new exchange sits through a silence of its own, and the answer 20 ms into it \
         arrives: {answered:?} after {:?}",
        began.elapsed()
    );
}

/// The same, begun at once rather than after an idle stretch: an exchange
/// given up on 190 ms into a 200 ms patience, and a new one begun straight
/// away and answered 60 ms in. The answer arrives: the new exchange's silence
/// is its own however soon it begins.
#[tokio::test(start_paused = true)]
async fn a_new_exchange_begun_at_once_sits_through_a_silence_of_its_own() {
    let patience = Duration::from_millis(200);
    let mut frames = Frames::new(Heard::new(
        Says::pausing(
            [Step::Waits, Step::Says(r#"{"method":"ready"}"#)],
            Duration::from_millis(250),
        ),
        patience,
        &tokio::runtime::Handle::current(),
    ));

    exchange(frames.stream_mut());
    let given_up =
        tokio::time::timeout(Duration::from_millis(190), frames.next_frame_async()).await;
    exchange(frames.stream_mut());
    let began = tokio::time::Instant::now();
    let answered = frames.next_frame_async().await;

    assert!(given_up.is_err(), "nothing had arrived yet: {given_up:?}");
    assert!(
        matches!(answered, Some(Ok(ref frame)) if frame.contains("ready")),
        "the answer 60 ms into the new exchange arrives: {answered:?} after {:?}",
        began.elapsed()
    );
}

/// A wait resumed within one exchange is the same wait however it is resumed:
/// a `select!` loop that takes a 60 ms step aside every 40 ms, letting go of
/// the read each time and taking it up again, still gives up on a quiet peer
/// at the patience.
#[tokio::test(start_paused = true)]
async fn a_quiet_peer_is_given_up_on_by_a_loop_that_keeps_stepping_aside() {
    let patience = Duration::from_millis(200);
    let mut frames = awaiting([Step::Waits], patience);
    exchange(frames.stream_mut());

    let began = tokio::time::Instant::now();
    let ended = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            tokio::select! {
                frame = frames.next_frame_async() => break frame,
                () = tokio::time::sleep(Duration::from_millis(40)) => {
                    tokio::time::sleep(Duration::from_millis(60)).await;
                }
            }
        }
    })
    .await
    .expect("a quiet peer is given up on, not waited on for ever");
    let why = ended
        .expect("silence is not the stream finishing")
        .expect_err("silence must end the exchange");

    assert!(because(&why).contains("said nothing"), "{}", because(&why));
    assert!(
        began.elapsed() < patience * 2,
        "given up on at the patience, not after it: {:?}",
        began.elapsed()
    );
}

/// Polls what it holds only after 60 ms of work on its own thread each time,
/// the way a caller busy with something synchronous between wakes does.
struct Busy<F>(F);

impl<F: std::future::Future + Unpin> std::future::Future for Busy<F> {
    type Output = F::Output;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<F::Output> {
        std::thread::sleep(Duration::from_millis(60));
        std::pin::Pin::new(&mut self.0).poll(cx)
    }
}

/// One read, never given up on, polled by a caller that works for 60 ms on
/// its own thread between polls: the quiet peer is still given up on at the
/// patience.
#[tokio::test]
async fn a_quiet_peer_is_given_up_on_however_slowly_its_wait_is_polled() {
    let patience = Duration::from_millis(200);
    let mut frames = awaiting([Step::Waits], patience);
    exchange(frames.stream_mut());

    let began = Instant::now();
    let ended = tokio::time::timeout(
        Duration::from_secs(3),
        Busy(Box::pin(frames.next_frame_async())),
    )
    .await
    .expect("a quiet peer is given up on, not waited on for ever");
    let why = ended
        .expect("silence is not the stream finishing")
        .expect_err("silence must end the exchange");

    assert!(because(&why).contains("said nothing"), "{}", because(&why));
    assert!(
        began.elapsed() < Duration::from_secs(2),
        "given up on near the patience: {:?}",
        began.elapsed()
    );
}

/// And a wait resumed at once is the same wait: a `select!` or a timeout that
/// lets go of a read and takes it up again straight away does not buy a quiet
/// peer a fresh patience each time.
#[tokio::test(start_paused = true)]
async fn a_wait_resumed_at_once_goes_on_sitting_through_the_same_silence() {
    let patience = Duration::from_millis(100);
    let mut frames = awaiting([Step::Waits], patience);

    let given_up = tokio::time::timeout(Duration::from_millis(60), frames.next_frame_async()).await;
    let resumed = tokio::time::Instant::now();
    let why = tokio::time::timeout(patience * 10, frames.next_frame_async())
        .await
        .expect("a quiet peer is given up on, not waited on for ever")
        .expect("silence is not the stream finishing")
        .expect_err("silence must end the conversation");

    assert!(given_up.is_err(), "nothing had arrived yet: {given_up:?}");
    assert!(because(&why).contains("said nothing"), "{}", because(&why));
    assert!(
        resumed.elapsed() < patience,
        "the silence begun before the resumption ran on through it: {:?}",
        resumed.elapsed()
    );
}
