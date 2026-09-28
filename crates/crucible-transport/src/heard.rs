//! What a confined process has said, as something a frame reader can read.
//!
//! A sandbox hands out its streams as [`SandboxOutput`], whose waiting read is
//! asynchronous. Something has to stand between that and the frame reader, and
//! this is it: a task on the runtime the host hands over reads the stream and
//! hands what it read across a bounded queue, and the host takes it from there
//! — as a [`BufRead`] waited on from the host's own thread, or as an
//! [`AsyncBufRead`] awaited — for a patience, a deadline and a cancellation
//! that the stream itself knows nothing about. Both are the same reader over
//! the same queue and give the same answers to the same waits; what an
//! awaited wait given up on leaves for the next is said on the
//! [`AsyncBufRead`] implementation.
//!
//! The queue is the bound on how far ahead of the host the task reads: the
//! task takes a place in it before each read, and the host gives the place
//! back as it takes the read. A host that stops asking leaves the task waiting
//! for a place, and everything after that in the peer's own pipe, so a peer
//! cannot fill crucible by out-talking it. Dropping the reader ends the task
//! wherever it is waiting.
//!
//! Two of the answers a confined stream gives are not ordinary reads. *Nothing
//! yet* is what a program thinking looks like, so it cannot be an ending — but
//! it is also what a program that has wedged looks like, and nothing here can
//! tell them apart, so crucible spends a patience on it and then says so.
//! *Bytes were dropped* is worse than a short read: the bytes that went past the
//! output ceiling included the newline somebody was going to use as a boundary,
//! so the stream is not shorter, it is unframeable.

use std::fmt;
use std::future::Future as _;
use std::io::{self, BufRead, Read};
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

use crucible_runtime::Cancel;
use crucible_sandbox::{SandboxOutput, SandboxRead};
use tokio::io::{AsyncBufRead, AsyncRead, ReadBuf};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{self, Receiver, Sender, error::TryRecvError};
use tokio::time::Sleep;

use crate::owned::{Doorbell, Owned, PAUSE, Ringing};

/// How often a host waiting for bytes looks at whether it was asked to stop.
///
/// A cancellation is a flag somebody raises rather than something that wakes
/// the waiting host, so it is looked at between waits of at most this long,
/// whether the host waits on its own thread or awaits.
const LOOK: Duration = Duration::from_millis(5);

/// How much of one read is taken at a time.
///
/// Not a ceiling on anything a program says. A frame's ceiling is the frame
/// reader's, which holds however many of these it takes; this is only how much
/// of a pipe is moved per read.
const CHUNK: usize = 8 * 1024;

/// How many reads the task may hold that the host has not taken: the places
/// in the queue.
///
/// With [`CHUNK`], the bound on what is read ahead of a host that has stopped
/// asking: a few reads' worth, so a host that asks again finds bytes waiting,
/// and nothing a peer can grow by talking faster than crucible listens.
const QUEUED: usize = 4;

/// One answer of the stream, as the reading task hands it over.
enum Arrived {
    /// Bytes it said.
    Bytes(Vec<u8>),
    /// Bytes went past the output ceiling, this many of them.
    Lost(usize),
    /// Its read failed, and this is why.
    Failed(io::Error),
    /// Every writer has closed it.
    End,
}

/// A confined stream, read as bytes that arrive rather than bytes that are
/// ready.
///
/// `O` is the kind of stream being read, which lives with the reading task;
/// this end holds only what the task has handed over.
pub struct Heard<O> {
    /// What the reading task hands over, one read at a time.
    arriving: Receiver<Arrived>,
    /// What the task rings as it hands something over, for a host waiting on
    /// its own thread.
    bell: Arc<Doorbell>,
    /// The reading task, which ends when this is dropped.
    _reading: Owned,
    /// How long a silence crucible sits through before giving up.
    patience: Duration,
    /// What can end the waiting before the patience does.
    abandon: Option<Cancel>,
    /// When this exchange runs out of time, whether or not the far end is
    /// saying anything.
    until: Option<Instant>,
    /// The most recent bytes, held until they have been consumed.
    held: Vec<u8>,
    /// How much of them has been.
    at: usize,
    /// Whether the stream has said it ended.
    ended: bool,
    /// The silence an awaiting host is sitting through, once one has begun,
    /// until something arrives or the next exchange is begun.
    silence: Option<Pin<Box<Sleep>>>,
    /// When an awaiting host next looks at its token, while it holds one.
    look: Option<Pin<Box<Sleep>>>,
    /// Which kind of stream the task reads.
    stream: PhantomData<fn() -> O>,
}

