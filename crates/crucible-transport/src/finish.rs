//! How a confined process finished, and the wait that gives it the chance to.
//!
//! Every host that speaks to a confined process ends the same way and for the
//! same reason. Crucible's end of the pipe closes first, which is how the
//! program is told there is nothing further to wait for; then it is given a
//! grace to act on that, because a process killed while it was still tidying up
//! left whatever it was tidying half done; and only then is it stopped.
//!
//! Four endings rather than a success and a failure. Going quietly and being
//! stopped are both ordinary — one program exits on a closed pipe and another
//! waits to be told twice — and neither is anybody's problem afterwards. The
//! other two are somebody's problem: an ending that went wrong, so that nothing
//! the program wrote was published, and not being able to stop it at all, where
//! the sandbox could not confirm scope termination and leader exit.
//!
//! A program that has ended is not stopped for the wait that follows. What it
//! wrote can wait its turn behind another command's publication for longer than
//! any grace, and stopping it then would discard what it did exactly as asked.
//! That wait has an end, because what it waits for can be held by a crucible
//! outside this process: past it the program is stopped, and the ending says
//! that nothing it wrote was published rather than reading as a clean stop.
//!
//! The wait comes in two kinds, as the framing does. [`Finish::after`] holds
//! the calling thread, and can only ask a stop once and drop it if it would
//! have had to wait. [`Finish::after_async`] waits on the caller's runtime, so
//! it can wait for a stop too — a stop that yields, up to a bound, past which
//! it is given up on and the ending is cleanup nobody confirmed. The blocking
//! kind stays until the transport above it is asynchronous throughout.

use std::io;
use std::process::ExitStatus;
use std::thread;
use std::time::{Duration, Instant};

use crucible_runtime::{Bridge, Unready};
use crucible_sandbox::SandboxProcess;

/// How long the wait for a process to finish sleeps between looks.
const WATCH: Duration = Duration::from_millis(5);

/// How long a process that has ended is given, past its grace, to finish
/// publishing what it wrote.
///
/// Bounded because what it waits for can be held by another crucible of this
/// user, and a wait with no end would keep this run from finishing. Seconds
/// rather than minutes, because every caller of this is a disposal or a
/// restart, and a turn cannot end until each of its servers has. Shorter under
/// test, where nothing holds a publication up.
#[cfg(not(test))]
const PUBLICATION: Duration = Duration::from_secs(5);
// Several times above the scheduling delay a loaded CI runner adds to a sleep, which
// has been seen past 140ms, so a test timed against it measures the rule.
#[cfg(test)]
const PUBLICATION: Duration = Duration::from_millis(1500);

/// How long the asynchronous finish waits for a stop to answer.
///
/// A stop that is working is waiting on the operating system to end a process
/// tree, which takes moments; one that has not answered in seconds is not
/// going to, and waiting on it further would keep a turn or a shutdown from
/// ending on its account. Longer than the publication ceiling, because giving
/// up here leaves an end nobody confirmed, where giving up there only loses
/// what the program wrote.
const STOPPING: Duration = Duration::from_secs(10);

/// How a confined process finished.
#[derive(Debug)]
pub enum Finish {
    /// It ended on its own within the grace it was given, and its ending
    /// completed.
    Exited(ExitStatus),

    /// It did not, so its owned scope was stopped and its leader was reaped.
    Stopped,

    /// It ended, but its ending could not be completed, so nothing it wrote was
    /// published: most often because a root it wrote into changed while it ran.
    Unpublished(io::Error),

    /// It did not, and stopping it failed, would have had to wait and was
    /// dropped, or was waited on and did not answer in time.
    ///
    /// The sandbox could not confirm scope termination and leader exit: one of
    /// the two endings, with an ending that went wrong, that are somebody's
    /// problem afterwards. It does not say the program is still running — a
    /// program stopped at its publication ceiling reaches this too, and then
    /// the error's message says what it lost, and its source is the stop that
    /// could not be confirmed. A stop dropped because it would have had to wait
    /// is carried as the [`Unready`] it was refused with: `get_ref` on this
    /// error finds it, or, at the publication ceiling, `get_ref` on the
    /// `io::Error` that is its source. A stop that was waited on and did not
    /// answer in time is an error of kind [`io::ErrorKind::TimedOut`], found
    /// the same way at the publication ceiling.
    Unreaped(io::Error),
}

