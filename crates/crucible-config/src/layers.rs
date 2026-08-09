//! The files a setting can come from, and the order they are read in.
//!
//! Three, nearest last, so that merging them in order leaves the nearest layer
//! holding what it set. The command line is the fourth and nearest layer and is
//! not a file, so it is applied by the wiring above rather than here.
//!
//! A file that is not there is not an error — most machines have none of these
//! and crucible has to run anyway. A file that *is* there and cannot be read is
//! a different thing and says so: silently skipping it would turn a permissions
//! mistake into settings that mysteriously stopped applying.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::document::{Document, Origin};
use crate::error::ConfigError;
use crate::home::Home;
use crate::settings::Settings;

/// What a configuration file is called, in the home directory and in a project.
const FILE: &str = "config.json";

/// The directory a project keeps crucible's files in.
const PROJECT: &str = ".crucible";

/// The project file git ignores, so it can hold what the other one may not.
const LOCAL: &str = "config.local.json";

impl Settings {
    /// Reads whichever of the three files exist and resolves them.
    ///
    /// `workspace` is the directory crucible was started in, which is what makes
    /// a project's settings a property of the checkout rather than of the shell
    /// that launched it.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Unreadable`] for a file that is there and will not open,
    /// and one of the document errors for a file that opens and is not a
    /// configuration document.
    pub fn read(home: &Home, workspace: &Path) -> Result<Self, ConfigError> {
        let mut documents = Vec::new();

        for (path, origin) in files(home, workspace) {
            // Named by its whole path. Two of these live in the project and one
            // does not, so `config.json` alone would leave the reader working
            // out which file the message is about.
            let file: Box<str> = path.display().to_string().into();

            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => return Err(ConfigError::Unreadable { file, source }),
            };

            documents.push(Document::parse(&text, &file, origin)?);
        }

        Ok(Self::resolve(documents))
    }
}

/// The three files, in the order they merge: furthest first.
fn files(home: &Home, workspace: &Path) -> [(PathBuf, Origin); 3] {
    let project = workspace.join(PROJECT);

    [
        (home.path().join(FILE), Origin::User),
        (project.join(FILE), Origin::Project),
        (project.join(LOCAL), Origin::ProjectLocal),
    ]
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use crate::sample::Scratch;

    /// A home directory inside the scratch tree, found the way the binary finds
    /// one.
    fn home(scratch: &Scratch) -> Home {
        let named = scratch.text("home");
        Home::find(&move |wanted| (wanted == crate::HOME).then(|| OsString::from(named.clone())))
            .expect("an absolute path")
    }

    #[test]
    fn a_machine_with_no_configuration_files_at_all_is_not_an_error() {
        // The common case, and the one that must not need a file to exist:
        // crucible is usable before anybody has configured anything.
        let scratch = Scratch::new("layers-none");

        let settings = Settings::read(&home(&scratch), scratch.root()).expect("nothing to read");

        assert_eq!(settings.model("anthropic"), None);
    }

    #[test]
    fn each_layer_takes_the_one_nearer_the_work() {
        let scratch = Scratch::new("layers-order");
        scratch.write(
            "home/config.json",
            r#"{"providers":{"a":{"model":"user"}}}"#,
        );

        let settings = Settings::read(&home(&scratch), scratch.root()).expect("one file");
        assert_eq!(settings.model("a"), Some("user"));

        scratch.write(
            ".crucible/config.json",
            r#"{"providers":{"a":{"model":"project"}}}"#,
        );
        let settings = Settings::read(&home(&scratch), scratch.root()).expect("two files");
        assert_eq!(settings.model("a"), Some("project"));

        scratch.write(
            ".crucible/config.local.json",
            r#"{"providers":{"a":{"model":"local"}}}"#,
        );
        let settings = Settings::read(&home(&scratch), scratch.root()).expect("three files");
        assert_eq!(settings.model("a"), Some("local"));
    }

    #[test]
    fn a_file_that_is_not_a_document_is_refused_by_the_name_it_has_on_disk() {
        let scratch = Scratch::new("layers-bad");
        scratch.write(".crucible/config.json", r#"{"providers": 1}"#);

        let err = Settings::read(&home(&scratch), scratch.root()).unwrap_err();

        // The whole path, because two of the three files are called
        // `config.json` and one of them is somewhere else entirely.
        let said = err.to_string();
        assert!(said.contains(".crucible/config.json"), "got {said}");
        assert!(said.contains("providers"), "got {said}");
    }

    #[test]
    fn a_file_that_is_there_and_will_not_open_is_reported_rather_than_skipped() {
        // A directory where a file should be: present, so not the missing-file
        // case, and unreadable for a reason nobody would guess from settings
        // that simply stopped applying.
        let scratch = Scratch::new("layers-shut");
        scratch.make(".crucible/config.local.json");

        let err = Settings::read(&home(&scratch), scratch.root()).unwrap_err();

        assert!(matches!(err, ConfigError::Unreadable { .. }), "got {err:?}");
    }
}
