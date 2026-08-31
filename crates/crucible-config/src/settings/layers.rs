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

use std::fs::File;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use crate::MAX_DOCUMENT_BYTES;
use crate::document::{Document, Origin};
use crate::error::ConfigError;
use crate::home::Home;

use super::Settings;

/// What a configuration file is called, in the home directory and in a project.
const FILE: &str = "config.json";

/// The directory a project keeps crucible's files in.
const PROJECT: &str = ".crucible";

/// The nearer project file for ordinary non-authority overrides.
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

            let opened = match File::open(&path) {
                Ok(opened) => opened,
                Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => return Err(ConfigError::Unreadable { file, source }),
            };
            let mut text = String::new();
            opened
                .take((MAX_DOCUMENT_BYTES + 1) as u64)
                .read_to_string(&mut text)
                .map_err(|source| ConfigError::Unreadable {
                    file: file.clone(),
                    source,
                })?;
            if text.len() > MAX_DOCUMENT_BYTES {
                return Err(ConfigError::TooLarge {
                    file,
                    maximum: MAX_DOCUMENT_BYTES,
                });
            }

            documents.push(Document::parse(&text, &file, origin)?);
        }

        Self::resolve_checked(documents)
    }
}

/// Where a project conventionally keeps its nearer non-authority settings.
///
/// Named here beside the layers it is read from so there is one answer to which
/// filename has that precedence. An ignore rule is a convention, not trust:
/// repositories can commit this file, so it cannot carry authority.
#[must_use]
pub fn local(workspace: &Path) -> PathBuf {
    workspace.join(PROJECT).join(LOCAL)
}

/// Where this machine keeps the settings that follow the person rather than the
/// checkout.
///
/// The other layer crucible itself writes to, and named here for the same
/// reason: which file a model chosen at the prompt lands in is the same fact as
/// which file it is read back from, and two answers to it would be a choice
/// written where nothing looks.
#[must_use]
pub fn user(home: &Home) -> PathBuf {
    home.path().join(FILE)
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
    use std::fs;

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
    fn the_local_file_still_holds_overrides_that_carry_no_authority() {
        // The local filename remains a nearer layer for ordinary preferences;
        // only settings that widen authority or select secrets are refused.
        let scratch = Scratch::new("layers-local");
        let path = local(scratch.root());

        fs::create_dir_all(path.parent().expect("a directory to write into"))
            .expect("a writable temporary directory");
        fs::write(&path, r#"{"providers": {"a": {"model": "local"}}}"#)
            .expect("a writable temporary directory");

        let settings = Settings::read(&home(&scratch), scratch.root()).expect("a layer it reads");

        assert_eq!(settings.model("a"), Some("local"));
    }

    #[test]
    fn a_file_that_is_not_a_document_is_refused_by_the_name_it_has_on_disk() {
        let scratch = Scratch::new("layers-bad");
        scratch.write(".crucible/config.json", r#"{"providers": 1}"#);

        let err = Settings::read(&home(&scratch), scratch.root()).unwrap_err();

        // The whole path, because two of the three files are called
        // `config.json` and one of them is somewhere else entirely. Joined
        // rather than written out, so this asks for the path the reader will be
        // shown rather than for the separator Unix happens to use.
        let named = Path::new(".crucible").join("config.json");
        let said = err.to_string();
        assert!(said.contains(&named.display().to_string()), "got {said}");
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

    #[test]
    fn a_configuration_file_over_the_byte_bound_is_refused_before_parsing() {
        let scratch = Scratch::new("layers-too-large");
        scratch.write(".crucible/config.json", &" ".repeat(MAX_DOCUMENT_BYTES + 1));

        let err = Settings::read(&home(&scratch), scratch.root()).unwrap_err();

        assert!(matches!(
            err,
            ConfigError::TooLarge {
                maximum: MAX_DOCUMENT_BYTES,
                ..
            }
        ));
    }
}
