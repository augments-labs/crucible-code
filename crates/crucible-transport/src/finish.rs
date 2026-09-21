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

    /// It did not, and stopping it failed, or would have had to wait and was
    /// dropped.
    ///
    /// The sandbox could not confirm scope termination and leader exit: one of
    /// the two endings, with an ending that went wrong, that are somebody's
    /// problem afterwards. It does not say the program is still running — a
    /// program stopped at its publication ceiling reaches this too, and then
    /// the error's message says what it lost, and its source is the stop that
    /// could not be confirmed. A stop dropped because it would have had to wait
    /// is carried as the [`Unready`] it was refused with: `get_ref` on this
    /// error finds it, or, at the publication ceiling, `get_ref` on the
    /// `io::Error` that is its source.
    Unreaped(io::Error),
}

impl Finish {
    /// Waits out `grace` for `process` to finish, and stops it where it does not.
    ///
    /// The caller closes its end of the conversation first; this is only the
    /// waiting and the stopping.
    #[must_use]
    pub fn after(process: &mut dyn SandboxProcess, grace: Duration) -> Self {
        let began = Instant::now();
        // Whether the wait ended at the publication ceiling rather than because
        // the process would not go. What it wrote is discarded either way, but
        // only one of the two is worth telling the caller about.
        let unpublished = loop {
            match process.try_wait() {
                Ok(Some(status)) => return Self::Exited(status),
                // From a process that has ended, an error is how its ending went
                // wrong.
                Err(problem) if process.ended() => return Self::Unpublished(problem),
                // From one still running it is not knowing, and the remedy for not
                // knowing is the same as for a process that will not go: stop it.
                Ok(None) | Err(_) => {}
            }
            match grace.checked_sub(began.elapsed()) {
                Some(left) => thread::sleep(left.min(WATCH)),
                // Past the grace, one that has ended is waiting its turn to
                // publish, not refusing to go — for as long as that wait can be
                // worth making.
                None if process.ended() && began.elapsed() < grace.saturating_add(PUBLICATION) => {
                    thread::sleep(WATCH);
                }
                None => break process.ended(),
            }
        };
        // A stop that would have had to wait is dropped rather than waited
        // on, which leaves the process's end as unconfirmed as a stop that
        // failed.
        match Bridge::TransportProcess
            .cross(process.stop())
            .unwrap_or_else(|unready| Err(io::Error::other(unready)))
        {
            // It had ended, and the stop below discarded what it wrote. Reported
            // as a clean stop, that reads as though nothing was lost.
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
/// reaches it nowhere else. A dropped stop's refusal already says that what the
/// stop began is unconfirmed, so the message gives the refusal as it stands; a
/// failed stop's words need not say it, so the message says it before them.
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
        if matches!(stop.get_ref(), Some(held) if held.is::<Unready>()) {
            write!(formatter, "{publication}, and {stop}")
        } else {
            write!(
                formatter,
                "{publication}, and stopping it could not be confirmed: {stop}"
            )
        }
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
