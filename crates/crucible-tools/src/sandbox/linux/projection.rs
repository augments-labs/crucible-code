//! Private writable-root projection and terminal publication.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _, symlink};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use crucible_core::{
    SandboxError, SandboxFilesystemAccess, SandboxInspection, SandboxOutput, SandboxProcess,
    SandboxRequest, SandboxUsage, SandboxViolation,
};
use sha2::{Digest as _, Sha256};

use super::super::process::Stage;
use super::broker::StatusChannel;
use super::command::View;
use super::materialize::Materialization;

const MAX_PROJECTED_ENTRIES: usize = 262_144;
const MAX_PROJECTED_DEPTH: usize = 64;

/// One complete semantic view used for source-stability checks.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    entries: BTreeMap<PathBuf, Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    Directory {
        mode: u32,
        modified: Option<std::time::SystemTime>,
    },
    File {
        mode: u32,
        modified: Option<std::time::SystemTime>,
        length: u64,
        digest: [u8; 32],
        linked_to: Option<PathBuf>,
    },
    Symlink(OsString),
}

struct Root {
    host: PathBuf,
    destination: PathBuf,
    staged: PathBuf,
    source: File,
    directory: bool,
    baseline: Snapshot,
}

/// Host-owned writable copies. None of their pathnames reaches the workload.
pub(super) struct Projection {
    stage: Stage,
    roots: Vec<Root>,
    published: bool,
}

impl Projection {
    pub(super) fn prepare(
        request: &SandboxRequest,
        view: &View,
        materialization: Option<&Materialization>,
    ) -> Result<Option<Self>, SandboxError> {
        let mut specifications = Vec::new();
        for bind in view.binds().iter().filter(|bind| !bind.read_only()) {
            specifications.push((
                bind.host().to_path_buf(),
                descriptor_path(bind.descriptor()),
                bind.destination().to_path_buf(),
                bind.directory(),
            ));
        }
        if let Some(materialization) = materialization {
            for mount in materialization
                .mounts()
                .iter()
                .filter(|mount| mount.access() == SandboxFilesystemAccess::ReadWrite)
            {
                specifications.push((
                    mount.host().to_path_buf(),
                    descriptor_path(mount.descriptor()),
                    mount.destination().to_path_buf(),
                    mount.directory(),
                ));
            }
        }
        if specifications.is_empty() {
            return Ok(None);
        }
        specifications.sort_by(|left, right| {
            left.0
                .components()
                .count()
                .cmp(&right.0.components().count())
                .then_with(|| left.0.cmp(&right.0))
                .then_with(|| left.2.cmp(&right.2))
        });

        let root = staging_root(request)?;
        create_private_directory(&root)
            .map_err(|source| failed("could not create writable projection", source))?;
        let stage = Stage::new(root);
        let roots_directory = stage.root().join("roots");
        create_private_directory(&roots_directory)
            .map_err(|source| failed("could not create projected roots", source))?;

        let mut roots = Vec::with_capacity(specifications.len());
        for (index, (host, pinned, destination, directory)) in
            specifications.into_iter().enumerate()
        {
            let staged = roots_directory.join(index.to_string());
            let before = snapshot(&pinned)
                .map_err(|source| failed("writable root could not be fingerprinted", source))?;
            copy_root(&pinned, &staged, directory, true)
                .map_err(|source| failed("writable root could not be projected", source))?;
            let copied = snapshot(&staged)
                .map_err(|source| failed("projected root could not be verified", source))?;
            let after = snapshot(&pinned)
                .map_err(|source| failed("writable root could not be revalidated", source))?;
            if before != copied || before != after {
                return Err(refused(
                    "writable root changed while its private projection was prepared",
                ));
            }
            let source = File::open(&staged)
                .map_err(|source| failed("projected root could not be pinned", source))?;
            roots.push(Root {
                host,
                destination,
                staged,
                source,
                directory,
                baseline: before,
            });
        }
        Ok(Some(Self {
            stage,
            roots,
            published: false,
        }))
    }

    pub(super) fn descriptor(&self, destination: &Path) -> Option<RawFd> {
        self.roots
            .iter()
            .find(|root| root.destination == destination)
            .map(|root| root.source.as_raw_fd())
    }

