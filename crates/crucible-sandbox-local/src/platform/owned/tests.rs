//! The thread-owned adapters over plain pipes, on every platform.
//!
//! Windows ships these; elsewhere they are compiled for these tests alone, so
//! the bound and the cleanup they promise are held on every cell rather than
//! only where they ship.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use super::*;

/// A runtime with a clock and nothing a thread-owned pipe needs besides.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a test runtime")
}

/// A pipe whose reading end is ready for the owned reader.
fn pipe() -> (io::PipeReader, io::PipeWriter) {
    let (reader, writer) = io::pipe().expect("a pipe");
    reader.prepare().expect("a pipe read without waiting");
    (reader, writer)
}

/// Waits for the reader's answer, failing the test after `WAIT`.
async fn read(reader: &mut Reader, buffer: &mut [u8]) -> io::Result<ReadState> {
    tokio::time::timeout(WAIT, reader.read(buffer))
        .await
        .expect("the owned reader answered within the test's wait")
}

const WAIT: Duration = Duration::from_secs(10);

/// Every byte `reader` gives until its end, read `width` bytes at a time.
async fn drained(reader: &mut Reader, width: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buffer = vec![0; width];
    loop {
        match read(reader, &mut buffer).await.expect("a read") {
            ReadState::Bytes(count) => {
                kept.extend_from_slice(buffer.get(..count).expect("a count within the buffer"));
            }
            ReadState::End => return kept,
            ReadState::Pending => panic!("a waiting read answered pending"),
        }
    }
}

#[test]
fn an_owned_reader_hands_over_every_byte_in_order_and_then_the_end() {
    let (reader, mut writer) = pipe();
    let expected: Vec<u8> = (0..3 * CHUNK + 17)
        .map(|at| u8::try_from(at % 251).expect("below 251"))
        .collect();
    let written = expected.clone();
    let writing = thread::spawn(move || writer.write_all(&written));
    let mut reader = Reader::start(reader).expect("a reader thread");

    // A buffer narrower than a chunk, so what one chunk leaves over is read
    // from this side of the channel.
    let kept = runtime().block_on(drained(&mut reader, 1000));

    writing
        .join()
        .expect("the writer")
        .expect("every byte written");
    assert_eq!(kept, expected);
}

#[test]
fn an_owned_reader_read_without_waiting_says_pending_until_bytes_arrive() {
    let (reader, mut writer) = pipe();
    let mut reader = Reader::start(reader).expect("a reader thread");
    let mut buffer = [0; 16];

    assert_eq!(
        reader.read_ready(&mut buffer).expect("a read"),
        ReadState::Pending
    );
    writer.write_all(b"ready").expect("a write");
    drop(writer);
    let deadline = Instant::now() + WAIT;
    let mut kept = Vec::new();
    loop {
        assert!(Instant::now() < deadline, "the bytes never arrived");
        match reader.read_ready(&mut buffer).expect("a read") {
            ReadState::Bytes(count) => {
                kept.extend_from_slice(buffer.get(..count).expect("a count within the buffer"));
            }
            ReadState::Pending => thread::sleep(Duration::from_millis(1)),
            ReadState::End => break,
        }
    }
    assert_eq!(kept, b"ready");
}

/// Dropped while its thread sleeps between asks of a quiet pipe: the drop
/// returns once the thread has, and the thread has closed the pipe.
#[test]
fn dropping_an_owned_reader_of_a_quiet_pipe_stops_its_thread_within_the_bound() {
    let (reader, mut writer) = pipe();
    let reader = Reader::start(reader).expect("a reader thread");
    thread::sleep(PAUSE * 3);

    let dropping = Instant::now();
    drop(reader);
    let took = dropping.elapsed();

    assert!(
        took < BOUND,
        "dropping the reader took {took:?}, past its bound of {BOUND:?}"
    );
    let refused = writer
        .write(b"x")
        .expect_err("the pipe outlived its reader");
    assert_eq!(refused.kind(), io::ErrorKind::BrokenPipe);
}

