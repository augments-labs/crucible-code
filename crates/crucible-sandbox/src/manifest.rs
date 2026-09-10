//! Bounded inert workspace materialization records.

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use super::policy::{MAX_SANDBOX_PATH_BYTES, SandboxFilesystemAccess, SandboxFilesystemProvenance};

/// Maximum entries materialized for one sandbox.
pub const MAX_SANDBOX_MANIFEST_ENTRIES: usize = 256;

/// Maximum bytes in one inline file.
pub const MAX_SANDBOX_MANIFEST_FILE_BYTES: usize = 256 * 1024;

/// Maximum aggregate inline bytes in one manifest.
pub const MAX_SANDBOX_MANIFEST_BYTES: usize = 1024 * 1024;

/// One inert materialization request.
#[derive(Clone, PartialEq, Eq)]
pub enum SandboxManifestEntry {
    /// A regular non-executable file supplied as bounded bytes.
    File {
        /// Relative destination below the materialization root.
        destination: PathBuf,
        /// Bytes copied before untrusted code starts.
        contents: Box<[u8]>,
        /// Non-sensitive authority source.
        provenance: SandboxFilesystemProvenance,
    },
    /// A directory created before files are committed.
    Directory {
        /// Relative destination below the materialization root.
        destination: PathBuf,
        /// Non-sensitive authority source.
        provenance: SandboxFilesystemProvenance,
    },
    /// An exact host source mounted at a relative sandbox destination.
    Mount {
        /// Absolute source, canonicalized again by the host at materialization.
        source: PathBuf,
        /// Relative destination below the materialization root.
        destination: PathBuf,
        /// Read-only or writable grant.
        access: SandboxFilesystemAccess,
        /// Non-sensitive authority source.
        provenance: SandboxFilesystemProvenance,
    },
}

impl SandboxManifestEntry {
    /// Builds one bounded, non-executable inline file.
    ///
    /// # Errors
    ///
    /// Unsafe destinations and oversized contents are rejected.
    pub fn file(
        destination: impl Into<PathBuf>,
        contents: impl Into<Box<[u8]>>,
        provenance: SandboxFilesystemProvenance,
    ) -> Result<Self, SandboxManifestError> {
        let destination = destination.into();
        validate_destination(&destination)?;
        let contents = contents.into();
        if contents.len() > MAX_SANDBOX_MANIFEST_FILE_BYTES {
            return Err(SandboxManifestError::FileTooLarge);
        }
        Ok(Self::File {
            destination,
            contents,
            provenance,
        })
    }

    /// Builds one bounded directory entry.
    ///
    /// # Errors
    ///
    /// Unsafe destinations are rejected.
    pub fn directory(
        destination: impl Into<PathBuf>,
        provenance: SandboxFilesystemProvenance,
    ) -> Result<Self, SandboxManifestError> {
        let destination = destination.into();
        validate_destination(&destination)?;
        Ok(Self::Directory {
            destination,
            provenance,
        })
    }

    /// Builds one exact host mount request.
    ///
    /// This is still inert: construction does not inspect or open the source.
    /// The host-owned materializer canonicalizes and verifies it immediately
    /// before committing a descriptor-backed mount.
    ///
    /// # Errors
    ///
    /// Relative/host-root sources, unsafe destinations, and non-mount access
    /// modes are rejected.
    pub fn mount(
        source: impl Into<PathBuf>,
        destination: impl Into<PathBuf>,
        access: SandboxFilesystemAccess,
        provenance: SandboxFilesystemProvenance,
    ) -> Result<Self, SandboxManifestError> {
        let source = source.into();
        let destination = destination.into();
        validate_source(&source)?;
        validate_destination(&destination)?;
        if !matches!(
            access,
            SandboxFilesystemAccess::ReadOnly | SandboxFilesystemAccess::ReadWrite
        ) {
            return Err(SandboxManifestError::InvalidMountAccess);
        }
        Ok(Self::Mount {
            source,
            destination,
            access,
            provenance,
        })
    }

    /// Relative destination below the fixed materialization root.
    #[must_use]
    pub fn destination(&self) -> &Path {
        match self {
            Self::File { destination, .. }
            | Self::Directory { destination, .. }
            | Self::Mount { destination, .. } => destination,
        }
    }