impl<O: SandboxOutput + 'static> Heard<O> {
    /// Reads what `output` says, by a task on `on`, giving up after
    /// `patience` of silence.
    ///
    /// The patience is spent on one silence and handed back whenever anything
    /// arrives. A budget for the whole conversation would end a program for
    /// having been useful for longer than crucible guessed it would be.
    #[must_use]
    pub fn new(output: O, patience: Duration, on: &Handle) -> Self {
        let (arrived, arriving) = mpsc::channel(QUEUED);
        let bell = Arc::new(Doorbell::default());
        Self {
            arriving,
            _reading: Owned::spawn(on, read(output, arrived, Ringing(Arc::clone(&bell)))),
            bell,
            patience,
            abandon: None,
            until: None,
            held: Vec::new(),
            at: 0,
            ended: false,
            silence: None,
            look: None,
            stream: PhantomData,
        }
    }
}

impl<O> Heard<O> {
    /// Waits a different silence out from here on.
    ///
    /// A conversation does not have one patience throughout: agreeing a
    /// protocol version is a handshake with a deadline of its own, and the
    /// requests after it are the peer doing work. A value fixed at construction
    /// would make the caller choose which of the two to be wrong about. A
    /// silence already being awaited is sat through to the patience it began
    /// under.
    pub const fn patient_for(&mut self, patience: Duration) {
        self.patience = patience;
    }

    /// Stops waiting the moment `abandon` is raised, as well as at the
    /// patience.
    ///
    /// Set around one exchange rather than for the life of the stream: a
    /// cancellation belongs to the call somebody interrupted, and a token left
    /// behind would end the next read for a press that was spent on the last
    /// one. `None` puts the reader back to answering only to its patience.
    ///
    /// Setting it marks an exchange's edge, so an awaited silence begun before
    /// it is over: the next awaited wait sits through a silence of its own.
    pub fn abandoned_when(&mut self, abandon: Option<Cancel>) {
        self.abandon = abandon;
        self.exchanged();
    }

    /// Stops waiting once `until` has passed, however busy the far end has
    /// been.
    ///
    /// The patience measures one silence and is handed back whenever anything
    /// arrives, which is the right measure for a slow peer and no measure at
    /// all for a peer that says a byte just short of it and then goes quiet
    /// again: that one is never silent for long enough to be given up on. A
    /// deadline counts the time rather than the gaps in it, so it is the
    /// ceiling the patience cannot be.
    ///
    /// Set around one exchange, like [`Self::abandoned_when`], and for the same
    /// reason: it is that exchange being given a length, not the stream. `None`
    /// puts the reader back to answering only to its patience. Setting it
    /// marks an exchange's edge as [`Self::abandoned_when`] does.
    pub fn bounded_until(&mut self, until: Option<Instant>) {
        self.until = until;
        self.exchanged();
    }

    /// Ends an awaited silence at an exchange's edge.
    ///
    /// An awaited wait given up on leaves its silence here, and nothing but
    /// the host can say whether the next wait carries on that exchange or
    /// begins another: a `select!` that lets go of a read and takes it up
    /// again is the same exchange however long it steps aside, and a new
    /// request is a new one however soon it follows. So the silence goes on
    /// until something arrives or the host marks the next exchange, which is
    /// what setting the exchange's token or deadline does.
    fn exchanged(&mut self) {
        self.silence = None;
        self.look = None;
    }

    /// Whether the caller asked to stop waiting.
    fn abandoned(&self) -> bool {
        self.abandon.as_ref().is_some_and(Cancel::requested)
    }

    /// Takes one answer of the stream, or the task's going without one, into
    /// what the reader holds.
    ///
    /// Leaving [`Self::held`] empty is how the end of the stream is said, which
    /// is what [`BufRead`] means by an empty fill.
    fn took(&mut self, arrived: Option<Arrived>) -> io::Result<()> {
        match arrived {
            Some(Arrived::Bytes(bytes)) => {
                // The deadline is asked here and the patience where the host
                // waits, because each answers the peer the other cannot. A
                // quiet peer is a silence and is reported as one; a peer that
                // keeps saying a byte is never quiet, and the only thing left
                // to measure it against is the time it has used.
                if self.until.is_some_and(|end| Instant::now() >= end) {
                    return Err(overdue());
                }
                self.held = bytes;
                self.at = 0;
                Ok(())
            }
            Some(Arrived::Lost(discarded)) => Err(lost(discarded)),
            Some(Arrived::Failed(problem)) => Err(problem),
            Some(Arrived::End) => {
                self.ended = true;
                Ok(())
            }
            None => Err(unread()),
        }
    }

