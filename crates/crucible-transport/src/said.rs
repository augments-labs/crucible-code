//! What crucible says to a confined process, as something a frame writer can
//! write.
//!
//! The mirror of [`Heard`](super::Heard), and it exists for the same reason
//! pointed the other way. A frame writer wants a [`Write`] or an
//! [`AsyncWrite`] whose failures are errors — this is both, over the same
//! writing task — and a pipe into a confined process is a writer whose failure
//! mode is
//! not an error at all: it stops taking bytes and the caller waits. A peer
//! that stopped reading and a peer that is thinking look identical from this
//! side, so crucible spends a patience on one frame and then says the peer is
//! gone — the same trade, and the same admission that nothing here can tell
//! them apart.
//!
//! The patience cannot be spent on the pipe directly. A write into a full pipe
//! is a wait with no deadline in it, so the writing is a task of its own on the
//! runtime the host hands over, and the patience is spent waiting for that task
//! to report back. A frame given up on is still in the task's hands, which is
//! why nothing further is said afterwards: those bytes may yet land, and a
//! later frame would arrive joined to the one crucible already called
//! undelivered. The task ends with this value, write or no write.
//!
//! Bytes are held until the frame is whole. A pipe takes whatever it is given,
//! so writing a frame in pieces puts a fragment in front of the far end that it
//! reads joined to whatever comes next; holding until the newline is what makes
//! a frame arrive as one thing or not at all.

use std::fmt;
use std::future::Future as _;
use std::io::{self, ErrorKind, Write};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use crucible_sandbox::SandboxInput;
use tokio::io::AsyncWrite;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{
    self, Receiver, Sender,
    error::{TryRecvError, TrySendError},
};
use tokio::time::Sleep;

use crate::FRAME_BYTES;
use crate::owned::{Doorbell, Owned, Ringing};

/// The most bytes one unfinished frame may hold before it is handed over.
///
/// A frame and the newline that ends it. A frame writer already refuses
/// anything longer before a byte is written; this is that same ceiling standing
/// where the bytes are actually retained, so a caller that never ends a frame
/// cannot grow this buffer instead.
const HELD: usize = FRAME_BYTES + 1;

/// What crucible says to a confined process, bounded in bytes and in patience.
pub struct Said {
    /// The frame being written, held until it is whole.
    held: Vec<u8>,
    /// Where a whole frame is handed to the task that owns the pipe.
    to: Sender<Vec<u8>>,
    /// What that task says once the bytes have gone.
    done: Receiver<io::Result<()>>,
    /// What the task rings as it says so, for a host waiting on its own
    /// thread.
    bell: Arc<Doorbell>,
    /// The task, which ends when this is dropped.
    _writing: Owned,
    /// How long crucible waits for one frame to be taken.
    patience: Duration,
    /// Whether a frame has been handed over whose answer has not been taken.
    handed: bool,
    /// The patience an awaited flush is spending on the frame handed over.
    waiting: Option<Pin<Box<Sleep>>>,
    /// Whether an ending has been reached already.
    stopped: bool,
}

impl Said {
    /// Says what crucible owes to `to`, giving up on one frame after
    /// `patience`.
    ///
    /// The pipe moves into a task on `on` that writes one frame at a time,
    /// and that task ends when this value is dropped — at once where it is
    /// waiting for the next frame, and at the pipe's next wait where it is
    /// parked in a write the peer is not taking. The pipe closes as the task
    /// lets go of it, which is how the far end is told nothing more is coming.
    #[must_use]
    pub fn new(to: Box<dyn SandboxInput>, patience: Duration, on: &Handle) -> Self {
        // One of each: a frame is handed over only once the one before it
        // has been answered, so nothing ever waits behind another.
        let (frames, waiting) = mpsc::channel::<Vec<u8>>(1);
        let (spoke, done) = mpsc::channel::<io::Result<()>>(1);
        let bell = Arc::new(Doorbell::default());
        Self {
            held: Vec::new(),
            to: frames,
            done,
            _writing: Owned::spawn(on, speak(to, waiting, spoke, Ringing(Arc::clone(&bell)))),
            bell,
            patience,
            handed: false,
            waiting: None,
            stopped: false,
        }
    }

    /// Waits a different time out for one frame from here on.
    ///
    /// The reading half's [`Heard::patient_for`] and this one move together:
    /// what a caller is setting is how patient one exchange is, and half an
    /// exchange is not a thing to be patient about on its own.
    ///
    /// [`Heard::patient_for`]: crate::Heard::patient_for
    pub const fn patient_for(&mut self, patience: Duration) {
        self.patience = patience;
    }

