//! Being told to stop from outside: a window that closed, or a `kill`.
//!
//! Neither arrives as a key. On Unix both are signals — a hang-up when the
//! terminal goes away, a termination when something asks the process to end —
//! and a process that installs nothing dies where it stands. For crucible that
//! is in the middle of a turn: the answer on screen is still with the worker,
//! the lines before it are still queued for the log's writer, and none of it
//! reaches the disk, so `--continue` picks up a session that never heard the
//! answer its reader watched arrive.
//!
//! So while a turn runs, the signal is *noted* rather than obeyed. The handler
//! stores one number and returns, which is all a signal context allows. The
//! drawing thread reads the number on its next pass — it wakes every tick
//! while a turn runs, so no thread is added and nothing else ever writes to
//! the terminal — and ends the turn the way a terminal that stopped taking
//! writes ends it: the turn is cancelled, the worker records what it had, the
//! loop unwinds, the log is drained, every terminal mode guard restores from
//! its `Drop`, and only then is the signal obeyed, by [`Told::obeyed`], so
//! whoever sent it sees the death it asked for.
//!
//! Everywhere else the signal does at once what it always did. Between turns
//! there is no answer in flight, and the prompt waits on the keyboard with no
//! clock, so nothing would read a note left there. The same is true inside a
//! turn wherever this thread waits on a key with no clock — a permission
//! question, a panel — and [`Ending::unclocked`] marks those stretches: a
//! signal that was only noted there would be a `kill` that did nothing until
//! somebody pressed a key. The wait for the log that ends a turn is one more:
//! its worker has handed over everything it held by then, and a log that had
//! stopped answering would otherwise hold the process against every `kill`.
//! Once one signal has been read, the next is obeyed at once too, so a turn
//! that will not stop cannot hold the process.
//!
//! The handler cannot see which stretch it is in without being told, so the
//! one flag that says is stored by this thread and read by the handler, either
//! side of the number being stored: whichever order the two threads interleave
//! in, the signal is either read by the loop or obeyed on the spot, never
//! neither.
//!
//! A hang-up is heard only where there is a terminal to lose. A run with none
//! is one somebody may have started under `nohup`, whose whole meaning is that
//! a hang-up is ignored, and installing a handler would undo that.
//!
//! Windows has no signal for a closing console. It delivers a control event to
//! a registered routine, on a thread of the system's own, and ends the process
//! when that routine returns or five seconds later: the routine would have to
//! block until this loop had unwound, which is a different seam from a note
//! read on the next pass, and registering it is a foreign call this workspace
//! allows only in the modules named for one. It is not handled; there the
//! process ends as it did, and [`Ending::listening`] hears nothing.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// What this process has been told from outside, and when it may act on it.
///
/// Cloned freely: every clone is the same two atomics.
#[derive(Debug, Clone)]
pub(crate) struct Ending {
    /// The signal that arrived, or zero. Written by the handler alone, apart
    /// from a test standing in for one.
    told: Arc<AtomicUsize>,
    /// Whether a signal is obeyed where it lands rather than noted.
    at_once: Arc<AtomicBool>,
    /// Whether a handler was installed at all. Where none was, a signal is
    /// the system's to act on and nothing here may suggest otherwise.
    listening: bool,
}

/// The signal a turn was ended by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Told(i32);

/// A stretch during which a signal is treated one way, ended by dropping it.
#[derive(Debug)]
#[must_use = "the stretch ends when this is dropped"]
pub(crate) struct Stretch<'a> {
    ending: &'a Ending,
    before: bool,
}

impl Ending {
    /// One that hears nothing, for a loop no signal is ever sent to.
    #[cfg(test)]
    pub(crate) fn deaf() -> Self {
        Self::unheard()
    }

    fn unheard() -> Self {
        Self {
            told: Arc::new(AtomicUsize::new(0)),
            at_once: Arc::new(AtomicBool::new(true)),
            listening: false,
        }
    }

    /// Starts hearing a termination, and a hang-up where `terminal` says there
    /// is a terminal to lose.
    ///
    /// A handler that cannot be installed leaves the signal doing what it did
    /// before, which is the worst this can come to: no signal is ever left
    /// with nothing to act on it.
    #[cfg(unix)]
    pub(crate) fn listening(terminal: bool) -> Self {
        use signal_hook::consts::{SIGHUP, SIGTERM};

        let mut ending = Self::unheard();
        let hang_up = if terminal { Some(SIGHUP) } else { None };
        ending.listening = [Some(SIGTERM), hang_up]
            .into_iter()
            .flatten()
            .all(|signal| ending.hears(signal));
        ending
    }

    /// Hears nothing: see the module documentation for what this platform
    /// offers instead and why it is not taken up.
    #[cfg(not(unix))]
    pub(crate) fn listening(_terminal: bool) -> Self {
        Self::unheard()
    }

    /// Installs the three things done when `signal` lands, in the order they
    /// are done: obey it if this is a stretch for that, note it, and look once
    /// more in case the stretch began in between.
    ///
    /// The first of them is installed first, so a failure part-way leaves a
    /// signal that is obeyed at once rather than one that is swallowed.
    #[cfg(unix)]
    fn hears(&self, signal: i32) -> bool {
        use signal_hook::flag::{register_conditional_default, register_usize};

        let Ok(number) = usize::try_from(signal) else {
            return false;
        };

        register_conditional_default(signal, Arc::clone(&self.at_once)).is_ok()
            && register_usize(signal, Arc::clone(&self.told), number).is_ok()
            && register_conditional_default(signal, Arc::clone(&self.at_once)).is_ok()
    }

