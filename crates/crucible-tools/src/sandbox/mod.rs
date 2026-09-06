//! Concrete local sandbox composition.
//!
//! Core owns the policy and lifecycle contracts; this module owns the local
//! implementations that turn them into processes. Linux uses a verified system
//! Bubblewrap executable and macOS uses the system Seatbelt launcher. Platforms
//! without a native backend, and an explicit user compatibility mode, use the
//! same lifecycle wrapper while reporting that it is not kernel confinement.
//!
//! [`conformance`] is published alongside them: it is what a backend outside
//! this tree has to answer before it may be selected here, and it asks the two
//! backends below exactly the same questions.

mod local;
pub(crate) mod process;

pub mod conformance;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

pub use local::LocalSandbox;