    fn publish(&mut self) -> io::Result<()> {
        if self.published {
            return Ok(());
        }

        let mut finals = Vec::with_capacity(self.roots.len());
        for root in &self.roots {
            finals.push(snapshot(&root.staged)?);
        }
        if self
            .roots
            .iter()
            .zip(&finals)
            .all(|(root, final_snapshot)| &root.baseline == final_snapshot)
        {
            self.published = true;
            return Ok(());
        }
        for root in &self.roots {
            let current = snapshot(&root.host)?;
            if current != root.baseline {
                return Err(io::Error::other(
                    "writable root changed outside the sandbox before publication",
                ));
            }
        }

        let backup_directory = self.stage.root().join("backups");
        create_private_directory(&backup_directory)?;
        let mut backups = Vec::with_capacity(self.roots.len());
        for (index, root) in self.roots.iter().enumerate() {
            let changed = finals
                .get(index)
                .is_some_and(|final_snapshot| final_snapshot != &root.baseline);
            if !changed {
                backups.push(None);
                continue;
            }
            let backup = backup_directory.join(index.to_string());
            copy_root(&root.host, &backup, root.directory, false)?;
            if snapshot(&backup)? != root.baseline {
                return Err(io::Error::other(
                    "writable root changed while its rollback image was prepared",
                ));
            }
            backups.push(Some(backup));
        }

        let mut applied = Vec::new();
        for index in 0..self.roots.len() {
            let root = self
                .roots
                .get(index)
                .ok_or_else(|| io::Error::other("projection root index is unavailable"))?;
            let desired = finals
                .get(index)
                .ok_or_else(|| io::Error::other("projection result index is unavailable"))?;
            if desired == &root.baseline {
                continue;
            }
            let result = replace_root(&root.host, &root.staged, root.directory)
                .and_then(|()| verify_snapshot(&root.host, desired));
            if let Err(problem) = result {
                let rollback = rollback(&self.roots, &backups, &applied, Some(index));
                return match rollback {
                    Ok(()) => Err(problem),
                    Err(rollback_problem) => Err(io::Error::other(format!(
                        "publication failed and rollback could not be proved: {problem}; {rollback_problem}"
                    ))),
                };
            }
            applied.push(index);
        }

        self.published = true;
        Ok(())
    }
}

fn rollback(
    roots: &[Root],
    backups: &[Option<PathBuf>],
    applied: &[usize],
    failed: Option<usize>,
) -> io::Result<()> {
    for index in failed.into_iter().chain(applied.iter().rev().copied()) {
        let root = roots
            .get(index)
            .ok_or_else(|| io::Error::other("rollback root index is unavailable"))?;
        let backup = backups
            .get(index)
            .and_then(Option::as_deref)
            .ok_or_else(|| io::Error::other("rollback image index is unavailable"))?;
        replace_root(&root.host, backup, root.directory)?;
        verify_snapshot(&root.host, &root.baseline)?;
    }
    Ok(())
}

fn descriptor_path(descriptor: RawFd) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{descriptor}"))
}

fn verify_snapshot(path: &Path, expected: &Snapshot) -> io::Result<()> {
    if &snapshot(path)? == expected {
        Ok(())
    } else {
        Err(io::Error::other(
            "published writable root does not match its staged semantic view",
        ))
    }
}

fn staging_root(request: &SandboxRequest) -> Result<PathBuf, SandboxError> {
    for base in [Path::new("/tmp"), Path::new("/var/tmp")] {
        let Ok(canonical) = base.canonicalize() else {
            continue;
        };
        if canonical != base {
            continue;
        }
        let candidate = base.join(format!("crucible-projection-{}", request.id()));
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
        "no host-owned projection root is outside the sandbox filesystem view",
    ))
}

fn snapshot(root: &Path) -> io::Result<Snapshot> {
    let mut builder = SnapshotBuilder::default();
    builder.visit(root, Path::new(""), 0)?;
    Ok(Snapshot {
        entries: builder.entries,
    })
}

#[derive(Default)]
struct SnapshotBuilder {
    entries: BTreeMap<PathBuf, Entry>,
    hard_links: BTreeMap<(u64, u64), PathBuf>,
    retained: usize,
}

