//! Private writable-root projection and terminal publication.

mod authority;
pub(super) mod bounded;
mod protocol;
mod publish;
#[cfg(test)]
mod stop_tests;

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Read, Seek as _, SeekFrom, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::{Arc, Mutex};

use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxAudit, SandboxError, SandboxFactKind, SandboxFilesystemAccess, SandboxInspection,
    SandboxInvocationMode, SandboxLifecycle, SandboxOutput, SandboxProcess, SandboxRequest,
    SandboxUsage, SandboxViolation,
};
use crucible_sandbox_broker::{CANCEL_FRAME, MAX_SCAN_EXTENTS, MAX_SCAN_FILE_BYTES};
use crucible_storage::{CallResultKey, CallResultReceipt};
use crucible_types::SandboxId;
use sha2::{Digest as _, Sha256};

use super::super::process::{Stage, StopMark};
use super::broker::StatusChannel;
use super::command::View;
use super::materialize::Materialization;
use super::transaction;

pub(crate) use bounded::BoundedPublication;
use bounded::OutputBoundary;

const MAX_PROJECTED_ENTRIES: usize = 262_144;
const MAX_PROJECTED_DEPTH: usize = 64;

type SharedProcess = Arc<Mutex<Box<dyn SandboxProcess>>>;

fn share_process(process: Box<dyn SandboxProcess>) -> SharedProcess {
    Arc::new(Mutex::new(process))
}

fn with_process<T>(process: &SharedProcess, work: impl FnOnce(&mut dyn SandboxProcess) -> T) -> T {
    let mut process = process
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    work(process.as_mut())
}

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
    /// Whether the broker's scan is journaled. An ending that waits for the
    /// lock is written again on a later look, and journals the scan once.
    scanned: bool,
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

#[cfg(test)]
pub(super) use bounded::{ScanGate, clear_scan_gate, scan_gate_taken};

#[cfg(test)]
pub(super) fn install_scan_gate(
    sandbox: SandboxId,
    reached: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    bounded::install_scan_gate(sandbox, reached, release);
}

