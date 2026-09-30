//! The speed a conversation asks its model to answer at.
//!
//! Chosen by the user and kept in the user's own file, under the provider. It
//! reaches a request only where the model in force has a fast form the request
//! carries, which the provider answers for through `Provider::fast`: a model
//! with none, a model that is itself a fast id, and a provider sent to another
//! address than its vendor's all ask for standard whatever the file says. A
//! speed left in the file where it does not apply is left there and counts as
//! standard.
//!
//! It goes back to standard, in the runner and in the file, when another model
//! is chosen, when the vendor refuses the fast form, and when the credential in
//! force for the provider changes: each is a moment the price that was shown
//! when fast was chosen may no longer be the price.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crucible_auth::Moving;
use crucible_models::{FastForm, Speed};

use crate::Conversation;
use crate::remember::{self, RememberError};
use crate::switching::Switching;

/// What came of asking for a speed.
#[derive(Debug)]
pub enum Hastened {
    /// Nobody is being asked, or no model is.
    Unasked,
    /// The model in force has no fast form. The speed in force still is.
    Unsupported,
    /// The model in force is itself a fast model: there is no standard form of
    /// it to switch to, and the others are chosen with the model.
    Own,
    /// The next turn asks at this speed.
    Taken {
        /// Why the speed will not outlive this run, where it will not.
        unwritten: Option<RememberError>,
    },
}

/// What the credential store asks before a write moves a provider's
/// credential: that provider's speed taken out of `file`, the user's own,
/// before the store is written. What fast costs was shown for the credential
/// it was chosen under.
#[must_use]
pub fn moving(file: PathBuf) -> Moving {
    Arc::new(move |providers: &[&str]| {
        for provider in providers {
            remember::slowing(&file, provider).map_err(|problem| {
                Box::<str>::from(format!(
                    "the speed that goes with it could not be taken out: {problem}"
                ))
            })?;
        }
        Ok(())
    })
}

impl Conversation {
    /// Asks the model in force at `speed` from the next turn on, and writes it
    /// down for the next run: fast together with the model in force, which a
    /// plain start then asks for, and standard by taking fast out.
    pub fn hasten(&mut self, speed: Speed, with: &Switching<'_>) -> Hastened {
        let Some(provider) = self.serving.filter(|_| !self.runner.model().is_empty()) else {
            return Hastened::Unasked;
        };
        match self.runner.provider().fast(self.runner.model()) {
            FastForm::None => Hastened::Unsupported,
            FastForm::Own(_) => Hastened::Own,
            FastForm::Field(_) => {
                self.runner.hasten(speed);
                let written = match speed {
                    Speed::Fast => {
                        remember::hastening(with.choosing, provider, self.runner.model())
                    }
                    Speed::Standard => remember::slowing(with.choosing, provider),
                };
                Hastened::Taken {
                    unwritten: written.err(),
                }
            }
        }
    }

    /// Asks at the speed `file`, the user's own, says now for the provider in
    /// force, where it names the model in force and that model can be asked
    /// for it: what a run starts at, and what a credential written or
    /// forgotten leaves.
    ///
    /// Read from the file rather than from the settings the run started with,
    /// because a credential that moved took its provider's speed out of the
    /// file after those were read. A file that cannot be read asks standard.
    pub fn hastened_as_kept(&mut self, file: &Path) {
        let switched = self.runner.provider().fast(self.runner.model()).switched();
        let kept = self
            .serving
            .and_then(|provider| remember::hastened(file, provider, self.runner.model()).ok());
        let speed = match kept {
            Some(Speed::Fast) if switched => Speed::Fast,
            Some(_) | None => Speed::Standard,
        };
        self.runner.hasten(speed);
    }

    /// Standard from the next turn on, for `provider`, in the runner and in
    /// `file`.
    pub(crate) fn slowed(&mut self, provider: &str, file: &Path) -> Option<RememberError> {
        self.runner.hasten(Speed::Standard);
        remember::slowing(file, provider).err()
    }

    /// Writes down that the vendor refused the fast form, where the turn or
    /// the compaction that just ended asked at `asked` and the runner turned
    /// it off.
    ///
    /// Written to the file a yes is written to, which is the user's own. A file
    /// that could not be written is not said again here: the line of the
    /// refusal already said fast is off, and the next run asks once, is
    /// refused once and says so.
    pub(crate) fn refusal_written(&self, asked: Speed) {
        if asked != Speed::Fast || self.runner.speed() != Speed::Standard {
            return;
        }
        let (Some(provider), Some(file)) = (
            self.serving,
            self.consent().and_then(|consent| consent.file()),
        ) else {
            return;
        };
        let _ = remember::slowing(file, provider);
    }
}

#[cfg(test)]
mod tests;
