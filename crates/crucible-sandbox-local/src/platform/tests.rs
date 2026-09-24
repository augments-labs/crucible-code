//! A real command's pipes, read and written waiting and without waiting, on
//! every platform.
//!
//! The command is the platform's own shell, and each test compares what the
//! waiting adapters carried with what the synchronous path carries for the
//! same command, rather than with a spelling of the output one platform alone
//! would produce.

use std::io::Write as _;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::*;

const WAIT: Duration = Duration::from_secs(30);

/// A runtime with the reactor a Unix pipe is waited on with, and the clock
/// the tests' deadlines are kept by.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a test runtime")
}

/// The platform's shell running `script`, with every pipe crucible uses.
fn shell(script: &str) -> Child {
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    };
    // Given to cmd as written: its quoting is its own, not the C runtime's
    // the standard library would otherwise quote for.
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("cmd.exe");
        std::os::windows::process::CommandExt::raw_arg(command.args(["/d", "/c"]), script);
        command
    };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the platform's shell")
}

/// Prints two thousand numbered lines to standard output.
#[cfg(unix)]
const LINES: &str = "i=0; while [ $i -lt 2000 ]; do echo line $i; i=$((i+1)); done";
#[cfg(windows)]
const LINES: &str = "for /l %i in (1,1,2000) do @echo line %i";

/// Floods standard error with more than a pipe holds, then says it is done.
#[cfg(unix)]
const FLOOD: &str = "head -c 1048576 /dev/zero >&2; echo done";
#[cfg(windows)]
const FLOOD: &str = "(for /l %i in (1,1,5000) do @echo 0123456789012345678901234567890123456789012345678901234567890123) 1>&2 & echo done";

/// Says back every line it is told.
#[cfg(unix)]
const ECHO: &str = "cat";
#[cfg(windows)]
const ECHO: &str = "findstr /r \"^\"";

/// Everything `stream` gives until its end, read without waiting.
fn polled(stream: &mut dyn Stream) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buffer = [0; 1000];
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(Instant::now() < deadline, "the stream did not end");
        match stream.read_ready(&mut buffer).expect("a read") {
            ReadState::Bytes(count) => {
                kept.extend_from_slice(buffer.get(..count).expect("a count within the buffer"));
            }
            ReadState::Pending => std::thread::sleep(Duration::from_millis(1)),
            ReadState::End => return kept,
        }
    }
}

/// Everything `stream` gives until its end, waited for, `width` bytes at a
/// time.
async fn waited(mut stream: Box<dyn Stream>, width: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buffer = vec![0; width];
    loop {
        match stream.read(&mut buffer).await.expect("a read") {
            ReadState::Bytes(count) => {
                kept.extend_from_slice(buffer.get(..count).expect("a count within the buffer"));
            }
            ReadState::End => return kept,
            ReadState::Pending => panic!("a waiting read answered pending"),
        }
    }
}

fn stdout(child: &mut Child) -> Box<dyn Stream> {
    stream(child.stdout.take().expect("stdout")).expect("a prepared stdout")
}

fn stderr(child: &mut Child) -> Box<dyn Stream> {
    stream(child.stderr.take().expect("stderr")).expect("a prepared stderr")
}

