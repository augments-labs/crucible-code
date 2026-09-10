//! A set of tasks with one owner.
//!
//! The thing a group is for is the sentence "when this is over, nothing it
//! started is still running". A task spawned and forgotten keeps a socket, a
//! child process or a lock alive past the turn that wanted it, and the only
//! evidence is a hang somewhere else much later. Every task here is held, and
//! [`Group::shutdown`] answers for each of them by name.
//!
//! Admission is bounded and refused rather than queued: a caller told [`Full`]
//! can decide what to do about it, where a caller whose spawn silently waited
//! would have handed the bound over to nobody. There is no second refusal to
//! tell it apart from — [`Group::shutdown`] takes the group by value, so a
//! group that has been shut down is one nobody still holds to spawn into.
//!
//! Cancellation is cooperative, because that is the only kind that leaves a
//! half-written file impossible — see [`Cancel`]. Shutdown therefore asks
//! first and waits, and only what is still running when the grace runs out is
//! aborted, reported as [`Ended::Stopped`] rather than counted as finished.

use std::future::Future;
use std::time::Duration;

use tokio::task::JoinSet;

use crate::Cancel;

/// The group already holds every task it was bounded to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

/// What a task came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended<T> {
    /// It ran to the end, and this is what it answered.
    Done(T),
    /// It unwound. The panic is not resumed on the owner's thread: one task
    /// coming apart is not a reason for the shutdown reaping it to stop.
    Panicked,
    /// It was still running when the grace ran out, and was aborted.
    Stopped,
}

/// Tasks owned as one thing.
///
/// Dropping a group aborts whatever it still holds, so a group that goes out
/// of scope on an error path leaves nothing running either. That is the floor;
/// [`Group::shutdown`] is the door, and it is the one that reports.
#[derive(Debug)]
pub struct Group<T> {
    tasks: JoinSet<T>,
    ended: Vec<Ended<T>>,
    cancel: Cancel,
    limit: usize,
}

impl<T: Send + 'static> Group<T> {
    /// A group that holds at most `limit` tasks at once, stopped by `cancel`.
    ///
    /// The token is the group's own to raise: [`Group::shutdown`] raises it.
    /// Pass a child of the run's token — see [`Cancel::child`] — so that
    /// shutting one group down does not stop the run around it.
    #[must_use]
    pub fn new(limit: usize, cancel: Cancel) -> Self {
        Self {
            tasks: JoinSet::new(),
            ended: Vec::new(),
            cancel,
            limit,
        }
    }

    /// The token every task in this group should be checking.
    #[must_use]
    pub fn cancel(&self) -> &Cancel {
        &self.cancel
    }

    /// Starts `work`, or says why it did not.
    ///
    /// # Errors
    ///
    /// [`Full`] where the group already holds `limit` tasks that have not
    /// finished.
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

    /// How many tasks are held, finished ones excluded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Whether the group holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Stops admitting, asks every task to stop, and answers for each one.
    ///
    /// Cooperative first: the token is raised and the group waits up to
    /// `grace` for tasks to notice and return, which is what lets a task
    /// finish the write it had started. Whatever is still running when that
    /// runs out is aborted and reported [`Ended::Stopped`] — a task the owner
    /// had to take away is a cleanup that did not go to plan, and it is
    /// visible in the answer rather than counted among the finished.
    ///
    /// The order of the returned ends is the order they arrived in, which is
    /// not the order they were spawned in.
    pub async fn shutdown(mut self, grace: Duration) -> Vec<Ended<T>> {
        self.cancel.request();

        let deadline = tokio::time::Instant::now() + grace;
        while !self.tasks.is_empty() {
            match tokio::time::timeout_at(deadline, self.tasks.join_next()).await {
                Ok(Some(joined)) => self.ended.push(ended(joined)),
                // No tasks left to join; the loop's own condition ends it.
                Ok(None) => break,
                Err(_) => {
                    self.tasks.abort_all();
                    while let Some(joined) = self.tasks.join_next().await {
                        self.ended.push(ended(joined));
                    }
                    break;
                }
            }
        }

        self.ended
    }

    /// Moves every task that has already finished into the collected ends.
    fn collect_finished(&mut self) {
        while let Some(joined) = self.tasks.try_join_next() {
            self.ended.push(ended(joined));
        }
    }
}