/// Dropped while its thread waits to hand over a chunk nobody took: the
/// hand-over is refused at once, and the pipe is closed with the thread.
///
/// The writer goes on writing from a thread of its own until the pipe refuses
/// it, so the test holds whatever a platform's pipe buffers, and ends in the
/// refusal the reader's drop causes.
#[test]
fn dropping_an_owned_reader_nobody_reads_stops_its_thread_within_the_bound() {
    let (reader, mut writer) = pipe();
    let writing = thread::spawn(move || {
        let chunk = vec![b'z'; CHUNK];
        loop {
            if let Err(refused) = writer.write_all(&chunk) {
                return refused;
            }
        }
    });
    let reader = Reader::start(reader).expect("a reader thread");
    // Long enough for the thread to fill the channel and wait to hand over
    // the next chunk, whatever the pipe held.
    thread::sleep(PAUSE * 20);

    let dropping = Instant::now();
    drop(reader);
    let took = dropping.elapsed();

    assert!(
        took < BOUND,
        "dropping the reader took {took:?}, past its bound of {BOUND:?}"
    );
    let deadline = Instant::now() + WAIT;
    while !writing.is_finished() {
        assert!(
            Instant::now() < deadline,
            "the pipe's writer was never refused"
        );
        thread::sleep(Duration::from_millis(1));
    }
    let refused = writing.join().expect("the writer");
    assert_eq!(refused.kind(), io::ErrorKind::BrokenPipe);
}

/// One pause, one read that does not wait and the join, with room for a busy
/// test host; the drop tests fail past it rather than hang.
const BOUND: Duration = Duration::from_secs(1);

#[test]
fn an_owned_writer_delivers_what_it_was_given_in_order() {
    let (mut reader, writer) = io::pipe().expect("a pipe");
    let (mut input, _thread) =
        Writer::start(writer, Box::new(|_: &JoinHandle<()>| {})).expect("a writer");
    let reading = thread::spawn(move || {
        let mut kept = Vec::new();
        io::Read::read_to_end(&mut reader, &mut kept).map(|_| kept)
    });
    let expected: Vec<u8> = (0..3 * CHUNK + 5)
        .map(|at| u8::try_from(at % 253).expect("below 253"))
        .collect();

    runtime().block_on(async {
        let mut rest = expected.as_slice();
        while !rest.is_empty() {
            let written = input.write(rest).await.expect("a write");
            assert!(written > 0 && written <= CHUNK, "a write took {written}");
            rest = rest
                .get(written..)
                .expect("a count within what was written");
        }
        assert_eq!(input.write(b"").await.expect("an empty write"), 0);
    });
    drop(input);

    assert_eq!(
        reading
            .join()
            .expect("the reader")
            .expect("every byte read"),
        expected
    );
}

/// A pipe whose write parks until the interruption releases it, as a write to
/// a peer that stopped reading does, and which says when its thread returned.
struct Parked {
    released: Arc<(Mutex<bool>, Condvar)>,
    returned: Arc<AtomicBool>,
}

