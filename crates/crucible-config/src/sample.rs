//! Real files on a real disk, for the tests that are about finding them.
//!
//! This crate's job is where files are and what is in them, so a fake
//! filesystem would only be testing the fake's answer to those questions. Each
//! fixture gets its own directory under the system temporary directory and
//! removes it when it drops.

use std::fs;
use std::path::{Path, PathBuf};

/// A tree that exists while the test does.
pub(crate) struct Scratch {
    base: PathBuf,
}

impl Scratch {
    /// A fresh, empty tree. `name` only has to be unique within this crate's
    /// tests, which run in one process.
    pub(crate) fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("crucible-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("a writable temporary directory");

        Self { base }
    }

    /// Creates a directory inside it, and returns the whole path.
    pub(crate) fn make(&self, at: &str) -> PathBuf {
        let path = self.at(at);
        fs::create_dir_all(&path).expect("a writable temporary directory");
        path
    }

    /// Writes a file inside it, creating the directories above it.
    pub(crate) fn write(&self, at: &str, text: &str) {
        let path = self.at(at);
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory).expect("a writable temporary directory");
        }
        fs::write(path, text).expect("a writable temporary directory");
    }

    /// A path inside it, whether or not anything is there.
    pub(crate) fn at(&self, at: &str) -> PathBuf {
        self.base.join(at)
    }

    /// The same, as an environment variable would carry it.
    pub(crate) fn text(&self, at: &str) -> String {
        self.at(at).display().to_string()
    }

    /// The tree's own root, for the tests that hand it over as a workspace.
    pub(crate) fn root(&self) -> &Path {
        &self.base
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
