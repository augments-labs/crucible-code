//! A pipe the runtime cannot wait on, waited on by a thread it owns.
//!
//! A Windows anonymous pipe tells nobody when it becomes readable or writable,
//! so no reactor can be asked to wait on one. Each pipe read or written
//! asynchronously there gets a thread of its own, and the adapter talks to
//! that thread over a channel holding one message. This is the final shape of
//! the adapter on that platform: what it costs is one thread per pipe in use,
//! and what it retains is at most three chunks of [`CHUNK`] bytes for a reader,
//! as below, and one for a writer.
//!
//! A [`Reader`]'s thread never blocks in the pipe. It asks without waiting, as
//! the synchronous callers do, and sleeps [`PAUSE`] between answers of
//! nothing, looking before each ask for the adapter having gone. Dropping the
//! reader closes the channel and joins the thread, which therefore returns
//! within one pause and one read that does not wait, and closes the pipe as it
//! goes. At most three chunks are held at once: one the caller has not taken
//! all of, one in the channel, and one the thread is handing over.
//!
//! A [`Writer`]'s thread does block, in a write the peer is not reading, and
//! nothing but the platform can end that write. The thread is owned by a
//! [`WriterThread`], which whoever owns the command holds and ends when it
//! stops the command: it hangs the thread up and asks the platform, through
//! the interruption the writer was built with, to abandon a parked write, again
//! and again until the thread returns and is joined, or [`STOP`] has passed,
//! which it reports as a failed end and can be asked again. Dropping the
//! [`Writer`] waits for nothing: it hangs an idle thread up and interrupts a
//! parked write once, so the thread usually returns on its own, closing the
//! pipe; one the interruption did not reach returns when the command is
//! stopped, which closes the pipe's other end.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crucible_runtime::BoxFuture;
use crucible_sandbox::SandboxInput;
use tokio::sync::mpsc::{self, error::TryRecvError};

use super::{Output, ReadState};

/// The most bytes one read or write hands across the channel.
pub(super) const CHUNK: usize = 4096;

/// How long a reader's thread leaves a quiet pipe before asking it again: the
/// pause the synchronous readers of a pipe already take.
pub(super) const PAUSE: Duration = Duration::from_millis(5);

/// How long [`WriterThread::end`] keeps interrupting a write parked in the
/// pipe before it reports the thread as not returned.
pub(super) const STOP: Duration = Duration::from_millis(250);

/// What a reader's thread hands over: bytes, or the end of the stream.
enum Chunk {
    Bytes(Vec<u8>),
    End,
}

/// An output pipe read on a thread of its own.
pub(crate) struct Reader {
    chunks: mpsc::Receiver<io::Result<Chunk>>,
    /// The chunk the caller's buffer had no room for all of, and how much of
    /// it has been handed over.
    held: Vec<u8>,
    taken: usize,
    ended: bool,
    thread: Option<JoinHandle<()>>,
}

impl Reader {
    /// Moves `pipe`, prepared for reads that do not wait, onto a thread.
    ///
    /// # Errors
    ///
    /// The thread could not be started, and the pipe is closed.
    pub(crate) fn start(pipe: impl Output) -> io::Result<Self> {
        let (to, chunks) = mpsc::channel(1);
        let thread = thread::Builder::new()
            .name("crucible-sandbox-pipe-reader".into())
            .spawn(move || drain(pipe, &to))?;
        Ok(Self {
            chunks,
            held: Vec::new(),
            taken: 0,
            ended: false,
            thread: Some(thread),
        })
    }

    /// Reads bytes the thread has handed over, or says why there are none.
    pub(crate) fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<ReadState> {
        if buffer.is_empty() {
            return Ok(ReadState::Pending);
        }
        if let Some(answer) = self.answer(buffer) {
            return Ok(answer);
        }
        match self.chunks.try_recv() {
            Ok(chunk) => self.took(Some(chunk), buffer),
            Err(TryRecvError::Empty) => Ok(ReadState::Pending),
            Err(TryRecvError::Disconnected) => self.took(None, buffer),
        }
    }