    /// The stretch a turn runs for, in which a signal is noted for the loop.
    pub(crate) fn turn(&self) -> Stretch<'_> {
        self.stretch(false)
    }

    /// A stretch inside a turn where this thread waits on a key with no clock,
    /// so a signal has to be obeyed where it lands or not at all.
    ///
    /// # Errors
    ///
    /// [`Fatal::Ended`] where one was noted before the stretch began: nothing
    /// would read it during the wait, so the wait must not start.
    ///
    /// [`Fatal::Ended`]: super::Fatal::Ended
    pub(crate) fn unclocked(&self) -> Result<Stretch<'_>, super::Fatal> {
        let stretch = self.stretch(true);

        // After the flag is stored, never before. A signal landing now finds
        // the flag and is obeyed; one that landed earlier left its number, and
        // this is what reads it.
        match self.told() {
            Some(told) => Err(super::Fatal::Ended(told)),
            None => Ok(stretch),
        }
    }

    fn stretch(&self, at_once: bool) -> Stretch<'_> {
        let before = if self.listening {
            self.at_once.swap(at_once, Ordering::SeqCst)
        } else {
            true
        };

        Stretch {
            ending: self,
            before,
        }
    }

    /// The signal that has been noted, if one has.
    ///
    /// Reading it is what spends the patience: from here on another signal is
    /// obeyed where it lands.
    pub(crate) fn told(&self) -> Option<Told> {
        let signal = self.told.load(Ordering::SeqCst);
        if signal == 0 {
            return None;
        }

        self.at_once.store(true, Ordering::SeqCst);
        i32::try_from(signal).ok().map(Told)
    }

    /// Notes `signal` the way the handler does.
    #[cfg(test)]
    pub(crate) fn tell(&self, signal: i32) {
        self.told
            .store(usize::try_from(signal).unwrap_or(0), Ordering::SeqCst);
    }
}

impl Stretch<'_> {
    /// Ends the stretch a turn ran for, once its worker has been joined, and
    /// says how the turn ended: as `drawn`, or by the signal noted during it.
    ///
    /// The stretch ends first. What a signal was held back for is the answer
    /// the worker held, and the worker has handed everything it had to the
    /// log's writer by now; what is left is a wait with no clock that nothing
    /// reads a note during, so a signal is obeyed where it lands from here on.
    /// Held back through that wait, a log that had stopped answering would
    /// keep the process against every `kill`.
    ///
    /// `finish` waits until the session's log holds everything recorded so
    /// far, and may be asked twice. A turn that failed takes the session out
    /// with it, and the last thing its worker did was record what it had: the
    /// answer as far as it got, still in the writer's queue. Dropping the
    /// session joins the writer only if nothing else holds it, and the way
    /// this process is about to leave may not unwind far enough to find out,
    /// so a failed turn is finished here, whoever else holds the session.
    ///
    /// The note is read one last time after that: a signal that landed since
    /// the loop's last pass was held back for a loop that is no longer
    /// running, and the prompt this would otherwise return to reads no notes
    /// at all. One that is found outranks a failure, as it does in the loop:
    /// a window that closed fails the write and hangs up, and of the two it is
    /// the hang-up that says how the process should be seen to have ended.
    ///
    /// # Errors
    ///
    /// [`Fatal::Ended`] where a signal was noted, and otherwise whatever
    /// `drawn` failed with.
    ///
    /// [`Fatal::Ended`]: super::Fatal::Ended
    pub(crate) fn over(
        self,
        drawn: Result<(), super::Fatal>,
        finish: impl Fn(),
    ) -> Result<(), super::Fatal> {
        let ending = self.ending;
        drop(self);

        if drawn.is_err() {
            finish();
        }

        match ending.told() {
            Some(told) => {
                finish();
                Err(super::Fatal::Ended(told))
            }
            None => drawn,
        }
    }
}

impl Drop for Stretch<'_> {
    fn drop(&mut self) {
        if !self.ending.listening {
            return;
        }

        // What it was before, unless a signal has been noted since: then the
        // patience is spent whether or not anybody has read the note yet.
        let noted = self.ending.told.load(Ordering::SeqCst) != 0;
        self.ending
            .at_once
            .store(self.before || noted, Ordering::SeqCst);
    }
}

impl Told {
    /// Does what the signal asked, now that everything has been put away.
    ///
    /// The process ends by the signal itself rather than by an exit status
    /// chosen to look like it, because that is what a shell, a supervisor and
    /// `wait` all read: a process that was told to terminate and did. The
    /// status returned is for the case that cannot be relied on not to happen —
    /// the signal could not be raised — and is the one a shell would have
    /// reported.
    pub(crate) fn obeyed(self) -> ExitCode {
        #[cfg(unix)]
        let _ = signal_hook::low_level::emulate_default_handler(self.0);

        u8::try_from(self.0)
            .ok()
            .and_then(|signal| signal.checked_add(128))
            .map_or(ExitCode::FAILURE, ExitCode::from)
    }
}

#[cfg(test)]
mod tests;
