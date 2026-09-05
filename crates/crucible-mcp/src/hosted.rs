//! One MCP server, spoken to over the process the sandbox started for it.
//!
//! [`Talking`] holds a conversation over any reader and any writer, and core
//! turns a confined process's streams into exactly those. What is left is
//! joining them to a process, and answering the one question neither can: a
//! server that has stopped saying anything is either thinking or gone, and the
//! only thing that tells them apart is the process itself.
//!
//! It takes a process rather than starting one. What to run, under what
//! confinement, with what environment and on whose authority is a lifecycle
//! with its own owner and its own failure modes; a session that reached into it
//! would be deciding sandbox policy on the way past. What arrives here is a
//! command that has already been through all of that, and the only thing this
//! asks of it is that crucible kept the writing end of its input — a server
//! crucible cannot speak to is not a server, it is a process.
//!
//! Nothing here connects on its own. A [`Hosted`] exists because something
//! above it decided to start one server, for one run, and no part of reading a
//! configuration file or registering an adapter reaches this module.

use std::fmt;
use std::io;
use std::time::{Duration, Instant};

use crucible_core::{
    Cancel, Finish, Heard, Muttered, Said, SandboxOutput, SandboxProcess, SandboxUsage,
    SandboxViolation,
};

use crate::calling::{Answered, Unanswered};
use crate::catalogue::{Greeting, Offered, Rebuffed};
use crate::talking::Talking;
use serde_json::Value;

/// An MCP server, hosted over a confined process.
pub struct Hosted {
    /// The process, kept only so it can be watched and stopped.
    process: Box<dyn SandboxProcess>,
    /// The conversation, which owns both pipes it runs over.
    talking: Talking<Heard<Box<dyn SandboxOutput>>, Said>,
    /// What it has said beside the conversation.
    muttered: Muttered,
    /// How long one exchange with it may take, whatever it fills the time with.
    patience: Duration,
}

impl Hosted {
    /// Speaks to `process`, giving up on one silence after `patience`.
    ///
    /// The patience is spent on a single quiet stretch in either direction and
    /// handed back whenever anything moves, so a slow server is slow rather
    /// than dead. Standard error is drained from here on, which is what keeps a
    /// talkative server from wedging in a write nobody is reading.
    ///
    /// # Errors
    ///
    /// [`Unstarted`] where the process has no pipe to speak over or none to
    /// listen to. Stopping the process is attempted before either is returned, and
    /// [`Unstarted::Unreaped`] preserves an unconfirmed stop: a peer
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
            .map_or_else(Muttered::silent, Muttered::draining);
        Ok(Self {
            process,
            talking: Talking::new(Heard::new(output, patience), Said::new(input, patience)),
            muttered,
            patience,
        })
    }

    /// Agrees a protocol version and finishes the handshake.
    ///
    /// `interrupt` ends the waiting the moment it is raised, on the same terms
    /// as [`Self::call`]. A start-up is where a caller most needs that: nothing
    /// has been asked of the far end yet, so a handshake given up on has left
    /// nothing behind to wonder about.
    ///
    /// # Errors
    ///
    /// [`Rebuffed`] where the conversation fails, crucible is asked to stop
    /// waiting, the server answers with no version, or it answers with one
    /// crucible does not speak.
    pub fn greet(&mut self, interrupt: Option<&Cancel>) -> Result<Greeting, Rebuffed> {
        self.during(interrupt, crate::catalogue::hello)
    }

    /// Waits a different silence out from here on, in both directions.
    ///
    /// A handshake and a request are not the same wait. Agreeing a version is
    /// a peer answering from a table; a call is a peer doing the work, and the
    /// number that is generous for one is a hang for the other. So the
    /// conversation starts under whichever the caller opened it with and is
    /// moved once the greeting is behind it.
    pub fn patient_for(&mut self, patience: Duration) {
        self.patience = patience;
        let (heard, said) = self.talking.streams_mut();
        heard.patient_for(patience);
        said.patient_for(patience);
    }

    /// Runs one exchange under a token that ends it at the patience however the
    /// far end fills the time, and at `interrupt` whenever that is raised.
    ///
    /// The patience the streams themselves hold is spent on a single quiet
    /// stretch and handed back whenever anything moves, which is the right
    /// measure for a slow server and no measure at all for a server that says
    /// one byte just short of it and then goes quiet again. That one is never
    /// silent for long enough to be given up on, and the wait it holds open is
    /// as long as it cares to make it. A deadline over the whole exchange is
    /// the ceiling the per-silence number cannot be: it counts the time, not
    /// the gaps in it.
    ///
    /// Both are put down again afterwards, because both belong to this
    /// exchange: a deadline left behind would end the next one early, and a
    /// press spent here would end it before it began.
    ///
    /// They stay two things rather than one deadline-carrying token, because
    /// what they mean is different. A press is the near end losing interest,
    /// and running out of time is the far end failing to answer; a caller that
    /// could not tell them apart would report one as the other.
    fn during<T, E>(
        &mut self,
        interrupt: Option<&Cancel>,
        work: impl FnOnce(&mut Talking<Heard<Box<dyn SandboxOutput>>, Said>) -> Result<T, E>,
    ) -> Result<T, E> {
        let (heard, _) = self.talking.streams_mut();
        heard.abandoned_when(interrupt.cloned());
        heard.bounded_until(Instant::now().checked_add(self.patience));
        let done = work(&mut self.talking);
        let (heard, _) = self.talking.streams_mut();
        heard.abandoned_when(None);
        heard.bounded_until(None);
        done
    }

    /// Reads every tool the server offers, under crucible's own bounds.
    ///
    /// # Errors
    ///
    /// `interrupt` ends the waiting the moment it is raised, on the same terms
    /// as [`Self::call`].
    ///
    /// # Errors
    ///
    /// [`Rebuffed`] where the conversation fails, crucible is asked to stop
    /// waiting, or the catalogue is past one of those bounds. A catalogue is
    /// refused whole rather than shortened.
    pub fn catalogue(
        &mut self,
        greeting: &Greeting,
        interrupt: Option<&Cancel>,
    ) -> Result<Vec<Offered>, Rebuffed> {
        self.during(interrupt, |talking| {
            crate::catalogue::tools(talking, greeting)
        })
    }

    /// Calls one tool the server offered, under crucible's own bounds.
    ///
    /// The tool has to be one this server's own catalogue carried, so there is
    /// no way to try a name on a server that never mentioned it.
    ///
    /// `interrupt` ends the waiting the moment it is raised, instead of at the
    /// request patience. It is taken per call and put down again afterwards,
    /// because a cancellation belongs to the call somebody interrupted: a token
    /// left on the stream would end the next call for a press spent on the
    /// last. What it cannot do is unask the question — the frame has gone, and
    /// [`Unanswered::outstanding`] is how the caller finds out that the tool
    /// may be running still.
    ///
    /// # Errors
    ///
    /// [`Unanswered`] where the conversation fails, crucible is asked to stop
    /// waiting, or the result is not the shape the protocol gives. A tool that
    /// ran and failed is not an error.
    pub fn call(
        &mut self,
        tool: &Offered,
        arguments: &Value,
        interrupt: Option<&Cancel>,
    ) -> Result<Answered, Unanswered> {
        self.during(interrupt, |talking| {
            crate::calling::call(talking, tool, arguments)
        })
    }

    /// What the server has written to standard error so far.
    #[must_use]
    pub const fn muttered(&self) -> &Muttered {
        &self.muttered
    }

    /// The first hard resource violation the sandbox saw, where there was one.
    ///
    /// The usual answer to why a server stopped saying anything. Nothing
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
    /// Crucible's end of the pipe closes first, which is how a server is told
    /// there is nothing further to wait for, and the grace is the chance to act
    /// on it.
    #[must_use]
    pub fn stop(mut self, grace: Duration) -> Ended {
        drop(self.talking);
        let finish = Finish::after(self.process.as_mut(), grace);
        Ended {
            // Asked after the process has finished, because the supervisor
            // records a violation at the moment it acts on one and this is the
            // last point anything can ask. It is also the only ending that
            // explains itself from nowhere else: a command killed for running
            // too long says nothing on the way out and leaves an empty standard
            // error behind.
            violation: self.process.violation(),
            finish,
            muttered: self.muttered,
        }
    }
}