    /// Waits on the host's thread until something arrives, the stream ends,
    /// or the patience is out.
    fn hear(&mut self) -> io::Result<()> {
        self.held.clear();
        self.at = 0;
        // A silence an awaited wait was sitting through is not this wait's.
        self.silence = None;
        if self.ended {
            return Ok(());
        }
        let began = Instant::now();
        loop {
            // Asked before the queue rather than after a quiet look, because
            // it does not depend on what the far end does next: a peer with a
            // byte always ready would otherwise never be asked about, and a
            // caller who asked to stop is not waiting to find out how long the
            // program was allowed to be quiet for.
            if self.abandoned() {
                return Err(abandoned());
            }
            // Read before the queue is, so a ring after the look is heard.
            let seen = self.bell.rung();
            match self.arriving.try_recv() {
                Ok(arrived) => return self.took(Some(arrived)),
                Err(TryRecvError::Disconnected) => return self.took(None),
                Err(TryRecvError::Empty) => {}
            }
            let waited = began.elapsed();
            if waited >= self.patience {
                return Err(silent(self.patience));
            }
            self.bell
                .wait(seen, self.patience.saturating_sub(waited).min(LOOK));
        }
    }

    /// Asks, for an awaiting host, whether something has arrived, the stream
    /// has ended, or the patience is out.
    ///
    /// The silence is measured from the first ask that found nothing, and
    /// kept until something arrives, the silence ends it, or the host marks
    /// the next exchange (see [`Self::abandoned_when`]): an await given up on
    /// and taken up again within the same exchange sits through the same
    /// silence, however long it was put down for.
    fn poll_hear(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.ended {
            return Poll::Ready(Ok(()));
        }
        if self.abandoned() {
            return Poll::Ready(Err(abandoned()));
        }
        if let Poll::Ready(arrived) = self.arriving.poll_recv(cx) {
            return Poll::Ready(self.took(arrived));
        }
        let patience = self.patience;
        let silence = self
            .silence
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(patience)));
        if silence.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(silent(patience)));
        }
        if self.abandon.is_some() {
            let look = self
                .look
                .get_or_insert_with(|| Box::pin(tokio::time::sleep(LOOK)));
            if look.as_mut().poll(cx).is_ready() {
                // Looked at again on the next poll, which this asks for.
                self.look = None;
                cx.waker().wake_by_ref();
            }
        }
        Poll::Pending
    }
}

/// Reads `output` and hands each answer over, until the stream ends, fails,
/// loses a boundary, or nobody is left to hand anything to.
///
/// A place in the queue is waited for before each read rather than after it,
/// so a task with nowhere to put a read leaves it in the pipe.
async fn read<O: SandboxOutput>(mut output: O, arrived: Sender<Arrived>, bell: Ringing) {
    let mut buffer = vec![0_u8; CHUNK];
    loop {
        // Refused only once the host has gone, and then there is nobody to
        // read for.
        let Ok(place) = arrived.reserve().await else {
            return;
        };
        let (said, last) = match output.read(&mut buffer).await {
            // Zero bytes is not an ending. A stream saying it has nothing is
            // the same thing whether it says so with a count or a word.
            Ok(SandboxRead::Bytes(0) | SandboxRead::Pending) => {
                drop(place);
                tokio::time::sleep(PAUSE).await;
                continue;
            }
            Ok(SandboxRead::Bytes(count)) => (
                Arrived::Bytes(buffer.get(..count).unwrap_or_default().to_vec()),
                false,
            ),
            // The retained prefix is dropped along with everything else. It is
            // bytes crucible could still read, but the conversation ends on
            // this error either way, so handing them up would only mean a
            // caller finding one more frame on the way out.
            Ok(SandboxRead::Limited { discarded, .. }) => (Arrived::Lost(discarded), true),
            Ok(SandboxRead::End) => (Arrived::End, true),
            Err(problem) => (Arrived::Failed(mistaken(problem)), true),
        };
        place.send(said);
        bell.ring();
        if last {
            return;
        }
        // A stream that always has more would otherwise be read for as long as
        // the queue had room without the task reaching a wait.
        tokio::task::consume_budget().await;
    }
}

impl<O> fmt::Debug for Heard<O> {
    /// Without the stream, which is a pipe with the reading task and has
    /// nothing to show.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Heard")
            .field("patience", &self.patience)
            .field("abandon", &self.abandon)
            .field("until", &self.until)
            .field("held", &self.held.len())
            .field("at", &self.at)
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

impl<O> Read for Heard<O> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let arrived = self.fill_buf()?;
        let taken = arrived.len().min(buffer.len());
        if let Some((into, from)) = buffer.get_mut(..taken).zip(arrived.get(..taken)) {
            into.copy_from_slice(from);
        }
        BufRead::consume(self, taken);
        Ok(taken)
    }
}

impl<O> BufRead for Heard<O> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.at >= self.held.len() {
            self.hear()?;
        }
        Ok(self.held.get(self.at..).unwrap_or_default())
    }

    fn consume(&mut self, amount: usize) {
        self.at = self.at.saturating_add(amount).min(self.held.len());
    }
}

