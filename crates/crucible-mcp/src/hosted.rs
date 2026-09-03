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
use std::time::Duration;

use crucible_core::{
    Finish, Heard, Muttered, Said, SandboxOutput, SandboxProcess, SandboxUsage, SandboxViolation,
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
            .map_or_else(Muttered::silent, Muttered::draining);
        Ok(Self {
            process,
            talking: Talking::new(Heard::new(output, patience), Said::new(input, patience)),
            muttered,
        })
    }

    /// Agrees a protocol version and finishes the handshake.
    ///
    /// # Errors
    ///
    /// [`Rebuffed`] where the conversation fails, the server answers with no
    /// version, or it answers with one crucible does not speak.
    pub fn greet(&mut self) -> Result<Greeting, Rebuffed> {
        crate::catalogue::hello(&mut self.talking)
    }

    /// Reads every tool the server offers, under crucible's own bounds.
    ///
    /// # Errors
    ///
    /// [`Rebuffed`] where the conversation fails or the catalogue is past one
    /// of those bounds. A catalogue is refused whole rather than shortened.
    pub fn catalogue(&mut self, greeting: &Greeting) -> Result<Vec<Offered>, Rebuffed> {
        crate::catalogue::tools(&mut self.talking, greeting)
    }

    /// Calls one tool the server offered, under crucible's own bounds.
    ///
    /// The tool has to be one this server's own catalogue carried, so there is
    /// no way to try a name on a server that never mentioned it.
    ///
    /// # Errors
    ///
    /// [`Unanswered`] where the conversation fails or the result is not the
    /// shape the protocol gives. A tool that ran and failed is not an error.
    pub fn call(&mut self, tool: &Offered, arguments: &Value) -> Result<Answered, Unanswered> {
        crate::calling::call(&mut self.talking, tool, arguments)
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

/// Stops a process that will not be hosted after all, keeping the first reason.
///
/// Whatever went wrong stopping it is the second thing that went wrong, and the
/// caller can do nothing about either; the one worth returning is the one that
/// says why there is no server.
fn abandon(process: &mut Box<dyn SandboxProcess>, why: Unstarted) -> Unstarted {
    drop(process.stop());
    why
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
