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

use crucible_sandbox::{
    SandboxAudit, SandboxError, SandboxFactKind, SandboxFilesystemAccess, SandboxInspection,
    SandboxInvocationMode, SandboxLifecycle, SandboxOutput, SandboxProcess, SandboxRequest,
    SandboxUsage, SandboxViolation,
};
use crucible_sandbox_broker::{CANCEL_FRAME, MAX_SCAN_EXTENTS, MAX_SCAN_FILE_BYTES};
use crucible_storage::{CallResultKey, CallResultReceipt};
use crucible_types::SandboxId;
use sha2::{Digest as _, Sha256};

use super::super::process::Stage;
use super::broker::StatusChannel;
use super::command::View;
use super::materialize::Materialization;
use super::transaction;

const MAX_PROJECTED_ENTRIES: usize = 262_144;
const MAX_PROJECTED_DEPTH: usize = 64;

/// How long a command being prepared waits for a publication to finish before
/// it is refused instead.
///
/// Refused rather than waited out, because the lock may be held by another
/// crucible of this user, and a caller can act on a refusal. Shorter under test,
/// where the holder is a fixture rather than a command.
#[cfg(not(test))]
const PUBLICATION_PATIENCE: std::time::Duration = std::time::Duration::from_mins(1);
#[cfg(test)]
const PUBLICATION_PATIENCE: std::time::Duration = std::time::Duration::from_secs(2);

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
    /// Where the root is on this machine, which is what a publication into it
    /// is remembered under. The destination below is only the name the sandbox
    /// gives it, and two names for one root would be remembered apart.
    host: PathBuf,
    destination: PathBuf,
    source: Option<File>,
    directory: bool,
    exclusions: Vec<PathBuf>,
    baseline: Snapshot,
    /// What this user's publications had done to this root when the baseline was
    /// taken. `None` where nothing remembered it, which counts as moved.
    generation: Option<u64>,
}

/// One durable command lifecycle plus any host-owned writable copies.
/// None of their host pathnames reaches the workload.
pub(super) struct Projection {
    /// This user's transaction state directory, where the publication lock lives.
    state: PathBuf,
    stage: Stage,
    roots: Vec<Root>,
    published: bool,
    transaction: transaction::Transaction,
}

/// Leave for one projection to publish now.
///
/// Holds this user's publication lock for as long as it lives, where the
/// projection has a root to publish into.
struct Admission {
    _lease: Option<transaction::Lease>,
}

impl Admission {
    /// Whether the lock this admission was let in on is still that lock.
    #[expect(
        clippy::used_underscore_binding,
        reason = "the admission holds the lease only to keep the lock; confirming reads it"
    )]
    fn confirm(&self) -> io::Result<()> {
        self._lease
            .as_ref()
            .map_or(Ok(()), transaction::Lease::confirm)
    }
}

impl Projection {
    pub(super) fn network_socket(&self) -> PathBuf {
        self.stage.root().join("network.sock")
    }

