//! Recording the context one provider pass is sent under, and handing the
//! model the answers to questions it asked without waiting.
//!
//! Which facts become which words is `crucible-context`'s. What is left here is
//! the runner's half: reading the live owners it holds, and recording the
//! result through [`Runner::record`] so request load and durable history
//! observe the same bytes, before the merge patch that makes those words
//! replayable as typed state.
//!
//! The questions a root run holds are also durable through that state, and
//! the order of one assembly is what keeps an answer from being lost to a
//! crash. The registry's state is taken first, while a settled batch is still
//! listed as waiting in what the log last recorded; the answers are taken and
//! recorded as the person's words after the fragments; the patch that stops
//! listing them is recorded last. A log that ends anywhere in between either
//! holds the answer or brings its batch back as waiting, so the person may be
//! asked twice and is never ignored. The one exception is a record the
//! registry would refuse to read back, such as one of too many batches or bytes,
//! from which `asked` drops the oldest it no longer lists so the rest still
//! come back. A batch asked during a tool pass is recorded, with the next id,
//! before the result naming it ([`Runner::asked_since`]), so a log that holds
//! the id brings the batch back and never hands the id out again.
//!
//! The person's task is the root run's as well. Each answer delivered amends
//! it, as an answer, and the patch recorded after the answers carries the task
//! with them, so a log that holds the patch holds the answers it names. The
//! other direction does not hold: words reach the log before the task that
//! keeps them, which is recorded with the next assembly's state. A log that
//! ends between the two holds the words in the transcript but not in the task,
//! and a session picked up from it restates the task without them once a
//! compaction has taken them. A log that ends before any task was recorded
//! beside the words it holds, as the first telling's can, lost the request: a
//! session picked up from it says the request was not kept, and takes nothing
//! said afterwards for it. A turn over a note the application wrote never
//! amends it, whatever the note quotes.
//!
//! An answer is the person's words, and this agent's input checks judge each
//! one before it amends the task or is recorded, as they judged the prompt
//! that opened the turn. The first a check refuses, or cannot decide about, is
//! handed back to the registry with the answers taken after it
//! ([`Deferred::withheld`]): its batch waits again, the rest stay settled for
//! a later pass, and the pass ends the turn on that decision without sending
//! anything. Those answered before it are recorded, and the patch still
//! follows them.

use crucible_context::{
    ContextSection, Live, Questions, QuestionsSection, Task, TaskSection, asked, assemble,
};
use crucible_tools::Deferred;
use crucible_types::{Ancestry, Message};

use super::Runner;
use super::passes::Ending;

use crate::context::RunContext;
use crate::{Event, TurnError};

