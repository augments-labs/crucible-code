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
//! other two are somebody's problem: an ending that went wrong, so that no
//! successful publication can be claimed, and not being able to stop it at all,
//! where the sandbox could not confirm scope termination and leader exit.
//!
//! A program that has ended is not stopped for the wait that follows. What it
//! wrote can wait its turn behind another command's publication for longer than
//! any grace, and stopping it then would cut short an ending that may still
//! publish. That wait has an end, because what it waits for can be held by a
//! crucible outside this process: past it the program is stopped; an ending
//! that completes during that stop is reported as [`Finish::Exited`], while an
//! ending that does not complete is reported as an unpublished or unconfirmed
//! result.
//!
//! The wait is [`Finish::after_async`]'s: on the caller's runtime, so a stop
//! that yields is waited for too, up to a bound, past which it is given up on
//! and the ending is cleanup nobody confirmed.

use std::io;
use std::process::ExitStatus;
use std::time::Duration;

use crucible_runtime::Unready;
use crucible_sandbox::{SandboxLifecycle, SandboxProcess};

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

/// How long an awaited stop is given to answer, in [`Finish::after_async`]
/// and in [`crate::Unspoken::after`]'s cleanup.
///
/// A stop that is working is waiting on the operating system to end a process
/// tree, which takes moments; one that has not answered in seconds is not
/// going to, and waiting on it further would keep a turn or a shutdown from
/// ending on its account. Longer than the publication ceiling, because giving
/// up here leaves an end nobody confirmed, where giving up there only loses
/// what the program wrote. Shorter under test, where nothing holds a stop up.
#[cfg(not(test))]
pub(crate) const STOPPING: Duration = Duration::from_secs(10);
// Short enough that a suite exercising an unanswering process stays fast;
// nothing in a test holds a stop up, so nothing needs the production bound.
#[cfg(test)]
pub(crate) const STOPPING: Duration = Duration::from_millis(50);

/// How a confined process finished.
#[derive(Debug)]
pub enum Finish {
    /// It ended within the grace it was given, or its already-writing ending
    /// completed during a stop, and that ending produced a status.
    Exited(ExitStatus),

    /// It did not, so its owned scope was stopped and its leader was reaped.
    Stopped,

    /// It ended, but its ending did not complete with a confirmed successful
    /// publication: most often because a root it wrote into changed while it
    /// ran. A quarantined ending can make that outcome uncertain rather than
    /// proving that nothing was published.
    Unpublished(io::Error),

    /// It did not, and stopping it failed, or was waited on and did not
    /// answer in time.
    ///
    /// The sandbox could not confirm scope termination and leader exit: one of
    /// the two endings, with an ending that went wrong, that are somebody's
    /// problem afterwards. It does not say the program is still running — a
    /// program stopped at its publication ceiling reaches this too, and then
    /// the error's message says what it lost, and its source is the stop that
    /// could not be confirmed. A stop that was waited on and did not answer in
    /// time is carried as the [`Unanswered`] it gave up on: `get_ref` on this
    /// error finds it, or, at the publication ceiling, `get_ref` on the
    /// `io::Error` that is its source, both of kind
    /// [`io::ErrorKind::TimedOut`].
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
        /// Whether the ending had already begun when the stop was requested.
        /// The joined stop may discard it or let its publication complete.
        unpublished: bool,
    },
}

impl Finish {
    /// Waits out `grace` for `process` to finish, and stops it where it does
    /// not, on the caller's runtime.
    ///
    /// What bounds the wait for a stop that yields to the runtime while it
    /// works is a fixed ceiling, ten seconds in production and shorter under
    /// test. One still unanswered then is given up on, and the ending is
    /// [`Finish::Unreaped`] with an error of kind [`io::ErrorKind::TimedOut`]:
    /// a failed cleanup, never an ordinary stop. A stop that does its work
    /// synchronously inside its first poll is not bounded by it: it runs to
    /// completion on the calling task before the timer is looked at, and its
    /// answer is taken however long it took.
    ///
    /// The waits for the process to finish are bounded by `grace` and the
    /// publication ceiling, provided `process`'s synchronous looks answer at
    /// once as its contract has them do; with a stop that yields, the whole
    /// finish answers within those and the stop's own ceiling together.
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
                    Unanswered::new(STOPPING),
                ))
            });
        Self::stopped(process, stop, unpublished)
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
    fn stopped(process: &mut dyn SandboxProcess, stop: io::Result<()>, unpublished: bool) -> Self {
        let publication = process.publication_outcome();
        match stop {
            // The ending was already being written when the stop arrived, and
            // its publication stood. Report the status that publication left
            // behind rather than the stale pre-stop outcome.
            Ok(()) if unpublished && publication == Some(SandboxLifecycle::Published) => {
                match process.try_wait() {
                    Ok(Some(status)) => Self::Exited(status),
                    Ok(None) => Self::Stopped,
                    Err(source) => Self::Unreaped(source),
                }
            }
            // It had ended, and the joined stop did not leave a confirmed
            // publication. Reported as a clean stop, that reads as though no
            // ending outcome was known.
            Ok(()) if unpublished => Self::Unpublished(unfinished()),
            Ok(()) => Self::Stopped,
            // Both facts: a caller told only that cleanup is unconfirmed reads
            // it as a process that may still be running, and retires it for
            // that, where what happened is that it ended and lost its writes.
            // The stop stays the error's source, recoverable as itself; the
            // lost publication is said in the message.
            Err(stop) if unpublished && publication != Some(SandboxLifecycle::Published) => {
                Self::Unreaped(io::Error::new(
                    stop.kind(),
                    Unconfirmed {
                        publication: unfinished(),
                        stop,
                    },
                ))
            }
            Err(source) => Self::Unreaped(source),
        }
    }
}

/// A process stopped at its publication ceiling whose stop was not confirmed.
///
/// Why its end is not known is its source, kept as the error it is, so that a
/// stop that never answered is still told apart from one
/// that failed. What the ceiling cost it is said in its message, and a caller
/// reaches it nowhere else. A stop that never answered, and a stop dropped
/// because it would have had to wait, already say that what they began is
/// unconfirmed, so the
/// message gives their words as they stand; no current stop path produces the
/// dropped one, and the arm stays for the refusal it names. A failed stop's words need not say
/// it, so the message says it before them.
#[derive(Debug, thiserror::Error)]
struct Unconfirmed {
    /// What it lost: no successful publication was confirmed.
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

/// A stop an awaited caller waited its whole bound on: [`Finish::after_async`]
/// stopping a program whose finish gave up on it, or an
/// [`Unspoken`](crate::Unspoken) refusal stopping one whose pipes could not
/// be taken.
///
/// Its words say that the stop's end is unconfirmed, which is what a caller
/// reading only this has to learn.
#[derive(Debug, thiserror::Error)]
#[error(
    "stopping a hosted program did not answer within {waited:?}, so whatever it began is \
     unconfirmed"
)]
pub struct Unanswered {
    /// How long it was waited on.
    waited: Duration,
}

impl Unanswered {
    /// Built for a stop that was waited on for `waited` and never answered.
    pub(crate) const fn new(waited: Duration) -> Self {
        Self { waited }
    }
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
