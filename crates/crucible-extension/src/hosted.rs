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
//! Everything that waits is awaited, on the runtime the host is already on,
//! over the tasks the transport keeps reading and writing the process's
//! streams. A silence begins when a turn first finds nothing to read, and
//! lasts until the extension says something or crucible sends it a request or
//! an answer, so a turn given up on and taken up again is still waiting
//! through the same one: stepping away does not buy a quiet extension more
//! time.
//!
//! Ending it is two separate facts, and both are handed back. How the process
//! finished says whether it went quietly or had to be stopped, and the calls
//! crucible was still waiting on say what it owes its own callers — those are
//! promises made before the extension went away, and dropping them on the floor
//! because the ending was untidy leaves somebody upstairs waiting forever.
//!
//! **A replaced extension is uncallable from the moment its replacement is
//! hosted.** A replacement is another process — restarted, or rebuilt by
//! whoever can write to where the extension lives — and nothing it says has
//! been heard yet. It may well have a call open under a number the one before
//! it also used, so a call the host holds is a [`Call`], stamped with the
//! generation it was made in — one no other process crucible has hosted
//! shares, whichever host started it — and a call of any generation but the
//! one being spoken to is refused with [`CallError::Elsewhere`](crate::CallError) rather
//! than settled by its number. The replaced process is ended after its
//! replacement is in place, and from that moment nothing but its ending
//! reaches it.

use std::fmt;
use std::io;
use std::time::Duration;

use crucible_runtime::Unready;
use crucible_sandbox::{SandboxOutput, SandboxProcess, SandboxUsage, SandboxViolation};
use crucible_transport::{Absent, Finish, Heard, Muttered, Pipes, Said, Unspoken};

use crate::calls::{Call, Generation};
use crate::{Asking, Outcome, Over, Speaking, Turn};
use serde_json::Value;
use tokio::runtime::Handle;