impl Runner {
    /// Reconciles and records every section for the exact pass about to send,
    /// then the answers a root run's registry holds that this agent's input
    /// checks allow, and says how many answers it recorded and the ending an
    /// answer a check withheld makes.
    pub(super) async fn assemble_context(
        &mut self,
        run: &RunContext<'_>,
    ) -> Result<(usize, Option<Ending>), TurnError> {
        let ancestry = run.ancestry();
        // Taken whole rather than borrowed: the snapshot is typed state a
        // store reconstructs, and holding a borrow into it across the records
        // below would be this run reading its own store while it writes to it.
        let snapshot = self.store.context_snapshot();
        let root = ancestry.parent().is_none();
        let held = self.holding(ancestry).cloned();
        let mut questions = match &held {
            Some(held) => Questions::Held(held),
            None if root => Questions::Unheld,
            None => Questions::Beneath,
        };
        if let Some(held) = &held
            && !self.recalled
        {
            self.recalled = true;
            // The first assembly of a session started or picked up: the
            // batches its log recorded as waiting come back. A registry
            // handed in already holding something keeps what it holds, which
            // is right only for this session's own (see
            // [`Runner::deferring`]), and numbers past the ids the log spent;
            // [`Runner::pick_up`] empties the one it holds, so a session
            // picked up always gets its own batches back.
            if let Some(saved) = snapshot
                .as_ref()
                .and_then(|saved| saved.get(QuestionsSection::ID))
            {
                // Read and then replaced under two locks, which is safe only
                // because nothing registers between them: this runs in a
                // pass's assembly, while none of the tools that register is
                // running.
                let empty = held.waiting().is_empty() && !held.any_settled();
                if !empty {
                    held.continue_from(saved);
                } else if held.restore(saved).is_err() {
                    // A state this build cannot read is told to the model as
                    // lapsed, as it would be to a runner holding none, and
                    // the empty state recorded in its place.
                    questions = Questions::Unheld;
                    held.continue_from(saved);
                }
            }
        }

        if root {
            self.task();
        }
        let assembled = assemble(
            &self.context,
            snapshot.as_ref(),
            &self.state.transcript,
            Live {
                model: &self.agent.model().name,
                effort: self.agent.model().effort,
                tools: &self.state.tools,
                permission: &self.permission,
                questions,
                task: self.task.as_ref().filter(|_| root),
            },
        )?;

        // Words first, state second; the opposite order could claim the model
        // saw words never retained. A crash between them replays against the
        // state recorded before them: a section told for the first time comes
        // back Unknown and is restated whole, and a later change comes back
        // Known at the state before it, which the next pass compares against.
        for fragment in assembled.fragments {
            self.record(ancestry, Message::Context(fragment)).await?;
        }

        // Taken after the state above, never before: a batch settled between
        // the two is still listed as waiting in that state, and is delivered
        // here all the same; the patch below lists it as waiting until the
        // next assembly records the state without it, so a log that ends
        // before then still holds the answer. Taken in the other order, an
        // answer settled between them would be in neither the patch nor the
        // transcript.
        let mut delivered = 0;
        let mut withheld = None;
        if let Some(held) = &held {
            let mut settled = held.take_settled().into_iter();
            while let Some(answer) = settled.next() {
                // Judged by what the person said in it alone: the framing and
                // the headings around it are not theirs, and an answer that
                // says nothing, such as a batch declined whole, is not judged.
                let replied = answer.replied();
                if !replied.is_empty() {
                    withheld = Ending::withheld(self.judging_input(&replied, run));
                }
                if withheld.is_some() {
                    held.withheld(answer, settled.collect());
                    break;
                }
                let said = answer.said();
                // Only the root run holds questions, so this is its task.
                self.task().answered(&said);
                self.record(ancestry, Message::said(said)).await?;
                delivered += 1;
            }
        }

        let mut patch = assembled.patch;
        if delivered > 0
            && let Some(task) = &self.task
        {
            // The words told above are not told again: the answers follow
            // them in the transcript, and the next compaction restates them
            // from the task this records.
            let prior = snapshot.unwrap_or_default();
            let mut now = match &patch {
                Some(patch) => patch.apply(&prior)?,
                None => prior.clone(),
            };
            now.record(
                TaskSection::ID,
                TaskSection::new(Some(task), None).checked_snapshot()?,
            )?;
            patch = now.patch_from(&prior);
        }
        if let Some(patch) = patch {
            self.store.contextual(&patch).await?;
        }

        Ok((delivered, withheld))
    }

    /// Adds what the person said to the task, where `ancestry` is the root
    /// run's: what a run beneath it is told is its parent's words.
    pub(super) fn amend(&mut self, ancestry: Ancestry, said: &str) {
        if ancestry.parent().is_none() {
            self.task().said(said);
        }
    }

    /// The root run's task, read back from what the session's log recorded
    /// the first time this process needs it.
    ///
    /// A log that recorded no task beside words it holds lost the request, so
    /// what the person says next is not taken for it.
    fn task(&mut self) -> &mut Task {
        let task = match self.task.take() {
            Some(task) => task,
            None => self
                .store
                .context_snapshot()
                .and_then(|recorded| recorded.get(TaskSection::ID).map(Task::restored))
                .unwrap_or_else(|| {
                    if self.inherited_words {
                        Task::unkept()
                    } else {
                        Task::default()
                    }
                }),
        };
        self.task.insert(task)
    }

    /// Records the batches a root run's registry was asked during the tool
    /// pass that just ran, before the results that name them.
    pub(super) async fn asked_since(&mut self, ancestry: Ancestry) -> Result<(), TurnError> {
        let Some(held) = self.holding(ancestry) else {
            return Ok(());
        };
        if let Some(patch) = asked(self.store.context_snapshot().as_ref(), held)? {
            self.store.contextual(&patch).await?;
        }
        Ok(())
    }

    /// Tells a root run's reader which questions wait, where that changed
    /// since they were last told.
    pub(super) fn report_waiting(&mut self, run: &RunContext<'_>) {
        let Some(held) = self.holding(run.ancestry()) else {
            return;
        };
        let revision = held.revision();
        if revision == self.told {
            return;
        }
        let batches = held.waiting();
        self.told = revision;
        run.reporting().post(Event::Waiting { batches });
    }

    /// Whether a root run's registry holds an answer no pass has delivered.
    pub(super) fn undelivered(&self, ancestry: Ancestry) -> bool {
        self.holding(ancestry).is_some_and(Deferred::any_settled)
    }

    /// The registry of questions asked without waiting, where this run holds
    /// it: only the root does. A run beneath it asks through the root's tool,
    /// and the answers are the root's to deliver and record.
    fn holding(&self, ancestry: Ancestry) -> Option<&Deferred> {
        self.deferred
            .as_ref()
            .filter(|_| ancestry.parent().is_none())
    }
}
