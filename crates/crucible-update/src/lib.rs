//! Release discovery and the cached release check.
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

mod release;

pub use release::{Newer, SHUTDOWN, Unjoined, UpdateCrateReleaseCheck, newer};