    /// Inline contents where this is a file.
    #[must_use]
    pub fn contents(&self) -> Option<&[u8]> {
        match self {
            Self::File { contents, .. } => Some(contents),
            Self::Directory { .. } | Self::Mount { .. } => None,
        }
    }

    /// Host source where this is a mount.
    #[must_use]
    pub fn source(&self) -> Option<&Path> {
        match self {
            Self::Mount { source, .. } => Some(source),
            Self::File { .. } | Self::Directory { .. } => None,
        }
    }

    /// Requested mount access, where applicable.
    #[must_use]
    pub const fn access(&self) -> Option<SandboxFilesystemAccess> {
        match self {
            Self::Mount { access, .. } => Some(*access),
            Self::File { .. } | Self::Directory { .. } => None,
        }
    }

    /// Non-sensitive authority source.
    #[must_use]
    pub const fn provenance(&self) -> SandboxFilesystemProvenance {
        match self {
            Self::File { provenance, .. }
            | Self::Directory { provenance, .. }
            | Self::Mount { provenance, .. } => *provenance,
        }
    }
}

impl std::fmt::Debug for SandboxManifestEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File {
                contents,
                provenance,
                ..
            } => f
                .debug_struct("File")
                .field("destination", &"[relative path]")
                .field("bytes", &contents.len())
                .field("provenance", provenance)
                .finish(),
            Self::Directory { provenance, .. } => f
                .debug_struct("Directory")
                .field("destination", &"[relative path]")
                .field("provenance", provenance)
                .finish(),
            Self::Mount {
                access, provenance, ..
            } => f
                .debug_struct("Mount")
                .field("source", &"[absolute path]")
                .field("destination", &"[relative path]")
                .field("access", access)
                .field("provenance", provenance)
                .finish(),
        }
    }
}

/// Canonical ordered materialization plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxManifest {
    entries: Box<[SandboxManifestEntry]>,
    digest: [u8; 32],
}

impl SandboxManifest {
    /// An empty manifest for a command that uses only mounted workspace roots.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            entries: Box::new([]),
            digest: Sha256::digest(b"crucible-sandbox-manifest-v1\0").into(),
        }
    }

    /// Validates, orders, and digests inert entries without touching sources.
    ///
    /// # Errors
    ///
    /// Duplicate destinations, too many entries, or too many inline bytes are
    /// rejected as a whole.
    pub fn new(
        entries: impl IntoIterator<Item = SandboxManifestEntry>,
    ) -> Result<Self, SandboxManifestError> {
        let mut entries: Vec<_> = entries.into_iter().collect();
        if entries.len() > MAX_SANDBOX_MANIFEST_ENTRIES {
            return Err(SandboxManifestError::TooManyEntries);
        }
        let bytes = entries.iter().fold(0_usize, |total, entry| {
            total.saturating_add(entry.contents().map_or(0, <[u8]>::len))
        });
        if bytes > MAX_SANDBOX_MANIFEST_BYTES {
            return Err(SandboxManifestError::ManifestTooLarge);
        }
        entries.sort_by(|left, right| left.destination().cmp(right.destination()));
        if entries.windows(2).any(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(left, right)| left.destination() == right.destination())
        }) {
            return Err(SandboxManifestError::DuplicateDestination);
        }
        if entries.windows(2).any(|pair| {
            pair.first().zip(pair.get(1)).is_some_and(|(left, right)| {
                right.destination().starts_with(left.destination())
                    && !matches!(left, SandboxManifestEntry::Directory { .. })
            })
        }) {
            return Err(SandboxManifestError::OverlappingDestination);
        }
        let digest = digest(&entries);
        Ok(Self {
            entries: entries.into_boxed_slice(),
            digest,
        })
    }

    /// Ordered entries.
    #[must_use]
    pub fn entries(&self) -> &[SandboxManifestEntry] {
        &self.entries
    }

    /// Domain-separated digest over types, destinations, bytes, sources, modes,
    /// and provenance. The raw values do not enter inspection records.
    #[must_use]
    pub const fn digest(&self) -> [u8; 32] {
        self.digest
    }

    /// Whether materialization has no entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for SandboxManifest {
    fn default() -> Self {
        Self::empty()
    }
}

