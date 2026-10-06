//! Release discovery, the cached release check and the installed release's
//! receipt.
//!
//! This crate owns two deliberately separate operations. [`UpdateCrateReleaseCheck::cached`]
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
//! On Unix it also reads the receipt the shell installer leaves in each
//! release it installs. [`ReceiptLayout::of_executable`] takes the layout the
//! running executable belongs to only once every entry of it, the receipt and
//! the recorded hashes agree, and reads nothing over the network.

#[cfg(unix)]
mod install;
mod release;

#[cfg(unix)]
pub use install::{
    Digest, Installation, LayoutError, Receipt, ReceiptError, ReceiptLayout, Target, Version,
};
pub use release::{Newer, SHUTDOWN, Unjoined, UpdateCrateReleaseCheck, newer};