#[cfg(test)]
fn take_scan_gate(sandbox: SandboxId) -> Option<ScanGate> {
    bounded::take_scan_gate(sandbox)
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
            scanned: false,
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
        if self.scanned {
            return Ok(());
        }
        for record in [
            transaction::Record::CommandExited,
            transaction::Record::WorkloadReapIntent,
            transaction::Record::WorkloadReaped,
            transaction::Record::ScanIntent,
            transaction::Record::ScanTransferred,
        ] {
            self.record(record)?;
        }
        self.scanned = true;
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

    /// Leave to publish now, or `None` while the publication lock is unavailable;
    /// a copied descriptor can make it look held, so the caller asks again.
    /// A projection with no writable root is let in without the lock.
    fn admission(&self) -> io::Result<Option<Admission>> {
        if self.roots.is_empty() {
            return Ok(Some(Admission { _lease: None }));
        }
        let lease = transaction::Lease::try_acquire_in(&self.state).map_err(|source| {
            io::Error::new(
                source.kind(),
                // The kind, not the message: this one carries the state
                // directory's own path, and the model reads what comes back.
                format!(
                    "the writable publication lock is unavailable: {}",
                    source.kind()
                ),
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
    /// Where the command's ending is written.
    pub(super) publications: BoundedPublication,
    pub(super) status_channel: StatusChannel,
    pub(super) stop_mark: Option<StopMark>,
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
        publications,
        status_channel,
        stop_mark,
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
    let process = share_process(process);
    Ok(Box::new(ProjectedProcess {
        process,
        output_boundary: Arc::new(OutputBoundary::default()),
        projection,
        publications,
        receiver: Some(receiver),
        status: None,
        terminal: false,
        reported: None,
        concluding: None,
        failure: None,
        unrecorded: None,
        publication: None,
        audit,
        sandbox,
        control: Some(control),
        invocation,
        call_result_key,
        acceptance_pending: false,
        inspection,
        cleanup: crucible_sandbox::SandboxCleanup::Pending,
        stop_mark,
        #[cfg(test)]
        on_cancel: None,
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
    // Whether the scope was reaped is read from the inspection below, which
    // a stop that failed leaves short of complete.
    let _ = process.stop_sync();
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
    process: SharedProcess,
    output_boundary: Arc<OutputBoundary>,
    projection: Option<Projection>,
    /// Where the command's ending is written once its report is read.
    publications: BoundedPublication,
    receiver: Option<protocol::Receiver>,
    status: Option<ExitStatus>,
    terminal: bool,
    /// The broker's terminal report, or why it could not be read, from when it
    /// is read until its ending is handed to a thread of its own, and again
    /// while that ending waits for a slot or for the lock.
    ///
    /// Kept because it can be read only once, and an ending that has to wait
    /// its turn is handed over again on a later look.
    reported: Option<io::Result<protocol::Terminal>>,
    /// The command's ending, being written on a thread of its own.
    concluding: Option<bounded::Running<Ended>>,
    /// How the ending went wrong, where it did, answered again on every later
    /// look.
    failure: Option<(io::ErrorKind, Box<str>)>,
    /// A fact about a publication that went ahead, which the audit could not
    /// take. Reported with the cleanup rather than as the ending.
    unrecorded: Option<io::Error>,
    /// The terminal publication outcome, once this command's ending has one.
    publication: Option<SandboxLifecycle>,
    audit: SandboxAudit,
    sandbox: SandboxId,
    control: Option<std::os::unix::net::UnixStream>,
    invocation: SandboxInvocationMode,
    call_result_key: Option<CallResultKey>,
    acceptance_pending: bool,
    inspection: SandboxInspection,
    cleanup: crucible_sandbox::SandboxCleanup,
    /// Set before the cancel, which lets the broker end the command's output.
    stop_mark: Option<StopMark>,
    /// Runs just before the cancel's first byte is written, standing in for a
    /// broker that ends the output as soon as it reads it.
    #[cfg(test)]
    on_cancel: Option<Box<dyn FnOnce() + Send>>,
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

    /// Takes the broker's terminal report, or why it could not be read, once
    /// the leader has exited and the receiver's thread has read it.
    fn report(&mut self) -> io::Result<()> {
        let Some(terminal) = self.receiver.as_mut().map(protocol::Receiver::finish) else {
            return Err(self.failed(io::Error::other(
                "sandbox terminal scan receiver is unavailable",
            )));
        };
        self.receiver.take();
        self.control.take();
        self.reported = Some(terminal);
        Ok(())
    }

    /// Settles an ending with nothing to publish into, and hands one with a
    /// projection to a thread of its own.
    ///
    /// That thread journals the scan and publishes or discards what the command
    /// wrote, all of which waits on the disk; this look answers `None` and a
    /// later one takes what it came to. With every slot taken, the ending
    /// waits its turn and is handed over again on a later look.
    fn conclude(&mut self) -> io::Result<Option<ExitStatus>> {
        let Some(reported) = self.reported.take() else {
            return Err(self.failed(io::Error::other("sandbox terminal report is unavailable")));
        };
        if self.projection.is_none() {
            let reported = reported.map_err(|problem| self.failed(problem))?;
            if !reported.roots.is_empty() {
                return Err(self.failed(io::Error::other(
                    "sandbox broker reported roots outside the immutable projection plan",
                )));
            }
            self.terminal = true;
            self.status = Some(reported.status);
            self.publication = Some(SandboxLifecycle::Published);
            return Ok(Some(reported.status));
        }
        let ending = Ending {
            projection: self.projection.take(),
            process: Arc::clone(&self.process),
            output_boundary: Arc::clone(&self.output_boundary),
            audit: self.audit.clone(),
            sandbox: self.sandbox,
            #[cfg(test)]
            asked_by: std::thread::current().id(),
            #[cfg(test)]
            scan_gate: take_scan_gate(self.sandbox),
            publication: None,
        };
        match self
            .publications
            .start((ending, reported), |(ending, reported)| {
                ending.write(reported)
            }) {
            Ok(concluding) => self.concluding = Some(concluding),
            Err((ending, reported)) => {
                self.projection = ending.projection;
                self.reported = Some(reported);
            }
        }
        Ok(None)
    }

    /// Takes what the ending handed to its thread came to, waiting for that
    /// thread where it has not ended, and answers as that ending does.
    fn settle(&mut self) -> io::Result<Option<ExitStatus>> {
        let Some(concluding) = self.concluding.take() else {
            return Ok(None);
        };
        let ended = match concluding.finish() {
            Ok(ended) => ended,
            Err(problem) => {
                self.publication = Some(SandboxLifecycle::Quarantined);
                return Err(self.failed(problem));
            }
        };
        self.projection = ended.projection;
        self.publication = ended.publication;
        match ended.outcome {
            Outcome::Waiting(terminal) => {
                self.reported = Some(Ok(terminal));
                Ok(None)
            }
            Outcome::Settled { status, unrecorded } => {
                self.terminal = true;
                self.status = Some(status);
                self.unrecorded = unrecorded;
                Ok(Some(status))
            }
            Outcome::Failed(problem) => Err(self.failed(problem)),
        }
    }

    /// What [`SandboxProcess::stop`] does, synchronously, so `Drop` can stop
    /// the process without a future to drive.
    fn stop(&mut self) -> io::Result<()> {
        if self.cleanup != crucible_sandbox::SandboxCleanup::Pending {
            if let Some((kind, problem)) = &self.failure {
                return Err(io::Error::new(*kind, problem.to_string()));
            }
            return if self.cleanup == crucible_sandbox::SandboxCleanup::Complete {
                Ok(())
            } else {
                Err(io::Error::other("sandbox cleanup previously failed"))
            };
        }
        // An ending already on its thread is let finish: it holds the journal
        // and the roots, and one cut short would leave a publication half made.
        // What it came to is kept as a look keeps it, so a publication that
        // finished is not rolled back below.
        let settled = self.settle();
        let needs_terminal = self.status.is_none() && !self.terminal;
        // The broker kills the workload on reading the cancel, which can end
        // its output before the stop below is reached, so mark the cut first.
        if let Some(mark) = &self.stop_mark {
            mark.stopping();
        }
        let cancellation = self.control.as_mut().map_or(Ok(()), |control| {
            #[cfg(test)]
            let control = &mut tests::BeforeFirstByte::new(control, self.on_cancel.take());
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
        let process_cleanup = with_process(&self.process, |process| process.stop_sync());
        let scope_reaped = with_process(&self.process, |process| {
            process.inspection().cleanup() == crucible_sandbox::SandboxCleanup::Complete
        });
        if let Some(receiver) = &mut self.receiver {
            // The report is discarded, so a scan that has not ended gets no
            // more of the stream than is already queued on it: a broker that
            // stalled, whose end of the stream a failed stop left open, would
            // otherwise hold this join for as long as it stalls. See
            // `protocol::Receiver` for what the join still waits for.
            if let Some(control) = &self.control {
                let _ = control.shutdown(std::net::Shutdown::Both);
            }
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
                self.publication = Some(lifecycle);
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
        let ending = match settled {
            Err(problem) => Err(problem),
            Ok(_) => match &self.failure {
                Some((kind, problem)) => Err(io::Error::new(*kind, problem.to_string())),
                None => Ok(()),
            },
        };
        let mut result = cancellation
            .and(process_cleanup)
            .and(terminal_cleanup)
            .and(projection_cleanup)
            .and(unrecorded)
            .and(ending);
        self.inspection = self.inspection.clone().cleaned(cleanup);
        self.cleanup = cleanup;
        let audited = self.audit_cleanup(cleanup);
        result = result.and(audited);
        result
    }

    /// What [`SandboxProcess::begin_background_acceptance`] does, synchronously.
    fn begin_background_acceptance(&mut self, key: CallResultKey) -> Result<(), SandboxError> {
        // An ending on its thread holds the journal; the acceptance is decided
        // against what that ending wrote.
        self.settle().map_err(SandboxError::Lifecycle)?;
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

    /// What [`SandboxProcess::complete_background_acceptance`] does,
    /// synchronously.
    fn complete_background_acceptance(
        &mut self,
        receipt: CallResultReceipt,
    ) -> Result<(), SandboxError> {
        self.settle().map_err(SandboxError::Lifecycle)?;
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
            let _ = with_process(&self.process, |process| process.stop_sync());
            projection.retain_evidence();
            let _ = self.lifecycle(SandboxLifecycle::Quarantined);
            self.terminal = true;
            return Err(SandboxError::Lifecycle(source));
        }
        self.acceptance_pending = false;
        Ok(())
    }
}

impl SandboxProcess for ProjectedProcess {
    fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
        with_process(&self.process, |process| process.take_stdin())
    }

    fn take_async_stdin(&mut self) -> Option<Box<dyn crucible_sandbox::SandboxInput>> {
        with_process(&self.process, |process| process.take_async_stdin())
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        bounded::wrap_output(
            with_process(&self.process, |process| process.take_stdout()),
            Arc::clone(&self.output_boundary),
        )
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        bounded::wrap_output(
            with_process(&self.process, |process| process.take_stderr()),
            Arc::clone(&self.output_boundary),
        )
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
            return with_process(&self.process, |process| process.try_wait());
        }
        if let Some(concluding) = &self.concluding {
            // Its ending is being written on a thread of its own, which is
            // joined here only once it has ended: this look may be on the
            // thread that draws. One that waits for the lock is handed over
            // again on the next look.
            if !concluding.finished() {
                return Ok(None);
            }
            return self.settle();
        }
        if self.reported.is_none() {
            if with_process(&self.process, |process| process.try_wait())?.is_none() {
                return Ok(None);
            }
            // The leader has exited, but the broker's scan may still be
            // arriving, or have stalled. It finishes on the receiver's own
            // thread; this look is answered again on the next one rather than
            // waiting for it here, where the caller may be the thread that
            // draws.
            if !self
                .receiver
                .as_ref()
                .is_none_or(protocol::Receiver::finished)
            {
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
            || self.concluding.is_some()
            || matches!(
                with_process(&self.process, |process| process.try_wait()),
                Ok(Some(_))
            )
    }

    fn publication_outcome(&self) -> Option<SandboxLifecycle> {
        self.publication
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move { self.stop() })
    }

    /// The same stop the future above drives, for the owners that have no
    /// future to drive, bounded the way that body is and reported as failed
    /// cleanup where it gives out.
    fn stop_sync(&mut self) -> io::Result<()> {
        ProjectedProcess::stop(self)
    }

    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn usage(&self) -> SandboxUsage {
        with_process(&self.process, |process| process.usage())
    }

    fn violation(&self) -> Option<SandboxViolation> {
        with_process(&self.process, |process| process.violation())
    }

    fn begin_background_acceptance(
        &mut self,
        key: CallResultKey,
    ) -> BoxFuture<'_, Result<(), SandboxError>> {
        Box::pin(async move { self.begin_background_acceptance(key) })
    }

    fn complete_background_acceptance(
        &mut self,
        receipt: CallResultReceipt,
    ) -> BoxFuture<'_, Result<(), SandboxError>> {
        Box::pin(async move { self.complete_background_acceptance(receipt) })
    }
}

impl Drop for ProjectedProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// What a command's ending is written from on its own thread: everything it
/// journals, publishes or discards through, and the facts it records.
struct Ending {
    projection: Option<Projection>,
    process: SharedProcess,
    output_boundary: Arc<OutputBoundary>,
    audit: SandboxAudit,
    sandbox: SandboxId,
    /// Under test, the thread that handed this ending over, whose change to
    /// this user's state directory it reads through, as that thread would.
    #[cfg(test)]
    asked_by: std::thread::ThreadId,
    /// Under test, a gate that holds the worker after its scan is durable.
    #[cfg(test)]
    scan_gate: Option<ScanGate>,
    /// The terminal publication outcome, once this ending has one.
    publication: Option<SandboxLifecycle>,
}

/// What an ending came to, with the projection it was written through, which
/// goes back to the command.
struct Ended {
    projection: Option<Projection>,
    outcome: Outcome,
    publication: Option<SandboxLifecycle>,
}

enum Outcome {
    /// The publication lock is unavailable: the report goes back, to be handed
    /// over again on a later look.
    Waiting(protocol::Terminal),
    /// Published or discarded: how the command ended, and a fact about a
    /// publication that went ahead which the audit could not take.
    Settled {
        status: ExitStatus,
        unrecorded: Option<io::Error>,
    },
    /// How the ending went wrong. On paths where a cleanup failure is itself
    /// recorded, that cleanup result is returned; paths that retain the
    /// original ending error return it unless a later lifecycle audit also
    /// fails.
    Failed(io::Error),
}

impl Ending {
    fn write(mut self, reported: io::Result<protocol::Terminal>) -> Ended {
        #[cfg(test)]
        transaction::read_for(self.asked_by);
        let outcome = if let Ok(outcome) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match reported {
                Ok(terminal) => self.reported(terminal),
                Err(problem) => self.unreadable(problem),
            })) {
            outcome
        } else {
            self.publication = Some(SandboxLifecycle::Quarantined);
            if let Some(projection) = self.projection.as_mut() {
                projection.retain_evidence();
            }
            Outcome::Failed(io::Error::other(
                "the thread writing a command's ending panicked",
            ))
        };
        Ended {
            projection: self.projection,
            outcome,
            publication: self.publication,
        }
    }

    fn lifecycle(&self, lifecycle: SandboxLifecycle) -> io::Result<()> {
        self.audit
            .record(self.sandbox, SandboxFactKind::Lifecycle(lifecycle))
            .map_err(io::Error::other)
    }

    /// Rolls back what the command wrote after its report could not be read.
    fn unreadable(&mut self, problem: io::Error) -> Outcome {
        if let Some(projection) = self.projection.as_mut() {
            let lifecycle = if projection.abort(true).is_ok() {
                SandboxLifecycle::RolledBack
            } else {
                projection.retain_evidence();
                SandboxLifecycle::Quarantined
            };
            self.publication = Some(lifecycle);
            if let Err(cleanup) = self.lifecycle(lifecycle) {
                return Outcome::Failed(cleanup);
            }
        }
        Outcome::Failed(problem)
    }

    fn violated(&self) -> bool {
        with_process(&self.process, |process| process.violation().is_some())
    }

    fn discarded_status(&mut self, status: ExitStatus) -> Outcome {
        match self.discard() {
            Ok(()) => Outcome::Settled {
                status,
                unrecorded: None,
            },
            Err(problem) => Outcome::Failed(problem),
        }
    }

    /// Journals the scan, then publishes or discards what the command wrote.
    ///
    /// Only a clean ending publishes, so only a clean ending asks for the lock:
    /// a command killed, or stopped by a limit, is discarded without waiting for
    /// another command's publication.
    fn reported(&mut self, terminal: protocol::Terminal) -> Outcome {
        if let Some(projection) = self.projection.as_mut()
            && let Err(problem) = projection.record_terminal_scan()
        {
            let _ = projection.abort(true);
            projection.retain_evidence();
            self.publication = Some(SandboxLifecycle::Quarantined);
            if let Err(cleanup) = self.lifecycle(SandboxLifecycle::Quarantined) {
                return Outcome::Failed(cleanup);
            }
            return Outcome::Failed(problem);
        }
        #[cfg(test)]
        if let Some((_, reached, release)) = self.scan_gate.take() {
            let _ = reached.send(());
            let _ = release.recv();
        }
        let status = terminal.status;
        if status.signal().is_some() || self.violated() {
            return self.discarded_status(status);
        }
        let admitted = self
            .projection
            .as_ref()
            .map_or(Ok(None), Projection::admission);
        let admission = match admitted {
            Ok(Some(admission)) => admission,
            // The publication lock is unavailable. Asked again on a later look
            // rather than waited for here while it remains unavailable.
            Ok(None) => return Outcome::Waiting(terminal),
            Err(problem) => return self.discarded_after(problem),
        };
        if self.violated() {
            drop(admission);
            return self.discarded_status(status);
        }
        #[cfg(test)]
        bounded::hold_final_check(self.sandbox);
        // The handoff check above is not the boundary: seal every output read
        // before publication; a seal timeout discards rather than publishing.
        match self.output_boundary.seal() {
            Ok(true) => {
                drop(admission);
                return self.discarded_status(status);
            }
            Ok(false) => {}
            Err(problem) => {
                drop(admission);
                return self.discarded_after(problem);
            }
        }
        if self.violated() {
            drop(admission);
            return self.discarded_status(status);
        }
        if let Err(problem) = self.lifecycle(SandboxLifecycle::PublicationStarted) {
            drop(admission);
            return self.discarded_after(problem);
        }
        let publication = self.projection.as_mut().map_or(Ok(()), |projection| {
            projection.publish(&admission, &terminal.baselines, &terminal.roots)
        });
        if let Err(problem) = publication {
            let lifecycle = if problem.requires_quarantine() {
                SandboxLifecycle::Quarantined
            } else {
                SandboxLifecycle::RolledBack
            };
            self.publication = Some(lifecycle);
            return match self.lifecycle(lifecycle) {
                Ok(()) => Outcome::Failed(problem.into_io()),
                Err(cleanup) => Outcome::Failed(cleanup),
            };
        }
        // The publication is done. A fact that cannot be recorded is a cleanup
        // failure, not an ending that went wrong: reported as one, it would tell
        // the model to write everything again over the files that are already
        // there.
        self.publication = Some(SandboxLifecycle::Published);
        Outcome::Settled {
            status,
            unrecorded: self.lifecycle(SandboxLifecycle::Published).err(),
        }
    }

    /// Discards what the command wrote after `problem` kept it from being
    /// published.
    fn discarded_after(&mut self, problem: io::Error) -> Outcome {
        match self.discard() {
            Ok(()) => Outcome::Failed(problem),
            Err(cleanup) => Outcome::Failed(cleanup),
        }
    }

    /// Discards what the command wrote, and records how that went.
    fn discard(&mut self) -> io::Result<()> {
        let Some(projection) = self.projection.as_mut() else {
            return Ok(());
        };
        if let Err(problem) = projection.abort(true) {
            projection.retain_evidence();
            self.projection.take();
            self.publication = Some(SandboxLifecycle::Quarantined);
            self.lifecycle(SandboxLifecycle::Quarantined)?;
            return Err(problem);
        }
        match self.lifecycle(SandboxLifecycle::RolledBack) {
            Ok(()) => {
                self.publication = Some(SandboxLifecycle::RolledBack);
                Ok(())
            }
            Err(problem) => {
                self.publication = Some(SandboxLifecycle::Quarantined);
                Err(problem)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a hook before the first byte reaches the writer it wraps: what the
    /// hook sees is what a peer that acts on the first byte it reads would see.
    pub(super) struct BeforeFirstByte<'a, W> {
        inner: &'a mut W,
        hook: Option<Box<dyn FnOnce() + Send>>,
    }

    impl<'a, W> BeforeFirstByte<'a, W> {
        pub(super) fn new(inner: &'a mut W, hook: Option<Box<dyn FnOnce() + Send>>) -> Self {
            Self { inner, hook }
        }
    }

    impl<W: Write> Write for BeforeFirstByte<'_, W> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if let Some(hook) = self.hook.take() {
                hook();
            }
            self.inner.write(bytes)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    /// A projected stop sends the broker a cancel, and the broker kills the
    /// workload, ending its output, before the stop reaches the process
    /// itself. Here the command is ended, and its output read to the end as a
    /// reader thread may, before the cancel's first byte is written: the start
    /// of a credential it printed must already read as cut.
    #[test]
    fn a_credential_start_is_masked_when_the_broker_ends_the_output_on_cancel() {
        use crucible_sandbox::{SandboxDomainPolicy, SandboxNetworkProvenance, SandboxRead};

        let policy =
            SandboxDomainPolicy::new([], [], false, [], SandboxNetworkProvenance::User).unwrap();
        let proxy = crate::network::Mediator::tcp(
            policy,
            SandboxId::new(),
            Some(std::time::Duration::from_secs(5)),
        )
        .unwrap();
        let mut command = std::process::Command::new("/bin/sh");
        command.args([
            "-c",
            "printf 'id=%.20s' \"${HTTP_PROXY#http://crucible:}\"; read -r _",
        ]);
        command.envs(proxy.environment(proxy.address()));
        let mut plan =
            crate::process::testing_plan(crucible_sandbox::SandboxSpeech::Held, None).unwrap();
        plan.network = Some(proxy);
        let audit = plan.audit.clone();
        let sandbox = plan.sandbox;
        let (mut process, stop_mark) = crate::process::spawn_marked(command, plan).unwrap();
        let stdin = process.take_stdin().unwrap();
        let mut stdout = process.take_stdout().unwrap();

        let (read, printed) = std::sync::mpsc::channel();
        let broker_kills_the_workload = move || {
            drop(stdin);
            let mut kept = Vec::new();
            let mut buffer = [0; 256];
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                match stdout.read_ready(&mut buffer) {
                    Ok(SandboxRead::Bytes(count)) => {
                        kept.extend_from_slice(buffer.get(..count).unwrap_or_default());
                    }
                    Ok(SandboxRead::Pending) => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Ok(SandboxRead::End) => break,
                    Ok(SandboxRead::Limited { .. }) | Err(_) => return,
                }
            }
            let _ = read.send(kept);
        };
        let (control, _broker) = std::os::unix::net::UnixStream::pair().unwrap();
        let inspection = process.inspection().clone();
        let mut projected = ProjectedProcess {
            process: share_process(process),
            output_boundary: Arc::new(OutputBoundary::default()),
            projection: None,
            publications: BoundedPublication::default(),
            receiver: None,
            status: None,
            terminal: false,
            reported: None,
            concluding: None,
            failure: None,
            unrecorded: None,
            publication: None,
            audit,
            sandbox,
            control: Some(control),
            invocation: SandboxInvocationMode::Foreground,
            call_result_key: None,
            acceptance_pending: false,
            inspection,
            cleanup: crucible_sandbox::SandboxCleanup::Pending,
            stop_mark: Some(stop_mark),
            on_cancel: Some(Box::new(broker_kills_the_workload)),
            _serial: None,
        };

        projected.stop().unwrap();

        let masked = [b"id=".as_slice(), &[b'*'; 20]].concat();
        assert_eq!(printed.recv().unwrap(), masked);
    }

    /// A projected command's input is the process's own asynchronous input,
    /// not the default adapter over its synchronous one, which would write on
    /// the thread polling it: a write to a command that never reads leaves
    /// the runtime free.
    #[test]
    fn a_projected_command_forwards_its_asynchronous_input() {
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "exec sleep 3"]);
        let plan =
            crate::process::testing_plan(crucible_sandbox::SandboxSpeech::Held, None).unwrap();
        let audit = plan.audit.clone();
        let sandbox = plan.sandbox;
        let (process, stop_mark) = crate::process::spawn_marked(command, plan).unwrap();
        let (control, _broker) = std::os::unix::net::UnixStream::pair().unwrap();
        let inspection = process.inspection().clone();
        let mut projected = ProjectedProcess {
            process: share_process(process),
            output_boundary: Arc::new(OutputBoundary::default()),
            projection: None,
            publications: BoundedPublication::default(),
            receiver: None,
            status: None,
            terminal: false,
            reported: None,
            concluding: None,
            failure: None,
            unrecorded: None,
            publication: None,
            audit,
            sandbox,
            control: Some(control),
            invocation: SandboxInvocationMode::Foreground,
            call_result_key: None,
            acceptance_pending: false,
            inspection,
            cleanup: crucible_sandbox::SandboxCleanup::Pending,
            stop_mark: Some(stop_mark),
            on_cancel: None,
            _serial: None,
        };
        let input = crucible_sandbox::SandboxProcess::take_async_stdin(&mut projected)
            .expect("a command built Held hands back an input");

        let (gave_up, ticks) = crate::process::tests::pipes::writing_to_a_deaf_command(input)
            .expect("the write held the runtime's only thread");

        assert!(gave_up, "a write to a command that never reads answered");
        assert_eq!(ticks, Some(10), "other work stopped while the write waited");
        projected.stop().unwrap();
    }

    /// A command with nothing to publish whose leader has already exited, and
    /// the broker's end of its status stream, which the test writes the
    /// terminal report into as slowly as it likes.
    fn exited_with_its_report_to_come() -> (ProjectedProcess, std::os::unix::net::UnixStream) {
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "exit 0"]);
        let plan =
            crate::process::testing_plan(crucible_sandbox::SandboxSpeech::Closed, None).unwrap();
        let audit = plan.audit.clone();
        let sandbox = plan.sandbox;
        let (mut process, stop_mark) = crate::process::spawn_marked(command, plan).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !matches!(process.try_wait(), Ok(Some(_))) {
            assert!(
                std::time::Instant::now() < deadline,
                "the leader did not exit"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (control, broker) = std::os::unix::net::UnixStream::pair().unwrap();
        let receiver = protocol::Receiver::spawn(control.try_clone().unwrap(), None).unwrap();
        let inspection = process.inspection().clone();
        let projected = ProjectedProcess {
            process: share_process(process),
            output_boundary: Arc::new(OutputBoundary::default()),
            projection: None,
            publications: BoundedPublication::default(),
            receiver: Some(receiver),
            status: None,
            terminal: false,
            reported: None,
            concluding: None,
            failure: None,
            unrecorded: None,
            publication: None,
            audit,
            sandbox,
            control: Some(control),
            invocation: SandboxInvocationMode::Foreground,
            call_result_key: None,
            acceptance_pending: false,
            inspection,
            cleanup: crucible_sandbox::SandboxCleanup::Pending,
            stop_mark: Some(stop_mark),
            on_cancel: None,
            _serial: None,
        };
        (projected, broker)
    }

    /// Says the command exited with `code`, and starts the scan without
    /// finishing it: what a broker still sending, or one that has stalled,
    /// has written so far.
    fn begin_the_report(broker: &mut std::os::unix::net::UnixStream, code: i32) {
        broker
            .write_all(&crucible_sandbox_broker::encode_wait_status(code << 8))
            .unwrap();
        broker
            .write_all(&crucible_sandbox_broker::SCAN_FRAME)
            .unwrap();
    }

    /// Finishes a report [`begin_the_report`] began, with no root in it.
    fn end_the_report(broker: &mut std::os::unix::net::UnixStream) {
        broker.write_all(&0_u32.to_le_bytes()).unwrap();
        broker
            .write_all(&crucible_sandbox_broker::SCAN_END_FRAME)
            .unwrap();
    }

    /// How long a look or a stop may take here before it counts as waiting on
    /// the scan: far more than either takes, far less than a stall.
    const PROMPT: std::time::Duration = std::time::Duration::from_secs(2);

    /// A command whose leader has exited but whose terminal scan is still
    /// arriving is asked how it ended. The answer is that it has ended and is
    /// not settled yet, given at once, rather than a look that waits for the
    /// scan; and once the scan arrives, the status is the one it reported.
    #[test]
    fn a_stalled_scan_does_not_hold_up_a_status() {
        let (mut projected, mut broker) = exited_with_its_report_to_come();
        begin_the_report(&mut broker, 3);

        let (answered, answer) = std::sync::mpsc::channel();
        let looking = std::thread::spawn(move || {
            let look = projected.try_wait().map_err(|problem| problem.to_string());
            let ended = projected.ended();
            let _ = answered.send((look, ended));
            projected
        });
        let looked = answer.recv_timeout(PROMPT);
        // Ended either way, so a look that was waiting on it is let go.
        end_the_report(&mut broker);
        drop(broker);
        let mut projected = looking.join().unwrap();

        let (look, ended) = looked.expect("a status waited for a stalled scan");
        assert_eq!(look, Ok(None), "a status settled before its scan arrived");
        assert!(ended, "a command whose leader exited did not read as ended");
        let deadline = std::time::Instant::now() + PROMPT;
        let status = loop {
            if let Some(status) = projected.try_wait().unwrap() {
                break status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the status never settled once its scan arrived"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(status.code(), Some(3), "{status}");
        projected.stop().unwrap();
    }

    /// A stop of a command whose terminal scan has stalled, with the broker's
    /// end of the stream held open, takes no more of that scan than was
    /// already sent rather than waiting for the rest, and still joins the
    /// thread it ran on.
    #[test]
    fn a_stalled_scan_does_not_hold_up_a_stop() {
        let (mut projected, mut broker) = exited_with_its_report_to_come();
        begin_the_report(&mut broker, 0);

        let (answered, answer) = std::sync::mpsc::channel();
        let stopping = std::thread::spawn(move || {
            let stopped = projected.stop().map_err(|problem| problem.to_string());
            let _ = answered.send(stopped);
            projected
        });
        let stopped = answer.recv_timeout(PROMPT);
        // Held until now, so the stalled scan cannot have ended by itself.
        drop(broker);
        let projected = stopping.join().unwrap();

        stopped
            .expect("a stop waited for a stalled scan")
            .expect("cleanup");
        assert_eq!(
            projected.inspection.cleanup(),
            crucible_sandbox::SandboxCleanup::Complete
        );
        assert!(
            projected.receiver.is_none(),
            "the scan's thread was left to a later stop"
        );
    }

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
