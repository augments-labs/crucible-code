//! A workspace, a sessions directory and a configuration file, all disposable.
//!
//! Shared by the tests either side of the wiring: what a startup does and what
//! the files it read said are two questions about the same temporary tree.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use crucible_auth::{Store, StoredCredentials};
use crucible_config::{Home, Settings};
use crucible_core::Workspace;

/// The key [`Sample::stored`] writes down.
///
/// Spelled differently from the one the tests export, because which of the two
/// signed a request is the whole of what a test about precedence can watch.
pub(super) const WRITTEN: &str = "a-written-key";

/// A tree under the system temporary directory, removed when this is dropped.
pub(super) struct Sample {
    base: PathBuf,
}

impl Sample {
    /// A tree of its own. `name` keeps two tests in one process apart.
    pub(super) fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("crucible-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("work")).expect("a temporary directory");

        Self { base }
    }

    /// Where sessions would go. Deliberately not created: whether a startup
    /// makes it is the thing being watched.
    pub(super) fn logs(&self) -> PathBuf {
        self.base.join("logs")
    }

    pub(super) fn workspace(&self) -> Workspace {
        Workspace::open(self.root()).expect("the directory exists")
    }

    /// The directory crucible would have been started in, for the tests that
    /// want the project's own files rather than a workspace.
    pub(super) fn root(&self) -> PathBuf {
        self.base.join("work")
    }

    /// The settings this workspace's own `.crucible/config.json` resolves to.
    ///
    /// Read through [`Settings::read`] rather than assembled by hand, so what a
    /// test hands the wiring is a document that went through the parser the same
    /// way a user's would.
    pub(super) fn settings(&self, document: &str) -> Settings {
        self.written("config.json", document)
    }

    /// The same, from the user-owned configuration file outside the workspace.
    pub(super) fn user(&self, document: &str) -> Settings {
        let file = self.user_file();
        fs::create_dir_all(file.parent().expect("a configuration directory"))
            .expect("a temporary directory");
        fs::write(file, document).expect("a temporary directory");

        self.read()
    }

    /// The user-owned configuration file in this disposable tree.
    pub(super) fn user_file(&self) -> PathBuf {
        self.base.join("home/config.json")
    }

    /// The store this tree holds, having been told `provider`'s key.
    ///
    /// Written through [`Store::keep`] and read back through [`Store::read`],
    /// rather than assembled: what a test hands the wiring is a file that went
    /// out and came back the way a real one does, which is the half of `/login`
    /// the wiring depends on and cannot see.
    pub(super) fn stored(&self, provider: &str) -> StoredCredentials {
        let store = self.store();
        store.keep(provider, WRITTEN).expect("a writable home");

        store.read()
    }

    /// This tree's home directory as crucible would find it, rather than
    /// whatever the machine running the test keeps in its own.
    fn found(&self) -> Home {
        Home::find(&|name: &str| {
            (name == crucible_config::HOME).then(|| OsString::from(self.base.join("home")))
        })
        .expect("an absolute path was given")
    }

    /// The store this tree keeps, which is the file `/login` writes and
    /// `/logout` takes a name back out of.
    pub(super) fn store(&self) -> Store {
        Store::in_home(&self.base.join("home"))
    }

    /// Resolves `document`, written as the project's `.crucible/<file>`.
    fn written(&self, file: &str, document: &str) -> Settings {
        let project = self.base.join("work").join(".crucible");
        fs::create_dir_all(&project).expect("a temporary directory");
        fs::write(project.join(file), document).expect("a temporary directory");

        self.read()
    }

    fn read(&self) -> Settings {
        Settings::read(&self.found(), &self.root()).expect("a document this test wrote")
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
