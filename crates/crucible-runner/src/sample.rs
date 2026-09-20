//! A workspace with a real directory behind it, made and removed per test.
//!
//! An attachment is resolved by reading the file again, so a fake filesystem
//! would test something other than what runs.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use crucible_core::Workspace;

/// Tells apart two samples that were given the same name.
static NTH: AtomicU32 = AtomicU32::new(0);

/// Something for a test's attachments and paths to be about.
pub(crate) struct Sample {
    base: PathBuf,
}

impl Sample {
    /// Makes the directories, discarding whatever an earlier run left.
    ///
    /// The counter is what stops two tests that happened to pick the same name
    /// from sharing a path: the tests run at once, this constructor deletes what
    /// it finds, and the loser sees its fixtures vanish mid-test.
    pub(crate) fn new(name: &str) -> Self {
        let nth = NTH.fetch_add(1, Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("crucible-{name}-{}-{nth}", std::process::id()));
        let _ = fs::remove_dir_all(&base);

        fs::create_dir_all(base.join("work")).expect("a temporary directory");

        Self { base }
    }

    /// The workspace a session is about.
    pub(crate) fn workspace(&self) -> Workspace {
        Workspace::open(self.base.join("work")).expect("the directory exists")
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
