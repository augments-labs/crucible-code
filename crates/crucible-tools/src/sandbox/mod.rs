//! Concrete local sandbox composition.
//!
//! Core owns the policy and lifecycle contracts; this module owns the local
//! implementations that turn them into processes. Linux prefers a verified
//! system Bubblewrap executable. Every other platform, and an explicit user
//! compatibility mode, uses the same lifecycle wrapper while reporting that it
//! is not kernel confinement.

mod local;
pub(crate) mod process;

#[cfg(test)]
mod conformance;

#[cfg(target_os = "linux")]
mod linux;

pub use local::LocalSandbox;