    /// Reads as [`Self::read_ready`] does, once there is something to answer.
    pub(crate) async fn read(&mut self, buffer: &mut [u8]) -> io::Result<ReadState> {
        if buffer.is_empty() {
            return Ok(ReadState::Bytes(0));
        }
        loop {
            if let Some(answer) = self.answer(buffer) {
                return Ok(answer);
            }
            let chunk = self.chunks.recv().await;
            match self.took(chunk, buffer)? {
                ReadState::Pending => {}
                answer => return Ok(answer),
            }
        }
    }

    /// What is already on this side of the channel, if anything.
    fn answer(&mut self, buffer: &mut [u8]) -> Option<ReadState> {
        let rest = self.held.get(self.taken..).unwrap_or_default();
        if !rest.is_empty() {
            let count = rest.len().min(buffer.len());
            let (Some(from), Some(to)) = (rest.get(..count), buffer.get_mut(..count)) else {
                return None;
            };
            to.copy_from_slice(from);
            self.taken += count;
            return Some(ReadState::Bytes(count));
        }
        self.ended.then_some(ReadState::End)
    }

    /// Takes what the thread handed over, or its absence.
    fn took(
        &mut self,
        chunk: Option<io::Result<Chunk>>,
        buffer: &mut [u8],
    ) -> io::Result<ReadState> {
        match chunk {
            Some(Ok(Chunk::Bytes(bytes))) => {
                self.held = bytes;
                self.taken = 0;
                Ok(self.answer(buffer).unwrap_or(ReadState::Pending))
            }
            Some(Ok(Chunk::End)) => {
                self.ended = true;
                Ok(ReadState::End)
            }
            Some(Err(problem)) => Err(problem),
            None => Err(io::Error::other(
                "the thread reading the command's output has stopped",
            )),
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // The thread looks for this before every read and is woken by it in a
        // hand-over, so the join below waits one pause at most.
        self.chunks.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader")
            .field("held", &self.held.len().saturating_sub(self.taken))
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

/// The body of a reader's thread: asks the pipe without waiting, pauses when
/// it had nothing, and hands over what it had until the stream ends, the pipe
/// fails, or the reader has gone.
fn drain(mut pipe: impl Output, to: &mpsc::Sender<io::Result<Chunk>>) {
    let mut buffer = vec![0; CHUNK];
    loop {
        if to.is_closed() {
            return;
        }
        let chunk = match pipe.read_ready(&mut buffer) {
            Ok(ReadState::Pending) => {
                thread::sleep(PAUSE);
                continue;
            }
            Ok(ReadState::Bytes(count)) => buffer
                .get(..count)
                .map(|bytes| Chunk::Bytes(bytes.to_vec()))
                .ok_or_else(|| io::Error::other("the pipe reported more bytes than it was given")),
            Ok(ReadState::End) => Ok(Chunk::End),
            Err(problem) => Err(problem),
        };
        let last = !matches!(chunk, Ok(Chunk::Bytes(_)));
        if to.blocking_send(chunk).is_err() || last {
            return;
        }
    }
}

/// What abandons a write the thread named by the handle is parked in.
pub(crate) type Interrupt = Box<dyn Fn(&JoinHandle<()>) + Send + Sync>;

/// What a writer's thread is handed: a chunk to write, or the word to return.
enum Message {
    Chunk(Vec<u8>),
    HangUp,
}

/// What a [`Writer`] and its [`WriterThread`] share: the thread, and how to
/// abandon a write it is parked in.
struct Shared {
    thread: Mutex<Option<JoinHandle<()>>>,
    interrupt: Interrupt,
}

impl Shared {
    /// Abandons whatever write the thread is parked in, once, if it has not
    /// already returned.
    fn interrupt(&self) {
        if let Ok(thread) = self.thread.lock()
            && let Some(thread) = thread.as_ref()
            && !thread.is_finished()
        {
            (self.interrupt)(thread);
        }
    }
}

/// An input pipe written on a thread of its own.
pub(crate) struct Writer {
    chunks: mpsc::Sender<Message>,
    written: mpsc::Receiver<io::Result<usize>>,
    /// Whether a chunk was handed over and its answer not collected, because
    /// the write that handed it over was dropped first.
    owed: bool,
    shared: Arc<Shared>,
}

/// The thread a [`Writer`] writes on, held by whoever owns the command the
/// pipe belongs to, which ends it with [`Self::end`].
pub(crate) struct WriterThread {
    hang_up: mpsc::Sender<Message>,
    shared: Arc<Shared>,
}

impl Writer {
    /// Moves `pipe` onto a thread, and hands back the writer and the thread's
    /// owner; `interrupt` is how either abandons a write that thread is
    /// parked in.
    ///
    /// # Errors
    ///
    /// The thread could not be started, and the pipe is closed.
    pub(crate) fn start(
        mut pipe: impl Write + Send + 'static,
        interrupt: Interrupt,
    ) -> io::Result<(Self, WriterThread)> {
        let (chunks, mut waiting) = mpsc::channel::<Message>(1);
        let (spoke, written) = mpsc::channel(1);
        let thread = thread::Builder::new()
            .name("crucible-sandbox-pipe-writer".into())
            .spawn(move || {
                while let Some(Message::Chunk(chunk)) = waiting.blocking_recv() {
                    let gone = pipe
                        .write_all(&chunk)
                        .and_then(|()| pipe.flush())
                        .map(|()| chunk.len());
                    let failed = gone.is_err();
                    // Refused once the writer has gone: nobody is left to
                    // hand another chunk over.
                    if spoke.blocking_send(gone).is_err() || failed {
                        return;
                    }
                }
            })?;
        let shared = Arc::new(Shared {
            thread: Mutex::new(Some(thread)),
            interrupt,
        });
        Ok((
            Self {
                chunks: chunks.clone(),
                written,
                owed: false,
                shared: Arc::clone(&shared),
            },
            WriterThread {
                hang_up: chunks,
                shared,
            },
        ))
    }

    async fn written(&mut self) -> io::Result<usize> {
        let written = self.written.recv().await.ok_or_else(closed)?;
        self.owed = false;
        written
    }
}

fn closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "the thread writing the command's input has stopped",
    )
}

/// A write hands over at most one [`CHUNK`] and answers once the thread has
/// written all of it. A write dropped after handing its chunk over may still
/// deliver it; the next write collects that answer first, and fails with it if
/// it was a failure.
impl SandboxInput for Writer {
    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> BoxFuture<'a, io::Result<usize>> {
        Box::pin(async move {
            if bytes.is_empty() {
                return Ok(0);
            }
            if self.owed {
                self.written().await?;
            }
            let chunk = bytes.get(..bytes.len().min(CHUNK)).unwrap_or(bytes);
            self.chunks
                .send(Message::Chunk(chunk.to_vec()))
                .await
                .map_err(|_| closed())?;
            self.owed = true;
            self.written().await
        })
    }
}