impl Write for Parked {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        let (released, woken) = &*self.released;
        let mut released = released.lock().expect("the fixture's lock");
        while !*released {
            released = woken.wait(released).expect("the fixture's lock");
        }
        // What an abandoned write answers; not `Interrupted`, which a caller
        // writing all of a chunk would take as a reason to write again.
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Parked {
    fn drop(&mut self) {
        self.returned.store(true, Ordering::SeqCst);
    }
}

/// A writer whose write is parked, with the flags a test reads: whether the
/// pipe was released and whether the thread has let go of it.
struct Fixture {
    input: Writer,
    thread: WriterThread,
    released: Arc<(Mutex<bool>, Condvar)>,
    returned: Arc<AtomicBool>,
}

/// Starts a writer on a [`Parked`] pipe, interrupted by releasing it where
/// `reaches` says the interruption reaches it, and parks one write in it.
fn parked(reaches: bool) -> Fixture {
    let released = Arc::new((Mutex::new(false), Condvar::new()));
    let returned = Arc::new(AtomicBool::new(false));
    let interrupt: Interrupt = if reaches {
        let released = Arc::clone(&released);
        Box::new(move |_: &JoinHandle<()>| release(&released))
    } else {
        Box::new(|_: &JoinHandle<()>| {})
    };
    let (mut input, thread) = Writer::start(
        Parked {
            released: Arc::clone(&released),
            returned: Arc::clone(&returned),
        },
        interrupt,
    )
    .expect("a writer");
    let gave_up = runtime().block_on(async {
        tokio::time::timeout(Duration::from_millis(50), input.write(b"never taken"))
            .await
            .is_err()
    });
    assert!(gave_up, "a parked write answered");
    Fixture {
        input,
        thread,
        released,
        returned,
    }
}

/// What stopping the command does to a pipe: every write in it fails.
fn release(released: &(Mutex<bool>, Condvar)) {
    let (flag, woken) = released;
    *flag.lock().expect("the fixture's lock") = true;
    woken.notify_all();
}

/// Waits until the thread has let go of its pipe, failing after `WAIT`.
fn let_go(returned: &AtomicBool) {
    let deadline = Instant::now() + WAIT;
    while !returned.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the thread kept its pipe");
        thread::sleep(Duration::from_millis(1));
    }
}

/// How long a drop may take: it waits for nothing, so this is room for a busy
/// test host, far below the interruption's bound.
const DROP: Duration = Duration::from_millis(50);

/// Dropping the writer interrupts the parked write once and returns without
/// waiting; the interruption reaches the write, and the thread lets go of the
/// pipe on its own.
#[test]
fn dropping_an_owned_writer_parked_in_a_write_interrupts_it_without_waiting() {
    let Fixture {
        input,
        mut thread,
        returned,
        ..
    } = parked(true);

    let dropping = Instant::now();
    drop(input);
    let took = dropping.elapsed();

    assert!(took < DROP, "dropping the writer took {took:?}");
    let_go(&returned);
    thread.end().expect("the thread ended");
}

/// A drop the interruption cannot reach still returns at once; the write
/// stays parked until the pipe goes, and then the owner joins the thread.
#[test]
fn dropping_an_owned_writer_the_interruption_cannot_reach_returns_at_once() {
    let Fixture {
        input,
        mut thread,
        released,
        returned,
    } = parked(false);

    let dropping = Instant::now();
    drop(input);
    let took = dropping.elapsed();

    assert!(took < DROP, "dropping the writer took {took:?}");
    assert!(
        !returned.load(Ordering::SeqCst),
        "a write nothing interrupted let go of its pipe"
    );
    release(&released);
    thread.end().expect("the thread ended once its pipe went");
    assert!(
        returned.load(Ordering::SeqCst),
        "ending the thread returned before the thread had"
    );
}

/// The owner's end joins the thread whether or not the writer was dropped:
/// here it is still held and idle, and the end hangs the thread up.
#[test]
fn ending_an_idle_writer_thread_joins_it_while_the_writer_is_held() {
    let returned = Arc::new(AtomicBool::new(false));
    let (input, mut thread) = Writer::start(
        Parked {
            released: Arc::new((Mutex::new(true), Condvar::new())),
            returned: Arc::clone(&returned),
        },
        Box::new(|_: &JoinHandle<()>| {}),
    )
    .expect("a writer");

    let ending = Instant::now();
    thread.end().expect("the thread ended");
    let took = ending.elapsed();

    assert!(
        returned.load(Ordering::SeqCst),
        "ending the thread returned before the thread had"
    );
    assert!(took < STOP, "ending an idle thread took {took:?}");
    drop(input);
}

