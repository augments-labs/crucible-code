//! Transactional inline manifest staging.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use crucible_core::{SandboxError, SandboxFilesystemAccess, SandboxManifestEntry, SandboxRequest};

use super::super::process::Stage;

/// Explicit directory aliases are deliberately smaller than whole workspaces.
const MAX_MOUNT_TREE_ENTRIES: usize = 8192;

/// Prevent adversarial trees from turning validation into unbounded recursion.
const MAX_MOUNT_TREE_DEPTH: usize = 64;

/// Stages all inline entries under one owner-only generated root, then commits
/// them with a single rename. The command never observes the building tree.
pub(super) fn commit(request: &SandboxRequest) -> Result<Option<Materialization>, SandboxError> {
    if request.manifest().is_empty() {
        return Ok(None);
    }

    let root = staging_root(request)?;
    create_private_directory(&root)
        .map_err(|source| failed("could not create staging root", source))?;
    let stage = Stage::new(root.clone());
    let building = root.join("building");
    create_private_directory(&building)
        .map_err(|source| failed("could not create manifest transaction", source))?;

    let mut mounts = Vec::new();
    for entry in request.manifest().entries() {
        let destination = building.join(entry.destination());
        match entry {
            SandboxManifestEntry::Directory { .. } => {
                create_private_tree(&destination)
                    .map_err(|source| failed("could not stage a manifest directory", source))?;
            }
            SandboxManifestEntry::File { contents, .. } => {
                if let Some(parent) = destination.parent() {
                    create_private_tree(parent).map_err(|source| {
                        failed("could not stage a manifest file parent", source)
                    })?;
                }
                write_new(&destination, contents)
                    .map_err(|source| failed("could not stage a manifest file", source))?;
            }
            SandboxManifestEntry::Mount { source, access, .. } => {
                let mount =
                    prepare_mount(request, source, &destination, entry.destination(), *access)?;
                mounts.push(mount);
            }
        }
    }

    let committed = stage.manifest();
    fs::rename(&building, &committed)
        .map_err(|source| failed("could not commit the sandbox manifest", source))?;
    sync_directory(&root)
        .map_err(|source| failed("could not sync the sandbox manifest", source))?;
    let manifest = pin(&committed, "committed sandbox manifest could not be pinned")?;
    Ok(Some(Materialization {
        stage,
        manifest,
        mounts,
    }))
}

/// Chooses a host temporary root that the effective filesystem view cannot
/// reach. Host `TMPDIR` is deliberately ignored: it may point into the checked
/// out workspace that the command is allowed to mutate.
fn staging_root(request: &SandboxRequest) -> Result<PathBuf, SandboxError> {
    for base in [Path::new("/tmp"), Path::new("/var/tmp")] {
        let Ok(canonical) = base.canonicalize() else {
            continue;
        };
        if canonical != base {
            continue;
        }
        let candidate = base.join(format!("crucible-sandbox-{}", request.id()));
        if request
            .policy()
            .filesystem()
            .iter()
            .all(|rule| !candidate.starts_with(rule.path()))
        {
            return Ok(candidate);
        }
    }
    Err(refused(
        "no host-owned staging root is outside the sandbox filesystem view",
    ))
}

/// A committed inline tree and descriptor-pinned explicit mount sources.
pub(super) struct Materialization {
    stage: Stage,
    manifest: OwnedFd,
    mounts: Vec<PreparedMount>,
}

impl Materialization {
    pub(super) fn mounts(&self) -> &[PreparedMount] {
        &self.mounts
    }

    pub(super) fn descriptor(&self) -> RawFd {
        self.manifest.as_raw_fd()
    }

    pub(super) fn cleanup(&mut self) -> std::io::Result<()> {
        self.stage.cleanup()
    }

    pub(super) fn split(self) -> (Stage, Vec<OwnedFd>) {
        let Self {
            stage,
            manifest,
            mounts,
        } = self;
        let files = std::iter::once(manifest)
            .chain(mounts.into_iter().map(|mount| mount.source))
            .collect();
        (stage, files)
    }
}

impl std::fmt::Debug for Materialization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Materialization")
            .field("stage", &self.stage)
            .field("mounts", &self.mounts.len())
            .finish_non_exhaustive()
    }
}

