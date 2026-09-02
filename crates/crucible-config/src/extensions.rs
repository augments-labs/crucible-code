//! What is installed beside crucible, read without running any of it.
//!
//! Discovery is a sweep of one directory and a parse of each manifest in it.
//! Nothing here resolves an entrypoint, opens anything an extension names, or
//! starts a process: an extension is a file on disk that says what it would
//! like to be until something else decides to trust it.
//!
//! One broken installation does not hide the working ones. A directory that
//! cannot be read, a manifest that will not parse and an identifier claimed
//! twice are each collected as a [`Refusal`] beside whatever was read, because
//! the alternative is a startup that fails for the whole machine because one
//! extension's author shipped a trailing comma.
//!
//! Both boundaries here are about a directory nobody audited. The sweep looks
//! at [`MAX_EXTENSIONS`] directories, and each manifest is read to the boundary
//! `crucible-core` holds manifests to — so a planted tree of ten thousand
//! directories, or one manifest the size of a disk, costs a bounded amount of
//! startup rather than whatever it asked for.

use std::fs::{self, File};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use crucible_core::{EXTENSION_MANIFEST_BYTES, ExtensionError, ExtensionManifest, SourceKind};

use crate::home::Home;

/// The directory extensions are installed into, under the home directory.
const DIRECTORY: &str = "extensions";

/// What each installed directory keeps its manifest in.
const MANIFEST: &str = "manifest.json";

/// The most installed directories one sweep will look at.
///
/// Far beyond any machine that installs extensions by hand, and small enough
/// that a directory somebody filled turns into a bounded amount of startup work
/// rather than into however long it takes to parse whatever is there.
pub const MAX_EXTENSIONS: usize = 64;

/// Why one installed directory was not read.
///
/// Collected rather than returned: these are gathered beside the extensions
/// that were read, so that one broken installation is reported without hiding
/// the rest.
#[derive(Debug, thiserror::Error)]
pub enum Refusal {
    /// The directory or its manifest would not open.
    #[error("{file} could not be read: {source}")]
    Unreadable {
        /// The file, as the user would name it.
        file: Box<str>,
        /// What the operating system reported.
        source: io::Error,
    },

    /// The manifest opened and was not a manifest.
    #[error("{file}: {problem}")]
    Rejected {
        /// The file, as the user would name it.
        file: Box<str>,
        /// What was wrong with it.
        problem: ExtensionError,
    },

    /// A second directory claimed an identifier already read.
    #[error("{file} declares {id}, which {first} already declares")]
    Repeated {
        /// The directory that was refused.
        file: Box<str>,
        /// The one that keeps the identifier.
        first: Box<str>,
        /// The identifier both claim.
        id: Box<str>,
    },
}

/// One extension that was found and read.
///
/// The manifest and where it came from, held together because the identifier a
/// manifest states is the extension's own word for itself and the directory is
/// the machine's: somebody looking at two extensions with one name needs both.
#[derive(Debug)]
pub struct Installed {
    /// The manifest file, as the user would name it.
    file: Box<str>,
    /// What it said.
    manifest: ExtensionManifest,
}

impl Installed {
    /// The manifest file, as the user would name it.
    #[must_use]
    pub fn file(&self) -> &str {
        &self.file
    }

    /// What it said.
    #[must_use]
    pub const fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }
}

/// What one sweep of the extensions directory found.
#[derive(Debug)]
pub struct Extensions {
    /// The directory that was swept, whether or not it exists.
    at: PathBuf,
    /// What was read, in the order the directory names sort in.
    found: Vec<Installed>,
    /// What was not.
    refused: Vec<Refusal>,
    /// Whether the sweep stopped at [`MAX_EXTENSIONS`].
    stopped: bool,
}

impl Extensions {
    /// Sweeps the extensions directory under `home`.
    ///
    /// Never fails: a machine with no extensions directory is the ordinary
    /// case, and every other way this can go wrong is about one installation
    /// rather than about the run, so it is collected in [`Self::refused`]
    /// instead of ending startup.
    #[must_use]
    pub fn discover(home: &Home) -> Self {
        let at = home.path().join(DIRECTORY);
        let mut found: Vec<Installed> = Vec::new();
        let mut refused = Vec::new();

        let mut installed = match directories(&at) {
            Ok(installed) => installed,
            Err(source) if source.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(source) => {
                refused.push(Refusal::Unreadable {
                    file: named(&at),
                    source,
                });
                Vec::new()
            }
        };

        // Sorted before anything is read, because two things below depend on
        // the order being settled rather than being whatever the filesystem
        // handed back: which of two directories claiming one identifier keeps
        // it, and which are dropped when there are more than the sweep looks
        // at. Neither may change between two runs on the same disk.
        installed.sort();
        let stopped = installed.len() > MAX_EXTENSIONS;
        installed.truncate(MAX_EXTENSIONS);

        for directory in installed {
            let path = directory.join(MANIFEST);
            let file = named(&path);

            let text = match read(&path) {
                Ok(text) => text,
                Err(source) => {
                    refused.push(Refusal::Unreadable { file, source });
                    continue;
                }
            };

            match ExtensionManifest::parse(&text, SourceKind::Extension) {
                Ok(manifest) => {
                    if let Some(first) = found.iter().find(|one| one.manifest.id() == manifest.id())
                    {
                        refused.push(Refusal::Repeated {
                            file,
                            first: first.file.clone(),
                            id: manifest.id().into(),
                        });
                    } else {
                        found.push(Installed { file, manifest });
                    }
                }
                Err(problem) => refused.push(Refusal::Rejected { file, problem }),
            }
        }

        Self {
            at,
            found,
            refused,
            stopped,
        }
    }

    /// The directory that was swept, whether or not it exists.
    #[must_use]
    pub fn at(&self) -> &Path {
        &self.at
    }

    /// What was read, in the order the directory names sort in.
    #[must_use]
    pub fn found(&self) -> &[Installed] {
        &self.found
    }

    /// What was not, and why.
    #[must_use]
    pub fn refused(&self) -> &[Refusal] {
        &self.refused
    }

    /// Whether there were more directories than the sweep looks at.
    ///
    /// True means the answer is incomplete, which the listing has to say: a
    /// truncated sweep reported as a complete one is an extension that is
    /// installed, absent from the list, and impossible to explain.
    #[must_use]
    pub const fn stopped(&self) -> bool {
        self.stopped
    }
}

/// Every directory inside the extensions directory.
///
/// Directories only. A `README`, an archive left behind by an installer or a
/// file the desktop wrote is not a half-installed extension, and refusing one
/// would put a line in front of the user about a file that is doing no harm.
fn directories(at: &Path) -> io::Result<Vec<PathBuf>> {
    let mut directories = Vec::new();

    for entry in fs::read_dir(at)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            directories.push(entry.path());
        }
    }

    Ok(directories)
}

/// One manifest's text, held to the boundary manifests are held to.
///
/// One byte past the boundary is read rather than exactly it, so that a file
/// over the line arrives whole enough for `crucible-core` to refuse it by
/// length instead of arriving cut in half and being refused as broken JSON.
fn read(file: &Path) -> io::Result<String> {
    let opened = File::open(file)?;
    let mut text = String::new();
    opened
        .take(EXTENSION_MANIFEST_BYTES as u64 + 1)
        .read_to_string(&mut text)?;

    Ok(text)
}

/// A path, as the user would name it.
fn named(path: &Path) -> Box<str> {
    path.display().to_string().into()
}

#[cfg(test)]
mod tests;
