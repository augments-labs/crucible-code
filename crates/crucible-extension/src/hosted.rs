//! One extension, spoken to over the process the sandbox started for it.
//!
//! Everything either side of this is already written. [`Speaking`] drives a
//! conversation over any reader and writer and knows every way one can end;
//! [`Heard`] and [`Said`] turn a confined process's streams into that reader and
//! that writer. What is left is joining them to a process and answering the one
//! question neither can: when the talking stops, is the peer still there.
//!
//! It takes a process rather than starting one. Preparing a session,
//! materializing it and releasing a staged command is a lifecycle with its own
//! owner and its own failure modes, and a host that reached into it would be
//! deciding sandbox policy on the way past. What arrives here is a command that
//! has already been through all of that, and the only thing this asks of it is
//! that crucible kept the writing end of its input — a command built
//! [`spoken_to`](crucible_core::SandboxCommand::spoken_to). A command that was
//! not is refused rather than half-hosted, because a conversation crucible
//! cannot answer is not one worth starting.
//!
//! Ending it is two separate facts, and both are handed back. How the process
//! finished says whether it went quietly or had to be stopped, and the calls
//! crucible was still waiting on say what it owes its own callers — those are
//! promises made before the extension went away, and dropping them on the floor
//! because the ending was untidy leaves somebody upstairs waiting forever.

use std::fmt;
use std::io;
use std::process::ExitStatus;
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    Asking, CallId, Outcome, Over, SandboxOutput, SandboxProcess, SandboxUsage, SandboxViolation,
    Speaking, Turn,
};
use serde_json::Value;

use crate::{Heard, Muttered, Said};

/// How long the wait for a process to finish sleeps between looks.
const WATCH: Duration = Duration::from_millis(5);

/// An extension, hosted over a confined process.
///
/// `T` is whatever the host wants remembered about a call it made; it comes
/// back with the answer, or with [`Ended::waiting`] when no answer ever will.
pub struct Hosted<T> {
    /// The process, kept only so it can be watched and stopped.
    process: Box<dyn SandboxProcess>,
    /// The conversation, which owns both pipes it runs over.
    talk: Speaking<Heard<Box<dyn SandboxOutput>>, Said, T>,
    /// What it has said beside the conversation.
    muttered: Muttered,
}

impl<T> Hosted<T> {
    /// Speaks to `process`, giving up on one silence after `patience`.
    ///
    /// The patience is spent on a single quiet stretch in either direction and
    /// handed back whenever anything moves, so a slow extension is slow rather
    /// than dead. Standard error is drained from here on, which is what keeps a
    /// talkative extension from wedging in a write nobody is reading.
    ///
    /// # Errors
    ///
    /// [`Unstarted`] where the process has no pipe to speak over or none to
    /// listen to. The process is stopped before either is returned: a peer
    /// crucible cannot hold a conversation with is one it has no way to end
    /// politely later.
    pub fn over(
        mut process: Box<dyn SandboxProcess>,
        patience: Duration,
    ) -> Result<Self, Unstarted> {
        let Some(input) = process.take_stdin() else {
            return Err(abandon(&mut process, Unstarted::Unspeakable));
        };
        let Some(output) = process.take_stdout() else {
            return Err(abandon(&mut process, Unstarted::Unheard));
        };
        let muttered = process
            .take_stderr()
            .map_or_else(|| Muttered::draining(Quiet), Muttered::draining);
        Ok(Self {
            process,
            talk: Speaking::new(Heard::new(output, patience), Said::new(input, patience)),
            muttered,
        })
    }
}

impl<T> fmt::Debug for Hosted<T> {
    /// Without the process, which is a backend's handle and has nothing to show
    /// that its own inspection does not say better.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hosted")
            .field("inspection", self.process.inspection())
            .field("muttered", &self.muttered)
            .finish_non_exhaustive()
    }
}

impl<T> Hosted<T> {
    /// The next thing the extension said that the host has to act on.
    ///
    /// # Errors
    ///
    /// [`Over`] once there will be nothing further. What crucible was still
    /// waiting on comes back from [`Hosted::stop`].
    pub fn turn(&mut self) -> Result<Turn<T>, Over> {
        self.talk.turn()
    }

    /// Starts a call of crucible's own and sends it.
    ///
    /// # Errors
    ///
    /// [`Asking`] where crucible is already waiting on as many calls as it
    /// allows, or where the frame could not be sent.
    pub fn ask(
        &mut self,
        method: impl Into<Box<str>>,
        params: Value,
        about: T,
    ) -> Result<CallId, Asking> {
        self.talk.ask(method, params, about)
    }

    /// Answers a call the extension made and sends it.
    ///
    /// # Errors
    ///
    /// [`Asking`] where that is not a call crucible took on, or where the frame
    /// could not be sent.
    pub fn answer(&mut self, id: CallId, outcome: Outcome) -> Result<(), Asking> {
        self.talk.answer(id, outcome)
    }