/// A source opened before staging commits. Bubblewrap receives this descriptor,
/// so a later rename cannot retarget the mount.
pub(super) struct PreparedMount {
    source: OwnedFd,
    host: PathBuf,
    destination: std::path::PathBuf,
    access: SandboxFilesystemAccess,
    directory: bool,
    mode: u32,
}

impl PreparedMount {
    pub(super) fn descriptor(&self) -> RawFd {
        self.source.as_raw_fd()
    }

    pub(super) fn duplicate(&self) -> std::io::Result<OwnedFd> {
        rustix::io::fcntl_dupfd_cloexec(&self.source, 3).map_err(Into::into)
    }

    pub(super) fn destination(&self) -> &Path {
        &self.destination
    }

    pub(super) fn host(&self) -> &Path {
        &self.host
    }

    pub(super) const fn directory(&self) -> bool {
        self.directory
    }

    pub(super) const fn access(&self) -> SandboxFilesystemAccess {
        self.access
    }

    pub(super) const fn mode(&self) -> u32 {
        self.mode
    }
}

fn prepare_mount(
    request: &SandboxRequest,
    source: &Path,
    staged_destination: &Path,
    sandbox_destination: &Path,
    access: SandboxFilesystemAccess,
) -> Result<PreparedMount, SandboxError> {
    let named = fs::symlink_metadata(source)
        .map_err(|error| failed("manifest mount source is unavailable", error))?;
    if named.file_type().is_symlink() {
        return Err(refused("manifest mount source is a symbolic link"));
    }
    let canonical = source
        .canonicalize()
        .map_err(|error| failed("manifest mount source could not be canonicalized", error))?;
    if canonical != source {
        return Err(refused(
            "manifest mount source contains a symbolic-link or non-canonical component",
        ));
    }
    if !request.policy().permits_path(&canonical, access) {
        return Err(refused(
            "manifest mount source is outside its parent filesystem authority",
        ));
    }
    if !named.is_dir() && !named.is_file() {
        return Err(refused(
            "manifest mount source is not a regular file or directory",
        ));
    }
    if named.is_file() && named.nlink() > 1 {
        return Err(refused("manifest mount source is a hard-linked file"));
    }
    if named.is_dir() {
        validate_mount_tree(&canonical, named.dev(), access)?;
    }

    let source = if named.is_file() && access == SandboxFilesystemAccess::ReadWrite {
        OpenOptions::new().read(true).write(true).open(&canonical)
    } else {
        File::open(&canonical)
    }
    .map_err(|error| failed("manifest mount source could not be opened", error))?;
    let opened = source
        .metadata()
        .map_err(|error| failed("manifest mount source could not be verified", error))?;
    if (opened.dev(), opened.ino()) != (named.dev(), named.ino()) {
        return Err(refused(
            "manifest mount source changed while it was being prepared",
        ));
    }
    let source = rustix::io::fcntl_dupfd_cloexec(&source, 3).map_err(|error| {
        failed(
            "manifest mount descriptor could not be isolated",
            error.into(),
        )
    })?;

    if opened.is_dir() {
        create_private_tree(staged_destination)
            .map_err(|error| failed("could not stage a mount directory", error))?;
    } else {
        if let Some(parent) = staged_destination.parent() {
            create_private_tree(parent)
                .map_err(|error| failed("could not stage a mount parent", error))?;
        }
        write_new(staged_destination, &[])
            .map_err(|error| failed("could not stage a mount file", error))?;
    }

    Ok(PreparedMount {
        source,
        host: canonical,
        destination: Path::new("/crucible/manifest").join(sandbox_destination),
        access,
        directory: opened.is_dir(),
        mode: opened.mode() & 0o777,
    })
}

fn pin(path: &Path, problem: &'static str) -> Result<OwnedFd, SandboxError> {
    let source = File::open(path).map_err(|error| failed(problem, error))?;
    rustix::io::fcntl_dupfd_cloexec(&source, 3).map_err(|error| failed(problem, error.into()))
}