fn reaped(mut child: Child) {
    let deadline = Instant::now() + WAIT;
    while child.try_wait().expect("a status").is_none() {
        assert!(Instant::now() < deadline, "the command did not exit");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_waited_read_carries_what_a_read_without_waiting_carries() {
    let mut first = shell(LINES);
    let expected = polled(stdout(&mut first).as_mut());
    reaped(first);

    let mut second = shell(LINES);
    let output = stdout(&mut second);
    let kept = runtime().block_on(async { tokio::time::timeout(WAIT, waited(output, 7)).await });
    reaped(second);

    assert!(expected.len() > 2000 * 6, "only {} bytes", expected.len());
    assert_eq!(kept.expect("the stream ended in time"), expected);
}

/// What `script` says back to `told`, written through the waiting input and
/// read through the waiting output concurrently, so neither side's pipe can
/// fill and wedge the other.
fn said_back_waiting(told: Vec<u8>) -> Vec<u8> {
    let mut child = shell(ECHO);
    let thread = InputThread::default();
    let mut input = input(child.stdin.take().expect("stdin"), &thread);
    let output = stdout(&mut child);
    let kept = runtime().block_on(async move {
        let reading = tokio::spawn(waited(output, 4096));
        let mut rest = told.as_slice();
        while !rest.is_empty() {
            let written = tokio::time::timeout(WAIT, input.write(rest))
                .await
                .expect("the command took what it was told in time")
                .expect("a write");
            rest = rest
                .get(written..)
                .expect("a count within what was written");
        }
        drop(input);
        tokio::time::timeout(WAIT, reading)
            .await
            .expect("the command finished speaking in time")
            .expect("the reading task")
    });
    reaped(child);
    thread
        .end()
        .expect("the input's thread, where there is one, ended");
    kept
}

/// The same, written and read synchronously, the writing on a thread of its
/// own for the same reason.
fn said_back_polled(told: Vec<u8>) -> Vec<u8> {
    let mut child = shell(ECHO);
    let mut input = child.stdin.take().expect("stdin");
    let mut output = stdout(&mut child);
    let writing = std::thread::spawn(move || input.write_all(&told));
    let kept = polled(output.as_mut());
    writing
        .join()
        .expect("the writer")
        .expect("every byte written");
    reaped(child);
    kept
}

#[test]
fn a_waiting_write_is_heard_as_a_synchronous_one_is() {
    let told = b"a kettle\n".to_vec();

    let waiting = said_back_waiting(told.clone());
    let synchronous = said_back_polled(told);

    assert!(
        String::from_utf8_lossy(&waiting).contains("a kettle"),
        "{waiting:?}"
    );
    assert_eq!(waiting, synchronous);
}

/// More than a pipe holds each way, so the writer waits on a command that
/// reads only as fast as its own output is read.
#[test]
fn a_slow_peer_is_waited_on_in_both_directions_without_losing_a_byte() {
    let told: Vec<u8> = (0..8192)
        .flat_map(|line| format!("{line:031}\n").into_bytes())
        .collect();

    let waiting = said_back_waiting(told.clone());
    let synchronous = said_back_polled(told.clone());

    assert!(waiting.len() >= told.len(), "only {} bytes", waiting.len());
    assert_eq!(waiting, synchronous);
}

#[test]
fn a_flood_on_standard_error_is_read_beside_standard_output_to_both_ends() {
    let mut first = shell(FLOOD);
    let mut first_stderr = stderr(&mut first);
    let mut first_stdout = stdout(&mut first);
    let flooding = std::thread::spawn(move || polled(first_stderr.as_mut()));
    let said = polled(first_stdout.as_mut());
    let flooded = flooding.join().expect("the stderr reader");
    reaped(first);

    let mut second = shell(FLOOD);
    let errors = stderr(&mut second);
    let output = stdout(&mut second);
    let (waited_said, waited_flooded) = runtime().block_on(async move {
        let flood = tokio::spawn(waited(errors, 4096));
        let said = tokio::time::timeout(WAIT, waited(output, 4096))
            .await
            .expect("standard output ended in time");
        let flooded = tokio::time::timeout(WAIT, flood)
            .await
            .expect("standard error ended in time")
            .expect("the stderr task");
        (said, flooded)
    });
    reaped(second);

    assert!(flooded.len() > 64 * 1024, "only {} bytes", flooded.len());
    assert_eq!((waited_said, waited_flooded), (said, flooded));
}

/// A pipe with no command behind it, ready to be waited on.
#[cfg(unix)]
fn plain_pipe() -> (Box<dyn Stream>, io::PipeWriter) {
    let (reader, writer) = io::pipe().expect("a pipe");
    (stream(reader).expect("a prepared pipe"), writer)
}

/// A pipe whose reading end can be asked whether this process still holds it.
///
/// A writer refused is the pipe's answer about every copy of its reading end,
/// and not every copy is the adapter's: a test of this process that forks to
/// start a command holds one until its child execs, and a write made in that
/// window succeeds although the adapter closed its own. So the drop tests ask
/// this process, at the moment the drop returns: the value the adapter was
/// handed must have been dropped and, where the process's descriptors can be
/// listed, none of them may still be the pipe's reading end — which a copy the
/// adapter made and closes later would be, while a forked child's is not. The
/// writer's refusal is then only asked for once any such window has passed.
pub(super) struct Closing {
    pipe: io::PipeReader,
    // Declared after the pipe, so dropped after it: it says the pipe is closed.
    _closed: Closed,
}

struct Closed(Arc<AtomicBool>);

impl Drop for Closed {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// What a test asks, once the adapter holding a [`Closing`] pipe is dropped.
pub(super) struct Watch {
    dropped: Arc<AtomicBool>,
    /// The pipe's identity, and the descriptor of its writing end, which
    /// shares that identity on Linux and is the test's own to hold.
    #[cfg(target_os = "linux")]
    pipe: ((u64, u64), std::os::fd::RawFd),
}

impl Closing {
    /// `pipe`, whose writing end is `writer`, and what watches it close.
    pub(super) fn new(pipe: io::PipeReader, writer: &io::PipeWriter) -> (Self, Watch) {
        #[cfg(not(target_os = "linux"))]
        let _ = writer;
        let dropped = Arc::default();
        let watch = Watch {
            dropped: Arc::clone(&dropped),
            #[cfg(target_os = "linux")]
            pipe: (
                identity(std::os::fd::AsRawFd::as_raw_fd(&pipe)).expect("the pipe's identity"),
                std::os::fd::AsRawFd::as_raw_fd(writer),
            ),
        };
        let pipe = Self {
            pipe,
            _closed: Closed(dropped),
        };
        (pipe, watch)
    }
}

impl Watch {
    /// What in this process still holds the pipe's reading end: nothing, once
    /// it is closed here.
    pub(super) fn open(&self) -> Vec<String> {
        let mut open = Vec::new();
        if !self.dropped.load(Ordering::SeqCst) {
            open.push("the pipe the adapter was handed".to_owned());
        }
        #[cfg(target_os = "linux")]
        {
            let (pipe, writer) = self.pipe;
            let listed = std::fs::read_dir("/proc/self/fd").expect("this process's descriptors");
            for entry in listed.flatten() {
                let Some(descriptor) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<std::os::fd::RawFd>().ok())
                else {
                    continue;
                };
                // One another thread closed since the listing has no identity
                // left to compare, and is passed over; the listing's own stays
                // open throughout and is the directory, never the pipe.
                if descriptor != writer && identity(descriptor).is_ok_and(|seen| seen == pipe) {
                    open.push(format!("descriptor {descriptor}"));
                }
            }
        }
        open
    }
}

/// The device and inode `descriptor` of this process refers to.
#[cfg(target_os = "linux")]
fn identity(descriptor: std::os::fd::RawFd) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::metadata(format!("/proc/self/fd/{descriptor}"))?;
    Ok((metadata.dev(), metadata.ino()))
}

impl Read for Closing {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.pipe.read(buffer)
    }
}

