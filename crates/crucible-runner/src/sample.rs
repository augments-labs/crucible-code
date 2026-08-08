//! A sessions directory and a workspace, both real, both temporary.
//!
//! Sessions are read back from a file by a thread that is not the one that
//! wrote them, so a fake filesystem would test something other than what runs.

use std::fs;
use std::path::PathBuf;

use crucible_core::Workspace;

/// Somewhere for a test to keep sessions, and something for them to be about.
pub(crate) struct Sample {
    base: PathBuf,
}

impl Sample {
    /// Makes the directories, discarding whatever an earlier run left.
    pub(crate) fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("crucible-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);

        for under in ["logs", "work", "elsewhere"] {
            fs::create_dir_all(base.join(under)).expect("a temporary directory");
        }

        Self { base }
    }

    /// Where session logs go.
    pub(crate) fn logs(&self) -> PathBuf {
        self.base.join("logs")
    }

    /// The workspace a session is about.
    pub(crate) fn workspace(&self) -> Workspace {
        Workspace::open(self.base.join("work")).expect("the directory exists")
    }

    /// A second workspace, for what must not be offered to the first.
    pub(crate) fn elsewhere(&self) -> Workspace {
        Workspace::open(self.base.join("elsewhere")).expect("the directory exists")
    }

    /// A log written by hand, for the shapes a running session cannot produce.
    pub(crate) fn plant(&self, id: &str, lines: &[String]) -> PathBuf {
        let path = self.logs().join(format!("{id}.jsonl"));
        fs::write(&path, lines.join("\n") + "\n").expect("a writable temporary directory");
        path
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