fn validate_mount_tree(
    root: &Path,
    root_device: u64,
    access: SandboxFilesystemAccess,
) -> Result<(), SandboxError> {
    let mut pending = VecDeque::from([(root.to_path_buf(), 0_usize)]);
    let mut inspected = 0_usize;
    while let Some((directory, depth)) = pending.pop_front() {
        let entries = fs::read_dir(directory)
            .map_err(|error| failed("manifest mount tree could not be read", error))?;
        for entry in entries {
            inspected = inspected.saturating_add(1);
            if inspected > MAX_MOUNT_TREE_ENTRIES {
                return Err(refused("manifest mount tree exceeds its validation bound"));
            }
            let entry =
                entry.map_err(|error| failed("manifest mount entry could not be read", error))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| failed("manifest mount entry changed during validation", error))?;
            if metadata.dev() != root_device {
                return Err(refused("manifest mount tree crosses a filesystem boundary"));
            }
            if metadata.file_type().is_symlink() {
                return Err(refused("manifest mount tree contains a symbolic link"));
            }
            if metadata.is_file() {
                if metadata.nlink() != 1 {
                    return Err(refused("manifest mount tree contains a hard-linked file"));
                }
                continue;
            }
            if metadata.is_dir() {
                if access == SandboxFilesystemAccess::ReadWrite
                    && protected_name(&entry.file_name())
                {
                    return Err(refused(
                        "writable manifest mount contains protected control metadata",
                    ));
                }
                if depth >= MAX_MOUNT_TREE_DEPTH {
                    return Err(refused("manifest mount tree exceeds its depth bound"));
                }
                pending.push_back((path, depth.saturating_add(1)));
                continue;
            }
            return Err(refused("manifest mount tree contains a special file"));
        }
    }
    Ok(())
}

fn protected_name(name: &OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(".git" | ".agents" | ".codex" | ".crucible")
    )
}

fn create_private_tree(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    private(path)
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir(path)?;
    private(path)
}

