//! Recording the context one provider pass is sent under.
//!
//! Which facts become which words is `crucible-context`'s. What is left here is
//! the runner's half: reading the live owners it holds, and recording the
//! result through [`Runner::record`] so request load and durable history
//! observe the same bytes, before the merge patch that makes those words
//! replayable as typed state.

use crucible_context::{Live, assemble};
use crucible_core::{Ancestry, Message, TurnError};

use super::Runner;

impl Runner {
    /// Reconciles and records every section for the exact pass about to send.
    pub(super) fn assemble_context(&mut self, ancestry: Ancestry) -> Result<(), TurnError> {
        let assembled = assemble(
            &self.context,
            self.session.context_snapshot(),
            &self.transcript,
            Live {
                model: &self.spec.model.name,
                effort: self.spec.model.effort,
                tools: &self.tools,
                permission: &self.permission,
            },
        )?;

        // Words first, state second. A crash between them replays as Unknown;
        // the opposite order could claim the model saw words never retained.
        for fragment in assembled.fragments {
            self.record(ancestry, Message::Context(fragment))?;
        }
        if let Some(patch) = assembled.patch {
            self.session.contextual(&patch)?;
        }

        Ok(())
    }
}
