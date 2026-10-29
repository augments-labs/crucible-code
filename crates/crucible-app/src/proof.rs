//! What a test build runs on in place of what ships.
//!
//! Compiled only with the `proof` feature, which nothing but a dev-dependency
//! turns on, so a build that ships has none of it. A test of the whole program
//! steers the binary it runs with two variables declared and read here and
//! nowhere else, and with both unset the binary does what ships.
//!
//! The first chooses the agents the binary can start: `fake` lists every agent
//! this build knows how to run as though this release shipped it, and `empty`
//! lists none. The second names a catalog of agents the test serves itself, as
//! its address on this machine's loopback and the one key the test signed it
//! with, apart by a space. It
//! takes effect where the published catalog's source is built, so every fetch
//! of the catalog in that process asks the test's: the one an agents command
//! makes, and the one beside the release check where that is armed. Either
//! variable set to a value it does not take refuses the start, naming it.
//!
//! A test that calls something taking a registry hands it [`registry`] over
//! whatever rows it needs. [`ACCOUNTS`] is what a client driving an installed
//! agent is asked and told, step by step, for every suite that drives one to
//! assert against.

pub mod accounts;
pub use accounts::ACCOUNTS;

use std::ffi::OsStr;

use crucible_external_harnesses::catalog::Keys;

use crate::backends::{self, REGISTRY, Registry, Row};
use crate::{AppError, Conversation};

/// The variable a test sets to choose the agents a test build can start.
const VARIABLE: &str = "CRUCIBLE_CODE_PROOF_REGISTRY";

/// The variable a test sets to name a catalog it serves and signed.
pub const CATALOG: &str = "CRUCIBLE_CODE_PROOF_CATALOG";

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

/// A catalog a test serves, and the key it signed it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    /// The address its `index.json` and `index.json.minisig` are under.
    pub base: String,
    /// The base64 line of the minisign public key it verifies under.
    pub key: String,
}

impl Served {
    /// The one key the catalog verifies under, kept for the rest of the
    /// process.
    #[must_use]
    pub fn keys(&self) -> Keys {
        let key: &'static str = Box::leak(self.key.clone().into_boxed_str());
        Keys::new(Box::leak(Box::new([key])))
    }
}

/// The catalog `value` names, and none where there is no value.
///
/// # Errors
///
/// [`AppError::ProofCatalog`] where `value` is not an `http` or `https`
/// address on this machine's loopback (`127.0.0.1`, `[::1]` or `localhost`)
/// and a key, apart by one space, each with no space of its own.
pub fn catalog_from(value: Option<&OsStr>) -> Result<Option<Served>, AppError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let refused = || AppError::ProofCatalog {
        value: value.to_string_lossy().into(),
    };
    let (base, key) = value
        .to_str()
        .and_then(|text| text.split_once(' '))
        .ok_or_else(refused)?;
    if !looped(base) || key.is_empty() || key.contains(' ') {
        return Err(refused());
    }
    Ok(Some(Served {
        base: base.to_owned(),
        key: key.to_owned(),
    }))
}

/// The hosts a catalog a test serves can be named at: this machine's loopback,
/// so a variable set by anything else cannot point a test build at the network.
const LOOPBACK: [&str; 3] = ["127.0.0.1", "[::1]", "localhost"];

/// Whether `base` is an `http` or `https` address whose host is one of
/// [`LOOPBACK`], written exactly so, with no port or a port of digits alone.
///
/// Anything else between the host and the path, a user's name or a second
/// host among it, is refused rather than read the way a URL parser would.
fn looped(base: &str) -> bool {
    let Some(rest) = base
        .strip_prefix("http://")
        .or_else(|| base.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    LOOPBACK
        .iter()
        .filter_map(|host| authority.strip_prefix(host))
        .any(|port| {
            port.is_empty()
                || port.strip_prefix(':').is_some_and(|digits| {
                    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
                })
        })
}

/// The catalog this process's environment names, as [`catalog_from`] reads
/// it.
///
/// # Errors
///
/// As [`catalog_from`].
pub fn catalog_from_environment() -> Result<Option<Served>, AppError> {
    catalog_from(std::env::var_os(CATALOG).as_deref())
}

/// The catalog `from` names, where it names one that [`catalog_from`] takes.
///
/// One it does not take is no catalog here, since the start was refused over
/// it before anything fetched.
pub(crate) fn catalog(from: &dyn Fn(&str) -> Option<String>) -> Option<Served> {
    let value = from(CATALOG)?;
    catalog_from(Some(OsStr::new(&value))).ok().flatten()
}

#[cfg(test)]
mod tests;
