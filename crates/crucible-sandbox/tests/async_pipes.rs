//! A backend written outside this crate, read and written asynchronously
//! through nothing but the defaults the traits give it.
//!
//! Its streams say only whether bytes are ready now and its input is a plain
//! writer, which is what every backend written before the waiting read and the
//! asynchronous input existed looks like. Each test builds the runtime it
//! waits on; nothing in the crate under test builds one.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a test reports exactly what it expected and did not get"
)]

use std::collections::VecDeque;
use std::io;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex};

use crucible_runtime::{BoxFuture, answered};
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxInspection,
    SandboxManifest, SandboxNetworkPolicy, SandboxOutput, SandboxPolicy, SandboxProcess,
    SandboxRead, SandboxRequest, SandboxResourceLimits, SandboxUsage, SandboxViolation,
};
use crucible_types::{Ancestry, SandboxId, ToolId};

/// An absolute path spelled the way the running platform's path type accepts.
#[cfg(unix)]
const ROOT: &str = "/workspace";
#[cfg(windows)]
const ROOT: &str = r"C:\workspace";

/// A stream that answers from a script: each entry is one answer, and bytes
/// are copied into the caller's buffer. It counts how often it was asked.
struct Scripted {
    answers: VecDeque<Answer>,
    asked: usize,
}

enum Answer {
    Pending,
    Bytes(&'static [u8]),
    Limited(&'static [u8], usize),
    End,
}

impl Scripted {
    fn new(answers: impl IntoIterator<Item = Answer>) -> Self {
        Self {
            answers: answers.into_iter().collect(),
            asked: 0,
        }
    }
}

impl SandboxOutput for Scripted {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        self.asked += 1;
        Ok(match self.answers.pop_front().unwrap_or(Answer::End) {
            Answer::Pending => SandboxRead::Pending,
            Answer::End => SandboxRead::End,
            Answer::Bytes(bytes) => {
                buffer
                    .get_mut(..bytes.len())
                    .expect("the test's buffer holds the scripted bytes")
                    .copy_from_slice(bytes);
                SandboxRead::Bytes(bytes.len())
            }
            Answer::Limited(bytes, discarded) => {
                buffer
                    .get_mut(..bytes.len())
                    .expect("the test's buffer holds the scripted bytes")
                    .copy_from_slice(bytes);
                SandboxRead::Limited {
                    retained: bytes.len(),
                    discarded,
                }
            }
        })
    }
}

/// A runtime with the clock the default read waits on, and nothing else.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a test runtime")
}

#[test]
fn a_waiting_read_answers_what_read_ready_would_once_it_has_something() {
    let mut output = Scripted::new([
        Answer::Pending,
        Answer::Pending,
        Answer::Bytes(b"abc"),
        Answer::Pending,
        Answer::Limited(b"de", 7),
        Answer::Pending,
        Answer::End,
    ]);
    let mut buffer = [0; 8];

    let answers = runtime().block_on(async {
        let first = output.read(&mut buffer).await.expect("a first read");
        let bytes = buffer.get(..3).expect("three bytes").to_vec();
        let second = output.read(&mut buffer).await.expect("a second read");
        let limited = buffer.get(..2).expect("two bytes").to_vec();
        let third = output.read(&mut buffer).await.expect("a third read");
        (first, bytes, second, limited, third)
    });

    assert_eq!(
        answers,
        (
            SandboxRead::Bytes(3),
            b"abc".to_vec(),
            SandboxRead::Limited {
                retained: 2,
                discarded: 7
            },
            b"de".to_vec(),
            SandboxRead::End
        )
    );
    assert_eq!(output.asked, 7, "every answer in the script was asked for");
}

#[test]
fn a_waiting_read_into_nothing_answers_at_once_without_asking_the_stream() {
    let mut output = Scripted::new([Answer::Pending]);

    let answer = answered!(output.read(&mut [])).expect("an empty read");

    assert_eq!(answer, SandboxRead::Bytes(0));
    assert_eq!(output.asked, 0);
}

