//! A workspace, a sessions directory and a configuration file, all disposable.
//!
//! Shared by the tests either side of the wiring: what a startup does and what
//! the files it read said are two questions about the same temporary tree.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use crucible_auth::{Store, StoredCredentials};
use crucible_config::{Extensions, Home, Settings};
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

    /// The disposable user-home root handed to state stores.
    pub(super) fn home(&self) -> PathBuf {
        self.base.join("home")
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

    /// The store this tree holds, with a completed subscription login for
    /// `provider`.
    ///
    /// Written as a file at this wiring test boundary because the auth crate
    /// deliberately exposes no token constructor. The value is inert and far
    /// from expiry; tests can therefore prove endpoint and precedence wiring
    /// without a network request or a readable credential API.
    pub(super) fn subscribed(&self, provider: &str) -> StoredCredentials {
        let home = self.base.join("home");
        fs::create_dir_all(&home).expect("a temporary home");
        let details = if provider == "moonshot" {
            r#"{"device_id":"01234567-89ab-4cde-8fab-0123456789ab","expires_in":"3600"}"#
        } else {
            r#"{"account_id":"test-account"}"#
        };
        fs::write(
            home.join("auth.json"),
            format!(
                r#"{{"version":2,"keys":{{}},"subscriptions":{{"{provider}":{{"access_token":"test-access","refresh_token":"test-refresh","details":{details},"expires_at":18446744073709551615,"refreshed_at":1}}}},"identities":{{}}}}"#
            ),
        )
        .expect("a writable store");

        self.store().read()
    }

    /// Writes one extension's manifest into this tree's home directory.
    ///
    /// A real file in a real directory, because discovery's whole job is what
    /// is on disk and where: a fixture that handed the listing a manifest it
    /// had built in memory would be testing the fixture's answer to the
    /// question the sweep exists to ask.
    pub(super) fn installed(&self, directory: &str, manifest: &str) {
        let at = self.base.join("home").join("extensions").join(directory);
        fs::create_dir_all(&at).expect("a temporary directory");
        fs::write(at.join("manifest.json"), manifest).expect("a temporary directory");
    }

    /// What a sweep of this tree's home directory finds.
    pub(super) fn discovered(&self) -> Extensions {
        Extensions::discover(&self.found())
    }

    /// Writes this tree's home configuration file.
    pub(super) fn configured(&self, document: &str) {
        fs::create_dir_all(self.home()).expect("a temporary directory");
        fs::write(self.user_file(), document).expect("a temporary directory");
    }

    /// What this tree's home file decided, and nothing the checkout said.
    pub(super) fn decided(&self) -> Settings {
        Settings::read_home(&self.found()).expect("a readable home file")
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
