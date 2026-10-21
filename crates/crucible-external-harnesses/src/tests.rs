//! What supervising another vendor's agent has to guarantee, whichever agent
//! it is.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "proof")]
mod fake_peer;
mod host;
mod launch;
mod lifecycle;
mod pending;

/// A directory of the test's own, removed when it is dropped.
pub(crate) struct Owned(pub(crate) PathBuf);

impl Owned {
    // A test with nowhere to write has nothing to report.
    #[allow(clippy::expect_used)]
    pub(crate) fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let at = std::env::temp_dir().join(format!(
            "crucible-harness-{name}-{unique}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&at).expect("a directory");
        Self(fs::canonicalize(&at).expect("a canonical directory"))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