impl Output for Closing {
    fn prepare(&self) -> io::Result<()> {
        self.pipe.prepare()
    }

    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<ReadState> {
        self.pipe.read_ready(buffer)
    }
}

#[cfg(unix)]
impl std::os::fd::AsRawFd for Closing {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.pipe.as_raw_fd()
    }
}

/// How `writer` is refused, once no copy of its pipe's reading end is left.
pub(super) fn refused(writer: &mut io::PipeWriter) -> io::Error {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Err(refused) = writer.write_all(b"x") {
            return refused;
        }
        assert!(Instant::now() < deadline, "the pipe was never closed");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Whatever waits on the pipe for this stream — the reactor's registration on
/// Unix, the thread on Windows — is gone once the stream is, and the pipe with
/// it: closed by the time the drop returns, and the writer refused.
#[test]
fn dropping_a_stream_mid_wait_closes_its_pipe_within_the_bound() {
    let (reader, mut writer) = io::pipe().expect("a pipe");
    let (reader, watch) = Closing::new(reader, &writer);
    let mut stream = stream(reader).expect("a prepared pipe");
    let gave_up = runtime().block_on(async {
        let mut buffer = [0; 16];
        tokio::time::timeout(Duration::from_millis(50), stream.read(&mut buffer))
            .await
            .is_err()
    });
    assert!(gave_up, "a quiet pipe answered");

    let dropping = Instant::now();
    drop(stream);
    let took = dropping.elapsed();

    assert!(took < Duration::from_secs(1), "dropping took {took:?}");
    assert_eq!(
        watch.open(),
        Vec::<String>::new(),
        "the pipe outlived its stream"
    );
    assert_eq!(refused(&mut writer).kind(), io::ErrorKind::BrokenPipe);
}

/// Only Unix needs the runtime: the reactor is the runtime's. A Windows pipe
/// is waited on by its own thread and needs none.
#[cfg(unix)]
#[test]
fn a_stream_waited_on_off_a_runtime_is_an_error_rather_than_a_panic() {
    let (mut stream, _writer) = plain_pipe();
    let mut buffer = [0; 16];

    let answer = crucible_runtime::answered!(stream.read(&mut buffer));

    assert!(answer.is_err(), "a read off a runtime answered {answer:?}");
}

/// Never reads its standard input and never ends, in cmd itself, so killing
/// cmd leaves nothing behind.
#[cfg(windows)]
const DEAF: &str = "for /l %i in (0,0,1) do @rem";

/// A write parked in a real pipe a command never reads is abandoned by the
/// drop of the input through the platform's own cancellation, not a stand-in
/// for it: the thread lets go of the pipe within the bound while the command
/// still runs, and the command's owner then joins it at once.
#[cfg(windows)]
#[test]
fn dropping_an_input_parked_in_a_full_pipe_is_heard_within_the_bound() {
    let mut child = shell(DEAF);
    let thread = InputThread::default();
    let mut input = input(child.stdin.take().expect("stdin"), &thread);
    let parked = runtime().block_on(async {
        let chunk = vec![b'x'; owned::CHUNK];
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let wrote = tokio::time::timeout(Duration::from_millis(200), input.write(&chunk));
                if wrote.await.is_err() {
                    return;
                }
            }
        })
        .await
        .is_ok()
    });
    assert!(parked, "the pipe never filled");

    let dropping = Instant::now();
    drop(input);
    let took = dropping.elapsed();
    let deadline = Instant::now() + Duration::from_secs(1);
    while !thread.finished() {
        assert!(
            Instant::now() < deadline,
            "the parked write was not abandoned within the bound"
        );
        std::thread::sleep(Duration::from_millis(1));
    }

    assert!(took < Duration::from_millis(50), "dropping took {took:?}");
    assert!(
        child.try_wait().expect("a status").is_none(),
        "the command ended, so the pipe closing proves nothing"
    );
    let ending = Instant::now();
    thread.end().expect("the thread ended");
    assert!(ending.elapsed() < Duration::from_millis(50));
    child.kill().expect("the command stopped");
    reaped(child);
}
