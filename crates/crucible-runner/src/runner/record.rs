//! Writing a message down: the one way a message is appended to both the
//! transcript and the log. The two can still disagree where the session would
//! not take the message's line, as [`Runner::record`] says, and a compaction,
//! a pruning, a clearing and the context patch each write lines of their own.

use crucible_core::{Ancestry, Message, ProviderError, RunItem};
use crucible_runtime::Bridge;

use super::Runner;

use crate::TurnError;

impl Runner {
    /// Appends a message to the transcript, and its line to the log.
    ///
    /// The one way a message is appended to either, so the two are written
    /// together. Two calls that could be made separately would eventually be
    /// made separately, and a log that is missing one message is a session
    /// that cannot be continued. The one other message pushed onto the
    /// transcript is the recap request a compaction asks with, which is taken
    /// back out before the compaction returns and is never written down. A
    /// compaction's replacement, a pruning and a clearing change the
    /// transcript without appending to it, and each writes a line of its own.
    ///
    /// A message whose line the session would not take is left out of the
    /// transcript, except the results of a pass's calls. Those stay with the
    /// calls they answer: the calls were answered and only the line is in
    /// doubt, and a transcript left on calls nothing answered is one a
    /// provider can refuse to build a request from. They are still cleared of
    /// what this run's vendor may not be sent, and each clearing's line is
    /// still attempted, since the results may have reached the log. What
    /// accepted background work kept aside for them is not settled, since the
    /// line that would replace it is not known to be in the log. No reading of
    /// the window follows a results line either way: no response has measured
    /// a request that carries it yet.
    ///
    /// # Errors
    ///
    /// [`TurnError::Provider`] with [`ProviderError::Protocol`] ("invalid or
    /// oversized provider continuation") where the transcript refuses the
    /// private provider state the message carries. It is returned before
    /// anything is written.
    ///
    /// [`TurnError::Unready`] for the first session write that would have had
    /// to wait, in the order they are reported: the message's own line, a
    /// clearing's line, then the reading.
    pub(super) fn record(&mut self, ancestry: Ancestry, message: Message) -> Result<(), TurnError> {
        self.state
            .transcript
            .check_continuation(&message)
            .map_err(|_| ProviderError::Protocol {
                provider: self.provider.name(),
                problem: "invalid or oversized provider continuation".into(),
            })?;
        let answers_calls = matches!(&message, Message::ToolResults(_));
        let written = match RunItem::message(ancestry, message.clone()) {
            Ok(item) => {
                let written = Bridge::TurnSession.cross(self.store.append_message(&message));
                if written.is_ok() {
                    self.store.append_run_item(&item);
                }
                written
            }
            // The provider and tool admission boundaries already enforce
            // these bounds. Preserve the conversation if an internal caller
            // ever violates that contract, while its missing companion record
            // makes the defect visible instead of writing unsafe metadata.
            Err(_) => Bridge::TurnSession.cross(self.store.append_message(&message)),
        };
        if !answers_calls {
            written?;
        }
        self.state.load.recorded(&message);
        self.state
            .transcript
            .push(message)
            .map_err(|_| ProviderError::Protocol {
                provider: self.provider.name(),
                problem: "invalid or oversized provider continuation".into(),
            })?;

        // After the message and not beside it: what this says covers the
        // transcript including what was just appended, and a reader that found
        // it in the other order would have it covering one message less.
        let measured = self.state.load.calibrated().map_or(Ok(()), |calibration| {
            Bridge::TurnSession.cross(self.store.measured(&calibration))
        });
        if answers_calls {
            if written.is_ok() {
                self.store.settle_call_results();
            }
            // After the results line, so the log reads what was answered and
            // then what was taken out of it. The search source was chosen when
            // the run started, so a session that moved away from its vendor
            // still searches through that vendor, and the next request of this
            // turn is built from what was just recorded. Made whether or not
            // the session took the line: a refused line must not carry what
            // this vendor may not be sent into a later request.
            let admitted = self.admit_recorded();
            written?;
            admitted?;
        }
        // A reading the session would not take is reported last, since the
        // message it covers is recorded either way. Whether the log kept it is
        // not known: the store says nothing of a write dropped before it
        // answered.
        measured.map_err(TurnError::from)
    }
}