/// Where the wait for a process stands after one look at it.
enum Look {
    /// It finished, and this is how.
    Over(Finish),
    /// Look again after this long.
    Again(Duration),
    /// Stop it; and whether the wait ended at the publication ceiling rather
    /// than because the process would not go.
    Stop {
        /// What it wrote is discarded either way, but only one of the two is
        /// worth telling the caller about.
        unpublished: bool,
    },
}

impl Finish {
    /// Waits out `grace` for `process` to finish, and stops it where it does not.
    ///
    /// The caller closes its end of the conversation first; this is only the
    /// waiting and the stopping.
    #[must_use]
    pub fn after(process: &mut dyn SandboxProcess, grace: Duration) -> Self {
        let began = Instant::now();
        let unpublished = loop {
            match Self::look(process, grace, || began.elapsed()) {
                Look::Over(finish) => return finish,
                Look::Again(pause) => thread::sleep(pause),
                Look::Stop { unpublished } => break unpublished,
            }
        };
        // A stop that would have had to wait is dropped rather than waited
        // on, which leaves the process's end as unconfirmed as a stop that
        // failed.
        let stop = Bridge::TransportProcess
            .cross(process.stop())
            .unwrap_or_else(|unready| Err(io::Error::other(unready)));
        Self::stopped(stop, unpublished)
    }