/// The owner's end interrupts a parked write until the thread returns, and
/// joins it: the command's stop, while the writer is still held.
#[test]
fn ending_a_writer_thread_parked_in_a_write_interrupts_it_and_joins_it() {
    let Fixture {
        input,
        mut thread,
        returned,
        ..
    } = parked(true);

    thread.end().expect("the thread ended");

    assert!(
        returned.load(Ordering::SeqCst),
        "ending the thread returned before the thread had"
    );
    drop(input);
}

/// A write the interruption never reaches is a failed end, reported after the
/// bound, and the thread stays owned: a later end, once the pipe has gone,
/// joins it.
#[test]
fn ending_a_writer_thread_the_interruption_cannot_reach_fails_and_can_be_retried() {
    let Fixture {
        input,
        mut thread,
        released,
        returned,
    } = parked(false);

    let ending = Instant::now();
    let failed = thread.end().expect_err("a parked write was reported ended");
    let took = ending.elapsed();

    assert_eq!(failed.kind(), io::ErrorKind::TimedOut);
    assert!(took >= STOP && took < STOP + BOUND, "the end took {took:?}");
    release(&released);
    thread.end().expect("the retried end");
    assert!(returned.load(Ordering::SeqCst));
    drop(input);
}

#[test]
fn a_write_after_one_dropped_in_flight_collects_its_answer_first() {
    let (mut reader, writer) = io::pipe().expect("a pipe");
    let (mut input, _thread) =
        Writer::start(writer, Box::new(|_: &JoinHandle<()>| {})).expect("a writer");

    runtime().block_on(async {
        // Asked once, which hands its chunk over, and dropped whether or not
        // the thread had answered by then.
        let first = tokio::time::timeout(Duration::ZERO, input.write(b"first ")).await;
        if let Ok(answered) = first {
            assert_eq!(answered.expect("the first write"), 6);
        }
        let written = input.write(b"second").await.expect("the next write");
        assert_eq!(written, 6);
    });
    drop(input);

    let mut kept = Vec::new();
    io::Read::read_to_end(&mut reader, &mut kept).expect("the pipe's bytes");
    assert_eq!(kept, b"first second");
}

/// A command stopped before its input's first write: the owner is ended, and
/// the write that would have started a thread nothing would join is refused,
/// closing the pipe.
#[test]
fn an_owner_ended_before_its_first_write_starts_no_thread_and_refuses_the_write() {
    let (mut reader, writer) = io::pipe().expect("a pipe");
    let owner = WriterOwner::default();

    owner.end().expect("an owner with nothing to end");
    let refused = owner
        .start(writer, Box::new(|_: &JoinHandle<()>| {}))
        .expect_err("a writer started after its command was stopped");

    assert_eq!(refused.kind(), io::ErrorKind::BrokenPipe);
    assert!(!owner.started(), "a thread started after the stop");
    let mut kept = Vec::new();
    io::Read::read_to_end(&mut reader, &mut kept).expect("the pipe's end");
    assert!(kept.is_empty());
}

/// The owner's end joins the thread of the writer started through it, parked
/// or not: the command's stop.
#[test]
fn ending_an_owner_joins_the_thread_of_the_writer_it_started() {
    let released = Arc::new((Mutex::new(false), Condvar::new()));
    let returned = Arc::new(AtomicBool::new(false));
    let owner = WriterOwner::default();
    let interrupt: Interrupt = {
        let released = Arc::clone(&released);
        Box::new(move |_: &JoinHandle<()>| release(&released))
    };
    let mut input = owner
        .start(
            Parked {
                released,
                returned: Arc::clone(&returned),
            },
            interrupt,
        )
        .expect("a writer");
    let gave_up = runtime().block_on(async {
        tokio::time::timeout(Duration::from_millis(50), input.write(b"never taken"))
            .await
            .is_err()
    });
    assert!(gave_up, "a parked write answered");
    assert!(
        owner.started(),
        "the writer's thread was not left with its owner"
    );

    owner.end().expect("the thread ended");

    assert!(owner.joined(), "the owner's end left the thread unjoined");
    assert!(returned.load(Ordering::SeqCst));
    drop(input);
}