    /// Stops waiting on a call crucible made, handing back what it remembered.
    ///
    /// For a call whose answer stopped being wanted — the run it belonged to
    /// ended, or whoever asked went away — without ending the conversation and
    /// the extension with it. The extension is not told and may still answer;
    /// that answer is recognised and dropped.
    ///
    /// # Errors
    ///
    /// [`Asking`] where that is not a call crucible is waiting on, or where the
    /// conversation has already ended.
    pub fn give_up(&mut self, id: CallId) -> Result<T, Asking> {
        self.talk.give_up(id)
    }

    /// What the extension has written to standard error so far.
    #[must_use]
    pub fn muttered(&self) -> &Muttered {
        &self.muttered
    }

    /// The first hard resource violation the sandbox saw, where there was one.
    ///
    /// The usual answer to why an extension stopped saying anything. Nothing
    /// arrives over the conversation to explain it — the process was killed
    /// mid-sentence — so it has to be asked for.
    #[must_use]
    pub fn violation(&self) -> Option<SandboxViolation> {
        self.process.violation()
    }

    /// What the process has used so far, as the sandbox counts it.
    #[must_use]
    pub fn usage(&self) -> SandboxUsage {
        self.process.usage()
    }

    /// Ends the conversation and the process, giving `grace` to go quietly.
    ///
    /// Crucible's end of the pipe closes first, which is how an extension is
    /// told there is nothing further to wait for, and the grace is the chance
    /// to act on it. Only then is it stopped, because a process killed while it
    /// was still tidying up left whatever it was tidying half done.
    #[must_use]
    pub fn stop(mut self, grace: Duration) -> Ended<T> {
        // Collected before the conversation is dropped, because dropping it is
        // what makes these calls unanswerable and this is the last moment
        // anything knows they were outstanding.
        let waiting = self.talk.ended();
        drop(self.talk);
        Ended {
            finish: settle(self.process.as_mut(), grace),
            waiting,
            muttered: self.muttered,
        }
    }
}

/// Waits out `grace` for a process to finish, and stops it where it does not.
fn settle(process: &mut dyn SandboxProcess, grace: Duration) -> Finish {
    let began = Instant::now();
    loop {
        // An error here is not an ending, it is not knowing, and the remedy for
        // not knowing is the same as for a process that will not go: stop it.
        if let Ok(Some(status)) = process.try_wait() {
            return Finish::Exited(status);
        }
        let Some(left) = grace.checked_sub(began.elapsed()) else {
            break;
        };
        thread::sleep(left.min(WATCH));
    }
    match process.stop() {
        Ok(()) => Finish::Stopped,
        Err(source) => Finish::Unreaped(source),
    }
}

/// Stops a process that will not be hosted after all, keeping the first reason.
///
/// Whatever went wrong stopping it is the second thing that went wrong, and the
/// caller can do nothing about either; the one worth returning is the one that
/// says why there is no extension.
fn abandon(process: &mut Box<dyn SandboxProcess>, why: Unstarted) -> Unstarted {
    drop(process.stop());
    why
}

/// Why a process could not be hosted.
#[derive(Debug, thiserror::Error)]
pub enum Unstarted {
    /// Crucible did not keep the writing end of the process's input.
    #[error(
        "the extension was started without crucible keeping its input, so there is \
         no way to answer it"
    )]
    Unspeakable,

    /// The process's output was not there to read.
    #[error("the extension was started without an output to read, so there is nothing to host")]
    Unheard,
}

/// How a hosted extension finished.
#[derive(Debug)]
pub enum Finish {
    /// It ended on its own, within the grace it was given.
    Exited(ExitStatus),

    /// It did not, so it was stopped and its scope was reaped.
    Stopped,

    /// It did not, and stopping it failed.
    ///
    /// The sandbox could not confirm that everything the command owned is gone,
    /// which is the one ending that is somebody's problem afterwards.
    Unreaped(io::Error),
}

/// Everything an ended extension leaves behind.
#[derive(Debug)]
pub struct Ended<T> {
    /// How the process finished.
    pub finish: Finish,
    /// Calls crucible was still waiting on, which nothing will answer now.
    pub waiting: Vec<(CallId, T)>,
    /// What it said beside the conversation, which is usually why it ended.
    pub muttered: Muttered,
}

/// A stream that says nothing, for a process the sandbox gave no standard
/// error.
///
/// Nothing in this repository's own backends does that, and a host that carried
/// an `Option` through every use of it would be spelling out that possibility
/// everywhere in exchange for nothing.
struct Quiet;

impl SandboxOutput for Quiet {
    fn read_ready(&mut self, _buffer: &mut [u8]) -> io::Result<crucible_core::SandboxRead> {
        Ok(crucible_core::SandboxRead::End)
    }
}

#[cfg(test)]
mod tests;