#[test]
fn a_waiting_read_off_a_runtime_is_an_error_rather_than_a_panic() {
    let mut output = Scripted::new([Answer::Pending, Answer::Bytes(b"late")]);
    let mut buffer = [0; 8];

    let answer = answered!(output.read(&mut buffer));

    assert!(
        answer.is_err(),
        "a read that had to wait off a runtime answered {answer:?}"
    );
}

/// A stream that is never ready when asked without waiting, and answers only
/// the waiting read: what a stream the runtime can wait on looks like.
struct Waitable;

impl SandboxOutput for Waitable {
    fn read_ready(&mut self, _buffer: &mut [u8]) -> io::Result<SandboxRead> {
        Ok(SandboxRead::Pending)
    }

    fn read<'a>(&'a mut self, buffer: &'a mut [u8]) -> BoxFuture<'a, io::Result<SandboxRead>> {
        Box::pin(async move {
            let first = buffer.first_mut().expect("room for one byte");
            *first = b'w';
            Ok(SandboxRead::Bytes(1))
        })
    }
}

#[test]
fn a_boxed_output_waits_the_way_what_it_holds_does() {
    let mut output: Box<dyn SandboxOutput> = Box::new(Waitable);
    let mut buffer = [0; 4];

    let answer = answered!(output.read(&mut buffer)).expect("the boxed read");

    assert_eq!((answer, buffer[0]), (SandboxRead::Bytes(1), b'w'));
}

/// What a command was told, and whether it was flushed after each write.
#[derive(Default)]
struct Told {
    bytes: Vec<u8>,
    unflushed: usize,
}

struct Input(Arc<Mutex<Told>>);

impl io::Write for Input {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut told = self.0.lock().expect("the fixture's lock");
        let taken = bytes.len().min(3);
        told.bytes
            .extend_from_slice(bytes.get(..taken).expect("a prefix"));
        told.unflushed += taken;
        Ok(taken)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().expect("the fixture's lock").unflushed = 0;
        Ok(())
    }
}

/// A process with nothing but a standard input, which takes at most three
/// bytes a write.
struct Listening {
    input: Option<Input>,
    inspection: SandboxInspection,
}

impl SandboxProcess for Listening {
    fn take_stdin(&mut self) -> Option<Box<dyn io::Write + Send>> {
        self.input
            .take()
            .map(|input| Box::new(input) as Box<dyn io::Write + Send>)
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        None
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        None
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Ok(None)
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn usage(&self) -> SandboxUsage {
        SandboxUsage::default()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        None
    }
}

fn listening(told: &Arc<Mutex<Told>>) -> Listening {
    let rule = SandboxFilesystemRule::new(
        ROOT,
        SandboxFilesystemAccess::ReadWrite,
        SandboxFilesystemProvenance::Workspace,
    )
    .expect("a valid rule");
    let policy = SandboxPolicy::new(
        false,
        [rule],
        ROOT,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("a valid policy");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("listening"),
        policy,
        SandboxManifest::empty(),
    );
    let identity = SandboxBackendIdentity::new(
        SandboxBackendId::new("external").expect("a valid backend id"),
        "1",
        SandboxBackendProvenance::Compatibility,
        None,
    )
    .expect("a valid identity");
    let inspection = SandboxInspection::unconfined_for_request(
        identity,
        SandboxCapabilities::none(),
        &request,
        "a test, which confines nothing",
    )
    .expect("an inspection of a request a test built");
    Listening {
        input: Some(Input(Arc::clone(told))),
        inspection,
    }
}

#[test]
fn the_default_input_writes_and_flushes_the_pipe_take_stdin_hands_over() {
    let told = Arc::new(Mutex::new(Told::default()));
    let mut process = listening(&told);

    let mut input = process
        .take_async_stdin()
        .expect("a process with a standard input hands it over");
    let written = answered!(input.write(b"hello")).expect("a write");
    let nothing = answered!(input.write(b"")).expect("an empty write");

    let told = told.lock().expect("the fixture's lock");
    assert_eq!((written, nothing), (3, 0));
    assert_eq!(told.bytes, b"hel");
    assert_eq!(told.unflushed, 0, "a write was left unflushed");
    assert!(
        process.take_async_stdin().is_none() && process.take_stdin().is_none(),
        "standard input was handed over twice"
    );
}
