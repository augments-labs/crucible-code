//! Private writable-root projection and terminal publication.

mod authority;
mod protocol;
mod publish;

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Read, Seek as _, SeekFrom, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use crucible_core::{
    SandboxAudit, SandboxError, SandboxFactKind, SandboxFilesystemAccess, SandboxId,
    SandboxInspection, SandboxLifecycle, SandboxOutput, SandboxProcess, SandboxRequest,
    SandboxUsage, SandboxViolation,
};
use crucible_sandbox_broker::CANCEL_FRAME;
use crucible_sandbox_broker::MAX_SCAN_EXTENTS;
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

#[derive(Clone, Debug)]
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
        extents: Vec<(u64, u64)>,
        linked_to: Option<PathBuf>,
        payload: Option<PathBuf>,
    },
    Symlink(OsString),
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Directory {
                    mode: left_mode,
                    modified: left_modified,
                },
                Self::Directory {
                    mode: right_mode,
                    modified: right_modified,
                },
            ) => left_mode == right_mode && left_modified == right_modified,
            (
                Self::File {
                    mode: left_mode,
                    modified: left_modified,
                    length: left_length,
                    digest: left_digest,
                    extents: left_extents,
                    linked_to: left_link,
                    payload: _,
                },
                Self::File {
                    mode: right_mode,
                    modified: right_modified,
                    length: right_length,
                    digest: right_digest,
                    extents: right_extents,
                    linked_to: right_link,
                    payload: _,
                },
            ) => {
                left_mode == right_mode
                    && left_modified == right_modified
                    && left_length == right_length
                    && left_digest == right_digest
                    && left_extents == right_extents
                    && left_link == right_link
            }
            (Self::Symlink(left), Self::Symlink(right)) => left == right,
            (Self::Directory { .. } | Self::File { .. } | Self::Symlink(_), _) => false,
        }
    }
}

impl Eq for Entry {}

