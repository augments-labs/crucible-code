//! What a test build runs on in place of what ships.
//!
//! Compiled only with the `proof` feature, which nothing but a dev-dependency
//! turns on, so a build that ships has none of it. A test of the whole program
//! chooses the agents the binary it runs can start with one variable read here
//! and nowhere else: `fake` lists every agent this build knows how to run as
//! though this release shipped it, `empty` lists none, and with the variable
//! unset the binary starts what ships. A test that calls something taking a
//! registry hands it [`registry`] over whatever rows it needs.

use std::ffi::OsStr;

use crate::backends::{self, REGISTRY, Registry, Row};
use crate::{AppError, Conversation};

/// The variable a test sets to choose the agents a test build can start.
const VARIABLE: &str = "CRUCIBLE_CODE_PROOF_REGISTRY";

/// Every agent this build knows how to run, each as though this release
/// shipped it.
pub static FAKE_ROWS: [Row; 2] = [
    Row {
        shipping: true,
        ..backends::CLAUDE
    },
    Row {
        shipping: true,
        ..backends::ANTIGRAVITY
    },
];

/// The registry `fake` names.
static FAKE: Registry = Registry::new(&FAKE_ROWS);

/// The registry `empty` names.
static EMPTY: Registry = Registry::new(&[]);

/// A registry over `rows`, kept for the rest of the process.
#[must_use]
pub fn registry(rows: &'static [Row]) -> &'static Registry {
    Box::leak(Box::new(Registry::new(rows)))
}

/// `conversation`, its installed agents those `agents` lists, as a run whose
/// start was settled against them holds.
#[must_use]
pub fn agents(conversation: Conversation, agents: &'static Registry) -> Conversation {
    conversation.registry(agents)
}

/// The registry `value` names, and the one that ships where there is none.
///
/// # Errors
///
/// [`AppError::ProofRegistry`] where `value` is neither `fake` nor `empty`.
pub fn registry_from(value: Option<&OsStr>) -> Result<&'static Registry, AppError> {
    let Some(value) = value else {
        return Ok(&REGISTRY);
    };
    match value.to_str() {
        Some("fake") => Ok(&FAKE),
        Some("empty") => Ok(&EMPTY),
        _ => Err(AppError::ProofRegistry {
            value: value.to_string_lossy().into(),
        }),
    }
}

/// The registry this process's environment names, as [`registry_from`]
/// reads it.
///
/// # Errors
///
/// As [`registry_from`].
pub fn registry_from_environment() -> Result<&'static Registry, AppError> {
    registry_from(std::env::var_os(VARIABLE).as_deref())
}

#[cfg(test)]
mod tests;