impl fmt::Debug for Hosted {
    /// Without the process, which is a backend's handle and has nothing to show
    /// that its own inspection does not say better.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hosted")
            .field("inspection", self.process.inspection())
            .field("muttered", &self.muttered)
            .finish_non_exhaustive()
    }
}

/// Stops a process that will not be hosted, preserving both refusal and cleanup.
fn abandon(process: &mut Box<dyn SandboxProcess>, why: Unstarted) -> Unstarted {
    match process.stop() {
        Ok(()) => why,
        Err(cleanup) => Unstarted::Unreaped {
            cause: Box::new(why),
            cleanup,
        },
    }
}

/// Why a process could not host a server.
#[derive(Debug, thiserror::Error)]
pub enum Unstarted {
    /// Crucible did not keep the writing end of the process's input.
    #[error(
        "the MCP server was started without crucible keeping its input, so there \
         is no way to ask it anything"
    )]
    Unspeakable,

    /// The process's output was not there to read.
    #[error(
        "the MCP server was started without an output to read, so there is no \
         answer to wait for"
    )]
    Unheard,

    /// Hosting failed, and the backend could not confirm process-scope cleanup.
    ///
    /// Construction retains the original missing-pipe cause and the stop error;
    /// it emits one wrapper, never a chain of cleanup attempts.
    #[error("{cause}; process cleanup remains unconfirmed: {cleanup}")]
    Unreaped {
        /// Why the process could not be hosted.
        #[source]
        cause: Box<Self>,
        /// Why the backend could not confirm cleanup.
        cleanup: io::Error,
    },
}

/// Everything an ended server leaves behind.
#[derive(Debug)]
pub struct Ended {
    /// How the process finished.
    pub finish: Finish,
    /// The first hard resource violation the sandbox saw, where there was one.
    ///
    /// Why it finished that way, when the answer is that crucible's own
    /// confinement stopped it. Nothing else in here says so: the process was
    /// killed mid-sentence, so it wrote no complaint and its conversation just
    /// stopped.
    pub violation: Option<SandboxViolation>,
    /// What it said beside the conversation, which is usually why it ended.
    pub muttered: Muttered,
}

#[cfg(test)]
mod tests;