/// An extension, hosted over a confined process, one generation at a time.
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
    ///
    /// [`Unstarted::Spent`] where there is no generation left to host it as,
    /// and it is stopped then too.
    pub async fn over(
        process: Box<dyn SandboxProcess>,
        patience: Duration,
        runtime: &Handle,
    ) -> Result<Self, Unstarted> {
        Self::hosting(process, patience, runtime, Generation::fresh()).await
    }

    /// Speaks to `process` as `generation` of the extension, where there is
    /// one to speak to it as.
    async fn hosting(
        mut process: Box<dyn SandboxProcess>,
        patience: Duration,
        runtime: &Handle,
        generation: Option<Generation>,
    ) -> Result<Self, Unstarted> {
        let Some(generation) = generation else {
            // Stopped at once, as a process whose pipes could not be taken is:
            // it was never spoken to, so there is nothing it was told to
            // finish and no grace it could be using.
            let finish = Finish::after_async(process.as_mut(), Duration::ZERO).await;
            return Err(Unstarted::Spent { finish });
        };
        let pipes = Pipes::taken(process.as_mut(), patience, runtime)?;
        Ok(Self {
            process,
            talk: Speaking::new(pipes.heard, pipes.said, generation),
            muttered: pipes.muttered,
        })
    }

    /// Replaces the extension with `process`, a generation of its own, and
    /// ends the one it replaces, giving that `grace` to go quietly.
    ///
    /// `process` is spoken to as [`Self::over`] would, and what the replaced
    /// one leaves behind comes back as [`Self::stop`] hands it. From the moment
    /// `process` is hosted, every call the replaced one made or was asked is
    /// refused by [`Self::answer`] and [`Self::give_up`]; the ones crucible
    /// was still waiting on are among what comes back.
    ///
    /// The replaced process loses its conversation at that moment, not its
    /// sandbox. Until it finishes within `grace`, or is stopped after it, it
    /// can still do whatever its confinement allows, for as long as
    /// [`Finish::after_async`] takes to end it. A caller that does not trust
    /// it for that long passes a zero `grace`.
    ///
    /// # Errors
    ///
    /// [`Unstarted`] where `process` could not be hosted, as [`Self::over`]
    /// refuses one, and [`Unstarted::Spent`] where there is no generation left
    /// to host it as. Either way `process` is not hosted, its ending comes back
    /// with the refusal as that refusal says, and the extension it would have
    /// replaced is still the one being spoken to.
    ///
    /// # Cancel safety
    ///
    /// None. Dropped while the replaced extension is being ended, the
    /// replacement stands and the replaced one is left as a dropped
    /// [`Self::stop`] leaves it. Dropped while a refused replacement is being
    /// stopped, that stop is dropped with it.
    pub async fn replace(
        &mut self,
        process: Box<dyn SandboxProcess>,
        patience: Duration,
        runtime: &Handle,
        grace: Duration,
    ) -> Result<Ended<T>, Unstarted> {
        let replacement = Self::hosting(process, patience, runtime, Generation::fresh()).await?;
        let replaced = std::mem::replace(self, replacement);
        Ok(replaced.stop(grace).await)
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
    /// The extension's patience is spent on one silence, which ends when it
    /// says something or crucible sends it a request or an answer. A turn
    /// given up on and taken up again is still sitting through the same
    /// silence, so a host that steps away from a quiet extension and comes
    /// back still has it ended once the patience is out.
    ///
    /// # Errors
    ///
    /// [`Over`] once there will be nothing further. What crucible was still
    /// waiting on comes back from [`Hosted::stop`].
    ///
    /// # Cancel safety
    ///
    /// As [`Speaking::turn`]: dropped while it waits for the extension, it
    /// loses nothing, and the next turn carries on from whatever had arrived.
    /// Dropped while it sends a refusal of crucible's own, it leaves the
    /// conversation over, because part of that frame may already be on the
    /// wire.
    pub async fn turn(&mut self) -> Result<Turn<T>, Over> {
        self.talk.turn().await
    }

    /// Starts a call of crucible's own and sends it.
    ///
    /// A request sent begins an exchange: the wait for what the extension says
    /// next sits through a silence of its own, however long the one before it
    /// had run.
    ///
    /// # Errors
    ///
    /// [`Asking`] where crucible is already waiting on as many calls as it
    /// allows, or where the frame could not be sent.
    ///
    /// # Cancel safety
    ///
    /// None, as [`Speaking::ask`]: dropped partway, the call comes back from
    /// [`Hosted::stop`] and the conversation is over.
    pub async fn ask(
        &mut self,
        method: impl Into<Box<str>>,
        params: Value,
        about: T,
    ) -> Result<Call, Asking> {
        let call = self.talk.ask(method, params, about).await?;
        self.exchanged();
        Ok(call)
    }

    /// Answers a call the extension made and sends it.
    ///
    /// An answer sent begins an exchange, as a request does: the extension is
    /// the one that owes something next, and the wait for it sits through a
    /// silence of its own, however long crucible took to answer.
    ///
    /// # Errors
    ///
    /// [`Asking`] where that is not a call crucible took on — one made to a
    /// generation this host has replaced among them — or where the frame could
    /// not be sent.
    ///
    /// # Cancel safety
    ///
    /// None, as [`Speaking::answer`]: dropped partway, the conversation is
    /// over.
    pub async fn answer(&mut self, call: Call, outcome: Outcome) -> Result<(), Asking> {
        self.talk.answer(call, outcome).await?;
        self.exchanged();
        Ok(())
    }

    /// Marks where an exchange begins: crucible has sent something the
    /// extension must respond to, a request or an answer, so the wait for what
    /// it says next gets a whole patience of silence.
    ///
    /// Only a send marks one. A turn given up on leaves the silence it was
    /// sitting through with the reader, which cannot tell a turn taken up again
    /// from a wait for something new; setting the exchange's token is how it is
    /// told. This host holds no token over an exchange — dropping a turn is how
    /// one is given up on — so the one it sets is none.
    fn exchanged(&mut self) {
        self.talk.heard_mut().abandoned_when(None);
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
    /// [`Asking`] where that is not a call crucible is waiting on — one made to
    /// a generation this host has replaced among them — or where the
    /// conversation has already ended.
    pub fn give_up(&mut self, call: Call) -> Result<T, Asking> {
        self.talk.give_up(call)
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
    /// was still tidying up left whatever it was tidying half done. The wait
    /// is [`Finish::after_async`]'s, on the runtime this is awaited on.
    ///
    /// # Cancel safety
    ///
    /// None. Dropped before it answers, it stops nothing further, and what it
    /// would have handed back goes with it: the process's end is as
    /// unconfirmed as a stop that did not answer.
    #[must_use]
    pub async fn stop(mut self, grace: Duration) -> Ended<T> {
        // Collected before the conversation is dropped, because dropping it is
        // what makes these calls unanswerable and this is the last moment
        // anything knows they were outstanding.
        let waiting = self.talk.ended();
        drop(self.talk);
        let finish = Finish::after_async(self.process.as_mut(), grace).await;
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

    /// A process was offered after every generation this process can number
    /// had been handed out, so it was stopped rather than hosted.
    ///
    /// A generation's number is what refuses a call another process was
    /// asked, and a number handed out twice would refuse nothing.
    #[error("crucible has hosted as many extension processes as it can number")]
    Spent {
        /// How the process finished, which says whether stopping it was
        /// confirmed.
        finish: Finish,
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
    pub waiting: Vec<(Call, T)>,
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
