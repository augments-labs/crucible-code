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
//! awaited up to a bound, and a stop the backend could not confirm — including
//! one that never answered — is carried back beside the refusal: a
//! conversation that never started is not a reason to forget a scope nothing
//! will ever reap.
//!
//! What it does not do is say any of that in words. Which end was missing is a
//! fact; whether the sentence about it names an extension or a server is the
//! host's to write, and a shared sentence would be wrong for one of them.

use std::io;
use std::time::Duration;

use crucible_sandbox::{SandboxOutput, SandboxProcess};
use tokio::runtime::Handle;

use crate::finish::STOPPING;
use crate::{Heard, Muttered, Said, Unanswered};

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
    /// Each stream is handed to a task of its own on `on`: one reads the
    /// output, one writes the input, and one drains standard error, which is
    /// what keeps a talkative program from wedging in a write nobody is
    /// reading. Each task ends when the value holding it is dropped, or once
    /// its stream has ended or failed. A process that has no standard error is
    /// given one that will never say anything rather than an absence every
    /// caller would have to spell out.
    ///
    /// What comes back can be read and written by a host that awaits —
    /// [`Heard`] is an [`AsyncBufRead`](tokio::io::AsyncBufRead) and [`Said`]
    /// an [`AsyncWrite`](tokio::io::AsyncWrite) — and by one that waits on its
    /// own thread, since each is also a [`BufRead`](std::io::BufRead) or a
    /// [`Write`](std::io::Write) over the same task.
    ///
    /// The input is taken as the asynchronous writer
    /// [`SandboxProcess::take_async_stdin`] hands over, and the output and
    /// standard error are read through their waiting read, so the tasks need
    /// of `on` what the backend's streams need of the runtime polling them:
    /// the local backend's pipes on Unix need its I/O driver, every default
    /// waiting read its timer, and the default asynchronous input its blocking
    /// threads.
    ///
    /// # Errors
    ///
    /// [`Unspoken`] where the process has no pipe to speak over or none to
    /// listen to. The process is stopped before either is returned, awaited up
    /// to a bound, and a stop that could not be confirmed comes back with it,
    /// whether it failed or never answered.
    ///
    /// # Panics
    ///
    /// Panics if awaited on a runtime built without a time driver, which the
    /// bound on a stop that does not answer needs. That is the runtime this is
    /// awaited on, not `on`, which is reached only for the streams a process
    /// that could be hosted is talked over.
    pub async fn taken(
        process: &mut dyn SandboxProcess,
        patience: Duration,
        on: &Handle,
    ) -> Result<Self, Unspoken> {
        let Some(input) = process.take_async_stdin() else {
            return Err(Unspoken::after(process, Absent::Input).await);
        };
        let Some(output) = process.take_stdout() else {
            return Err(Unspoken::after(process, Absent::Output).await);
        };
        Ok(Self {
            heard: Heard::new(output, patience, on),
            said: Said::new(input, patience, on),
            muttered: process
                .take_stderr()
                .map_or_else(Muttered::silent, |stderr| Muttered::draining(stderr, on)),
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
    /// nothing has confirmed the end of. A stop that never answered is as
    /// unconfirmed as one that failed: the error then holds the [`Unanswered`]
    /// it gave up on, which `get_ref` finds.
    pub cleanup: Option<io::Error>,
}

impl Unspoken {
    /// Stops `process`, which will not be hosted, and keeps both facts.
    ///
    /// The stop is awaited up to [`STOPPING`]; one that has not answered by
    /// then is given up on, and is as unconfirmed as one that failed.
    async fn after(process: &mut dyn SandboxProcess, absent: Absent) -> Self {
        let cleanup = tokio::time::timeout(STOPPING, process.stop())
            .await
            .unwrap_or_else(|_| {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    Unanswered::new(STOPPING),
                ))
            })
            .err();
        Self { absent, cleanup }
    }
}

#[cfg(test)]
mod tests;
