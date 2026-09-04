//! Concrete local sandbox composition.
//!
//! Core owns the policy and lifecycle contracts; this module owns the local
//! implementations that turn them into processes. Linux prefers a verified
//! system Bubblewrap executable. Every other platform, and an explicit user
//! compatibility mode, uses the same lifecycle wrapper while reporting that it
//! is not kernel confinement.
//!
//! [`conformance`] is published alongside them: it is what a backend outside
//! this tree has to answer before it may be selected here, and it asks the two
//! backends below exactly the same questions.

mod local;
pub(crate) mod process;

pub mod conformance;

#[cfg(target_os = "linux")]
mod linux;

pub use local::LocalSandbox;
