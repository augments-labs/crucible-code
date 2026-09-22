//! What this run's vendor may not be sent, taken out of what it is sent.
//!
//! Whether a result may go to a vendor is `crucible_models::transfer`'s to
//! decide, from the provenance the result carries. What is left here is the
//! runner's half: finding the results the next request may not carry, clearing
//! them from the transcript, and writing the line that makes a continued
//! session clear them again.

use crucible_core::Provider;
use crucible_runtime::{Bridge, Unready};

use super::{Runner, load};

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
        let cleared = self.clear_untransferable(&clearing, self.state.transcript.messages().len());
        self.hold(cleared);
        if clearing.is_empty() {
            self.store.calibrated()
        } else {
            None
        }
    }

    /// Takes out of the message just recorded what this run's vendor may not be
    /// sent.
    ///
    /// # Errors
    ///
    /// [`Unready`] where the session would not take a clearing's line.
    pub(super) fn admit_recorded(&mut self) -> Result<(), Unready> {
        let recorded = self.state.transcript.messages().len().saturating_sub(1);
        let clearing = self.untransferable(recorded, self.provider.as_ref(), None);
        // No report has measured the message just recorded, which is the last.
        self.clear_untransferable(&clearing, recorded)
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
    /// and records that it happened.
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
    ///
    /// # Errors
    ///
    /// [`Unready`] for the first line the session would not take. Every
    /// clearing is still made and the load still follows them. Whether a
    /// refused line reached the log is not known, since the store says nothing
    /// of a write dropped before it answered.
    pub(super) fn clear_untransferable(
        &mut self,
        clearing: &[(crucible_core::ToolId, Box<str>)],
        unmeasured: usize,
    ) -> Result<(), Unready> {
        if clearing.is_empty() {
            return Ok(());
        }
        // Weighed a message at a time on both sides, because a clearing reaches
        // every result with a named id wherever it stands; and only here, so a
        // pass that clears nothing walks nothing.
        let before = load::Load::weights(self.state.transcript.messages());
        let mut refused = None;
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
            if let Err(unready) =
                Bridge::TurnSession.cross(self.store.restricted(freed, &results, notice))
            {
                refused.get_or_insert(unready);
            }
        }
        self.state
            .load
            .rewritten(&before, self.state.transcript.messages(), unmeasured);
        refused.map_or(Ok(()), Err)
    }

    /// Keeps a line the session would not take between turns, for the next
    /// turn or compaction to report.
    ///
    /// Picking a session up and changing vendor happen between turns and hand
    /// their caller nothing, so the turn or compaction that follows reports the
    /// hold, before it records or sends anything. Only the first refusal is
    /// kept. A hold survives picking a session up and is reported on the
    /// session picked up, naming the bridge rather than the session; one still
    /// held when the runner is dropped is reported to nobody.
    pub(super) fn hold(&mut self, written: Result<(), Unready>) {
        if let Err(unready) = written {
            self.state.unwritten.get_or_insert(unready);
        }
    }
}