    pub(super) fn prepare(
        request: &SandboxRequest,
        view: &View,
        materialization: Option<&Materialization>,
    ) -> Result<Self, SandboxError> {
        let registry = transaction::RegistryLease::acquire(request)?;
        transaction::RegistryLease::reconcile(&registry).map_err(|source| {
            failed(
                "stale sandbox lifecycle requires recovery or review",
                source,
            )
        })?;
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
        specifications.sort_by(|left, right| {
            left.0
                .components()
                .count()
                .cmp(&right.0.components().count())
                .then_with(|| left.0.cmp(&right.0))
                .then_with(|| left.2.cmp(&right.2))
        });

        let state_directory = transaction::state_directory(request)?;
        let root = staging_root(request)?;
        create_private_directory(&root)
            .map_err(|source| failed("could not create writable projection", source))?;
        let stage = Stage::new(root);
        let mut transaction = transaction::Transaction::start(
            stage.root(),
            request.id(),
            match request.invocation_mode() {
                crucible_sandbox::SandboxInvocationMode::Foreground => {
                    transaction::InvocationMode::Foreground
                }
                crucible_sandbox::SandboxInvocationMode::Detachable => {
                    transaction::InvocationMode::Detachable
                }
                crucible_sandbox::SandboxInvocationMode::Background => {
                    transaction::InvocationMode::Background
                }
            },
            request.call_result_key(),
        )
        .map_err(|source| failed("could not initialize writable transaction journal", source))?;
        drop(registry);
        // A baseline taken while another command publishes could be half of that
        // publication, so the baselines wait for it to finish. A projection with
        // no writable root has no baseline to take, and does not ask.
        let _publication = if specifications.is_empty() {
            None
        } else {
            match transaction::Lease::acquire_in(&state_directory, PUBLICATION_PATIENCE) {
                Ok(Some(lease)) => Some(lease),
                // The refusal this path gave before the lock was narrowed. A
                // caller can act on it; a wait with no end only holds the turn.
                Ok(None) => return Err(SandboxError::Concurrency),
                Err(source) => {
                    return Err(failed(
                        "the writable publication lock is unavailable",
                        source,
                    ));
                }
            }
        };
        let roots_directory = stage.root().join("roots");
        create_private_directory(&roots_directory)
            .map_err(|source| failed("could not create projected roots", source))?;

        let mut roots = Vec::with_capacity(specifications.len());
        for (index, (host, authority, destination, directory, exclusions)) in
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
            // Read while the publication lock is held, with the baseline, so
            // the two describe the same moment.
            let generation = super::generations::current(
                &state_directory,
                std::slice::from_ref(&super::generations::key(&host)),
            )
            .map_err(|source| failed("writable root generations are unavailable", source))?
            .into_iter()
            .next()
            .flatten();
            roots.push(Root {
                authority,
                host,
                destination,
                source,
                directory,
                exclusions,
                baseline: before,
                generation,
            });
        }
        transaction
            .append(transaction::Record::Prepared)
            .map_err(|source| failed("could not durably prepare writable transaction", source))?;
        Ok(Self {
            state: state_directory,
            stage,
            roots,
            published: false,
            transaction,
        })
    }

    pub(super) fn record(&mut self, record: transaction::Record) -> io::Result<()> {
        self.transaction.append(record)
    }

    pub(super) fn refuse(&mut self, cleanup_proved: bool) -> io::Result<()> {
        self.record(transaction::Record::RefusalObserved)?;
        self.record(transaction::Record::PreparationCleanupIntent)?;
        self.record(if cleanup_proved {
            transaction::Record::PreparationCleanupProved
        } else {
            transaction::Record::PreparationCleanupUnproved
        })?;
        self.record(if cleanup_proved {
            transaction::Record::Refused
        } else {
            transaction::Record::Quarantined
        })
    }

    pub(super) fn abort(&mut self, scope_reaped: bool) -> io::Result<()> {
        self.record(transaction::Record::AbortObserved)?;
        self.record(transaction::Record::ScopeReapIntent)?;
        self.record(if scope_reaped {
            transaction::Record::ScopeReapProved
        } else {
            transaction::Record::ScopeReapUnproved
        })?;
        self.record(if scope_reaped {
            transaction::Record::RolledBack
        } else {
            transaction::Record::Quarantined
        })
    }

    pub(super) fn retain_evidence(&mut self) {
        self.stage.retain();
    }

    /// Removes the stage with its journal last.
    ///
    /// The journal stays open and locked here, so a concurrent reconcile
    /// finds it busy and skips this stage until only the journal remains. The
    /// other order leaves a journal-less stage full of roots for as long as
    /// their removal takes, which a reconcile can only read as damage.
    pub(super) fn cleanup(&mut self) -> io::Result<()> {
        if !self.stage.retained() {
            transaction::clear_stage_before_journal(self.stage.root())?;
        }
        self.stage.cleanup()
    }

    fn record_terminal_scan(&mut self) -> io::Result<()> {
        for record in [
            transaction::Record::CommandExited,
            transaction::Record::WorkloadReapIntent,
            transaction::Record::WorkloadReaped,
            transaction::Record::ScanIntent,
            transaction::Record::ScanTransferred,
        ] {
            self.record(record)?;
        }
        Ok(())
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

    /// Leave to publish now, or `None` while another publication holds the lock.
    ///
    /// A projection with no writable root has nothing to publish into, and is
    /// let in without the lock.
    fn admission(&self) -> io::Result<Option<Admission>> {
        if self.roots.is_empty() {
            return Ok(Some(Admission { _lease: None }));
        }
        let lease = transaction::Lease::try_acquire_in(&self.state).map_err(|source| {
            io::Error::new(
                source.kind(),
                format!("the writable publication lock is unavailable: {source}"),
            )
        })?;
        Ok(lease.map(|lease| Admission {
            _lease: Some(lease),
        }))
    }

    /// Rolls the transaction back for a publication that will not happen.
    fn abort_publication(&mut self, problem: io::Error) -> publish::Failure {
        match self.transaction.finish_abort(false) {
            Ok(()) => publish::Failure::rolled_back(problem),
            Err(journal) => {
                self.retain_evidence();
                publish::Failure::quarantined(io::Error::other(format!(
                    "publication was refused and rollback could not be journaled: {problem}; {journal}"
                )))
            }
        }
    }

    /// Publishes the command's writes, which only a caller let in can ask for.
    fn publish(
        &mut self,
        admission: &Admission,
        broker_baselines: &[Snapshot],
        finals: &[Snapshot],
    ) -> Result<(), publish::Failure> {
        if self.published {
            return Ok(());
        }
        let canonical = match publish::reconcile(&self.roots, broker_baselines, finals) {
            Ok(canonical) => canonical,
            Err(problem) => {
                return match self.transaction.finish_abort(false) {
                    Ok(()) => Err(publish::Failure::rolled_back(problem)),
                    Err(journal) => {
                        self.retain_evidence();
                        Err(publish::Failure::quarantined(io::Error::other(format!(
                            "terminal reconciliation failed and rollback could not be journaled: {problem}; {journal}"
                        ))))
                    }
                };
            }
        };
        // Asked again before anything is written: a lock file removed and
        // remade under this lease would leave two publications each holding
        // what it believes is the only one.
        if let Err(problem) = admission.confirm() {
            return Err(self.abort_publication(problem));
        }
        let publication = publish::apply(
            &self.roots,
            self.stage.root(),
            &publish::Seen {
                first: broker_baselines,
                last: finals,
                state: &self.state,
            },
            &canonical,
            &mut self.transaction,
        );
        if publication
            .as_ref()
            .is_err_and(publish::Failure::requires_quarantine)
        {
            self.retain_evidence();
        }
        publication?;
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

/// The stage lives inside this user's private transaction state directory.
///
/// That directory is owner-only, so stale-stage reconciliation reads only
/// entries this user created: a shared temporary directory would let any local
/// user pad the scan with look-alike names. The state directory already refuses
/// to overlap the requested filesystem view.
fn staging_root(request: &SandboxRequest) -> Result<PathBuf, SandboxError> {
    Ok(transaction::state_directory(request)?.join(transaction::stage_name(request.id())))
}

/// Reads one directory's children in name order without buffering more of them
/// than the entry bound still allows.
fn bounded_children(path: &Path, retained: usize) -> io::Result<Vec<fs::DirEntry>> {
    let mut children = Vec::new();
    for child in fs::read_dir(path)? {
        if retained.saturating_add(children.len()) >= MAX_PROJECTED_ENTRIES {
            return Err(io::Error::other("projected tree exceeds its entry bound"));
        }
        children.push(child?);
    }
    children.sort_by_key(fs::DirEntry::file_name);
    Ok(children)
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
            if metadata.len() > MAX_SCAN_FILE_BYTES {
                return Err(io::Error::other("projected file exceeds its byte bound"));
            }
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
            let children = bounded_children(&path, self.retained)?;
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
pub(super) struct ProcessPlan {
    pub(super) projection: Option<Projection>,
    pub(super) status_channel: StatusChannel,
    pub(super) audit: SandboxAudit,
    pub(super) sandbox: SandboxId,
    pub(super) invocation: SandboxInvocationMode,
    pub(super) call_result_key: Option<CallResultKey>,
    /// Writers in one test process run one at a time, for as long as each runs.
    #[cfg(test)]
    pub(super) serial: Option<transaction::TestSerialLease>,
}

pub(super) fn wrap(
    mut process: Box<dyn SandboxProcess>,
    plan: ProcessPlan,
) -> io::Result<Box<dyn SandboxProcess>> {
    let ProcessPlan {
        mut projection,
        status_channel,
        audit,
        sandbox,
        invocation,
        call_result_key,
        #[cfg(test)]
        serial,
    } = plan;
    let stage = projection
        .as_ref()
        .map(|projection| projection.stage.root().to_path_buf());
    let inspection = process.inspection().clone();
    let control = status_channel.into_stream();
    let receiver_stream = match control.try_clone() {
        Ok(stream) => stream,
        Err(source) => {
            drop(control);
            cleanup_failed_wrap(process.as_mut(), projection.as_mut(), &audit, sandbox);
            return Err(source);
        }
    };
    let receiver = match protocol::Receiver::spawn(receiver_stream, stage) {
        Ok(receiver) => receiver,
        Err(source) => {
            drop(control);
            cleanup_failed_wrap(process.as_mut(), projection.as_mut(), &audit, sandbox);
            return Err(source);
        }
    };
    Ok(Box::new(ProjectedProcess {
        process,
        projection,
        receiver: Some(receiver),
        status: None,
        terminal: false,
        reported: None,
        failure: None,
        unrecorded: None,
        audit,
        sandbox,
        control: Some(control),
        invocation,
        call_result_key,
        acceptance_pending: false,
        inspection,
        cleanup: crucible_sandbox::SandboxCleanup::Pending,
        #[cfg(test)]
        _serial: serial,
    }))
}

fn cleanup_failed_wrap(
    process: &mut dyn SandboxProcess,
    mut projection: Option<&mut Projection>,
    audit: &SandboxAudit,
    sandbox: SandboxId,
) {
    let _ = process.stop();
    let scope_reaped = process.inspection().cleanup() == crucible_sandbox::SandboxCleanup::Complete;
    let rolled_back = projection
        .as_deref_mut()
        .map_or(Ok(()), |projection| projection.abort(scope_reaped));
    let lifecycle = if scope_reaped && rolled_back.is_ok() {
        SandboxLifecycle::RolledBack
    } else {
        if let Some(projection) = projection.as_deref_mut() {
            projection.retain_evidence();
        }
        SandboxLifecycle::Quarantined
    };
    let _ = audit
        .record(sandbox, SandboxFactKind::Lifecycle(lifecycle))
        .map_err(io::Error::other);
    let projection_cleanup = projection.map_or(Ok(()), Projection::cleanup);
    let cleanup = if scope_reaped && projection_cleanup.is_ok() {
        crucible_sandbox::SandboxCleanup::Complete
    } else {
        crucible_sandbox::SandboxCleanup::Failed
    };
    let _ = audit.record(sandbox, SandboxFactKind::Cleanup(cleanup));
}

struct ProjectedProcess {
    process: Box<dyn SandboxProcess>,
    projection: Option<Projection>,
    receiver: Option<protocol::Receiver>,
    status: Option<ExitStatus>,
    terminal: bool,
    /// The broker's terminal report, from when it is read until what the command
    /// wrote is published or discarded.
    ///
    /// Kept because it can be read only once, and a clean ending that has to wait
    /// its turn to publish is asked about again on the next look.
    reported: Option<protocol::Terminal>,
    /// How the ending went wrong, where it did, answered again on every later
    /// look.
    failure: Option<(io::ErrorKind, Box<str>)>,
    /// A fact about a publication that went ahead, which the audit could not
    /// take. Reported with the cleanup rather than as the ending.
    unrecorded: Option<io::Error>,
    audit: SandboxAudit,
    sandbox: SandboxId,
    control: Option<std::os::unix::net::UnixStream>,
    invocation: SandboxInvocationMode,
    call_result_key: Option<CallResultKey>,
    acceptance_pending: bool,
    inspection: SandboxInspection,
    cleanup: crucible_sandbox::SandboxCleanup,
    #[cfg(test)]
    _serial: Option<transaction::TestSerialLease>,
}

impl ProjectedProcess {
    fn lifecycle(&self, lifecycle: SandboxLifecycle) -> io::Result<()> {
        self.audit
            .record(self.sandbox, SandboxFactKind::Lifecycle(lifecycle))
            .map_err(io::Error::other)
    }

    fn audit_cleanup(&self, cleanup: crucible_sandbox::SandboxCleanup) -> io::Result<()> {
        self.audit
            .record(self.sandbox, SandboxFactKind::Cleanup(cleanup))
            .map_err(io::Error::other)
    }

    /// Settles an ending that went wrong, so every later look gives the same answer.
    fn failed(&mut self, problem: io::Error) -> io::Error {
        self.terminal = true;
        self.reported = None;
        self.failure = Some((problem.kind(), problem.to_string().into()));
        problem
    }

    /// Discards what the command wrote, and records how that went.
    fn discard(&mut self) -> io::Result<()> {
        let Some(projection) = self.projection.as_mut() else {
            return Ok(());
        };
        if let Err(problem) = projection.abort(true) {
            projection.retain_evidence();
            self.projection.take();
            self.lifecycle(SandboxLifecycle::Quarantined)?;
            return Err(problem);
        }
        self.lifecycle(SandboxLifecycle::RolledBack)
    }

    /// Reads the broker's terminal report once it has exited, and journals the scan.
    fn report(&mut self) -> io::Result<()> {
        let Some(terminal) = self.receiver.as_mut().map(protocol::Receiver::finish) else {
            return Err(self.failed(io::Error::other(
                "sandbox terminal scan receiver is unavailable",
            )));
        };
        self.receiver.take();
        self.control.take();
        let terminal = match terminal {
            Ok(terminal) => terminal,
            Err(problem) => {
                let problem = self.failed(problem);
                if let Some(projection) = self.projection.as_mut() {
                    let lifecycle = if projection.abort(true).is_ok() {
                        SandboxLifecycle::RolledBack
                    } else {
                        projection.retain_evidence();
                        SandboxLifecycle::Quarantined
                    };
                    if let Err(cleanup) = self.lifecycle(lifecycle) {
                        return Err(self.failed(cleanup));
                    }
                }
                return Err(problem);
            }
        };
        if let Some(projection) = self.projection.as_mut()
            && let Err(problem) = projection.record_terminal_scan()
        {
            let _ = projection.abort(true);
            projection.retain_evidence();
            let problem = self.failed(problem);
            if let Err(cleanup) = self.lifecycle(SandboxLifecycle::Quarantined) {
                return Err(self.failed(cleanup));
            }
            return Err(problem);
        }
        self.reported = Some(terminal);
        Ok(())
    }

    /// Publishes or discards what the command wrote, once its report is read.
    ///
    /// Only a clean ending publishes, so only a clean ending asks for the lock: a
    /// command killed, or stopped by a limit, is discarded without waiting for
    /// another command's publication.
    fn conclude(&mut self) -> io::Result<Option<ExitStatus>> {
        let Some(reported) = self.reported.take() else {
            return Err(self.failed(io::Error::other("sandbox terminal report is unavailable")));
        };
        let status = reported.status;
        if self.projection.is_none() {
            if !reported.roots.is_empty() {
                return Err(self.failed(io::Error::other(
                    "sandbox broker reported roots outside the immutable projection plan",
                )));
            }
        } else if status.signal().is_none() && self.process.violation().is_none() {
            let admitted = self
                .projection
                .as_ref()
                .map_or(Ok(None), Projection::admission);
            let admission = match admitted {
                Ok(Some(admission)) => admission,
                // Another command is publishing. This one is asked about again on
                // the next look rather than waited for here, where the caller may
                // be the thread that draws.
                Ok(None) => {
                    self.reported = Some(reported);
                    return Ok(None);
                }
                Err(problem) => {
                    let problem = self.failed(problem);
                    // The cleanup's own failure becomes the answer, so that the
                    // look that returns it and every later look agree.
                    if let Err(cleanup) = self.discard() {
                        return Err(self.failed(cleanup));
                    }
                    return Err(problem);
                }
            };
            if let Err(problem) = self.lifecycle(SandboxLifecycle::PublicationStarted) {
                let problem = self.failed(problem);
                if let Err(cleanup) = self.discard() {
                    return Err(self.failed(cleanup));
                }
                return Err(problem);
            }
            let publication = self.projection.as_mut().map_or(Ok(()), |projection| {
                projection.publish(&admission, &reported.baselines, &reported.roots)
            });
            if let Err(problem) = publication {
                let lifecycle = if problem.requires_quarantine() {
                    SandboxLifecycle::Quarantined
                } else {
                    SandboxLifecycle::RolledBack
                };
                let problem = self.failed(problem.into_io());
                if let Err(cleanup) = self.lifecycle(lifecycle) {
                    return Err(self.failed(cleanup));
                }
                return Err(problem);
            }
            self.terminal = true;
            self.status = Some(status);
            // The publication is done. A fact that cannot be recorded is a
            // cleanup failure, not an ending that went wrong: reported as one, it
            // would tell the model to write everything again over the files that
            // are already there.
            if let Err(problem) = self.lifecycle(SandboxLifecycle::Published) {
                self.unrecorded = Some(problem);
            }
            return Ok(Some(status));
        } else if let Err(problem) = self.discard() {
            return Err(self.failed(problem));
        }
        self.terminal = true;
        self.status = Some(status);
        Ok(Some(status))
    }
}

impl SandboxProcess for ProjectedProcess {
    fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
        self.process.take_stdin()
    }

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
        if let Some((kind, problem)) = &self.failure {
            return Err(io::Error::new(*kind, problem.to_string()));
        }
        if self.terminal {
            // Stopped before its report was read, so how the leader ended is how
            // the command ended. A zero here says the leader exited, not that
            // what the command wrote was published: stopping discards whatever
            // had not been published yet.
            return self.process.try_wait();
        }
        if self.reported.is_none() {
            if self.process.try_wait()?.is_none() {
                return Ok(None);
            }
            self.report()?;
        }
        self.conclude()
    }

    fn ended(&mut self) -> bool {
        self.status.is_some()
            || self.terminal
            || self.reported.is_some()
            || matches!(self.process.try_wait(), Ok(Some(_)))
    }

    fn stop(&mut self) -> io::Result<()> {
        if self.cleanup != crucible_sandbox::SandboxCleanup::Pending {
            return if self.cleanup == crucible_sandbox::SandboxCleanup::Complete {
                Ok(())
            } else {
                Err(io::Error::other("sandbox cleanup previously failed"))
            };
        }
        let needs_terminal = self.status.is_none() && !self.terminal;
        let cancellation = self.control.as_mut().map_or(Ok(()), |control| {
            match control
                .write_all(&CANCEL_FRAME)
                .and_then(|()| control.flush())
            {
                // A broker that has exited took its end of the socket with it and
                // has nothing left to cancel. That its scope has ended is what the
                // stop below confirms.
                Err(problem) if problem.kind() == io::ErrorKind::BrokenPipe => Ok(()),
                sent => sent,
            }
        });
        self.reported = None;
        let process_cleanup = self.process.stop();
        let scope_reaped =
            self.process.inspection().cleanup() == crucible_sandbox::SandboxCleanup::Complete;
        if let Some(receiver) = &mut self.receiver {
            let _ = receiver.finish();
        }
        self.receiver.take();
        self.control.take();
        let mut terminal_cleanup = Ok(());
        if needs_terminal {
            let lifecycle = self.projection.as_mut().map(|projection| {
                let aborted = projection.abort(scope_reaped);
                if scope_reaped && aborted.is_ok() {
                    SandboxLifecycle::RolledBack
                } else {
                    projection.retain_evidence();
                    SandboxLifecycle::Quarantined
                }
            });
            if let Some(lifecycle) = lifecycle {
                terminal_cleanup = self.lifecycle(lifecycle);
            }
            self.terminal = true;
        }
        let projection_cleanup = self.projection.as_mut().map_or(Ok(()), Projection::cleanup);
        let projection_cleaned = projection_cleanup.is_ok();
        if projection_cleaned {
            self.projection.take();
        }
        let cleanup = if scope_reaped && projection_cleaned {
            crucible_sandbox::SandboxCleanup::Complete
        } else {
            crucible_sandbox::SandboxCleanup::Failed
        };
        let unrecorded = self.unrecorded.take().map_or(Ok(()), Err);
        let mut result = cancellation
            .and(process_cleanup)
            .and(terminal_cleanup)
            .and(projection_cleanup)
            .and(unrecorded);
        self.inspection = self.inspection.clone().cleaned(cleanup);
        self.cleanup = cleanup;
        let audited = self.audit_cleanup(cleanup);
        result = result.and(audited);
        result
    }

    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn usage(&self) -> SandboxUsage {
        self.process.usage()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        self.process.violation()
    }

    fn begin_background_acceptance(&mut self, key: CallResultKey) -> Result<(), SandboxError> {
        if self.invocation == SandboxInvocationMode::Foreground
            || self.call_result_key.is_none()
            || self.call_result_key != Some(key)
            || self.acceptance_pending
        {
            return Err(SandboxError::Lifecycle(io::Error::other(
                "sandbox background result identity is invalid",
            )));
        }
        let projection = self.projection.as_mut().ok_or_else(|| {
            SandboxError::Lifecycle(io::Error::other(
                "background sandbox has no durable transaction",
            ))
        })?;
        projection
            .record(transaction::Record::CallAcceptIntent)
            .map_err(SandboxError::Lifecycle)?;
        self.acceptance_pending = true;
        Ok(())
    }

    fn complete_background_acceptance(
        &mut self,
        receipt: CallResultReceipt,
    ) -> Result<(), SandboxError> {
        if !self.acceptance_pending {
            return Err(SandboxError::Lifecycle(io::Error::other(
                "sandbox background result intent is unavailable",
            )));
        }
        let projection = self.projection.as_mut().ok_or_else(|| {
            SandboxError::Lifecycle(io::Error::other(
                "background sandbox has no durable transaction",
            ))
        })?;
        if let Err(source) = projection.record(transaction::Record::CallAccepted(receipt.bytes())) {
            let _ = self.process.stop();
            projection.retain_evidence();
            let _ = self.lifecycle(SandboxLifecycle::Quarantined);
            self.terminal = true;
            return Err(SandboxError::Lifecycle(source));
        }
        self.acceptance_pending = false;
        Ok(())
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
