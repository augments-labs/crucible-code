//! The streams one hosted program is spoken to and heard through, taken once.
//!
//! Every host in this workspace opens a conversation the same way: it takes the
//! writing end of the process's input, the reading end of its output, and
//! whatever it says beside both — and it refuses the process outright when
//! either of the first two is missing, because a peer crucible cannot speak to
//! or cannot hear is not a peer, it is a process.
//!
//! Refusing has a second half that is easy to leave out. A process that will
//! not be hosted is still running, so it is stopped here rather than dropped,
//! and a stop the backend could not confirm is carried back beside the refusal:
//! a conversation that never started is not a reason to forget a scope nothing
//! will ever reap.
//!
//! What it does not do is say any of that in words. Which end was missing is a
//! fact; whether the sentence about it names an extension or a server is the
//! host's to write, and a shared sentence would be wrong for one of them.

use std::io;
use std::time::Duration;

use crucible_runtime::Bridge;
use crucible_sandbox::{SandboxOutput, SandboxProcess};

use crate::{Heard, Muttered, Said};

/// The three streams a hosted program is talked to over.
#[derive(Debug)]
pub struct Pipes {
    /// What it has said, as something a frame reader can read.
    pub heard: Heard<Box<dyn SandboxOutput>>,
    /// What crucible says to it, as something a frame writer can write.
    pub said: Said,
    /// What it said beside the conversation, drained and bounded.
    pub muttered: Muttered,
}

impl Pipes {
    /// Takes `process`'s streams, giving up on one silence after `patience`.
    ///
    /// Standard error is drained from here on, which is what keeps a talkative
    /// program from wedging in a write nobody is reading. A process that has no
    /// standard error is given one that will never say anything rather than an
    /// absence every caller would have to spell out.
    ///
    /// # Errors
    ///
    /// [`Unspoken`] where the process has no pipe to speak over or none to
    /// listen to. The process is stopped before either is returned, and a stop
    /// that could not be confirmed comes back with it, whether it failed or
    /// would have had to wait and was dropped.
    pub fn taken(process: &mut dyn SandboxProcess, patience: Duration) -> Result<Self, Unspoken> {
        let Some(input) = process.take_stdin() else {
            return Err(Unspoken::after(process, Absent::Input));
        };
        let Some(output) = process.take_stdout() else {
            return Err(Unspoken::after(process, Absent::Output));
        };
        Ok(Self {
            heard: Heard::new(output, patience),
            said: Said::new(input, patience),
            muttered: process
                .take_stderr()
                .map_or_else(Muttered::silent, Muttered::draining),
        })
    }
}

/// Which end of a hosted program the sandbox did not hand back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Absent {
    /// Crucible did not keep the writing end of the process's input.
    Input,

    /// The process's output was not there to read.
    Output,
}

/// A process that could not be talked to, and what became of it.
#[derive(Debug)]
pub struct Unspoken {
    /// Which end was missing.
    pub absent: Absent,

    /// Why the backend could not confirm the stop, where it could not.
    ///
    /// A caller that reports only the missing pipe would retire a process scope
    /// nothing has confirmed the end of. A stop that would have had to wait was
    /// dropped, and is as unconfirmed: the error then holds the
    /// [`Unready`](crucible_runtime::Unready) it was refused with, which
    /// `get_ref` finds.
    pub cleanup: Option<io::Error>,
}

impl Unspoken {
    /// Stops `process`, which will not be hosted, and keeps both facts.
    fn after(process: &mut dyn SandboxProcess, absent: Absent) -> Self {
        Self {
            absent,
            // A stop that would have had to wait is as unconfirmed as one that
            // failed.
            cleanup: Bridge::TransportProcess
                .cross(process.stop())
                .unwrap_or_else(|unready| Err(io::Error::other(unready)))
                .err(),
        }
    }
}

#[cfg(test)]
mod tests;
