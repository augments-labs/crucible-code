//! Configuration: where crucible's own files are, what they may say, and which
//! of them wins.
//!
//! [`Home`] answers the first question and is the only place it is asked;
//! everything else in this workspace is handed a path. The rest of the crate
//! answers the second and third.
//!
//! Four layers decide a setting. Nearest wins:
//!
//! 1. the command line
//! 2. `.crucible/config.local.json` — conventionally ignored
//! 3. `.crucible/config.json` — checked in, read by everyone who clones
//! 4. the configuration file in the user's home directory
//!
//! The command line is not a document and is applied by the wiring above; this
//! crate resolves the three files.
//!
//! Merging is decided per block rather than by a general deep merge, because
//! the rules are not the same for every kind of value. A scalar takes the
//! nearest layer that set it. A map — `providers`, `env` — is merged key by
//! key, so a project can name one provider's model without erasing the other
//! one the user set. A list — the permission rules, the extra directories — is
//! concatenated, so a nearer layer adds entries and removes none. All three are
//! in `docs/configuration/configuration.md`.
//!
//! Two properties are structural here rather than documented:
//!
//! - **A key is a key exactly once.** `shape::DOCUMENT` declares the document,
//!   the parser walks values against it, and the schema is generated from it.
//!   There is no second list of key names to fall out of step.
//! - **A workspace layer cannot carry authority or choose a secret.** Either
//!   project filename can be committed, whatever its conventional ignore rule
//!   says. The only variables either may set are crucible's own, and keys that
//!   loosen permissions or select a credential source are read only from the
//!   user's file outside the checkout.
//!
//! `0.x` is unstable: every key here may be renamed or removed in any `0.x`
//! release with no deprecation period.

/// The greatest one configuration document may be.
///
/// A MiB is far beyond the shape a human-maintained settings document needs,
/// while preventing a planted project file from choosing startup memory. The
/// binary uses the same boundary before handing a document back here to splice.
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

mod document;
mod env;
mod error;
mod home;
mod remember;
#[cfg(test)]
mod sample;
mod settings;
mod shape;

// A document is not on this surface. [`Settings::read`] is the one way in, so
// the type that parses one file and the type that resolves several are both
// crate-private: a caller who could build a `Document` would then have nothing
// to do with it, and a second way in is a second answer to which files exist.
pub use error::{Accepted, At, ConfigError};
pub use home::{HOME, Home};
pub use remember::{allowing, asking, choosing, drawing, reading, thinking};
pub use settings::{
    ClearScreen, Color, Glyphs, Mouse, Settings, ThemeChoice, ToolDetail, Updates, local, user,
};
pub use shape::THEME;
pub use shape::schema::schema;
