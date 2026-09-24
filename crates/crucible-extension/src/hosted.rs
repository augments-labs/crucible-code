//! One extension, spoken to over the process the sandbox started for it.
//!
//! Everything either side of this is already written. [`Speaking`] drives a
//! conversation over any reader and writer and knows every way one can end; the
//! transport turns a confined process's streams into that reader and that
//! writer. What is left is joining them to a process and answering the one
//! question neither can: when the talking stops, is the peer still there.
//!
//! It takes a process rather than starting one. Preparing a session,
//! materializing it and releasing a staged command is a lifecycle with its own
//! owner and its own failure modes, and a host that reached into it would be
//! deciding sandbox policy on the way past. What arrives here is a command that
//! has already been through all of that, and the only thing this asks of it is
//! that crucible kept the writing end of its input — a command built
//! [`spoken_to`](crucible_sandbox::SandboxCommand::spoken_to). A command that was
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
use std::time::Duration;

use crucible_runtime::Unready;
use crucible_sandbox::{SandboxOutput, SandboxProcess, SandboxUsage, SandboxViolation};
use crucible_transport::{Absent, Finish, Heard, Muttered, Pipes, Said, Unspoken};

use crate::{Asking, CallId, Outcome, Over, Speaking, Turn};
use serde_json::Value;
use tokio::runtime::Handle;

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
    /// Speaks to `process`, giving up on one silence after `patience`, with
    /// its streams read and written by tasks on `runtime`.
    ///
    /// The patience is spent on a single quiet stretch in either direction and
    /// handed back whenever anything moves, so a slow extension is slow rather
    /// than dead. Standard error is drained from here on, which is what keeps a
    /// talkative extension from wedging in a write nobody is reading. The tasks
    /// that read and write the conversation end when this is stopped or
    /// dropped; the drain goes on into what [`Self::stop`] hands back, until
    /// the stream ends or that is dropped too.
    ///
    /// # Errors
    ///
    /// [`Unstarted`] where the process has no pipe to speak over or none to
    /// listen to. Stopping the process is attempted before either is returned, and
    /// [`Unstarted::Unreaped`] preserves an unconfirmed stop, whether it failed
    /// or would have had to wait and was dropped: a peer crucible cannot hold a
    /// conversation with is one it has no way to end politely later.
    pub fn over(
        mut process: Box<dyn SandboxProcess>,
        patience: Duration,
        runtime: &Handle,
    ) -> Result<Self, Unstarted> {
        let pipes = Pipes::taken(process.as_mut(), patience, runtime)?;
        Ok(Self {
            process,
            talk: Speaking::new(pipes.heard, pipes.said),
            muttered: pipes.muttered,
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
        let finish = Finish::after(self.process.as_mut(), grace);
        Ended {
            // Asked after the process has finished, because the supervisor
            // records a violation at the moment it acts on one and this is the
            // last point anything can ask. It is also the only ending that
            // explains itself from nowhere else: a command killed for running
            // too long says nothing on the way out and leaves an empty
            // standard error behind.
            violation: self.process.violation(),
            finish,
            waiting,
            muttered: self.muttered,
        }
    }
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

    /// Hosting failed, and cleanup of the process scope is unconfirmed: the
    /// stop failed, or would have had to wait and was dropped.
    ///
    /// Construction retains the missing-pipe cause and the stop's error or its
    /// refusal; it emits one wrapper, never a chain of cleanup attempts. A
    /// refusal already says that what the stop began is unconfirmed, so the
    /// message gives it as it stands; a failed stop's words need not say it, so
    /// the message says it before them.
    #[error("{cause}; {}: {cleanup}", process_cleanup(.cleanup))]
    Unreaped {
        /// Why the process could not be hosted.
        #[source]
        cause: Box<Self>,
        /// Why the backend could not confirm cleanup. A stop that would have had
        /// to wait was dropped, and is as unconfirmed: this then holds the
        /// refusal, which `get_ref` finds.
        cleanup: io::Error,
    },
}

/// Everything an ended extension leaves behind.
#[derive(Debug)]
pub struct Ended<T> {
    /// How the process finished.
    pub finish: Finish,
    /// The first hard resource violation the sandbox saw, where there was one.
    ///
    /// Why it finished that way, when the answer is that crucible's own
    /// confinement stopped it. Nothing else in here says so: the process was
    /// killed mid-sentence, so it wrote no complaint and its conversation just
    /// stopped.
    pub violation: Option<SandboxViolation>,
    /// Calls crucible was still waiting on, which nothing will answer now.
    pub waiting: Vec<(CallId, T)>,
    /// What it said beside the conversation, which is usually why it ended.
    pub muttered: Muttered,
}

impl From<Unspoken> for Unstarted {
    /// Says which end was missing in the words an extension's user reads.
    ///
    /// The transport knows a pipe was not handed back; only here is it known
    /// that the thing on the other end was supposed to be an extension, which
    /// is the noun the sentence has to use.
    fn from(unspoken: Unspoken) -> Self {
        let cause = match unspoken.absent {
            Absent::Input => Self::Unspeakable,
            Absent::Output => Self::Unheard,
        };
        match unspoken.cleanup {
            None => cause,
            Some(cleanup) => Self::Unreaped {
                cause: Box::new(cause),
                cleanup,
            },
        }
    }
}

/// What leads in a stop's error in [`Unstarted::Unreaped`]'s message.
///
/// A stop dropped because it would have had to wait is the refusal the error
/// holds, which `get_ref` finds; the transport keeps it there as it stands.
/// One level is enough because this `cleanup` only ever reaches here from
/// `Unspoken::after`, which hands back the stop's own error and never the
/// wrapper a process stopped at its publication ceiling is given.
fn process_cleanup(cleanup: &io::Error) -> &'static str {
    if matches!(cleanup.get_ref(), Some(held) if held.is::<Unready>()) {
        "process cleanup"
    } else {
        "process cleanup remains unconfirmed"
    }
}

#[cfg(test)]
mod tests;
