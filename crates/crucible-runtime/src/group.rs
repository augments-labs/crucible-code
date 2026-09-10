//! A set of tasks with one owner.
//!
//! The thing a group is for is the sentence "when this is over, every task the
//! owner started has an end in what it gets back" — an answer, or the reason
//! there is none. A task spawned and forgotten keeps a socket, a child process
//! or a lock alive past the turn that wanted it, and the only evidence is a
//! hang somewhere else much later. Every task here is held, and
//! [`Group::shutdown`] accounts for each of them.
//!
//! It accounts for them; it cannot promise they all stopped. Cancellation is
//! cooperative, because that is the only kind that leaves a half-written file
//! impossible — see [`Cancel`]. An abort is cooperative too, in a way that is
//! easy to miss: it takes the task at its next await point, so a task inside a
//! blocking read or a compute loop is not stopped by anyone, and waiting for
//! one to come back is waiting forever. So shutdown asks, then it aborts, then
//! it stops waiting, and a task that never came back is reported
//! [`Ended::Abandoned`] rather than quietly waited on.
//!
//! Admission is bounded and refused rather than queued: a caller told [`Full`]
//! can decide what to do about it, where a caller whose spawn silently waited
//! would have handed the bound over to nobody. There is no second refusal to
//! tell it apart from — [`Group::shutdown`] takes the group by value, so a
//! group that has been shut down is one nobody still holds to spawn into.
//!
//! The bound is on how many run at once, not on how much is remembered. Ends
//! pile up as tasks finish, so a group that outlives many of them wants
//! [`Group::ends`] as it goes; what is left is what shutdown returns.

use std::future::Future;
use std::time::Duration;

use tokio::task::JoinSet;

use crate::Cancel;

/// The group already holds every task it was bounded to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

impl std::fmt::Display for Full {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the group is already holding as many tasks as it admits")
    }
}

impl std::error::Error for Full {}

/// What a task came to.
#[derive(Clone, PartialEq, Eq)]
pub enum Ended<T> {
    /// It ran to the end, and this is what it answered.
    Done(T),
    /// It unwound, and this is what it came apart with, as far as the payload
    /// could be read. The panic is not resumed on the owner's thread: one task
    /// coming apart is not a reason for the shutdown reaping it to stop.
    ///
    /// A cleanup that unwinds while the task is being aborted arrives here
    /// too, carrying what the destructor came apart with: tokio catches it and
    /// reports the join as a panic rather than as the stop it was asked for.
    ///
    /// The message is carried so the owner can put it somewhere a reader will
    /// find it. It is not printed here, and nothing in this crate prints it.
    Panicked(String),
    /// It was aborted, and it stopped without its cleanup coming apart.
    Stopped,
    /// It was aborted, and it had not come back when the group gave up waiting.
    ///
    /// The owner is told this rather than blocked on it. Whether the task is
    /// still running is not something the group knows: a task that never
    /// reaches an await point cannot be taken at all, and one that would have
    /// come back reads the same way under a grace too short to reach it.
    Abandoned,
}

/// By hand, because [`Ended::Done`] carries whatever the task answered and
/// [`Ended::Panicked`] carries what it came apart with. A task's answer is a
/// command's output or a provider's reply, and a panic message is written by
/// whoever wrote the `panic!`; neither belongs in a `{:?}` that reaches a log.
/// Which end it was is the whole of what a reader of a `{:?}` needs.
impl<T> std::fmt::Debug for Ended<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Done(_) => f.write_str("Done(redacted)"),
            Self::Panicked(_) => f.write_str("Panicked(redacted)"),
            Self::Stopped => f.write_str("Stopped"),
            Self::Abandoned => f.write_str("Abandoned"),
        }
    }
}

/// Tasks owned as one thing.
///
/// Dropping a group aborts whatever it still holds and does not wait for the
/// aborts to land, so a task inside blocking work runs on until its next await
/// point — and any end already collected is discarded with the group. That is
/// the floor. [`Group::shutdown`] is the door, and it is the one that reports.
pub struct Group<T> {
    tasks: JoinSet<T>,
    ended: Vec<Ended<T>>,
    cancel: Cancel,
    limit: usize,
}