impl SnapshotBuilder {
    fn visit(&mut self, root: &Path, relative: &Path, depth: usize) -> io::Result<()> {
        if depth > MAX_PROJECTED_DEPTH {
            return Err(io::Error::other("projected tree exceeds its depth bound"));
        }
        self.retained = self.retained.saturating_add(1);
        if self.retained > MAX_PROJECTED_ENTRIES {
            return Err(io::Error::other("projected tree exceeds its entry bound"));
        }
        let path = if relative.as_os_str().is_empty() {
            root.to_path_buf()
        } else {
            root.join(relative)
        };
        let metadata = if relative.as_os_str().is_empty() {
            fs::metadata(&path)?
        } else {
            fs::symlink_metadata(&path)?
        };
        let entry = if metadata.is_dir() {
            Entry::Directory {
                mode: safe_mode(&metadata),
                modified: metadata.modified().ok(),
            }
        } else if metadata.is_file() {
            let linked_to = if metadata.nlink() > 1 {
                let identity = (metadata.dev(), metadata.ino());
                if let Some(first) = self.hard_links.get(&identity) {
                    Some(first.clone())
                } else {
                    self.hard_links.insert(identity, relative.to_path_buf());
                    None
                }
            } else {
                None
            };
            Entry::File {
                mode: safe_mode(&metadata),
                modified: metadata.modified().ok(),
                length: metadata.len(),
                digest: digest_file(&path)?,
                linked_to,
            }
        } else if metadata.file_type().is_symlink() {
            Entry::Symlink(fs::read_link(&path)?.into_os_string())
        } else {
            return Err(io::Error::other(
                "projected tree contains an unsupported special file",
            ));
        };
        self.entries.insert(relative.to_path_buf(), entry);

        if metadata.is_dir() {
            let mut children = fs::read_dir(&path)?.collect::<Result<Vec<_>, _>>()?;
            children.sort_by_key(fs::DirEntry::file_name);
            for child in children {
                let name = child.file_name();
                if protected_name(&name) {
                    continue;
                }
                self.visit(root, &relative.join(name), depth.saturating_add(1))?;
            }
        }
        Ok(())
    }
}

fn digest_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let bytes = buffer
            .get(..read)
            .ok_or_else(|| io::Error::other("file read exceeded its buffer"))?;
        digest.update(bytes);
    }
    Ok(digest.finalize().into())
}

fn copy_root(
    source: &Path,
    destination: &Path,
    directory: bool,
    include_protected: bool,
) -> io::Result<()> {
    if directory {
        fs::create_dir(destination)?;
        Copier::new(include_protected).copy_directory(source, destination, 0)?;
        copy_metadata(source, destination, true)
    } else {
        copy_file(source, destination)?;
        copy_metadata(source, destination, false)
    }
}

struct Copier {
    hard_links: BTreeMap<(u64, u64), PathBuf>,
    copied: usize,
    include_protected: bool,
}

impl Copier {
    fn new(include_protected: bool) -> Self {
        Self {
            hard_links: BTreeMap::new(),
            copied: 0,
            include_protected,
        }
    }

    fn copy_directory(
        &mut self,
        source: &Path,
        destination: &Path,
        depth: usize,
    ) -> io::Result<()> {
        if depth > MAX_PROJECTED_DEPTH {
            return Err(io::Error::other("projected copy exceeds its depth bound"));
        }
        let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name();
            if !self.include_protected && protected_name(&name) {
                continue;
            }
            self.copied = self.copied.saturating_add(1);
            if self.copied > MAX_PROJECTED_ENTRIES {
                return Err(io::Error::other("projected copy exceeds its entry bound"));
            }
            let source_path = entry.path();
            let destination_path = destination.join(&name);
            let metadata = fs::symlink_metadata(&source_path)?;
            if metadata.is_dir() {
                match fs::create_dir(&destination_path) {
                    Ok(()) => {}
                    Err(problem)
                        if problem.kind() == io::ErrorKind::AlreadyExists
                            && fs::symlink_metadata(&destination_path)
                                .is_ok_and(|existing| existing.is_dir()) => {}
                    Err(problem) => return Err(problem),
                }
                self.copy_directory(&source_path, &destination_path, depth.saturating_add(1))?;
                copy_metadata(&source_path, &destination_path, true)?;
            } else if metadata.is_file() {
                let identity = (metadata.dev(), metadata.ino());
                if metadata.nlink() > 1
                    && let Some(first) = self.hard_links.get(&identity)
                {
                    fs::hard_link(first, &destination_path)?;
                } else {
                    copy_file(&source_path, &destination_path)?;
                    copy_metadata(&source_path, &destination_path, false)?;
                    if metadata.nlink() > 1 {
                        self.hard_links.insert(identity, destination_path);
                    }
                }
            } else if metadata.file_type().is_symlink() {
                symlink(fs::read_link(&source_path)?, &destination_path)?;
            } else {
                return Err(io::Error::other(
                    "projected copy encountered an unsupported special file",
                ));
            }
        }
        Ok(())
    }
}