    /// Waits out `grace` for `process` to finish, and stops it where it does
    /// not, on the caller's runtime.
    ///
    /// The same looks, the same publication ceiling and the same four endings
    /// as [`after`](Self::after), with the waiting done on the runtime's clock
    /// rather than by holding a thread. The stop differs: it is waited for
    /// rather than dropped for not answering at once.
    ///
    /// What ten seconds bound is the wait for a stop that yields to the
    /// runtime while it works. One still unanswered then is given up on, and
    /// the ending is [`Finish::Unreaped`] with an error of kind
    /// [`io::ErrorKind::TimedOut`]: a failed cleanup, never an ordinary stop.
    /// A stop that does its work synchronously inside its first poll is not
    /// bounded by them: it runs to completion on the calling task before the
    /// timer is looked at, and its answer is taken however long it took.
    ///
    /// The waits for the process to finish are bounded by `grace` and the
    /// publication ceiling, provided `process`'s synchronous looks answer at
    /// once as its contract has them do; with a stop that yields, the whole
    /// finish answers within those and the stop's ten seconds together.
    ///
    /// # Cancel safety
    ///
    /// Dropped before it answers, it waits no further and stops nothing
    /// further: a stop it had begun is dropped with it, which leaves the
    /// process's end as unconfirmed as a stop that did not answer.
    ///
    /// # Panics
    ///
    /// Panics if awaited on a runtime built without a time driver, which is
    /// what Tokio's timers do. A runtime that finishes a hosted program needs
    /// `enable_time`.
    pub async fn after_async(process: &mut dyn SandboxProcess, grace: Duration) -> Self {
        let began = tokio::time::Instant::now();
        let unpublished = loop {
            match Self::look(process, grace, || began.elapsed()) {
                Look::Over(finish) => return finish,
                Look::Again(pause) => tokio::time::sleep(pause).await,
                Look::Stop { unpublished } => break unpublished,
            }
        };
        let stop = tokio::time::timeout(STOPPING, process.stop())
            .await
            .unwrap_or_else(|_| {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    Unanswered { waited: STOPPING },
                ))
            });
        Self::stopped(stop, unpublished)
    }

    /// One look at `process`, `elapsed` into a wait of `grace`.
    ///
    /// Both kinds of wait take every decision here and only sleep in between,
    /// which is what keeps them waiting the same wait.
    fn look(
        process: &mut dyn SandboxProcess,
        grace: Duration,
        elapsed: impl Fn() -> Duration,
    ) -> Look {
        match process.try_wait() {
            Ok(Some(status)) => return Look::Over(Self::Exited(status)),
            // From a process that has ended, an error is how its ending went
            // wrong.
            Err(problem) if process.ended() => return Look::Over(Self::Unpublished(problem)),
            // From one still running it is not knowing, and the remedy for not
            // knowing is the same as for a process that will not go: stop it.
            Ok(None) | Err(_) => {}
        }
        match grace.checked_sub(elapsed()) {
            // A grace with nothing left of it has passed. Sleeping out the
            // nothing would be a look no clock has moved past, and on a clock
            // that moves only once nothing is ready, one repeated for ever.
            Some(left) if !left.is_zero() => Look::Again(left.min(WATCH)),
            // Past the grace, one that has ended is waiting its turn to
            // publish, not refusing to go — for as long as that wait can be
            // worth making.
            _ if process.ended() && elapsed() < grace.saturating_add(PUBLICATION) => {
                Look::Again(WATCH)
            }
            _ => Look::Stop {
                unpublished: process.ended(),
            },
        }
    }

    /// How a process that was stopped finished, from what its stop answered.
    fn stopped(stop: io::Result<()>, unpublished: bool) -> Self {
        match stop {
            // It had ended, and the stop discarded what it wrote. Reported as
            // a clean stop, that reads as though nothing was lost.
            Ok(()) if unpublished => Self::Unpublished(unfinished()),
            Ok(()) => Self::Stopped,
            // Both facts: a caller told only that cleanup is unconfirmed reads
            // it as a process that may still be running, and retires it for
            // that, where what happened is that it ended and lost its writes.
            // The stop stays the error's source, recoverable as itself; the
            // lost publication is said in the message.
            Err(stop) if unpublished => Self::Unreaped(io::Error::new(
                stop.kind(),
                Unconfirmed {
                    publication: unfinished(),
                    stop,
                },
            )),
            Err(source) => Self::Unreaped(source),
        }
    }
}

/// A process stopped at its publication ceiling whose stop was not confirmed.
///
/// Why its end is not known is its source, kept as the error it is, so that a
/// stop dropped because it would have had to wait is still told apart from one
/// that failed. What the ceiling cost it is said in its message, and a caller
/// reaches it nowhere else. A dropped stop's refusal, and a stop that did not
/// answer in time, already say that what the stop began is unconfirmed, so the
/// message gives their words as they stand; a failed stop's words need not say
/// it, so the message says it before them.
#[derive(Debug, thiserror::Error)]
struct Unconfirmed {
    /// What it lost: nothing it wrote was published.
    publication: io::Error,
    /// Why its end is not known.
    #[source]
    stop: io::Error,
}

impl std::fmt::Display for Unconfirmed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { publication, stop } = self;
        if matches!(stop.get_ref(), Some(held) if held.is::<Unready>() || held.is::<Unanswered>()) {
            write!(formatter, "{publication}, and {stop}")
        } else {
            write!(
                formatter,
                "{publication}, and stopping it could not be confirmed: {stop}"
            )
        }
    }
}

/// A stop the asynchronous finish waited its whole bound on.
///
/// Its words say that the stop's end is unconfirmed, as a dropped stop's
/// refusal does, because that is what a caller reading only this has to learn.
#[derive(Debug, thiserror::Error)]
#[error(
    "stopping a hosted program did not answer within {waited:?}, so whatever it began is \
     unconfirmed"
)]
struct Unanswered {
    /// How long it was waited on.
    waited: Duration,
}

/// What a process that had ended lost when it was stopped at its publication
/// ceiling.
fn unfinished() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "its publication did not finish in time",
    )
}

#[cfg(test)]
mod tests;