fn digest(entries: &[SandboxManifestEntry]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"crucible-sandbox-manifest-v1\0");
    for entry in entries {
        match entry {
            SandboxManifestEntry::File {
                destination,
                contents,
                provenance,
            } => {
                digest.update(b"file\0");
                digest.update(destination.as_os_str().as_encoded_bytes());
                digest.update([0]);
                digest.update(Sha256::digest(contents));
                digest.update([*provenance as u8]);
            }
            SandboxManifestEntry::Directory {
                destination,
                provenance,
            } => {
                digest.update(b"directory\0");
                digest.update(destination.as_os_str().as_encoded_bytes());
                digest.update([0, *provenance as u8]);
            }
            SandboxManifestEntry::Mount {
                source,
                destination,
                access,
                provenance,
            } => {
                digest.update(b"mount\0");
                digest.update(source.as_os_str().as_encoded_bytes());
                digest.update([0]);
                digest.update(destination.as_os_str().as_encoded_bytes());
                digest.update([0, *access as u8, *provenance as u8]);
            }
        }
    }
    digest.finalize().into()
}

fn validate_destination(path: &Path) -> Result<(), SandboxManifestError> {
    let encoded = path.as_os_str().as_encoded_bytes();
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || encoded.len() > MAX_SANDBOX_PATH_BYTES
        || encoded.contains(&0)
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(SandboxManifestError::InvalidDestination);
    }
    Ok(())
}

fn validate_source(path: &Path) -> Result<(), SandboxManifestError> {
    let encoded = path.as_os_str().as_encoded_bytes();
    if !path.is_absolute()
        || path.parent().is_none()
        || encoded.len() > MAX_SANDBOX_PATH_BYTES
        || encoded.contains(&0)
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(SandboxManifestError::InvalidSource);
    }
    Ok(())
}

