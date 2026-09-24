//! Writing a message down: the one way a message is appended to both the
//! transcript and the log. A compaction, a pruning, a clearing and the context
//! patch each write lines of their own.

use crucible_core::{Ancestry, Message, ProviderError, RunItem};

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
    /// Each line is awaited: the message joins the transcript only once the
    /// session has taken its line, and the reading of the window that follows
    /// it, and any clearing of what this run's vendor may not be sent, are
    /// written after it the same way. A line the log could not keep is the
    /// session's to report, as its trouble, and does not end the turn. A pass's
    /// results are cleared of what this run's vendor may not be sent once
    /// they are recorded, and what accepted background work kept aside for
    /// them is settled first. No reading of the window follows a results line:
    /// no response has measured a request that carries it yet.
    ///
    /// # Errors
    ///
    /// [`TurnError::Provider`] with [`ProviderError::Protocol`] ("invalid or
    /// oversized provider continuation") where the transcript refuses the
    /// private provider state the message carries. It is returned before
    /// anything is written.
    pub(super) async fn record(
        &mut self,
        ancestry: Ancestry,
        message: Message,
    ) -> Result<(), TurnError> {
        self.state
            .transcript
            .check_continuation(&message)
            .map_err(|_| ProviderError::Protocol {
                provider: self.provider.name(),
                problem: "invalid or oversized provider continuation".into(),
            })?;
        let answers_calls = matches!(&message, Message::ToolResults(_));
        match RunItem::message(ancestry, message.clone()) {
            Ok(item) => {
                self.store.append_message(&message).await;
                self.store.append_run_item(&item);
            }
            // The provider and tool admission boundaries already enforce
            // these bounds. Preserve the conversation if an internal caller
            // ever violates that contract, while its missing companion record
            // makes the defect visible instead of writing unsafe metadata.
            Err(_) => self.store.append_message(&message).await,
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
        if let Some(calibration) = self.state.load.calibrated() {
            self.store.measured(&calibration).await;
        }
        if answers_calls {
            self.store.settle_call_results();
            // After the results line, so the log reads what was answered and
            // then what was taken out of it. The search source was chosen when
            // the run started, so a session that moved away from its vendor
            // still searches through that vendor, and the next request of this
            // turn is built from what was just recorded.
            self.admit_recorded().await;
        }
        Ok(())
    }
}