/// Signals and returns, waiting for nothing: an idle thread is hung up, and a
/// write it is parked in is interrupted once. What that costs the dropping
/// thread is one attempt to queue a message and one interruption. The thread
/// is joined by its [`WriterThread`], never here.
impl Drop for Writer {
    fn drop(&mut self) {
        self.written.close();
        let _ = self.chunks.try_send(Message::HangUp);
        self.shared.interrupt();
    }
}

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writer")
            .field("owed", &self.owed)
            .finish_non_exhaustive()
    }
}

impl WriterThread {
    /// Ends the thread and joins it: hangs it up, and interrupts a write it is
    /// parked in, again and again until it returns or [`STOP`] has passed.
    /// Ending a thread already joined does nothing.
    ///
    /// # Errors
    ///
    /// The thread had not returned by then, which is `TimedOut`, and it stays
    /// owned here for a later end; or it panicked.
    pub(crate) fn end(&mut self) -> io::Result<()> {
        let deadline = Instant::now() + STOP;
        loop {
            // Queued only while the channel has room; a full one holds a chunk
            // the thread takes next, and the loop tries again after it.
            let _ = self.hang_up.try_send(Message::HangUp);
            {
                let mut thread = self
                    .shared
                    .thread
                    .lock()
                    .map_err(|_| io::Error::other("the writer thread's owner was poisoned"))?;
                let Some(running) = thread.as_ref() else {
                    return Ok(());
                };
                if running.is_finished() {
                    return match thread.take().map(JoinHandle::join) {
                        Some(Err(_)) => Err(io::Error::other(
                            "the thread writing the command's input panicked",
                        )),
                        Some(Ok(())) | None => Ok(()),
                    };
                }
                if Instant::now() >= deadline {
                    // The one state in which the thread is not joined: it got
                    // no answer for `STOP`, a thread that had no CPU for that
                    // long included. It is kept here, not let go: reported as
                    // `TimedOut`, a failed cleanup, which the command's stop
                    // reports and a later end retries. A process dropped in
                    // this state is quarantined with its reservation, as the
                    // runtime reports a task it could not stop rather than
                    // wait on it.
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the thread writing the command's input did not return",
                    ));
                }
                (self.shared.interrupt)(running);
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Whether the thread was joined.
    #[cfg(test)]
    fn joined(&self) -> bool {
        self.shared
            .thread
            .lock()
            .is_ok_and(|thread| thread.is_none())
    }

    /// Whether the thread has returned, joined or not.
    #[cfg(all(test, windows))]
    pub(crate) fn finished(&self) -> bool {
        self.shared
            .thread
            .lock()
            .is_ok_and(|thread| thread.as_ref().is_none_or(JoinHandle::is_finished))
    }
}

impl std::fmt::Debug for WriterThread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriterThread").finish_non_exhaustive()
    }
}