/// Why an inert manifest was rejected.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SandboxManifestError {
    /// Destinations are bounded relative paths made only of normal components.
    #[error("sandbox manifest destination must be a bounded relative path without traversal")]
    InvalidDestination,
    /// Sources are bounded normalized absolute paths other than host root.
    #[error("sandbox manifest source must be a bounded normalized absolute path below host root")]
    InvalidSource,
    /// Inline files are individually bounded.
    #[error("sandbox manifest file exceeds {MAX_SANDBOX_MANIFEST_FILE_BYTES} bytes")]
    FileTooLarge,
    /// The aggregate manifest is bounded.
    #[error("sandbox manifest exceeds {MAX_SANDBOX_MANIFEST_BYTES} inline bytes")]
    ManifestTooLarge,
    /// Entry count is bounded.
    #[error("sandbox manifest exceeds {MAX_SANDBOX_MANIFEST_ENTRIES} entries")]
    TooManyEntries,
    /// One destination has one owner.
    #[error("sandbox manifest contains a duplicate destination")]
    DuplicateDestination,
    /// Only a directory entry may own a destination above another entry.
    #[error("sandbox manifest contains an overlapping non-directory destination")]
    OverlappingDestination,
    /// Protected/unreadable are policy carve-outs, not mount grants.
    #[error("sandbox manifest mounts must be read-only or read-write")]
    InvalidMountAccess,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_cannot_be_absolute_or_traverse() {
        let provenance = SandboxFilesystemProvenance::Manifest;
        assert!(SandboxManifestEntry::directory("nested/data", provenance).is_ok());
        assert!(SandboxManifestEntry::directory("/absolute", provenance).is_err());
        assert!(SandboxManifestEntry::directory("../escape", provenance).is_err());
        assert!(SandboxManifestEntry::directory("a/../escape", provenance).is_err());
        assert!(SandboxManifestEntry::directory("nul\0escape", provenance).is_err());
        assert!(
            SandboxManifestEntry::mount(
                "/workspace/nul\0source",
                "source",
                SandboxFilesystemAccess::ReadOnly,
                provenance,
            )
            .is_err()
        );
    }

    #[test]
    fn manifests_are_canonical_bounded_and_digest_content() {
        let provenance = SandboxFilesystemProvenance::Manifest;
        let first = SandboxManifest::new([
            SandboxManifestEntry::file("b", Box::<[u8]>::from(&b"two"[..]), provenance)
                .expect("entry"),
            SandboxManifestEntry::file("a", Box::<[u8]>::from(&b"one"[..]), provenance)
                .expect("entry"),
        ])
        .expect("manifest");
        let reordered = SandboxManifest::new([
            SandboxManifestEntry::file("a", Box::<[u8]>::from(&b"one"[..]), provenance)
                .expect("entry"),
            SandboxManifestEntry::file("b", Box::<[u8]>::from(&b"two"[..]), provenance)
                .expect("entry"),
        ])
        .expect("manifest");
        let changed = SandboxManifest::new([SandboxManifestEntry::file(
            "a",
            Box::<[u8]>::from(&b"different"[..]),
            provenance,
        )
        .expect("entry")])
        .expect("manifest");

        assert_eq!(first.digest(), reordered.digest());
        assert_ne!(first.digest(), changed.digest());
        assert_eq!(
            first
                .entries()
                .first()
                .map(SandboxManifestEntry::destination),
            Some(Path::new("a"))
        );
    }

    #[test]
    fn non_directory_destinations_cannot_hide_descendant_entries() {
        let provenance = SandboxFilesystemProvenance::Manifest;
        let file = SandboxManifestEntry::file("tree", Box::<[u8]>::from(&b"file"[..]), provenance)
            .expect("file entry");
        let child = SandboxManifestEntry::directory("tree/child", provenance).expect("child");
        assert_eq!(
            SandboxManifest::new([file, child]),
            Err(SandboxManifestError::OverlappingDestination)
        );

        let file = SandboxManifestEntry::file("tree", Box::<[u8]>::from(&b"file"[..]), provenance)
            .expect("file");
        let lexical_sibling =
            SandboxManifestEntry::directory("tree-sibling", provenance).expect("lexical sibling");
        let child = SandboxManifestEntry::directory("tree/child", provenance).expect("child");
        assert_eq!(
            SandboxManifest::new([file, lexical_sibling, child]),
            Err(SandboxManifestError::OverlappingDestination)
        );

        let directory = SandboxManifestEntry::directory("tree", provenance).expect("directory");
        let child =
            SandboxManifestEntry::file("tree/child", Box::<[u8]>::from(&b"child"[..]), provenance)
                .expect("child");
        assert!(SandboxManifest::new([directory, child]).is_ok());
    }

    // The mount fixtures are POSIX absolute paths, which no Windows path type
    // accepts.
    #[cfg(unix)]
    #[test]
    fn manifest_debug_never_contains_inline_bytes_or_mount_sources() {
        let provenance = SandboxFilesystemProvenance::Manifest;
        let entry = SandboxManifestEntry::file(
            "secret",
            Box::<[u8]>::from(&b"do-not-log-this"[..]),
            provenance,
        )
        .expect("entry");
        assert!(!format!("{entry:?}").contains("do-not-log-this"));

        let mount = SandboxManifestEntry::mount(
            "/workspace/private-source",
            "mounted",
            SandboxFilesystemAccess::ReadOnly,
            provenance,
        )
        .expect("mount");
        assert!(!format!("{mount:?}").contains("private-source"));
    }

    #[test]
    fn manifest_entry_and_aggregate_bounds_are_enforced() {
        let provenance = SandboxFilesystemProvenance::Manifest;
        assert!(matches!(
            SandboxManifestEntry::file(
                "large",
                vec![0_u8; MAX_SANDBOX_MANIFEST_FILE_BYTES + 1],
                provenance,
            ),
            Err(SandboxManifestError::FileTooLarge)
        ));

        let entries = (0..=MAX_SANDBOX_MANIFEST_ENTRIES).map(|index| {
            SandboxManifestEntry::directory(format!("directory-{index}"), provenance)
                .expect("entry")
        });
        assert_eq!(
            SandboxManifest::new(entries),
            Err(SandboxManifestError::TooManyEntries)
        );

        let entries =
            (0..=MAX_SANDBOX_MANIFEST_BYTES / MAX_SANDBOX_MANIFEST_FILE_BYTES).map(|index| {
                SandboxManifestEntry::file(
                    format!("file-{index}"),
                    vec![0_u8; MAX_SANDBOX_MANIFEST_FILE_BYTES],
                    provenance,
                )
                .expect("entry")
            });
        assert_eq!(
            SandboxManifest::new(entries),
            Err(SandboxManifestError::ManifestTooLarge)
        );

        assert!(SandboxManifestEntry::mount(
            "/",
            "root",
            SandboxFilesystemAccess::ReadOnly,
            provenance,
        )
        .is_err());
    }
}