#[cfg(unix)]
fn private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn private(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn write_new(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

fn failed(problem: &'static str, source: std::io::Error) -> SandboxError {
    SandboxError::Materialization {
        problem: problem.into(),
        source: Some(source),
    }
}

fn refused(problem: &'static str) -> SandboxError {
    SandboxError::Materialization {
        problem: problem.into(),
        source: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crucible_core::{
        Ancestry, SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxId, SandboxManifest,
        SandboxManifestEntry, SandboxMode, SandboxNetworkPolicy, SandboxPolicy, SandboxRequest,
        SandboxResourceLimits, ToolId,
    };

    use crate::sample::{Sample, symlink};

    fn request(sample: &Sample, manifest: SandboxManifest) -> SandboxRequest {
        SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("manifest"),
            SandboxPolicy::standard(&sample.workspace()).expect("policy"),
            manifest,
        )
    }

    #[test]
    fn private_directory_creation_never_reuses_an_existing_target() {
        let root =
            std::env::temp_dir().join(format!("crucible-stage-create-new-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        create_private_directory(&root).expect("first create");
        assert!(create_private_directory(&root).is_err());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn committed_and_failed_transactions_remove_their_private_stage() {
        let sample = Sample::new("sandbox-stage-cleanup");
        let committed = request(
            &sample,
            SandboxManifest::new([SandboxManifestEntry::file(
                "input.txt",
                Box::<[u8]>::from(&b"input"[..]),
                SandboxFilesystemProvenance::Manifest,
            )
            .expect("entry")])
            .expect("manifest"),
        );
        let committed_root = staging_root(&committed).expect("staging root");
        let materialization = commit(&committed).expect("commit").expect("stage");
        assert!(committed_root.is_dir());
        drop(materialization);
        assert!(!committed_root.exists());

        let outside = sample.outside("outside.txt", "outside");
        let refused = request(
            &sample,
            SandboxManifest::new([SandboxManifestEntry::mount(
                PathBuf::from(outside),
                "outside.txt",
                SandboxFilesystemAccess::ReadOnly,
                SandboxFilesystemProvenance::Manifest,
            )
            .expect("entry")])
            .expect("manifest"),
        );
        let refused_root = staging_root(&refused).expect("staging root");
        assert!(commit(&refused).is_err());
        assert!(!refused_root.exists());
    }

    #[test]
    fn staging_never_uses_a_root_visible_to_the_command() {
        let temporary = SandboxFilesystemRule::new(
            "/tmp",
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("temporary rule");
        let policy = SandboxPolicy::new(
            SandboxMode::Required,
            [temporary],
            "/tmp",
            SandboxNetworkPolicy::Closed,
            SandboxResourceLimits::default(),
        )
        .expect("policy");
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("manifest"),
            policy,
            SandboxManifest::empty(),
        );
        assert!(
            staging_root(&request)
                .expect("alternate root")
                .starts_with("/var/tmp")
        );
    }

    #[test]
    fn mount_sources_refuse_links_special_files_and_narrowed_subtrees() {
        let sample = Sample::new("sandbox-mount-source-validation");
        sample.write("plain.txt", "plain");
        symlink(
            sample.root().join("plain.txt"),
            sample.root().join("linked.txt"),
        );
        let linked = request(
            &sample,
            SandboxManifest::new([SandboxManifestEntry::mount(
                sample.root().join("linked.txt"),
                "linked.txt",
                SandboxFilesystemAccess::ReadOnly,
                SandboxFilesystemProvenance::Manifest,
            )
            .expect("entry")])
            .expect("manifest"),
        );
        assert!(commit(&linked).is_err());

        fs::hard_link(
            sample.root().join("plain.txt"),
            sample.root().join("hard-linked.txt"),
        )
        .expect("hard link");
        let hard_linked = request(
            &sample,
            SandboxManifest::new([SandboxManifestEntry::mount(
                sample.root().join("plain.txt"),
                "hard-linked.txt",
                SandboxFilesystemAccess::ReadOnly,
                SandboxFilesystemProvenance::Manifest,
            )
            .expect("entry")])
            .expect("manifest"),
        );
        assert!(commit(&hard_linked).is_err());

        let socket_path = sample.root().join("host.sock");
        let _socket = std::os::unix::net::UnixListener::bind(&socket_path).expect("socket");
        let special = request(
            &sample,
            SandboxManifest::new([SandboxManifestEntry::mount(
                socket_path,
                "host.sock",
                SandboxFilesystemAccess::ReadOnly,
                SandboxFilesystemProvenance::Manifest,
            )
            .expect("entry")])
            .expect("manifest"),
        );
        assert!(commit(&special).is_err());

        sample.write(".git/config", "protected");
        let protected = request(
            &sample,
            SandboxManifest::new([SandboxManifestEntry::mount(
                sample.root().clone(),
                "workspace",
                SandboxFilesystemAccess::ReadWrite,
                SandboxFilesystemProvenance::Manifest,
            )
            .expect("entry")])
            .expect("manifest"),
        );
        assert!(commit(&protected).is_err());
    }

    #[test]
    fn mount_directories_cannot_hide_hard_link_aliases() {
        let sample = Sample::new("sandbox-mount-tree-validation");
        let outside = PathBuf::from(sample.outside("outside.txt", "outside"));
        fs::create_dir(sample.root().join("mounted")).expect("mount source");
        fs::hard_link(&outside, sample.root().join("mounted/alias.txt")).expect("hard link");
        let request = request(
            &sample,
            SandboxManifest::new([SandboxManifestEntry::mount(
                sample.root().join("mounted"),
                "mounted",
                SandboxFilesystemAccess::ReadOnly,
                SandboxFilesystemProvenance::Manifest,
            )
            .expect("entry")])
            .expect("manifest"),
        );

        assert!(commit(&request).is_err());
    }

    #[test]
    fn a_source_deleted_before_materialization_is_refused_and_cleaned() {
        let sample = Sample::new("sandbox-deleted-mount-source");
        sample.write("source.txt", "source");
        let source = sample.root().join("source.txt");
        let request = request(
            &sample,
            SandboxManifest::new([SandboxManifestEntry::mount(
                source.clone(),
                "source.txt",
                SandboxFilesystemAccess::ReadOnly,
                SandboxFilesystemProvenance::Manifest,
            )
            .expect("entry")])
            .expect("manifest"),
        );
        let root = staging_root(&request).expect("staging root");
        fs::remove_file(source).expect("delete source");

        assert!(commit(&request).is_err());
        assert!(!root.exists());
    }
    #[test]
    fn an_overlapping_destination_is_staged_in_one_order_however_it_arrived() {
        let sample = Sample::new("sandbox-stage-overlap");
        sample.write("mounted.txt", "mounted");
        let provenance = SandboxFilesystemProvenance::Manifest;
        let entries = || {
            [
                SandboxManifestEntry::file(
                    "tree/deep/leaf",
                    Box::<[u8]>::from(&b"leaf"[..]),
                    provenance,
                )
                .expect("leaf"),
                SandboxManifestEntry::directory("tree", provenance).expect("parent"),
                SandboxManifestEntry::mount(
                    sample.root().join("mounted.txt"),
                    "tree/mounted",
                    SandboxFilesystemAccess::ReadOnly,
                    provenance,
                )
                .expect("mount"),
                SandboxManifestEntry::directory("tree-sibling", provenance)
                    .expect("lexical sibling"),
            ]
        };
        let mut arrived_reversed = entries();
        arrived_reversed.reverse();

        // One order, whichever order the caller wrote them in, and the parent
        // ahead of everything under it. The lexical sibling sorts after the
        // whole subtree because a path is ordered by its components, which is
        // what keeps a parent adjacent to its own descendants.
        let manifest = SandboxManifest::new(arrived_reversed).expect("manifest");
        let same = SandboxManifest::new(entries()).expect("manifest");
        assert_eq!(manifest.digest(), same.digest());
        let destinations: Vec<_> = manifest
            .entries()
            .iter()
            .map(SandboxManifestEntry::destination)
            .collect();
        assert_eq!(
            destinations,
            [
                Path::new("tree"),
                Path::new("tree/deep/leaf"),
                Path::new("tree/mounted"),
                Path::new("tree-sibling"),
            ]
        );

        let request = request(&sample, manifest);
        let root = staging_root(&request).expect("staging root");
        let materialization = commit(&request).expect("commit").expect("stage");
        let staged = root.join("manifest");

        // The directory above them did not become their owner: each entry is
        // still itself, and the mount kept a stub for the kernel to cover.
        assert!(staged.join("tree").is_dir());
        assert!(staged.join("tree-sibling").is_dir());
        assert_eq!(
            fs::read(staged.join("tree/deep/leaf")).expect("staged leaf"),
            b"leaf"
        );
        assert!(staged.join("tree/mounted").is_file());
        assert_eq!(
            materialization
                .mounts()
                .first()
                .map(PreparedMount::destination),
            Some(Path::new("/crucible/manifest/tree/mounted"))
        );
    }

    #[test]
    fn materializations_running_at_once_never_share_or_disturb_a_stage() {
        /// Enough at once that a shared root would be reached for.
        const AT_ONCE: usize = 4;

        let sample = Sample::new("sandbox-stage-at-once");
        let ready = std::sync::Barrier::new(AT_ONCE);
        let mut staged: Vec<(usize, PathBuf, Materialization)> = std::thread::scope(|scope| {
            let started: Vec<_> = (0..AT_ONCE)
                .map(|index| {
                    let ready = &ready;
                    let sample = &sample;
                    scope.spawn(move || {
                        let request = request(
                            sample,
                            SandboxManifest::new([SandboxManifestEntry::file(
                                "leaf",
                                format!("stage-{index}").into_bytes(),
                                SandboxFilesystemProvenance::Manifest,
                            )
                            .expect("entry")])
                            .expect("manifest"),
                        );
                        let root = staging_root(&request).expect("staging root");
                        ready.wait();
                        let materialization = commit(&request).expect("commit").expect("stage");
                        (index, root, materialization)
                    })
                })
                .collect();
            started
                .into_iter()
                .map(|thread| thread.join().expect("a staged manifest"))
                .collect()
        });

        // A stage is named by its request and created rather than opened, so
        // two of them cannot be the same directory, and neither can read what
        // the other wrote.
        let roots: std::collections::HashSet<&Path> =
            staged.iter().map(|(_, root, _)| root.as_path()).collect();
        assert_eq!(roots.len(), AT_ONCE, "two materializations shared a stage");
        for (index, root, _) in &staged {
            assert_eq!(
                fs::read(root.join("manifest").join("leaf")).expect("staged leaf"),
                format!("stage-{index}").into_bytes()
            );
        }

        // And one of them finishing takes its own stage and nothing else.
        let (_, finished, materialization) = staged.pop().expect("a staged manifest");
        drop(materialization);
        assert!(!finished.exists());
        for (_, root, _) in &staged {
            assert!(root.is_dir(), "one stage's cleanup removed another's");
        }
    }
}
