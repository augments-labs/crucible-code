//! Configuration: the document, its layers, and the schema generated from them.
//!
//! Four layers decide a setting. Nearest wins:
//!
//! 1. the command line
//! 2. `.crucible/config.local.json` — git ignores it
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
//! one the user set. No block in this release is a list, so no rule for lists
//! is implemented; the rule it will take when the first one arrives is written
//! down in `docs/configuration.md` rather than left to whoever adds it.
//!
//! Two properties are structural here rather than documented:
//!
//! - **A key is a key exactly once.** `shape::DOCUMENT` declares the document,
//!   the parser walks values against it, and the schema is generated from it.
//!   There is no second list of key names to fall out of step.
//! - **The layer that travels cannot carry a secret.** `.crucible/config.json`
//!   is checked in, so the only variables it may set are crucible's own — the
//!   `CRUCIBLE_CODE_` namespace, whose meanings crucible fixes. Any other name
//!   is refused there outright rather than warned about, so an arbitrary value
//!   has no path into the one file that reaches everyone who clones.
//!
//! `0.0.x` is unstable: every key here may be renamed or removed in any `0.0.x`
//! release with no deprecation period.

mod check;
mod document;
mod env;
mod error;
mod schema;
mod settings;
mod shape;

pub use document::{Document, Origin};
pub use env::NAMESPACE;
pub use error::{Accepted, At, ConfigError};
pub use schema::schema;
pub use settings::{Color, Settings, ToolDetail};