fn copy_file(source: &Path, destination: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    options.mode(0o600);
    let mut output = options.open(destination)?;
    let mut input = File::open(source)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()
}

fn copy_metadata(source: &Path, destination: &Path, directory: bool) -> io::Result<()> {
    let metadata = fs::metadata(source)?;
    fs::set_permissions(
        destination,
        fs::Permissions::from_mode(safe_mode(&metadata)),
    )?;
    let mut times = FileTimes::new();
    if let Ok(accessed) = metadata.accessed() {
        times = times.set_accessed(accessed);
    }
    if let Ok(modified) = metadata.modified() {
        times = times.set_modified(modified);
    }
    let file = if directory {
        File::open(destination)?
    } else {
        OpenOptions::new().write(true).open(destination)?
    };
    file.set_times(times)
}

fn replace_root(host: &Path, source: &Path, directory: bool) -> io::Result<()> {
    if !directory {
        let parent = host
            .parent()
            .ok_or_else(|| io::Error::other("projected file root has no parent"))?;
        let name = host
            .file_name()
            .ok_or_else(|| io::Error::other("projected file root has no name"))?;
        let temporary = parent.join(format!(".{}.crucible-publication", name.to_string_lossy()));
        let _ = fs::remove_file(&temporary);
        copy_file(source, &temporary)?;
        copy_metadata(source, &temporary, false)?;
        fs::rename(&temporary, host)?;
        File::open(parent)?.sync_all()?;
        return Ok(());
    }

    clear_directory(host)?;
    Copier::new(false).copy_directory(source, host, 0)?;
    copy_metadata(source, host, true)?;
    File::open(host)?.sync_all()
}

fn clear_directory(directory: &Path) -> io::Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        if protected_name(&name) {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            clear_directory(&path)?;
            if fs::read_dir(&path)?.next().is_none() {
                fs::remove_dir(&path)?;
            }
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn safe_mode(metadata: &fs::Metadata) -> u32 {
    metadata.permissions().mode() & 0o777
}

fn protected_name(name: &OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(".git" | ".agents" | ".codex" | ".crucible")
    )
}

fn failed(problem: &'static str, source: io::Error) -> SandboxError {
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

/// Adds publication/discard semantics to the ordinary process-tree owner.
pub(super) fn wrap(
    process: Box<dyn SandboxProcess>,
    projection: Option<Projection>,
    status_channel: StatusChannel,
) -> Box<dyn SandboxProcess> {
    Box::new(ProjectedProcess {
        process,
        projection,
        status_channel: Some(status_channel),
        status: None,
        terminal: false,
    })
}

struct ProjectedProcess {
    process: Box<dyn SandboxProcess>,
    projection: Option<Projection>,
    status_channel: Option<StatusChannel>,
    status: Option<ExitStatus>,
    terminal: bool,
}

impl SandboxProcess for ProjectedProcess {
    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.process.take_stdout()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.process.take_stderr()
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        let Some(_broker_status) = self.process.try_wait()? else {
            return Ok(None);
        };
        let status = self
            .status_channel
            .as_mut()
            .ok_or_else(|| io::Error::other("sandbox broker status channel is unavailable"))?
            .wait_status()?;
        self.status_channel.take();
        if !self.terminal {
            if status.signal().is_none()
                && self.process.violation().is_none()
                && let Some(projection) = &mut self.projection
            {
                projection.publish()?;
            }
            self.terminal = true;
        }
        self.status = Some(status);
        Ok(Some(status))
    }

    fn stop(&mut self) -> io::Result<()> {
        if self.status.is_none() {
            self.terminal = true;
        }
        let result = self.process.stop();
        self.status_channel.take();
        self.projection.take();
        result
    }

    fn inspection(&self) -> &SandboxInspection {
        self.process.inspection()
    }

    fn usage(&self) -> SandboxUsage {
        self.process.usage()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        self.process.violation()
    }
}

impl Drop for ProjectedProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_names_are_never_publication_entries() {
        for name in [".git", ".agents", ".codex", ".crucible"] {
            assert!(protected_name(OsStr::new(name)));
        }
        assert!(!protected_name(OsStr::new(".github")));
    }
}
