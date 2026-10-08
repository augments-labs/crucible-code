//! `crucible update`, run on the application's services.
//!
//! What is asked, compared, downloaded, installed and refused is
//! `crucible-update`'s to decide; this module hands it the release owner and
//! the runtime the rest of a run uses, so an update reaches GitHub with the
//! same TLS and proxy decisions a startup check does. Like [`crate::auth`], it
//! is a local command rather than a request of the client contract, and it
//! reads no configuration: asking is what overrides `updates.check = never`.

pub use crucible_update::{Answer, Asked, Refused, SOURCE, SelfUpdateCommand};

use crate::runtime::Unstarted;
use crate::services::{Unfinished, serving};

/// Why `crucible update` gave no answer.
#[derive(Debug, thiserror::Error)]
pub enum Failed {
    /// The update refused, or put back what it had changed.
    #[error(transparent)]
    Refused(#[from] Refused),
    /// The runtime the release source is asked on would not start.
    #[error(transparent)]
    Unstarted(#[from] Unstarted),
}

/// Runs `command`, then shuts the services it ran on down.
///
/// # Errors
///
/// The first half is [`Failed`] where nothing was answered. The second is
/// [`Unfinished`] where the services' shutdown ran out of its bounds, whatever
/// the first says.
pub fn update(command: &SelfUpdateCommand<'_>) -> (Result<Answer, Failed>, Result<(), Unfinished>) {
    serving(|services| {
        let runtime = services.runtime().handle()?;
        Ok(services.release().update(&runtime, command)?)
    })
}
