//! What this run's vendor may not be sent, taken out of what it is sent.
//!
//! Whether a result may go to a vendor is `crucible_models::transfer`'s to
//! decide, from the provenance the result carries. What is left here is the
//! runner's half: finding the results the next request may not carry, clearing
//! them from the transcript, and writing the line that makes a continued
//! session clear them again.
//!
//! A clearing is made where the transcript changes hands — a session picked
//! up, a vendor changed, a pass's results recorded — and only the last of
//! those is inside a turn that can wait for the session. So a clearing owes
//! its lines rather than writing them, and whatever can wait writes what is
//! owed: the recording of a pass's results at once, and a turn or a
//! compaction before it records or sends anything, and the application after
//! picking a session up or changing vendor, through
//! [`Runner::record_clearings`].

use std::sync::Arc;

use crucible_core::{JournalStore, Provider, ToolId};

use super::{Runner, load};

/// A clearing's line not yet written, and the session it is owed to.
///
/// The session is kept with the line because picking another one up does not
/// settle what the one left behind is owed. What a line holds is bounded by
/// the transcript it was cleared from: the names of results it already holds,
/// and one sentence.
#[derive(Debug)]
pub(super) struct Owed {
    store: Arc<dyn JournalStore>,
    freed: usize,
    results: Vec<ToolId>,
    notice: Box<str>,
}

impl Runner {
    /// Takes out of a transcript just read back what this run's vendor may not
    /// be sent, and hands back the reading the log kept where it still covers
    /// what is left.
    ///
    /// Nobody is being left: the run may have started on another vendor than the
    /// one the results came from, and no switch is ever observed to say so. What
    /// the results record about who answered them is the whole of the decision.
    /// The reading is `None` once anything was taken out: it measured a request
    /// that carried it, and replaying the clearing's line drops it for the same
    /// reason.
    pub(super) fn admit_restricted(&mut self) -> Option<crucible_types::Calibration> {
        let clearing = self.untransferable(0, self.provider.as_ref(), None);
        // As though a report had measured every message: the recount each
        // caller runs next rebuilds the load from the transcript either way.
        self.clear_untransferable(&clearing, self.state.transcript.messages().len());
        if clearing.is_empty() {
            self.store.calibrated()
        } else {
            None
        }
    }

    /// Takes out of the message just recorded what this run's vendor may not be
    /// sent, and writes the lines saying so.
    pub(super) async fn admit_recorded(&mut self) {
        let recorded = self.state.transcript.messages().len().saturating_sub(1);
        let clearing = self.untransferable(recorded, self.provider.as_ref(), None);
        // No report has measured the message just recorded, which is the last.
        self.clear_untransferable(&clearing, recorded);
        self.record_clearings().await;
    }

    /// The results from message `from` on that the next request may not carry to
    /// `recipient`, each with the sentence to leave in its place.
    ///
    /// What may go where is decided by the result's own provenance, through
    /// `crucible_models::transfer`, never from a vendor's or a tool's name here.
    pub(super) fn untransferable(
        &self,
        from: usize,
        recipient: &dyn Provider,
        leaving: Option<&dyn Provider>,
    ) -> Vec<(crucible_core::ToolId, Box<str>)> {
        let mut clearing = Vec::new();
        for message in self.state.transcript.messages().iter().skip(from) {
            if let crucible_core::Message::ToolResults(results) = message {
                for result in results {
                    if let crucible_models::Transfer::Clear(notice) =
                        crucible_models::transfer(result.output.provenance(), recipient, leaving)
                    {
                        clearing.push((result.id.clone(), notice.into()));
                    }
                }
            }
        }
        clearing
    }

    /// Takes the results a vendor restricts out of what is sent from here on,
    /// and owes the session the lines that record it.
    ///
    /// The results stay in the log holding what they held — the log is the
    /// record of the session, and a user reading their own history is not the
    /// third party the term is about. What the line buys is the session coming
    /// back the same way: without it the transcript loses them and the log does
    /// not, so the next resume reads them back and sends them on.
    ///
    /// One line per sentence, since a line carries one. Clearing takes the
    /// provenance with the content, so a result is never cleared twice.
    ///
    /// No report has measured any message from `unmeasured` on; what the
    /// clearing then does to the load is [`load::Load::rewritten`]'s to say.
    pub(super) fn clear_untransferable(
        &mut self,
        clearing: &[(crucible_core::ToolId, Box<str>)],
        unmeasured: usize,
    ) {
        if clearing.is_empty() {
            return;
        }
        // Weighed a message at a time on both sides, because a clearing reaches
        // every result with a named id wherever it stands; and only here, so a
        // pass that clears nothing walks nothing.
        let before = load::Load::weights(self.state.transcript.messages());
        let mut notices: Vec<&str> = Vec::new();
        for (_, notice) in clearing {
            if !notices.contains(&&**notice) {
                notices.push(notice);
            }
        }

        for notice in notices {
            let results: Vec<crucible_core::ToolId> = clearing
                .iter()
                .filter(|(_, left)| **left == *notice)
                .map(|(id, _)| id.clone())
                .collect();

            // The transcript first and the line after it, the way a pruning is
            // written and for the same reason: a crash between the two must not
            // leave a log claiming a clearing that the transcript never made.
            let freed = self.state.transcript.clear_tool_outputs(&results, notice);
            self.state.owed.push(Owed {
                store: Arc::clone(&self.store),
                freed,
                results,
                notice: notice.into(),
            });
        }
        self.state
            .load
            .rewritten(&before, self.state.transcript.messages(), unmeasured);
    }

    /// Whether a clearing still owes a session its line, which
    /// [`Runner::record_clearings`] would write.
    #[must_use]
    pub fn owes_clearings(&self) -> bool {
        !self.state.owed.is_empty()
    }

    /// Whether a clearing still owes `store` its line: the same store, not
    /// one that answers alike.
    #[must_use]
    pub fn owes_clearings_to(&self, store: &dyn JournalStore) -> bool {
        self.state
            .owed
            .iter()
            .any(|owed| std::ptr::addr_eq(Arc::as_ptr(&owed.store), store))
    }

    /// Writes the lines clearing what a vendor may not be sent still owes a
    /// session, in the order they were owed, and waits for each session to
    /// take its line.
    ///
    /// Picking a session up and changing vendor clear the transcript they are
    /// handed and cannot wait, so each owes the session its lines until this
    /// writes them: the caller that picked the session up or changed the
    /// vendor calls this next, and a turn or a compaction calls it before it
    /// records or sends anything, so a caller that does not still has them
    /// written before the next turn's or compaction's own lines. A line is
    /// written to the session it was owed to, which may be one this runner
    /// has since left. A line is taken off what is owed as its write begins,
    /// so one whose write is dropped part way is not written twice, and
    /// whether the log holds it is the store's to say.
    ///
    /// A line still owed keeps the session it is owed to, so a session this
    /// runner has left stays open until its line is written. One still owed
    /// when the runner is dropped is never written: a caller that could not
    /// wait for this says so to whoever reads the session, as the
    /// application does.
    pub async fn record_clearings(&mut self) {
        while !self.state.owed.is_empty() {
            let owed = self.state.owed.remove(0);
            owed.store
                .restricted(owed.freed, &owed.results, &owed.notice)
                .await;
        }
    }
}
