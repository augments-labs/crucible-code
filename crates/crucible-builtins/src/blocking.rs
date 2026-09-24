//! Where a tool's blocking work runs.
//!
//! A walk of a directory tree, a search of the files it reaches, and opening,
//! reading or replacing one file have no asynchronous form: done inside a
//! tool's run, they hold whichever thread polls the run for as long as the
//! disk takes. A call its caller lent a
//! [`ToolWorker`](crucible_tools::ToolWorker) hands that work to the worker
//! instead, where it takes one of the worker's bounded places and waits for
//! one when none is free. A call lent none does the same work on the thread
//! polling it, as every call did before a worker could be lent; it is the same
//! work either way, so the answer does not depend on where it ran.
//!
//! The work is handed a [`Cancel`]. On the worker that is the worker's token
//! for the job, raised when the call is cancelled and when the call is dropped
//! while the job runs; on the calling thread it is the call's own. A walk looks
//! at it between its bounded steps — each file it reaches and, for `grep`, each
//! read inside one — so a walk whose call has gone stops at its next step and
//! gives its place back. `read` looks at it between the pieces it reads a file
//! in, and `edit` between its reads and before its rename; `write` looks
//! before each directory it makes and before its rename, the steps whose
//! effect outlives the process, since nothing else stops a job already
//! started. `tool_search`'s work does not look at it: it is one short step over
//! a list held in memory, which ends as soon as it is started.
//!
//! A job cannot capture the call's context, which it may outlive, so it owns
//! everything it reads: for a walk, the workspace, the path it walks from and
//! the approval it asks about each file; for `read`, `write` and `edit`, the
//! workspace and the approval itself, whose arguments the job parses, and for
//! `write` the record of files already read; for
//! `tool_search`, the list of tools held back and the set it reveals them
//! into.

use crucible_runtime::Cancel;
use crucible_tools::{ToolContext, ToolError, Unrun};

/// Runs `job` on the worker `context` was lent, or on the calling thread where
/// it was lent none, and answers what the job answered.
///
/// `None` where the call was cancelled while it waited for room on the
/// worker, before the job started: nothing was done, and each caller answers
/// as that tool answers a call stopped before its first step — `glob` and
/// `grep` with their stopped answer and nothing in it, `tool_search`, `read`,
/// `write` and `edit` as cancelled.
///
/// # Errors
///
/// [`ToolError::Io`] naming `tool` where the worker stopped taking work before
/// the job started, because its runtime was shutting down.
///
/// # Panics
///
/// Where the job comes apart, wherever it ran: on the calling thread its panic
/// unwinds through here, and on the worker it is raised again here, so a
/// caller that contains a tool's panic records it the same way for both. What
/// the job came apart with stays on the worker, which does not carry it.
pub(crate) async fn run<T, F>(
    tool: &str,
    context: &ToolContext<'_>,
    job: F,
) -> Result<Option<T>, ToolError>
where
    F: FnOnce(&Cancel) -> T + Send + 'static,
    T: Send + 'static,
{
    let Some(worker) = context.worker() else {
        return Ok(Some(job(context.cancel())));
    };
    match worker.run(context.cancel(), job).await {
        Ok(answer) => Ok(Some(answer)),
        Err(Unrun::Cancelled) => Ok(None),
        Err(Unrun::Panicked) => std::panic::resume_unwind(Box::new(Unrun::Panicked.to_string())),
        Err(unrun @ Unrun::Stopped) => Err(ToolError::Io {
            tool: tool.into(),
            problem: unrun.to_string().into(),
            source: std::io::Error::other(unrun),
        }),
    }
}

#[cfg(test)]
mod tests;
