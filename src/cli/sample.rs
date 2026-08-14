//! A workspace, a sessions directory and a configuration file, all disposable.
//!
//! Shared by the tests either side of the wiring: what a startup does and what
//! the files it read said are two questions about the same temporary tree.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use crucible_auth::{Keys, Store};
use crucible_config::{Home, Settings};
use crucible_core::{Ask, Mode, Remember, Sensitivity, Settled, ToolCall, Verdict, Workspace};

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

    /// The same, from `.crucible/config.local.json` instead.
    ///
    /// The file git ignores, which is where the keys that could loosen what
    /// crucible does unasked are allowed to be written. A test about one of
    /// those cannot use the file above: the parser refuses it there, which is
    /// the property, and a test that met the refusal would be testing it twice
    /// rather than testing what the setting does.
    pub(super) fn local(&self, document: &str) -> Settings {
        self.written("config.local.json", document)
    }

    /// The store this tree holds, having been told `provider`'s key.
    ///
    /// Written through [`Store::keep`] and read back through [`Store::read`],
    /// rather than assembled: what a test hands the wiring is a file that went
    /// out and came back the way a real one does, which is the half of `/login`
    /// the wiring depends on and cannot see.
    pub(super) fn stored(&self, provider: &str) -> Keys {
        let store = Store::in_home(&self.base.join("home"));
        store.keep(provider, WRITTEN).expect("a writable home");

        store.read()
    }

    /// Resolves `document`, written as the project's `.crucible/<file>`.
    fn written(&self, file: &str, document: &str) -> Settings {
        let project = self.base.join("work").join(".crucible");
        fs::create_dir_all(&project).expect("a temporary directory");
        fs::write(project.join(file), document).expect("a temporary directory");

        // Pointed at a directory that does not exist, so the user layer is
        // absent rather than whatever the machine running this test has at home.
        let home = Home::find(&|name: &str| {
            (name == crucible_config::HOME).then(|| OsString::from(self.base.join("home")))
        })
        .expect("an absolute path was given");

        Settings::read(&home, &self.base.join("work")).expect("a document this test wrote")
    }

    /// What the permission engine makes of one call, with this tree's files
    /// read from the start again.
    ///
    /// The question a rule written down has to answer is not what the text of
    /// the file says but what the next crucible to start does with it, and
    /// starting is what reads these files. Nobody is here to be asked, so a
    /// call that still reaches the user comes back refused — which is how a
    /// rule that landed is told from one that did not.
    pub(super) fn settles(&self, call: &ToolCall, sensitivity: &Sensitivity) -> Settled {
        struct Nobody;

        impl Ask for Nobody {
            fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
                (Verdict::Deny, Remember::Never)
            }
        }

        // Pointed at a directory that does not exist, so nothing configured on
        // the machine running this test can allow anything.
        let home = Home::find(&|name: &str| {
            (name == crucible_config::HOME).then(|| OsString::from(self.base.join("home")))
        })
        .expect("an absolute path was given");

        Settings::read(&home, &self.root())
            .expect("a file crucible wrote")
            .permission(Mode::Ask)
            .decide(call, sensitivity, &mut Nobody)
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