/// The same reader, awaited.
///
/// The same answers as [`BufRead`], from the same queue: bytes, the stream's
/// end, a silence past the patience, a deadline passed, a press, lost bytes or
/// the stream's own failure.
///
/// # Cancel safety
///
/// Dropping a fill before it answers loses no bytes: what the task handed over
/// stays in the queue or in the reader, for the next fill. It keeps the
/// silence it was sitting through, too, until something arrives or the host
/// marks the next exchange by setting its token or its deadline
/// ([`Heard::abandoned_when`], [`Heard::bounded_until`]): a fill taken up again
/// within the same exchange, however long after, goes on sitting through the
/// same silence, and the first awaited fill of a new exchange begins its own,
/// as a wait on the host's own thread does.
///
/// # Panics
///
/// Where it has to wait and is polled on a runtime without a time driver,
/// which is what Tokio's timers do: the patience and the look at a token are
/// both measured on the runtime's clock.
impl<O> AsyncBufRead for Heard<O> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.get_mut();
        if this.at >= this.held.len() {
            this.held.clear();
            this.at = 0;
            let heard = ready!(this.poll_hear(cx));
            // A wait that answered, whichever way, is a silence over.
            this.silence = None;
            this.look = None;
            heard?;
        }
        Poll::Ready(Ok(this.held.get(this.at..).unwrap_or_default()))
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        BufRead::consume(self.get_mut(), amount);
    }
}

impl<O> AsyncRead for Heard<O> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let arrived = ready!(Pin::new(&mut *this).poll_fill_buf(cx))?;
        let taken = arrived.len().min(buffer.remaining());
        buffer.put_slice(arrived.get(..taken).unwrap_or_default());
        BufRead::consume(this, taken);
        Poll::Ready(Ok(()))
    }
}

/// A peer that has stopped saying anything at all.
fn silent(patience: Duration) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!("the confined program said nothing for {patience:?}"),
    )
}

/// A wait that ended because crucible was asked to stop, not because the peer
/// was too slow.
///
/// Its own kind, because the two endings mean opposite things about the far
/// end: a program that timed out is one nothing more should be asked of, and
/// an abandoned one is doing exactly what it was told and simply is not wanted
/// any more.
///
/// [`io::ErrorKind::Interrupted`] is the kind this reads as and the one it must
/// not use. That kind means *a signal arrived, try the call again*, and the
/// standard library's own readers act on it: [`BufRead::read_line`] and
/// everything built on it retry a fill that fails with it. A reader ending a
/// wait with it would be a reader whose caller immediately puts it back into
/// the same wait, forever. So the kind here is the one that says the near end
/// let go of the conversation, which is what happened.
/// An exchange that ran past the length it was given, whatever it spent that
/// length doing.
///
/// [`io::ErrorKind::TimedOut`], the same as a silence, because it is the same
/// news about the far end: crucible waited as long as it was going to and the
/// answer did not come. What it is not is the caller's doing, which is why it
/// is not the kind an interruption uses.
fn overdue() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "the confined program did not finish within the time the exchange was given",
    )
}

/// A backend's own failure, with the one kind this reader keeps for itself
/// taken off it.
///
/// [`abandoned`] is spelled [`io::ErrorKind::ConnectionAborted`], and
/// everything downstream reads that kind as crucible having let go: a backend
/// whose read failed that way would be handing a caller a sentence about a key
/// nobody pressed, and — where a call is being decided on — a half-done call
/// blamed on the reader rather than on the connection. What actually happened
/// is the far end going, which is what [`io::ErrorKind::BrokenPipe`] says here
/// already.
///
/// The words are kept: they are the operating system's account of it and this
/// only disagrees about which of the two ends stopped.
fn mistaken(problem: io::Error) -> io::Error {
    if problem.kind() != io::ErrorKind::ConnectionAborted {
        return problem;
    }
    io::Error::new(io::ErrorKind::BrokenPipe, problem.to_string())
}

fn abandoned() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "crucible was asked to stop waiting for the confined program",
    )
}

/// A reading task that went without saying the stream had ended.
///
/// Nothing ends it that way but the runtime it ran on going away, or the
/// task coming apart, and either way the stream is no longer being read:
/// [`io::ErrorKind::BrokenPipe`], the kind a far end that went already reads
/// as, because to the conversation it is the same news.
fn unread() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "crucible is no longer reading the confined program's output",
    )
}

/// A stream that lost bytes to the output ceiling, and a boundary with them.
fn lost(discarded: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "the confined program's output passed its ceiling, so {discarded} bytes \
             are missing from the middle of what it said"
        ),
    )
}

#[cfg(test)]
mod tests;