/// By hand, for the reason [`Ended`]'s is: the collected ends carry what the
/// tasks answered.
impl<T> std::fmt::Debug for Group<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Group")
            .field("running", &self.tasks.len())
            .field("ended", &self.ended.len())
            .field("limit", &self.limit)
            .field("cancel", &self.cancel)
            .finish()
    }
}

impl<T> Group<T> {
    /// The token this group's tasks are stopped by.
    ///
    /// A child of the one the group was made from, so raising it stops this
    /// group's tasks and nothing else, while a request on the parent still
    /// reaches every task here.
    #[must_use]
    pub fn cancel(&self) -> &Cancel {
        &self.cancel
    }

    /// How many tasks this group admits at once.
    #[must_use]
    pub fn limit(&self) -> usize {
        self.limit
    }
}

impl<T: Send + 'static> Group<T> {
    /// A group that holds at most `limit` tasks at once, stopped by a child of
    /// `cancel`.
    ///
    /// The child is made here rather than asked for, because [`Group::shutdown`]
    /// raises whatever token it was given: a group handed the run's own token
    /// would end the run on its way out. Taking a reference and narrowing it is
    /// what makes that unwritable.
    #[must_use]
    pub fn new(limit: usize, cancel: &Cancel) -> Self {
        Self {
            tasks: JoinSet::new(),
            ended: Vec::new(),
            cancel: cancel.child(),
            limit,
        }
    }

    /// Starts `work`, or says why it did not.
    ///
    /// # Errors
    ///
    /// [`Full`] where the group already holds `limit` tasks that have not
    /// finished. The refused future is dropped; a caller that wants to try
    /// again builds it again.
    ///
    /// # Panics
    ///
    /// Panics if called outside a Tokio runtime, which is what
    /// [`tokio::task::JoinSet::spawn`] does. A group is made and spawned into
    /// by code the runtime is already running; there is no fallible form of
    /// this to return instead.
    pub fn spawn<F>(&mut self, work: F) -> Result<(), Full>
    where
        F: Future<Output = T> + Send + 'static,
    {
        // Finished tasks are collected before the bound is read, so a group at
        // its limit whose tasks have all ended admits rather than refusing on
        // a count nobody has looked at since.
        self.collect_finished();
        if self.tasks.len() >= self.limit {
            return Err(Full);
        }

        self.tasks.spawn(work);
        Ok(())
    }

    /// How many tasks are running, finished ones excluded.
    ///
    /// Takes `&mut self` because excluding them means reaping them: a finished
    /// task stays in the set until it is joined, and a count that did not reap
    /// first would disagree with the one [`Group::spawn`] enforces the bound
    /// against. One fact, one answer.
    pub fn len(&mut self) -> usize {
        self.collect_finished();
        self.tasks.len()
    }

    /// Whether the group has anything still running.
    pub fn is_empty(&mut self) -> bool {
        self.len() == 0
    }

    /// Takes every end collected so far, and leaves the group running.
    ///
    /// Ends accumulate as tasks finish and are only otherwise handed over by
    /// [`Group::shutdown`], so a group that outlives many short tasks holds
    /// every answer they gave until it dies. Draining as they arrive is what
    /// bounds that, and it is also the only way to see a task come apart
    /// before the group closes.
    #[must_use]
    pub fn ends(&mut self) -> Vec<Ended<T>> {
        self.collect_finished();
        std::mem::take(&mut self.ended)
    }

    /// Stops admitting, asks every task to stop, and accounts for each one.
    ///
    /// Cooperative first: the token is raised and the group waits up to `grace`
    /// for tasks to notice and return, which is what lets a task finish the
    /// write it had started. Whatever is still running then is aborted and
    /// given a second `grace` to come back, reported [`Ended::Stopped`] if it
    /// does — a task the owner had to take away is a cleanup that did not go to
    /// plan, and it is visible in the answer rather than counted among the
    /// finished.
    ///
    /// What has not come back by then is reported [`Ended::Abandoned`] rather
    /// than waited on further.
    ///
    /// Both deadlines bound what this waits *for*, not how long the caller is
    /// here. It is a future, and it advances only while something polls it: a
    /// task inside synchronous work holds the worker it is on, so where no
    /// other worker is free this is not polled until that task returns of its
    /// own accord — and by then there is nothing left to give up on.
    ///
    /// # Panics
    ///
    /// Panics if the group is holding a task and the runtime this is awaited
    /// on was built without a time driver, which is what
    /// [`tokio::time::timeout_at`] does. A runtime that runs a group needs
    /// `enable_time`.
    #[must_use]
    pub async fn shutdown(mut self, grace: Duration) -> Vec<Ended<T>> {
        self.cancel.request();
        self.reap_until(deadline_in(grace)).await;

        if !self.tasks.is_empty() {
            self.tasks.abort_all();
            self.reap_until(deadline_in(grace)).await;

            // A task that finished while the deadline was passing is still in
            // the set until it is joined, and counting it here would report it
            // abandoned and throw away what it answered.
            self.collect_finished();

            // What is left was aborted and had not come back when the group
            // stopped waiting. Detached explicitly, so the last thing this does
            // to those tasks is named here rather than left to a drop.
            for _ in 0..self.tasks.len() {
                self.ended.push(Ended::Abandoned);
            }
            self.tasks.detach_all();
        }

        self.ended
    }

    /// Joins tasks into the collected ends until the set empties or `deadline`
    /// passes, whichever comes first.
    async fn reap_until(&mut self, deadline: tokio::time::Instant) {
        while !self.tasks.is_empty() {
            match tokio::time::timeout_at(deadline, self.tasks.join_next()).await {
                Ok(Some(joined)) => self.ended.push(ended(joined)),
                // `join_next` answers `None` only on an empty set, which the
                // loop's own condition already excludes; `Err` is the deadline.
                Ok(None) | Err(_) => break,
            }
        }
    }

    /// Moves every task that has already finished into the collected ends.
    fn collect_finished(&mut self) {
        while let Some(joined) = self.tasks.try_join_next() {
            self.ended.push(ended(joined));
        }
    }
}