    /// Hands the whole frame over to the task that owns the pipe.
    fn hand_over(&mut self) -> io::Result<()> {
        let frame = std::mem::take(&mut self.held);
        match self.to.try_send(frame) {
            Ok(()) => {
                self.handed = true;
                Ok(())
            }
            Err(TrySendError::Closed(_)) => Err(closed()),
            // Every frame before this one was answered or given up on, and a
            // frame given up on stops the conversation, so the one place is
            // never taken. Were it, a frame would be sitting with a writer
            // that has not finished the one before it.
            Err(TrySendError::Full(_)) => Err(deaf(self.patience)),
        }
    }

    /// Waits on the host's thread for the pipe to have taken the frame handed
    /// over, for as long as the patience allows.
    fn heard_back(&mut self) -> io::Result<()> {
        let began = Instant::now();
        let answer = loop {
            // Read before the answer is looked for, so a ring after the look
            // is heard.
            let seen = self.bell.rung();
            match self.done.try_recv() {
                Ok(gone) => break gone,
                Err(TryRecvError::Disconnected) => break Err(closed()),
                Err(TryRecvError::Empty) => {}
            }
            let waited = began.elapsed();
            if waited >= self.patience {
                break Err(deaf(self.patience));
            }
            self.bell.wait(seen, self.patience.saturating_sub(waited));
        };
        self.handed = false;
        self.waiting = None;
        answer
    }

    /// Asks, for an awaiting host, whether the pipe has taken the frame handed
    /// over, and gives up once the patience is spent.
    ///
    /// The patience starts when the frame is handed over and is kept until the
    /// answer is taken, so a flush given up on and begun again waits out the
    /// same patience rather than a fresh one.
    fn poll_heard_back(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let answer = match self.done.poll_recv(cx) {
            Poll::Ready(Some(gone)) => gone,
            Poll::Ready(None) => Err(closed()),
            Poll::Pending => {
                let patience = self.patience;
                let waiting = self
                    .waiting
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(patience)));
                if waiting.as_mut().poll(cx).is_pending() {
                    return Poll::Pending;
                }
                Err(deaf(patience))
            }
        };
        self.handed = false;
        self.waiting = None;
        Poll::Ready(answer)
    }

    /// Holds `bytes` as part of the frame being written, refusing a frame that
    /// would grow past its ceiling.
    fn hold(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.stopped {
            return Err(over());
        }
        if self.held.len().saturating_add(bytes.len()) > HELD {
            // The part already held can never be finished now, and sending it
            // would put a fragment in front of the far end.
            self.stopped = true;
            return Err(unbounded());
        }
        self.held.extend_from_slice(bytes);
        Ok(bytes.len())
    }
}

/// Writes each frame handed over into `to`, and says how it went.
///
/// Ends once a write fails, since a pipe that has answered once answers the
/// same way forever, or once nobody is left to hand it frames or hear how
/// they went.
async fn speak(
    mut to: Box<dyn SandboxInput>,
    mut waiting: Receiver<Vec<u8>>,
    spoke: Sender<io::Result<()>>,
    bell: Ringing,
) {
    while let Some(frame) = waiting.recv().await {
        let gone = whole(to.as_mut(), &frame).await;
        let failed = gone.is_err();
        // There is always room: one frame is handed over at a time, and the
        // host takes its answer or gives up on the conversation before the
        // next. What fails is a host that has gone.
        let told = spoke.try_send(gone);
        bell.ring();
        if told.is_err() || failed {
            return;
        }
    }
}

/// Writes all of `bytes`, however many writes the pipe takes them in.
///
/// A write that took nothing is refused rather than asked again, and one
/// that was interrupted is asked again, as a blocking writer's `write_all`
/// does with both.
async fn whole(to: &mut dyn SandboxInput, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match to.write(bytes).await {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(taken) => bytes = bytes.get(taken..).unwrap_or_default(),
            Err(problem) if problem.kind() == ErrorKind::Interrupted => {}
            Err(problem) => return Err(problem),
        }
    }
    Ok(())
}

impl fmt::Debug for Said {
    /// Without the pipe, which is with the writing task and has nothing to
    /// show.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Said")
            .field("patience", &self.patience)
            .field("held", &self.held.len())
            .field("stopped", &self.stopped)
            .finish_non_exhaustive()
    }
}

