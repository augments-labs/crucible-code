//! Release discovery, the cached release check and the installed release's
//! receipt.
//!
//! This crate owns two deliberately separate release-check operations. [`UpdateCrateReleaseCheck::cached`]
//! reads the small answer left by an earlier check and is safe on the startup
//! path: it opens no socket and starts no runtime work. [`UpdateCrateReleaseCheck::refresh`]
//! is the separate caller intent that may ask GitHub for a newer release; it
//! runs as a task held by the owner and is asked to stop, then aborted within
//! [`SHUTDOWN`] when the run ends.
//!
//! The HTTP client used by the check has its own pool over an unpoisoned
//! resolver. Its TLS configuration, proxy settings and plain lookup owner are
//! built once and can be lent to the application's other HTTP client, so
//! separate pools do not become separate trust or environment decisions.
//!
//! On Unix it also reads the versioned layout an installer-managed release
//! lives in: the layout the running executable belongs to is taken only once
//! its directories and files, its receipt and the recorded hashes agree, and
//! nothing about it is read over the network. A release's archive, once it is
//! the one its `SHA256SUMS` lists, is staged into that layout as a unit beside
//! the active one, which staging never changes. Under the install's lock the
//! staged unit is checked again and made active by one rename, and the unit
//! that was active is kept so the switch can be rolled back.

mod command;
#[cfg(unix)]
mod install;
mod release;
mod version;

pub use command::{Answer, Asked, Refused, SOURCE, SelfUpdateCommand};
#[cfg(unix)]
pub use install::{
    Activated, ActivationError, ActivationStep, Digest, EntryKind, Installation, LayoutEntry,
    LayoutError, Receipt, ReceiptClaim, ReceiptError, ReceiptLayout, RecoverableActivation,
    StageError, StagePart, StagedUnit, Target, UnitRole,
};
pub use release::{Newer, SHUTDOWN, Unjoined, UpdateCrateReleaseCheck, newer};
pub use version::Version;