/// A century. Not forever, but further off than any grace a shutdown means,
/// and near enough to now that the clock can still represent it.
const AS_GOOD_AS_FOREVER: Duration = Duration::from_hours(24 * 365 * 100);

/// `grace` from now, or as far from now as the clock can say.
///
/// A `Duration` that overflows the clock is not a grace anyone meant, and the
/// addition that would name it panics. Every step here is checked, because a
/// panic would take the caller's thread with it.
fn deadline_in(grace: Duration) -> tokio::time::Instant {
    let now = tokio::time::Instant::now();
    now.checked_add(grace)
        .or_else(|| now.checked_add(AS_GOOD_AS_FOREVER))
        .unwrap_or(now)
}

/// What a join answered, said in this crate's words.
fn ended<T>(joined: Result<T, tokio::task::JoinError>) -> Ended<T> {
    match joined {
        Ok(answer) => Ended::Done(answer),
        Err(join) if join.is_cancelled() => Ended::Stopped,
        // Everything else is the task coming apart. `JoinError` is opaque over
        // a private enum, so a tokio that grows a third way to fail arrives
        // here rather than being reported as a clean stop it was not.
        Err(join) => Ended::Panicked(came_apart_with(join)),
    }
}

/// The panic message, or the best account of the join that can be given.
fn came_apart_with(join: tokio::task::JoinError) -> String {
    match join.try_into_panic() {
        Ok(payload) => payload
            .downcast_ref::<&'static str>()
            .map(|said| (*said).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic payload that is not a string".to_owned()),
        Err(join) => join.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Ended, Group};
    use crate::Cancel;

    /// Long enough that a cooperative task returning inside it is a fact about
    /// the code rather than about how busy the machine is. The tests that
    /// spend it run on a paused clock, so it costs no wall time.
    const GRACE: std::time::Duration = std::time::Duration::from_millis(500);

    /// How long a cooperative task waits between checks of the token. Small
    /// against `GRACE`, and a real await point rather than a yield: a task that
    /// only yields keeps the runtime from ever going idle, and a paused clock
    /// only advances when it does — which would turn a broken shutdown into a
    /// hang instead of a failure.
    const TICK: std::time::Duration = std::time::Duration::from_millis(10);

    /// The most turns of the scheduler a test will give a task to reach a
    /// state, before deciding it never will. A bound rather than a fixed count
    /// of yields: the count that happens to work today is one scheduler change
    /// away from hanging the suite.
    const TURNS: usize = 1_000;

    /// A task that returns as soon as its group is shut down, and says so.
    async fn cooperative(cancel: Cancel) -> &'static str {
        loop {
            if cancel.requested() {
                return "noticed";
            }
            tokio::time::sleep(TICK).await;
        }
    }

    #[tokio::test]
    async fn a_full_group_refuses_rather_than_queueing() {
        let mut group = Group::new(1, &Cancel::new());

        assert!(group.spawn(cooperative(group.cancel().clone())).is_ok());
        assert_eq!(
            group.spawn(cooperative(group.cancel().clone())),
            Err(super::Full),
            "the second task was admitted past the limit of one"
        );
        assert_eq!(
            group.len(),
            1,
            "the refused task was started anyway and only the answer was withheld"
        );
        assert_eq!(group.limit(), 1, "the group misreported what it admits");
        assert_eq!(
            super::Full.to_string(),
            "the group is already holding as many tasks as it admits"
        );
    }

    #[tokio::test]
    async fn a_group_admits_again_without_being_asked_what_it_holds() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let done = Arc::new(AtomicBool::new(false));
        let said = Arc::clone(&done);
        let mut group = Group::new(1, &Cancel::new());

        assert!(
            group
                .spawn(async move {
                    said.store(true, Ordering::Release);
                    "first"
                })
                .is_ok()
        );

        // The task's own flag, never the group: asking the group whether it is
        // empty reaps for it, which is the very thing this is here to make the
        // next `spawn` do for itself.
        for _ in 0..TURNS {
            if done.load(Ordering::Acquire) {
                break;
            }
            tokio::task::yield_now().await;
        }
        tokio::task::yield_now().await;

        assert!(
            group.spawn(async { "second" }).is_ok(),
            "the group refused on a count nobody had looked at since"
        );
    }

    #[tokio::test]
    async fn a_group_admits_again_once_a_task_has_finished() {
        let mut group = Group::new(1, &Cancel::new());

        assert!(group.spawn(async { "first" }).is_ok());
        for _ in 0..TURNS {
            if group.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }

        assert!(
            group.spawn(async { "second" }).is_ok(),
            "the group refused although the task it was holding had finished"
        );
    }

    #[tokio::test]
    async fn a_task_that_finished_is_not_counted_as_running() {
        let mut group = Group::new(2, &Cancel::new());
        assert!(group.is_empty());

        assert!(group.spawn(async { "done" }).is_ok());
        assert_eq!(group.len(), 1);

        for _ in 0..TURNS {
            if group.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(group.len(), 0, "a finished task was still counted");
        assert!(group.is_empty());
    }

    #[tokio::test]
    async fn ends_hands_over_what_finished_and_leaves_the_group_running() {
        let mut group = Group::new(2, &Cancel::new());

        assert!(group.spawn(async { "early" }).is_ok());
        let mut taken = Vec::new();
        for _ in 0..TURNS {
            taken = group.ends();
            if !taken.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }

        assert_eq!(taken, vec![Ended::Done("early")]);
        assert!(
            group.ends().is_empty(),
            "the same end was handed over twice"
        );
        assert!(
            group.spawn(async { "later" }).is_ok(),
            "draining the ends left the group unusable"
        );
        assert_eq!(group.shutdown(GRACE).await, vec![Ended::Done("later")]);
    }

    #[tokio::test]
    async fn shutdown_hands_over_ends_collected_before_it_was_called() {
        let mut group = Group::new(2, &Cancel::new());
        assert!(group.spawn(async { "early" }).is_ok());

        // `is_empty` reaps without draining, so the end is held by the group rather
        // than by this test when shutdown is called.
        for _ in 0..TURNS {
            if group.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }

        assert_eq!(
            group.shutdown(GRACE).await,
            vec![Ended::Done("early")],
            "shutdown threw away what the group had already collected"
        );
    }

    #[tokio::test]
    async fn a_task_that_came_apart_before_shutdown_is_still_accounted_for() {
        let mut group: Group<()> = Group::new(2, &Cancel::new());
        assert!(group.spawn(async { panic!("the task came apart") }).is_ok());

        for _ in 0..TURNS {
            if group.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }

        match group.shutdown(GRACE).await.as_slice() {
            [Ended::Panicked(said)] => assert!(
                said.contains("the task came apart"),
                "the panic message was lost: {said}"
            ),
            other => panic!("a task that came apart was dropped from the account: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_panic_carrying_something_that_is_not_a_message_is_still_an_end() {
        let mut group: Group<()> = Group::new(1, &Cancel::new());
        assert!(group.spawn(async { std::panic::panic_any(42_u32) }).is_ok());

        match group.shutdown(GRACE).await.as_slice() {
            [Ended::Panicked(said)] => assert_eq!(
                said, "a panic payload that is not a string",
                "a panic nobody wrote a message for was reported as one that was"
            ),
            other => panic!("a task that came apart was dropped from the account: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_panic_message_built_at_runtime_is_carried_too() {
        let mut group: Group<()> = Group::new(1, &Cancel::new());
        let said = "the task came apart".to_owned();
        assert!(group.spawn(async move { panic!("{said}") }).is_ok());

        match group.shutdown(GRACE).await.as_slice() {
            [Ended::Panicked(said)] => assert!(
                said.contains("the task came apart"),
                "an owned panic message was reported as unreadable: {said}"
            ),
            other => panic!("expected one panicked task, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_panicking_task_is_reported_rather_than_lost() {
        let mut group = Group::new(1, &Cancel::new());

        assert!(group.spawn(async { panic!("the task came apart") }).is_ok());
        let ends: Vec<Ended<()>> = group.shutdown(GRACE).await;

        match ends.as_slice() {
            [Ended::Panicked(said)] => assert!(
                said.contains("the task came apart"),
                "the panic message was lost: {said}"
            ),
            other => panic!("expected one panicked task, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn what_a_task_answered_stays_out_of_the_debug_rendering() {
        let ends = vec![
            Ended::Done("sk-live-0123456789"),
            Ended::Panicked("sk-live-0123456789".to_owned()),
        ];

        let rendered = format!("{ends:?}");
        assert!(
            !rendered.contains("sk-live"),
            "the ends carried their payloads into a `{{:?}}`: {rendered}"
        );
        assert_eq!(rendered, "[Done(redacted), Panicked(redacted)]");
    }

    #[tokio::test]
    async fn a_group_shows_its_counts_without_showing_its_answers() {
        let mut group = Group::new(3, &Cancel::new());
        assert!(group.spawn(async { "sk-live-0123456789" }).is_ok());
        // `is_empty` reaps into the collected ends without draining them, so
        // the answer is still inside the group when it is rendered. Waiting on
        // `ends()` would take it out first and leave nothing to leak.
        for _ in 0..TURNS {
            if group.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }

        let rendered = format!("{group:?}");
        assert!(
            !rendered.contains("sk-live"),
            "the group carried a task's answer into a `{{:?}}`: {rendered}"
        );
        assert!(
            rendered.contains("ended: 1"),
            "the group rendered its ends as something other than a count: {rendered}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_cooperative_task_returns_inside_the_grace() {
        let mut group = Group::new(1, &Cancel::new());
        assert!(group.spawn(cooperative(group.cancel().clone())).is_ok());

        let began = tokio::time::Instant::now();
        let ends = group.shutdown(GRACE).await;

        assert_eq!(
            ends,
            vec![Ended::Done("noticed")],
            "the task was taken away rather than being allowed to return"
        );
        assert!(
            began.elapsed() < GRACE,
            "returning took the whole grace: {:?}",
            began.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn every_task_is_accounted_for_when_several_are_running() {
        let mut group = Group::new(3, &Cancel::new());
        for _ in 0..3 {
            assert!(group.spawn(cooperative(group.cancel().clone())).is_ok());
        }

        let ends = group.shutdown(GRACE).await;

        assert_eq!(
            ends,
            vec![
                Ended::Done("noticed"),
                Ended::Done("noticed"),
                Ended::Done("noticed")
            ],
            "the shutdown stopped accounting before every task was joined"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn no_task_survives_the_owner_shutting_down() {
        let held = Arc::new(());
        let mut group = Group::new(1, &Cancel::new());
        let kept = Arc::clone(&held);
        assert!(
            group
                .spawn(async move {
                    // Named so the task's future owns it; the count outside is
                    // what says whether that future is still alive.
                    let _ = &kept;
                    loop {
                        tokio::time::sleep(TICK).await;
                    }
                })
                .is_ok()
        );

        let began = tokio::time::Instant::now();
        let ends = group.shutdown(GRACE).await;

        assert_eq!(
            ends,
            vec![Ended::Stopped],
            "a task that ignored the request was not reported as taken away"
        );
        assert!(
            began.elapsed() >= GRACE,
            "the grace was cut short: {:?}",
            began.elapsed()
        );
        assert_eq!(
            Arc::strong_count(&held),
            1,
            "the task was still holding what it borrowed after shutdown returned"
        );
    }

    /// The multi-threaded flavor is what makes this a test of the shutdown
    /// rather than of the runtime: a task doing synchronous work occupies a
    /// worker, and the shutdown reaping it has to be on another one to get an
    /// answer at all.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_task_that_never_yields_is_given_up_on_rather_than_waited_for() {
        /// Long enough that no scheduling delay can make the grace outlast it.
        const BLOCKED_FOR: std::time::Duration = std::time::Duration::from_millis(500);
        /// Short enough that twice it is far inside `BLOCKED_FOR`.
        const BRIEF: std::time::Duration = std::time::Duration::from_millis(20);

        let mut group = Group::new(1, &Cancel::new());
        assert!(
            group
                .spawn(async {
                    std::thread::sleep(BLOCKED_FOR);
                    "finished anyway"
                })
                .is_ok()
        );

        let began = std::time::Instant::now();
        let ends = group.shutdown(BRIEF).await;
        let took = began.elapsed();

        assert_eq!(
            ends,
            vec![Ended::Abandoned],
            "a task nothing can stop was reported as though it had ended"
        );
        // Measured against the task's own duration rather than a multiple of
        // the grace. One of the two workers is inside the blocking sleep, so
        // the two deadlines and the timer that fires them all share the other
        // one, and how long that scheduling takes is a fact about the machine
        // rather than about this code. What is not is whether the shutdown
        // outlived the task it could not stop, and that is the whole claim.
        assert!(
            took < BLOCKED_FOR,
            "the shutdown waited for a task no abort can reach: {took:?}"
        );
    }

    /// Three workers so the shutdown reaping the tasks is not the thing being
    /// starved, and a limit above the number spawned so admission is not
    /// either. Both tasks are inside synchronous work no abort can reach.
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn every_task_no_abort_could_reach_is_reported_rather_than_one_of_them() {
        /// Longer than any grace this test spends, so neither task can finish
        /// inside the shutdown and turn this into a test of something else.
        const BLOCKED_FOR: std::time::Duration = std::time::Duration::from_millis(500);
        const BRIEF: std::time::Duration = std::time::Duration::from_millis(20);

        let mut group = Group::new(4, &Cancel::new());
        for _ in 0..2 {
            assert!(
                group
                    .spawn(async {
                        std::thread::sleep(BLOCKED_FOR);
                        "finished anyway"
                    })
                    .is_ok()
            );
        }

        assert_eq!(
            group.shutdown(BRIEF).await,
            vec![Ended::Abandoned, Ended::Abandoned],
            "the account named fewer tasks than the group was still holding"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_cleanup_that_came_apart_is_reported_as_coming_apart() {
        /// Unwinds when the task's future is dropped, which for an aborted
        /// task is the abort itself doing the dropping.
        struct ComesApartOnTheWayOut;

        impl Drop for ComesApartOnTheWayOut {
            fn drop(&mut self) {
                panic!("the cleanup came apart");
            }
        }

        let mut group = Group::new(1, &Cancel::new());
        assert!(
            group
                .spawn(async {
                    let _cleanup = ComesApartOnTheWayOut;
                    loop {
                        tokio::time::sleep(TICK).await;
                    }
                })
                .is_ok()
        );

        match group.shutdown(GRACE).await.as_slice() {
            [Ended::Panicked(said)] => assert!(
                said.contains("the cleanup came apart"),
                "the destructor's message was lost: {said}"
            ),
            other => panic!("a cleanup that came apart was reported as a clean stop: {other:?}"),
        }
    }

    #[test]
    fn a_grace_the_clock_cannot_name_becomes_one_it_can() {
        let now = tokio::time::Instant::now();

        // The distance, not the ordering: `deadline_in` samples its own `now`
        // after this one, so any answer at all is no earlier than this `now`
        // and an ordering assertion would hold for a function that gave up
        // entirely.
        assert!(
            super::deadline_in(std::time::Duration::MAX).saturating_duration_since(now)
                > std::time::Duration::from_hours(24 * 365),
            "a grace too large to add to the clock became no grace at all"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_grace_too_short_to_reach_a_task_reports_it_the_same_as_one_nothing_could_reach() {
        let mut group = Group::new(1, &Cancel::new());

        assert!(group.spawn(cooperative(group.cancel().clone())).is_ok());
        assert_eq!(
            group.shutdown(std::time::Duration::ZERO).await,
            vec![Ended::Abandoned],
            "a cooperative task under a zero grace was reported as something \
             other than not having come back"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_grace_the_clock_cannot_name_still_lets_a_task_return() {
        let mut group = Group::new(1, &Cancel::new());

        assert!(group.spawn(cooperative(group.cancel().clone())).is_ok());
        assert_eq!(
            group.shutdown(std::time::Duration::MAX).await,
            vec![Ended::Done("noticed")],
            "a grace too large to name left no time for a task to return in"
        );
    }

    #[tokio::test]
    async fn a_group_dropped_on_an_error_path_leaves_nothing_running() {
        let held = Arc::new(());
        let run = Cancel::new();
        let kept = Arc::clone(&held);

        {
            let mut group = Group::new(1, &run);
            assert!(
                group
                    .spawn(async move {
                        // Named so the task's future owns it; the count outside
                        // is what says whether that future is still alive.
                        let _ = &kept;
                        loop {
                            tokio::task::yield_now().await;
                        }
                    })
                    .is_ok()
            );
        }

        for _ in 0..TURNS {
            if Arc::strong_count(&held) == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            Arc::strong_count(&held),
            1,
            "dropping the group left its task running"
        );
    }

    #[tokio::test]
    async fn shutting_a_group_down_does_not_stop_the_run_it_belongs_to() {
        let run = Cancel::new();
        let group: Group<()> = Group::new(1, &run);

        assert!(group.shutdown(GRACE).await.is_empty());
        assert!(
            !run.requested(),
            "the group raised the token of the run that owns it"
        );
    }

    #[tokio::test]
    async fn stopping_the_run_reaches_the_tasks_of_a_group_inside_it() {
        let run = Cancel::new();
        let mut group = Group::new(1, &run);
        assert!(group.spawn(cooperative(group.cancel().clone())).is_ok());

        run.request();

        assert_eq!(group.shutdown(GRACE).await, vec![Ended::Done("noticed")]);
    }
}
