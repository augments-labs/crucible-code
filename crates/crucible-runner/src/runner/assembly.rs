//! Recording the context one provider pass is sent under.
//!
//! Which facts become which words is `crucible-context`'s. What is left here is
//! the runner's half: reading the live owners it holds, and recording the
//! result through [`Runner::record`] so request load and durable history
//! observe the same bytes, before the merge patch that makes those words
//! replayable as typed state.

use crucible_context::{Live, assemble};
use crucible_core::{Ancestry, Message};

use super::Runner;

use crate::TurnError;
impl Runner {
    /// Reconciles and records every section for the exact pass about to send.
    pub(super) async fn assemble_context(&mut self, ancestry: Ancestry) -> Result<(), TurnError> {
        // Taken whole rather than borrowed: the snapshot is typed state a
        // store reconstructs, and holding a borrow into it across the records
        // below would be this run reading its own store while it writes to it.
        let snapshot = self.store.context_snapshot();
        let assembled = assemble(
            &self.context,
            snapshot.as_ref(),
            &self.state.transcript,
            Live {
                model: &self.agent.model().name,
                effort: self.agent.model().effort,
                tools: &self.state.tools,
                permission: &self.permission,
            },
        )?;

        // Words first, state second. A crash between them replays as Unknown;
        // the opposite order could claim the model saw words never retained.
        for fragment in assembled.fragments {
            self.record(ancestry, Message::Context(fragment)).await?;
        }
        if let Some(patch) = assembled.patch {
            self.store.contextual(&patch).await?;
        }

        Ok(())
    }
}