struct Root {
    authority: OwnedFd,
    destination: PathBuf,
    source: Option<File>,
    directory: bool,
    exclusions: Vec<PathBuf>,
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
            let authority = bind.duplicate().map_err(|source| {
                failed("writable root authority could not be retained", source)
            })?;
            specifications.push((
                bind.host().to_path_buf(),
                authority,
                bind.destination().to_path_buf(),
                bind.directory(),
                view.exclusions_beneath(bind.destination()),
            ));
        }
        if let Some(materialization) = materialization {
            for mount in materialization
                .mounts()
                .iter()
                .filter(|mount| mount.access() == SandboxFilesystemAccess::ReadWrite)
            {
                let authority = mount.duplicate().map_err(|source| {
                    failed("writable mount authority could not be retained", source)
                })?;
                specifications.push((
                    mount.host().to_path_buf(),
                    authority,
                    mount.destination().to_path_buf(),
                    mount.directory(),
                    Vec::new(),
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
        for (index, (_host, authority, destination, directory, exclusions)) in
            specifications.into_iter().enumerate()
        {
            let pinned = descriptor_path(authority.as_raw_fd());
            let before = snapshot_filtered(&pinned, &exclusions)
                .map_err(|source| failed("writable root could not be fingerprinted", source))?;
            let source = if directory {
                None
            } else {
                let staged = roots_directory.join(index.to_string());
                copy_root(&pinned, &staged)
                    .map_err(|source| failed("writable root could not be projected", source))?;
                let copied = snapshot_filtered(&staged, &exclusions)
                    .map_err(|source| failed("projected root could not be verified", source))?;
                if before != copied {
                    return Err(refused(
                        "writable file changed while its private projection was prepared",
                    ));
                }
                let source = File::open(&staged)
                    .map_err(|source| failed("projected root could not be pinned", source))?;
                Some(source)
            };
            let after = snapshot_filtered(&pinned, &exclusions)
                .map_err(|source| failed("writable root could not be revalidated", source))?;
            if before != after {
                return Err(refused(
                    "writable root changed while its private projection was prepared",
                ));
            }
            roots.push(Root {
                authority,
                destination,
                source,
                directory,
                exclusions,
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
            .and_then(|root| root.source.as_ref().map(AsRawFd::as_raw_fd))
    }

    pub(super) fn uses_overlay(&self, destination: &Path) -> bool {
        self.roots
            .iter()
            .any(|root| root.destination == destination && root.directory)
    }

    pub(super) fn destinations(&self) -> impl Iterator<Item = &Path> {
        self.roots.iter().map(|root| root.destination.as_path())
    }

    pub(super) fn excluded_destinations(&self) -> impl Iterator<Item = PathBuf> + '_ {
        self.roots.iter().flat_map(|root| {
            root.exclusions
                .iter()
                .map(|relative| root.destination.join(relative))
        })
    }

    fn publish(
        &mut self,
        broker_baselines: &[Snapshot],
        finals: &[Snapshot],
    ) -> Result<(), publish::Failure> {
        if self.published {
            return Ok(());
        }
        let canonical = publish::reconcile(&self.roots, broker_baselines, finals)?;
        publish::apply(&self.roots, self.stage.root(), &canonical)?;
        self.published = true;
        Ok(())
    }
}

impl Root {
    fn publication_path(&self) -> PathBuf {
        descriptor_path(self.authority.as_raw_fd())
    }
}

fn descriptor_path(descriptor: RawFd) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{descriptor}"))
}

fn staging_root(request: &SandboxRequest) -> Result<PathBuf, SandboxError> {
    for base in [Path::new("/var/tmp"), Path::new("/tmp")] {
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

fn snapshot_filtered(root: &Path, exclusions: &[PathBuf]) -> io::Result<Snapshot> {
    let mut builder = SnapshotBuilder {
        exclusions: exclusions.to_vec(),
        ..SnapshotBuilder::default()
    };
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
    exclusions: Vec<PathBuf>,
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
        validate_publishable_metadata(&path, &metadata)?;
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
                extents: sparse_extents(&path, metadata.len())?,
                linked_to,
                payload: None,
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
                let child = relative.join(name);
                if self
                    .exclusions
                    .iter()
                    .any(|excluded| child == *excluded || child.starts_with(excluded))
                {
                    continue;
                }
                self.visit(root, &child, depth.saturating_add(1))?;
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

fn copy_root(source: &Path, destination: &Path) -> io::Result<()> {
    copy_file(source, destination)?;
    copy_metadata(source, destination, false)
}

fn copy_file(source: &Path, destination: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    options.mode(0o600);
    let mut output = options.open(destination)?;
    copy_file_into(source, &mut output)
}

fn copy_file_into(source: &Path, output: &mut File) -> io::Result<()> {
    let metadata = fs::metadata(source)?;
    let extents = sparse_extents(source, metadata.len())?;
    output.set_len(0)?;
    output.set_len(metadata.len())?;
    let mut input = File::open(source)?;
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    for (offset, length) in extents {
        input.seek(SeekFrom::Start(offset))?;
        output.seek(SeekFrom::Start(offset))?;
        copy_exact(&mut input, &mut *output, length, &mut buffer)?;
    }
    output.sync_all()
}

fn sparse_extents(path: &Path, length: u64) -> io::Result<Vec<(u64, u64)>> {
    use rustix::fs::SeekFrom as RustixSeekFrom;
    use rustix::io::Errno;

    let file = File::open(path)?;
    let mut extents = Vec::new();
    let mut cursor = 0_u64;
    while cursor < length {
        let data = match rustix::fs::seek(&file, RustixSeekFrom::Data(cursor)) {
            Ok(data) => data,
            Err(Errno::NXIO) => break,
            Err(problem) => return Err(problem.into()),
        };
        if data >= length {
            break;
        }
        let hole = rustix::fs::seek(&file, RustixSeekFrom::Hole(data))?.min(length);
        if hole <= data {
            return Err(io::Error::other("file extent map did not advance"));
        }
        extents.push((data, hole.saturating_sub(data)));
        if extents.len() > MAX_SCAN_EXTENTS {
            return Err(io::Error::other("file extent count exceeds its bound"));
        }
        cursor = hole;
    }
    Ok(extents)
}

fn copy_exact(
    source: &mut impl Read,
    destination: &mut impl Write,
    mut remaining: u64,
    buffer: &mut [u8],
) -> io::Result<()> {
    while remaining > 0 {
        let maximum = u64::try_from(buffer.len())
            .map_err(|_| io::Error::other("copy buffer length is invalid"))?;
        let wanted = usize::try_from(remaining.min(maximum))
            .map_err(|_| io::Error::other("copy chunk length is invalid"))?;
        let chunk = buffer
            .get_mut(..wanted)
            .ok_or_else(|| io::Error::other("copy chunk exceeded its buffer"))?;
        source.read_exact(chunk)?;
        destination.write_all(chunk)?;
        remaining = remaining.saturating_sub(
            u64::try_from(wanted).map_err(|_| io::Error::other("copy byte count overflow"))?,
        );
    }
    Ok(())
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

fn validate_publishable_metadata(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    use rustix::io::Errno;

    if metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.gid() != rustix::process::getgid().as_raw()
    {
        return Err(io::Error::other(
            "writable projection contains ownership the publisher cannot preserve",
        ));
    }
    if metadata.mode() & 0o7000 != 0 {
        return Err(io::Error::other(
            "writable projection contains special mode bits the publisher cannot preserve",
        ));
    }
    let mut names = [0_u8; 4096];
    match rustix::fs::llistxattr(path, &mut names) {
        Ok(0) | Err(Errno::NOTSUP) => Ok(()),
        Ok(_) | Err(Errno::RANGE) => Err(io::Error::other(
            "writable projection contains extended metadata the publisher cannot preserve",
        )),
        Err(problem) => Err(problem.into()),
    }
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
    audit: SandboxAudit,
    sandbox: SandboxId,
) -> io::Result<Box<dyn SandboxProcess>> {
    let stage = projection
        .as_ref()
        .map(|projection| projection.stage.root().to_path_buf());
    let control = status_channel.into_stream();
    let receiver = protocol::Receiver::spawn(control.try_clone()?, stage)?;
    Ok(Box::new(ProjectedProcess {
        process,
        projection,
        receiver: Some(receiver),
        status: None,
        terminal: false,
        audit,
        sandbox,
        control: Some(control),
    }))
}

struct ProjectedProcess {
    process: Box<dyn SandboxProcess>,
    projection: Option<Projection>,
    receiver: Option<protocol::Receiver>,
    status: Option<ExitStatus>,
    terminal: bool,
    audit: SandboxAudit,
    sandbox: SandboxId,
    control: Option<std::os::unix::net::UnixStream>,
}

impl ProjectedProcess {
    fn lifecycle(&self, lifecycle: SandboxLifecycle) -> io::Result<()> {
        self.audit
            .record(self.sandbox, SandboxFactKind::Lifecycle(lifecycle))
            .map_err(io::Error::other)
    }
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
        let terminal = self
            .receiver
            .as_mut()
            .ok_or_else(|| io::Error::other("sandbox terminal scan receiver is unavailable"))?
            .finish();
        let terminal = match terminal {
            Ok(terminal) => terminal,
            Err(problem) => {
                let discarded = self.projection.take().is_some() && !self.terminal;
                self.receiver.take();
                self.control.take();
                self.terminal = true;
                if discarded {
                    self.lifecycle(SandboxLifecycle::RolledBack)?;
                }
                return Err(problem);
            }
        };
        self.receiver.take();
        self.control.take();
        let status = terminal.status;
        if !self.terminal {
            if status.signal().is_none()
                && self.process.violation().is_none()
                && self.projection.is_some()
            {
                self.lifecycle(SandboxLifecycle::PublicationStarted)?;
                let publication = self
                    .projection
                    .as_mut()
                    .ok_or_else(|| io::Error::other("sandbox projection disappeared"))?
                    .publish(&terminal.baselines, &terminal.roots);
                match publication {
                    Ok(()) => self.lifecycle(SandboxLifecycle::Published)?,
                    Err(problem) => {
                        let lifecycle = if problem.requires_quarantine() {
                            SandboxLifecycle::Quarantined
                        } else {
                            SandboxLifecycle::RolledBack
                        };
                        self.lifecycle(lifecycle)?;
                        return Err(problem.into_io());
                    }
                }
            } else if self.projection.is_some() {
                self.lifecycle(SandboxLifecycle::RolledBack)?;
            } else if self.projection.is_none() && !terminal.roots.is_empty() {
                return Err(io::Error::other(
                    "sandbox broker reported roots outside the immutable projection plan",
                ));
            }
            self.terminal = true;
        }
        self.status = Some(status);
        Ok(Some(status))
    }

    fn stop(&mut self) -> io::Result<()> {
        if self.status.is_none() {
            if self.projection.is_some() && !self.terminal {
                self.lifecycle(SandboxLifecycle::RolledBack)?;
            }
            self.terminal = true;
        }
        let cancellation = self.control.as_mut().map_or(Ok(()), |control| {
            control
                .write_all(&CANCEL_FRAME)
                .and_then(|()| control.flush())
        });
        let result = self.process.stop();
        if let Some(receiver) = &mut self.receiver {
            let _ = receiver.finish();
        }
        self.receiver.take();
        self.control.take();
        self.projection.take();
        cancellation.and(result)
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

    #[test]
    fn unsupported_extended_and_special_metadata_is_refused() {
        use rustix::fs::XattrFlags;
        use rustix::io::Errno;

        let sample = crate::sample::Sample::new("sandbox-publication-metadata");
        let path = sample.root().join("metadata.txt");
        std::fs::write(&path, "metadata\n").expect("fixture");
        match rustix::fs::setxattr(&path, "user.crucible-test", b"value", XattrFlags::empty()) {
            Ok(()) => {
                let metadata = std::fs::symlink_metadata(&path).expect("metadata");
                assert!(validate_publishable_metadata(&path, &metadata).is_err());
                rustix::fs::removexattr(&path, "user.crucible-test").expect("remove xattr");
            }
            Err(Errno::NOTSUP) => {}
            Err(problem) => panic!("could not create xattr fixture: {problem}"),
        }

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o4755))
            .expect("special mode fixture");
        let metadata = std::fs::symlink_metadata(&path).expect("metadata");
        assert!(validate_publishable_metadata(&path, &metadata).is_err());
    }
}
