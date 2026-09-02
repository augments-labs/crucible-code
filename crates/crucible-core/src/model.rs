//! What one model accepts, produces and serves.
//!
//! A provider is a name that builds an arm; this is what that arm can be asked
//! for. The two are separate records because they go stale differently: an arm
//! changes when this build gains a wire module, and a model's limits change
//! whenever its vendor publishes a new figure.
//!
//! A model nobody wrote a record for has no entry here at all, rather than an
//! entry full of zeroes. The difference matters at every reader: a window of
//! zero is a session that throws itself away on the first turn, while nothing
//! known is a caller free to fall back to a configured figure, to a provider's
//! conservative default, or to letting the vendor answer.

use crate::{Effort, Modalities};

/// The most bytes a model name or its shown spelling may retain.
///
/// Generous next to any name a vendor has published, and small enough that a
/// registration carrying a document where a name belongs is refused at the
/// boundary rather than kept for the life of the process.
pub const MODEL_NAME_BYTES: usize = 256;

/// Why a model record could not be described.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// A retained string was empty.
    #[error("{field} must not be empty")]
    Empty {
        /// Which field.
        field: &'static str,
    },
    /// A retained string crossed its boundary.
    #[error("{field} is {actual} bytes; the maximum is {maximum}")]
    TooLong {
        /// Which field.
        field: &'static str,
        /// Its boundary.
        maximum: usize,
        /// What was supplied.
        actual: usize,
    },
    /// A limit was stated as nothing.
    #[error("{model} states {field} of zero")]
    Nothing {
        /// The model that stated it.
        model: Box<str>,
        /// Which limit, spelled with its article.
        field: &'static str,
    },
    /// The rungs were not the ladder's own order, or repeated one.
    #[error("{model} serves {} rather than weakest first without repeating one", .rungs.iter().map(|rung| rung.as_str()).collect::<Vec<_>>().join(", "))]
    Rungs {
        /// The model that stated them.
        model: Box<str>,
        /// What was stated.
        rungs: Box<[Effort]>,
    },
}

/// The figures a vendor publishes beside one model's name.
///
/// Plain data with no invariant of its own: [`ModelCapabilities::new`] is what
/// refuses a figure that cannot be true. Held apart from the names because the
/// two halves are written down in different places and go stale at different
/// times — a name is the spelling a request has to carry, and these are the
/// numbers in that vendor's documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelLimits {
    /// The most one request may carry, in tokens.
    pub window: u32,
    /// The most one answer may produce, in tokens.
    pub output: u32,
    /// What the model reads.
    pub accepts: Modalities,
}

/// What one model accepts, produces and serves.
///
/// Owned rather than borrowed from a table compiled into this build, because a
/// provider registered at run time names models this build never heard of and
/// has to describe them in the same words the built-in ones are described in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCapabilities {
    name: Box<str>,
    shown: Box<str>,
    window: u32,
    output: u32,
    accepts: Modalities,
    rungs: Box<[Effort]>,
}

impl ModelCapabilities {
    /// Describes one model.
    ///
    /// `name` is the spelling a request carries, which is the vendor's own;
    /// `shown` is what a picker row calls it, where the product name and the
    /// wire identifier differ.
    ///
    /// # Errors
    ///
    /// [`ModelError`] where a name is empty or over its boundary, where a limit
    /// is zero, or where the rungs are not the ladder's order without a
    /// repeat. That last one is refused here rather than drawn: the panel puts
    /// faster on the left and smarter on the right, so a set written down out
    /// of order draws a track whose ends are wrong and says so nowhere.
    pub fn new(
        name: impl Into<Box<str>>,
        shown: impl Into<Box<str>>,
        limits: ModelLimits,
        rungs: impl Into<Box<[Effort]>>,
    ) -> Result<Self, ModelError> {
        let ModelLimits {
            window,
            output,
            accepts,
        } = limits;
        let name = name.into();
        bounded("model name", &name)?;
        let shown = shown.into();
        bounded("shown name", &shown)?;

        if window == 0 {
            return Err(ModelError::Nothing {
                model: name,
                field: "a context window",
            });
        }
        if output == 0 {
            return Err(ModelError::Nothing {
                model: name,
                field: "an output ceiling",
            });
        }

        let rungs = rungs.into();
        if !ascending(&rungs) {
            return Err(ModelError::Rungs { model: name, rungs });
        }

        Ok(Self {
            name,
            shown,
            window,
            output,
            accepts,
            rungs,
        })
    }

    /// The spelling a request carries.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What a picker row calls it.
    #[must_use]
    pub fn shown(&self) -> &str {
        &self.shown
    }

    /// The most one request may carry, in tokens.
    #[must_use]
    pub const fn window(&self) -> u32 {
        self.window
    }

    /// The most one answer may produce, in tokens.
    #[must_use]
    pub const fn output(&self) -> u32 {
        self.output
    }

    /// What the model reads.
    ///
    /// The model's half alone. What may actually be attached is this met with
    /// what the provider's wire module can spell, and the two drift apart.
    #[must_use]
    pub const fn accepts(&self) -> Modalities {
        self.accepts
    }

    /// The rungs of [`Effort`] it serves, weakest first.
    ///
    /// Empty is a model that serves none, which several do — not a model
    /// nothing is known about. That one has no record here at all.
    #[must_use]
    pub fn rungs(&self) -> &[Effort] {
        &self.rungs
    }

    /// The bytes this record keeps alive.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.name
            .len()
            .saturating_add(self.shown.len())
            .saturating_add(size_of_val(&*self.rungs))
    }
}

/// Whether the rungs are the ladder's own order with nothing repeated.
///
/// Strictly ascending, which is what makes this the uniqueness check as well: a
/// rung written down twice is a rung that stands still under an arrow key.
fn ascending(rungs: &[Effort]) -> bool {
    rungs.is_sorted_by(|here, next| here < next)
}

/// One retained spelling, held to [`MODEL_NAME_BYTES`].
fn bounded(field: &'static str, value: &str) -> Result<(), ModelError> {
    if value.is_empty() {
        return Err(ModelError::Empty { field });
    }
    if value.len() > MODEL_NAME_BYTES {
        return Err(ModelError::TooLong {
            field,
            maximum: MODEL_NAME_BYTES,
            actual: value.len(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