/// Who owns the thread a command's input is written on, for the command:
/// empty until a writer is started through it, and ended once, when the
/// command is stopped, after which it starts no writer.
#[derive(Clone, Default)]
pub(crate) struct WriterOwner(Arc<Mutex<Owned>>);

#[derive(Default)]
struct Owned {
    thread: Option<WriterThread>,
    ended: bool,
}

impl WriterOwner {
    /// Starts a writer on `pipe`, keeping its thread here; see
    /// [`Writer::start`].
    ///
    /// # Errors
    ///
    /// The command was stopped, so no thread may start for it
    /// (`BrokenPipe`); or the thread could not be started. Either way the
    /// pipe is closed.
    pub(crate) fn start(
        &self,
        pipe: impl Write + Send + 'static,
        interrupt: Interrupt,
    ) -> io::Result<Writer> {
        let mut owned = self.owned()?;
        if owned.ended {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the command's input was closed when the command was stopped",
            ));
        }
        let (writer, thread) = Writer::start(pipe, interrupt)?;
        owned.thread = Some(thread);
        Ok(writer)
    }

    /// Ends and joins the thread, if one was started, and starts none after;
    /// see [`WriterThread::end`].
    ///
    /// # Errors
    ///
    /// As [`WriterThread::end`].
    pub(crate) fn end(&self) -> io::Result<()> {
        let mut owned = self.owned()?;
        owned.ended = true;
        owned.thread.as_mut().map_or(Ok(()), WriterThread::end)
    }

    fn owned(&self) -> io::Result<std::sync::MutexGuard<'_, Owned>> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("the input thread's owner was poisoned"))
    }

    /// Whether a writer was started through this owner.
    #[cfg(test)]
    pub(crate) fn started(&self) -> bool {
        self.0.lock().is_ok_and(|owned| owned.thread.is_some())
    }

    /// Whether this owner was ended and joined the thread it holds, if any.
    #[cfg(test)]
    pub(crate) fn joined(&self) -> bool {
        self.0.lock().is_ok_and(|owned| {
            owned.ended && owned.thread.as_ref().is_none_or(WriterThread::joined)
        })
    }

    /// Whether a writer was started and its thread has returned.
    #[cfg(all(test, windows))]
    pub(crate) fn finished(&self) -> bool {
        self.0
            .lock()
            .is_ok_and(|owned| owned.thread.as_ref().is_some_and(WriterThread::finished))
    }
}

impl std::fmt::Debug for WriterOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriterOwner").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