/// What a join answered, said in this crate's words.
fn ended<T>(joined: Result<T, tokio::task::JoinError>) -> Ended<T> {
    match joined {
        Ok(answer) => Ended::Done(answer),
        Err(join) if join.is_cancelled() => Ended::Stopped,
        Err(_) => Ended::Panicked,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// Long enough that a cooperative task returns inside it, short enough that
    /// a test waiting out the whole of it is still quick.
    const GRACE: Duration = Duration::from_millis(500);

    #[tokio::test]
    async fn a_full_group_refuses_rather_than_queueing() {
        let mut group = Group::new(1, Cancel::new());

        group
            .spawn(std::future::pending::<()>())
            .expect("the first task fits");

        assert_eq!(
            group.spawn(std::future::pending::<()>()),
            Err(Full),
            "a bounded group must refuse, not queue behind the bound"
        );
    }

    #[tokio::test]
    async fn a_group_admits_again_once_a_task_has_finished() {
        let mut group = Group::new(1, Cancel::new());

        group.spawn(async { 1 }).expect("the first task fits");

        // The task has to be given the chance to run before its slot is free.
        tokio::task::yield_now().await;

        group
            .spawn(async { 2 })
            .expect("a finished task must not go on holding its slot");
    }

    #[tokio::test]
    async fn a_panicking_task_is_reported_rather_than_lost() {
        let mut group = Group::new(2, Cancel::new());

        group.spawn(async { 1 }).expect("room");
        group
            .spawn(async { panic!("a task came apart") })
            .expect("room");

        let ends = group.shutdown(GRACE).await;

        assert_eq!(ends.len(), 2, "shutdown answers for every task it held");
        assert!(
            ends.contains(&Ended::Panicked),
            "a panic must reach the owner as an end, not vanish: got {ends:?}"
        );
        assert!(ends.contains(&Ended::Done(1)));
    }

    /// The clock is paused, so `GRACE` is spent only if the shutdown actually
    /// waits it out; a test that returns before it costs no wall time at all.
    #[tokio::test(start_paused = true)]
    async fn a_cooperative_task_returns_inside_the_grace() {
        let mut group = Group::new(1, Cancel::new());
        let cancel = group.cancel().clone();

        group
            .spawn(async move {
                while !cancel.requested() {
                    tokio::task::yield_now().await;
                }
                "noticed"
            })
            .expect("room");

        let began = tokio::time::Instant::now();
        let ends = group.shutdown(GRACE).await;

        assert_eq!(ends, vec![Ended::Done("noticed")]);
        assert!(
            began.elapsed() < GRACE,
            "a task that stopped when asked must not be waited out to the grace"
        );
    }

    /// Paused, so the grace below is virtual: the task never finishes, the
    /// runtime goes idle, and the clock jumps to the deadline. What the
    /// assertions then read is the deadline doing the stopping, rather than a
    /// wall-clock wait that happened to be long enough.
    #[tokio::test(start_paused = true)]
    async fn no_task_survives_the_owner_shutting_down() {
        let mut group = Group::new(1, Cancel::new());

        // Held by the task and by this test. If the task is still alive after
        // shutdown, so is its clone, and the count says so.
        let held = Arc::new(());
        let carried = Arc::clone(&held);

        group
            .spawn(async move {
                std::future::pending::<()>().await;
                drop(carried);
            })
            .expect("room");

        let began = tokio::time::Instant::now();
        let ends = group.shutdown(GRACE).await;

        assert_eq!(
            began.elapsed(),
            GRACE,
            "a task that will not stop is given the grace, and no longer"
        );
        assert_eq!(
            ends,
            vec![Ended::Stopped],
            "a task that would not stop must be reported as taken away"
        );
        assert_eq!(
            Arc::strong_count(&held),
            1,
            "the task was still holding what it was given, so it is still running"
        );
    }

    #[tokio::test]
    async fn a_group_dropped_on_an_error_path_leaves_nothing_running() {
        let held = Arc::new(());
        let carried = Arc::clone(&held);

        {
            let mut group = Group::new(1, Cancel::new());
            group
                .spawn(async move {
                    std::future::pending::<()>().await;
                    drop(carried);
                })
                .expect("room");
        }

        // Dropping the group aborts what it held; the abort is what releases
        // the task's own clone, and it takes a scheduler pass to land.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        assert_eq!(Arc::strong_count(&held), 1);
    }
}