impl Write for Said {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.hold(bytes)
    }

    /// Nothing reaches the pipe until here, which is what makes a frame arrive
    /// whole.
    ///
    /// A frame an awaited flush handed over and then gave up on is answered
    /// first, under a patience of its own here. If the pipe took it, what was
    /// written since is handed over and this flush answers for that; if not,
    /// what was written since is reported as never sent.
    fn flush(&mut self) -> io::Result<()> {
        loop {
            if self.stopped {
                return Err(over());
            }
            if self.handed {
                let earlier = self.heard_back();
                self.settled(earlier)?;
                continue;
            }
            if self.held.is_empty() {
                return Ok(());
            }
            self.hand_over().inspect_err(|_| {
                self.stopped = true;
            })?;
        }
    }
}

impl Said {
    /// What the answer to the frame handed over comes to for the flush that
    /// took it.
    ///
    /// Taken, it is nothing: the flush goes on to hand over whatever was
    /// written since, and answers for that. Not taken, the conversation is
    /// over, and the flush answers for what it was flushing: the frame handed
    /// over, when nothing has been written since, or else the frame written
    /// since, which was never handed over and is said to be unsent. An answer
    /// is always the answer of the frame it is reported for.
    fn settled(&mut self, answer: io::Result<()>) -> io::Result<()> {
        let Err(failure) = answer else {
            return Ok(());
        };
        self.stopped = true;
        if self.held.is_empty() {
            return Err(failure);
        }
        self.held = Vec::new();
        Err(unsent(&failure))
    }
}

/// The same writer, awaited.
///
/// The same answers as [`Write`], from the same task: a frame taken, a peer
/// that stopped reading once the patience is spent, a pipe that has gone, a
/// frame past its ceiling, and nothing further once any of those has ended the
/// conversation.
///
/// # Cancel safety
///
/// A write only holds bytes, and bytes a write accepted belong to the writer
/// from then on, whether or not the call that wrote them is still waiting:
/// the next flush hands over everything held, and its answer covers all of
/// it. So a send given up on before its flush handed the frame over is sent by
/// the next flush, together with whatever was written after it, under that
/// flush's answer.
///
/// A flush dropped after it handed its frame over leaves the frame with the
/// task, which goes on writing it. The next flush, awaited or not, takes that
/// frame's answer before anything else; if the pipe took it, that flush then
/// hands over whatever was written since and answers for that, and if the pipe
/// did not, what was written since is reported as never sent. So nothing is
/// sent twice, nothing written is reported taken before it has been, and an
/// awaited flush that resumes waits out what is left of the patience the frame
/// was handed over with, where a flush that waits on its own thread waits a
/// patience of its own.
///
/// # Panics
///
/// Where a flush has to wait and is polled on a runtime without a time
/// driver, which is what Tokio's timers do: the patience is measured on the
/// runtime's clock.
impl AsyncWrite for Said {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(self.get_mut().hold(bytes))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.stopped {
                return Poll::Ready(Err(over()));
            }
            if this.handed {
                let earlier = std::task::ready!(this.poll_heard_back(cx));
                if let Err(failure) = this.settled(earlier) {
                    return Poll::Ready(Err(failure));
                }
                continue;
            }
            if this.held.is_empty() {
                return Poll::Ready(Ok(()));
            }
            if let Err(failed) = this.hand_over() {
                this.stopped = true;
                return Poll::Ready(Err(failed));
            }
            this.waiting = Some(Box::pin(tokio::time::sleep(this.patience)));
        }
    }

    /// A flush: the pipe itself closes when this value is dropped.
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

/// A peer that has stopped taking what crucible says.
fn deaf(patience: Duration) -> io::Error {
    io::Error::new(
        ErrorKind::TimedOut,
        format!("the confined program stopped reading for {patience:?}"),
    )
}

/// A pipe with nobody left on the other end of it.
fn closed() -> io::Error {
    io::Error::new(
        ErrorKind::BrokenPipe,
        "the confined program's input is closed",
    )
}

/// A frame crucible never handed over, because the one handed over before it
/// was not taken.
///
/// [`ErrorKind::BrokenPipe`], because nothing of it reached the far end, which
/// is what that kind says here; the earlier frame's failure is kept in the
/// words.
fn unsent(earlier: &io::Error) -> io::Error {
    io::Error::new(
        ErrorKind::BrokenPipe,
        format!("this frame was not sent: the frame before it was not taken ({earlier})"),
    )
}

/// A conversation crucible has already stopped holding up its end of.
fn over() -> io::Error {
    io::Error::new(
        ErrorKind::BrokenPipe,
        "crucible already stopped speaking to this program",
    )
}

/// A frame that grew past what one frame is allowed to be.
fn unbounded() -> io::Error {
    io::Error::new(
        ErrorKind::InvalidInput,
        format!("crucible tried to say more than {HELD} bytes without ending a frame"),
    )
}

#[cfg(test)]
mod tests;
