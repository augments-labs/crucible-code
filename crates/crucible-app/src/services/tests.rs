//! The order a run's services are taken apart in, and what a run that asks
//! for nothing costs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::runtime::Handle;

use super::serving;
use crate::runtime::WORKERS;

/// Longer than a free runtime takes to run a task it was just handed, by
/// orders of magnitude, and short enough that a stop handed to a runtime that
/// has already shut down fails the test rather than stalling it.
const LANDING: Duration = Duration::from_secs(2);

/// Stands for a command left running once its stop is work the runtime owns:
/// dropped on the way out, it hands its stop to the runtime it was given and
/// waits, within a bound, for the stop to have run.
struct StopsOnDrop {
    runtime: OnceLock<Handle>,
    landed: Arc<AtomicBool>,
}

impl StopsOnDrop {
    /// One that has no runtime to hand its stop to until it is given one.
    fn unarmed(landed: &Arc<AtomicBool>) -> Self {
        Self {
            runtime: OnceLock::new(),
            landed: Arc::clone(landed),
        }
    }
}

impl Drop for StopsOnDrop {
    fn drop(&mut self) {
        let Some(runtime) = self.runtime.get() else {
            return;
        };
        let landed = Arc::clone(&self.landed);
        let (ran, heard) = mpsc::channel();
        let _stop = runtime.spawn(async move {
            landed.store(true, Ordering::Release);
            let _ = ran.send(());
        });
        let _ = heard.recv_timeout(LANDING);
    }
}

/// What `serving`'s own contract adds to a run's locals, which are dropped at
/// the end of the run whatever `serving` does: what the run was given to own.
/// A stop handed to the runtime by something moved into the run has to find
/// the runtime still running, which it does only if `serving` consumes the run
/// before shutting down. The command line's registry of commands left running
/// is held to the same order by the command line's own test.
#[test]
fn the_runtime_is_shut_down_only_after_what_the_run_was_given_has_been_dropped() {
    let moved = Arc::new(AtomicBool::new(false));
    let moved_in = StopsOnDrop::unarmed(&moved);

    let (asked, stopped) = serving(move |services| {
        let runtime = services.runtime().handle()?;
        let _ = moved_in.runtime.set(runtime);
        Ok::<_, crate::runtime::Unstarted>(())
    });

    assert!(asked.is_ok(), "the runtime could not be started: {asked:?}");
    assert_eq!(stopped, Ok(()));
    assert!(
        moved.load(Ordering::Acquire),
        "a stop handed over by something moved into the run reached a runtime already shut down"
    );
}

#[test]
fn a_run_that_asks_for_no_runtime_starts_none() {
    let (built, stopped) = serving(|services| services.runtime().is_built());

    assert_eq!(
        (built, stopped),
        (false, Ok(())),
        "a run that never asked for the runtime started one anyway"
    );
}

#[test]
fn every_ask_is_answered_by_the_one_runtime_with_the_workers_it_states() {
    let (asked, stopped) = serving(|services| {
        let first = services.runtime().handle()?;
        let second = services.runtime().handle()?;
        Ok::<_, crate::runtime::Unstarted>((
            first.id() == second.id(),
            first.metrics().num_workers(),
        ))
    });

    assert_eq!(
        asked.map_err(|unstarted| unstarted.to_string()),
        Ok((true, WORKERS)),
        "a second ask built a second runtime, or the runtime has another number of workers"
    );
    assert_eq!(stopped, Ok(()));
}

/// The worker tools hand their blocking work to is the run's, on the run's
/// runtime: not built until something asks for it, the same one to everything
/// that asks, and running its work on the blocking threads the runtime states.
#[test]
fn the_tool_worker_is_built_on_the_run_s_runtime_when_asked_for_and_is_one_worker() {
    let (asked, stopped) = serving(|services| {
        let built_before = services.runtime().is_built();
        let first = services.tool_worker()?;
        let second = services.tool_worker()?;
        let cancel = crucible_runtime::Cancel::new();
        let ran_on = services
            .runtime()
            .handle()?
            .block_on(first.run(&cancel, |_| {
                std::thread::current().name().map(str::to_owned)
            }));
        Ok::<_, crate::runtime::Unstarted>((built_before, std::ptr::eq(first, second), ran_on))
    });

    assert_eq!(
        asked.map_err(|unstarted| unstarted.to_string()),
        Ok((false, true, Ok(Some("crucible-runtime".to_owned())))),
        "the tool worker was built before it was asked for, a second ask built a second \
         worker, or its work ran somewhere other than the run's runtime"
    );
    assert_eq!(stopped, Ok(()));
}

#[test]
fn the_http_client_is_one_lazy_run_service() {
    let (asked, stopped) = serving(|services| {
        let built_before = services.http.get().is_some();
        let first = services.http();
        let second = services.http();
        (built_before, std::ptr::eq(first, second))
    });

    assert_eq!(
        asked,
        (false, true),
        "the HTTP client was built before it was asked for, or a second ask built another one"
    );
    assert_eq!(stopped, Ok(()));
}
