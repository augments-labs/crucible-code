//! Telling a runtime worker task apart from the thread that merely entered a
//! runtime to drive one.
//!
//! [`tokio::runtime::Handle::try_current`] cannot make this distinction: it
//! answers `Ok` both on a worker task and on the thread that called
//! [`Bridge::wait`](crate::Bridge::wait), which enters the runtime it waits on
//! without being spawned onto it. Only a spawned task has a
//! [`tokio::task::Id`], so [`not_worker`] asks for that instead.
//!
//! A step that must never run on a worker checks this itself, at the point in
//! its own body that would otherwise start the work: nothing here stops
//! anything by itself.

use std::fmt;

/// [`not_worker`] found the caller polled as a spawned task rather than on the
/// turn thread or a crossing's caller thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnWorker;

impl fmt::Display for OnWorker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "this step cannot run on a runtime worker task, only on the turn thread or a \
             crossing's caller thread"
        )
    }
}

impl std::error::Error for OnWorker {}

/// `Ok(())` everywhere but a spawned task, including a thread that entered a
/// runtime through [`Bridge::wait`](crate::Bridge::wait)'s
/// [`Handle::enter`](tokio::runtime::Handle::enter) without being spawned onto
/// it.
///
/// # Errors
///
/// [`OnWorker`] where the caller is currently polled as a spawned task — with
/// [`tokio::spawn`] or onto a [`Group`](crate::Group).
pub fn not_worker() -> Result<(), OnWorker> {
    if tokio::task::try_id().is_some() {
        Err(OnWorker)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{OnWorker, not_worker};

    #[test]
    fn a_plain_thread_is_not_a_worker() {
        assert_eq!(not_worker(), Ok(()));
    }

    #[test]
    fn a_thread_that_entered_a_runtime_without_being_spawned_onto_it_is_not_a_worker() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let _entered = runtime.enter();

        assert_eq!(
            not_worker(),
            Ok(()),
            "Handle::enter alone was mistaken for a spawned task"
        );
    }

    #[test]
    fn a_thread_blocked_on_a_future_without_being_spawned_is_not_a_worker() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let seen = runtime.block_on(async { not_worker() });

        assert_eq!(
            seen,
            Ok(()),
            "the thread that drives the runtime was mistaken for a spawned task"
        );
    }

    #[test]
    fn a_task_spawned_onto_a_runtime_is_a_worker() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let seen = runtime.block_on(async { tokio::spawn(async { not_worker() }).await.unwrap() });

        assert_eq!(seen, Err(OnWorker));
    }

    #[test]
    fn a_task_spawned_onto_a_multi_thread_runtimes_worker_is_a_worker() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .unwrap();

        let seen = runtime.block_on(async { tokio::spawn(async { not_worker() }).await.unwrap() });

        assert_eq!(seen, Err(OnWorker));
    }

    #[test]
    fn the_refusal_names_what_it_refuses() {
        assert_eq!(
            OnWorker.to_string(),
            "this step cannot run on a runtime worker task, only on the turn thread or a \
             crossing's caller thread"
        );
    }
}
