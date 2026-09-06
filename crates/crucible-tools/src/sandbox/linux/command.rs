//! Exact Bubblewrap filesystem/namespace command planning.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use crucible_core::{
    SandboxCommand, SandboxError, SandboxFeature, SandboxFilesystemAccess, SandboxFilesystemRule,
    SandboxNetworkPolicy, SandboxRequest, SandboxUnreadablePattern,
};

use super::broker::Broker;
use super::materialize::Materialization;
use super::probe::Bwrap;
use super::projection::Projection;

/// Runtime roots that contain binaries and dynamic libraries, not user state.
const RUNTIME_DIRECTORIES: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib64",
    "/nix/store",
    "/run/current-system/sw",
];

/// Minimum host configuration files commonly required by dynamic programs.
const RUNTIME_FILES: &[&str] = &[
    "/etc/ld.so.cache",
    "/etc/ld.so.conf",
    "/etc/ld.so.conf.d",
    "/etc/alternatives",
    "/etc/passwd",
    "/etc/group",
    "/etc/nsswitch.conf",
    "/etc/localtime",
    "/etc/ssl/certs",
    "/etc/pki",
];

/// Bounded validation scan for reached workspace trees.
const MAX_WORKSPACE_SCAN_ENTRIES: usize = 262_144;

/// Maximum nested directory depth considered part of one workspace tree.
const MAX_PROTECTED_SCAN_DEPTH: usize = 64;

/// `/proc/self/mountinfo` is host-owned but still bounded before retention.
const MAX_MOUNTINFO_BYTES: usize = 4 * 1024 * 1024;

/// Linux mount tables are finite input to preparation, not an unbounded log.
const MAX_MOUNT_POINTS: usize = 8192;

/// Policy roots pinned before materialization so a later rename cannot retarget
/// the namespace plan.
pub(super) struct View {
    binds: Vec<Bind>,
    masks: Vec<Mask>,
    sockets: Vec<super::network::SocketMount>,
}

impl View {
    pub(super) fn binds(&self) -> &[Bind] {
        &self.binds
    }

    pub(super) fn exclusions_beneath(&self, root: &Path) -> Vec<PathBuf> {
        self.binds
            .iter()
            .filter(|bind| {
                bind.read_only && bind.destination != root && bind.destination.starts_with(root)
            })
            .map(|bind| bind.destination.clone())
            .chain(
                self.masks
                    .iter()
                    .filter(|mask| mask.destination.starts_with(root))
                    .map(|mask| mask.destination.clone()),
            )
            .chain(
                self.sockets
                    .iter()
                    .map(|socket| socket.destination().to_owned()),
            )
            .filter_map(|destination| destination.strip_prefix(root).ok().map(Path::to_path_buf))
            .collect()
    }
}

impl std::fmt::Debug for View {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("View")
            .field("binds", &self.binds.len())
            .field("masks", &self.masks.len())
            .field("sockets", &self.sockets.len())
            .finish()
    }
}

pub(super) struct Bind {
    source: OwnedFd,
    host: PathBuf,
    destination: PathBuf,
    read_only: bool,
    directory: bool,
    mode: u32,
}

impl Bind {
    fn open(mount: Mount) -> Result<Self, SandboxError> {
        let named = fs::symlink_metadata(&mount.source).map_err(|source| {
            materialization("sandbox filesystem source is unavailable", Some(source))
        })?;
        if named.file_type().is_symlink() || (!named.is_dir() && !named.is_file()) {
            return Err(materialization(
                "sandbox filesystem source is not a regular file or directory",
                None,
            ));
        }
        if named.is_file() && named.nlink() != 1 {
            return Err(materialization(
                "sandbox filesystem source is a hard-linked file",
                None,
            ));
        }
        let canonical = mount.source.canonicalize().map_err(|source| {
            materialization(
                "sandbox filesystem source could not be canonicalized",
                Some(source),
            )
        })?;
        if canonical != mount.source {
            return Err(materialization(
                "sandbox filesystem source changed after policy resolution",
                None,
            ));
        }
        let source = if named.is_file() && !mount.read_only {
            OpenOptions::new().read(true).write(true).open(&canonical)
        } else {
            fs::File::open(&canonical)
        }
        .map_err(|source| {
            materialization(
                "sandbox filesystem source could not be opened",
                Some(source),
            )
        })?;
        let opened = source.metadata().map_err(|source| {
            materialization(
                "sandbox filesystem source could not be verified",
                Some(source),
            )
        })?;
        if (opened.dev(), opened.ino()) != (named.dev(), named.ino()) {
            return Err(materialization(
                "sandbox filesystem source changed while it was being pinned",
                None,
            ));
        }
        let source = rustix::io::fcntl_dupfd_cloexec(&source, 3).map_err(|source| {
            materialization(
                "sandbox filesystem descriptor could not be isolated",
                Some(source.into()),
            )
        })?;
        Ok(Self {
            source,
            host: canonical,
            destination: mount.destination,
            read_only: mount.read_only,
            directory: opened.is_dir(),
            mode: opened.mode() & 0o777,
        })
    }

    pub(super) fn descriptor(&self) -> RawFd {
        self.source.as_raw_fd()
    }

    pub(super) fn duplicate(&self) -> std::io::Result<OwnedFd> {
        rustix::io::fcntl_dupfd_cloexec(&self.source, 3).map_err(Into::into)
    }

    pub(super) fn host(&self) -> &Path {
        &self.host
    }

    pub(super) fn destination(&self) -> &Path {
        &self.destination
    }

    pub(super) const fn read_only(&self) -> bool {
        self.read_only
    }

    pub(super) const fn directory(&self) -> bool {
        self.directory
    }

    pub(super) const fn mode(&self) -> u32 {
        self.mode
    }
}

struct Mask {
    destination: PathBuf,
    directory: bool,
}

pub(super) fn prepare(request: &SandboxRequest) -> Result<View, SandboxError> {
    let limits = request.policy().limits();
    // The process ceiling is the one this list cannot decide on its own: what
    // the kernel counts it against is a property of the host, so the backend's
    // own claim is what says whether a stated ceiling can be honoured here.
    if limits.processes.is_some()
        && super::probe::process_limit() != crucible_core::SandboxCapability::Enforced
    {
        return Err(SandboxError::Unsupported {
            feature: SandboxFeature::ProcessLimit,
        });
    }
    for (present, feature) in [
        (limits.disk_bytes.is_some(), SandboxFeature::DiskLimit),
        (
            limits.session_time.is_some(),
            SandboxFeature::SessionTimeLimit,
        ),
        (
            limits.outbound_bytes.is_some(),
            SandboxFeature::OutboundByteLimit,
        ),
        (limits.cost_micros.is_some(), SandboxFeature::CostLimit),
    ] {
        if present {
            return Err(SandboxError::Unsupported { feature });
        }
    }

    validate_host(request.policy().filesystem())?;
    let socket_paths = match request.policy().network() {
        SandboxNetworkPolicy::Domains(policy) => policy.unix_sockets(),
        SandboxNetworkPolicy::Closed => &[],
    };
    let sockets = socket_paths
        .iter()
        .map(|path| {
            if request.policy().filesystem().iter().any(|rule| {
                rule.access() == SandboxFilesystemAccess::Unreadable
                    && path.starts_with(rule.path())
            }) || request
                .policy()
                .unreadable_patterns()
                .iter()
                .any(|pattern| pattern.matches(path))
            {
                return Err(materialization(
                    "an unreadable path cannot be granted as a Unix endpoint",
                    None,
                ));
            }
            super::network::SocketMount::open(path, path)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let cwd = request
        .policy()
        .working_directory()
        .canonicalize()
        .map_err(|source| SandboxError::Materialization {
            problem: "sandbox working directory is unavailable".into(),
            source: Some(source),
        })?;
    if cwd != request.policy().working_directory() {
        return Err(SandboxError::Materialization {
            problem: "sandbox working directory changed after policy resolution".into(),
            source: None,
        });
    }

    let mut grants = Vec::new();
    let mut masks = Vec::new();
    for rule in request.policy().filesystem() {
        match rule.access() {
            SandboxFilesystemAccess::ReadWrite => grants.push(Mount::read_write(rule.path())),
            SandboxFilesystemAccess::ReadOnly => grants.push(Mount::read_only(rule.path())),
            SandboxFilesystemAccess::Protected => {
                if rule.path().exists() {
                    grants.push(Mount::read_only(rule.path()));
                }
            }
            SandboxFilesystemAccess::Unreadable => {
                if rule.path().exists() {
                    masks.push(rule.path().to_path_buf());
                }
            }
        }
    }
    masks.extend(expand_unreadable_patterns(
        request.policy().unreadable_patterns(),
    )?);
    let mut validated_trees: BTreeMap<PathBuf, bool> = BTreeMap::new();
    for rule in request.policy().filesystem().iter().filter(|rule| {
        rule.access() != SandboxFilesystemAccess::Unreadable
            && fs::metadata(rule.path()).is_ok_and(|metadata| metadata.is_dir())
    }) {
        if let Some((_, protects_metadata)) = validated_trees
            .iter_mut()
            .find(|(root, _)| rule.path().starts_with(root))
        {
            *protects_metadata |= rule.access() == SandboxFilesystemAccess::ReadWrite;
        } else {
            validated_trees.insert(
                rule.path().to_path_buf(),
                rule.access() == SandboxFilesystemAccess::ReadWrite,
            );
        }
    }
    for (root, protects_metadata) in validated_trees {
        for protected in validate_granted_tree(&root, protects_metadata, socket_paths)? {
            if !grants.iter().any(|mount| mount.destination == protected) {
                grants.push(Mount::read_only(&protected));
            }
            grants.extend(linked_worktree_mounts(&protected)?);
        }
    }
    grants.sort_by(mount_order);
    grants.dedup_by(|left, right| {
        left.source == right.source
            && left.destination == right.destination
            && left.read_only == right.read_only
    });
    let mount_points = host_mount_points()?;
    for grant in &grants {
        if fs::metadata(&grant.source).is_ok_and(|metadata| metadata.is_dir())
            && mount_points
                .iter()
                .any(|mount| mount != &grant.source && mount.starts_with(&grant.source))
        {
            return Err(materialization(
                "sandbox filesystem source contains a nested host mount",
                None,
            ));
        }
    }

    masks.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });
    let mut reduced_masks: Vec<PathBuf> = Vec::new();
    for mask in masks {
        if reduced_masks.iter().any(|parent| mask.starts_with(parent)) {
            continue;
        }
        reduced_masks.push(mask);
    }
    let masks = reduced_masks
        .into_iter()
        .map(|destination| {
            let metadata = fs::symlink_metadata(&destination).map_err(|source| {
                materialization(
                    "unreadable sandbox path could not be inspected",
                    Some(source),
                )
            })?;
            Ok(Mask {
                destination,
                directory: metadata.is_dir(),
            })
        })
        .collect::<Result<Vec<_>, SandboxError>>()?;
    let binds = grants
        .into_iter()
        .map(Bind::open)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(View {
        binds,
        masks,
        sockets,
    })
}

fn validate_host(rules: &[SandboxFilesystemRule]) -> Result<(), SandboxError> {
    validate_host_for(rules, linux_host())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinuxHost {
    Native,
    Wsl1,
    Wsl2,
}

fn validate_host_for(rules: &[SandboxFilesystemRule], host: LinuxHost) -> Result<(), SandboxError> {
    if host == LinuxHost::Wsl1 {
        return Err(unavailable("Bubblewrap confinement is unsupported on WSL1"));
    }
    for rule in rules {
        if rule.path() == Path::new("/") {
            return Err(SandboxError::Materialization {
                problem: "host root cannot be granted as a sandbox root".into(),
                source: None,
            });
        }
        if host == LinuxHost::Wsl2 && rule.path().starts_with("/mnt/") {
            return Err(unavailable(
                "Bubblewrap confinement for WSL DrvFS workspace roots is unsupported",
            ));
        }
        match fs::symlink_metadata(rule.path()) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(SandboxError::Materialization {
                    problem: "sandbox policy root or protected path is a symbolic link".into(),
                    source: None,
                });
            }
            Ok(_) => {
                let canonical =
                    rule.path()
                        .canonicalize()
                        .map_err(|source| SandboxError::Materialization {
                            problem: "sandbox policy path could not be canonicalized".into(),
                            source: Some(source),
                        })?;
                if canonical != rule.path() {
                    return Err(SandboxError::Materialization {
                        problem: "sandbox policy path changed after resolution".into(),
                        source: None,
                    });
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(SandboxError::Materialization {
                    problem: "sandbox policy path disappeared before preparation".into(),
                    source: Some(source),
                });
            }
            Err(source) => {
                return Err(SandboxError::Materialization {
                    problem: "sandbox policy path could not be inspected".into(),
                    source: Some(source),
                });
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) struct Plan<'a> {
    pub(super) backend: &'a Bwrap,
    pub(super) broker: &'a Broker,
    pub(super) request: &'a SandboxRequest,
    pub(super) command: &'a SandboxCommand,
    pub(super) view: &'a View,
    pub(super) materialization: Option<&'a Materialization>,
    pub(super) projection: Option<&'a Projection>,
    pub(super) status_descriptor: RawFd,
    pub(super) network: Option<&'a super::super::network::Mediator>,
    pub(super) proxy_socket: Option<&'a super::network::SocketMount>,
}

pub(super) fn build(plan: Plan<'_>) -> Result<Command, SandboxError> {
    let Plan {
        backend,
        broker,
        request,
        command,
        view,
        materialization,
        projection,
        status_descriptor,
        network,
        proxy_socket,
    } = plan;
    let runtime = runtime_mounts();
    let directories = mount_directories(&runtime, view, materialization.is_some());

    let mut process = Command::new(backend.path());
    process.env_clear().args([
        "--die-with-parent",
        "--new-session",
        "--unshare-user",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-net",
        "--unshare-uts",
        "--disable-userns",
        "--as-pid-1",
        "--tmpfs",
        "/",
    ]);

    for directory in directories {
        process.arg("--dir").arg(directory);
    }
    process.args(["--dev", "/dev", "--proc", "/proc"]);
    process
        .args(["--tmpfs", "/tmp", "--chmod", "1777", "/tmp"])
        .args([
            "--tmpfs",
            "/crucible-home",
            "--chmod",
            "0700",
            "/crucible-home",
        ]);

    for mount in &runtime {
        process
            .arg(if mount.read_only {
                "--ro-bind"
            } else {
                "--bind"
            })
            .arg(&mount.source)
            .arg(&mount.destination);
    }
    let mut inherited = vec![broker.descriptor(), status_descriptor];
    process
        .arg("--ro-bind-fd")
        .arg(broker.descriptor().to_string())
        .arg("/run/crucible/broker")
        .arg("--sync-fd")
        .arg(status_descriptor.to_string());
    for bind in &view.binds {
        let overlay = !bind.read_only
            && projection.is_some_and(|projection| projection.uses_overlay(bind.destination()));
        let descriptor = if bind.read_only || overlay {
            bind.descriptor()
        } else {
            projection
                .and_then(|projection| projection.descriptor(bind.destination()))
                .ok_or_else(|| {
                    self::materialization("writable sandbox root was not projected", None)
                })?
        };
        inherited.push(descriptor);
        if overlay {
            process
                .arg("--overlay-src")
                .arg(format!("/proc/self/fd/{descriptor}"))
                .arg("--tmp-overlay")
                .arg(&bind.destination)
                .arg("--chmod")
                .arg(format!("{:04o}", bind.mode()))
                .arg(&bind.destination);
        } else {
            process
                .arg(if bind.read_only {
                    "--ro-bind-fd"
                } else {
                    "--bind-fd"
                })
                .arg(descriptor.to_string())
                .arg(&bind.destination);
        }
    }
    if let Some(materialization) = materialization {
        inherited.push(materialization.descriptor());
        process
            .arg("--ro-bind-fd")
            .arg(materialization.descriptor().to_string())
            .arg("/crucible/manifest");
        for mount in materialization.mounts() {
            let overlay = mount.access() == SandboxFilesystemAccess::ReadWrite
                && projection
                    .is_some_and(|projection| projection.uses_overlay(mount.destination()));
            let descriptor = match mount.access() {
                SandboxFilesystemAccess::ReadOnly => mount.descriptor(),
                SandboxFilesystemAccess::ReadWrite if overlay => mount.descriptor(),
                SandboxFilesystemAccess::ReadWrite => projection
                    .and_then(|projection| projection.descriptor(mount.destination()))
                    .ok_or_else(|| {
                        self::materialization("writable manifest mount was not projected", None)
                    })?,
                SandboxFilesystemAccess::Protected | SandboxFilesystemAccess::Unreadable => {
                    return Err(SandboxError::Unsupported {
                        feature: SandboxFeature::Materialization,
                    });
                }
            };
            inherited.push(descriptor);
            if overlay {
                process
                    .arg("--overlay-src")
                    .arg(format!("/proc/self/fd/{descriptor}"))
                    .arg("--tmp-overlay")
                    .arg(mount.destination())
                    .arg("--chmod")
                    .arg(format!("{:04o}", mount.mode()))
                    .arg(mount.destination());
            } else {
                process
                    .arg(if mount.access() == SandboxFilesystemAccess::ReadOnly {
                        "--ro-bind-fd"
                    } else {
                        "--bind-fd"
                    })
                    .arg(descriptor.to_string())
                    .arg(mount.destination());
            }
        }
    }
    for socket in view.sockets.iter().chain(proxy_socket) {
        inherited.push(socket.descriptor());
        process
            .arg("--ro-bind-fd")
            .arg(socket.descriptor().to_string())
            .arg(socket.destination());
    }
    for mask in &view.masks {
        if mask.directory {
            process
                .arg("--tmpfs")
                .arg(&mask.destination)
                .arg("--remount-ro")
                .arg(&mask.destination);
        } else {
            process
                .arg("--ro-bind")
                .arg("/dev/null")
                .arg(&mask.destination);
        }
    }

    append_environment(&mut process, command, network);
    process
        .arg("--chdir")
        .arg(request.policy().working_directory())
        .arg("--")
        .arg("/run/crucible/broker")
        .arg("--status-fd")
        .arg(status_descriptor.to_string());
    append_projection_plan(&mut process, projection);
    append_resource_limits(&mut process, request.policy().limits());
    if network.is_some() {
        process.arg("--network-proxy");
    }
    if matches!(request.policy().network(), SandboxNetworkPolicy::Domains(policy) if policy.allow_local_binding())
    {
        process.arg("--allow-local-binding");
    }
    process
        .arg("--")
        .arg(command.program())
        .args(command.arguments());
    super::fd::inherit(&mut process, &inherited)?;
    Ok(process)
}

fn mount_directories(
    runtime: &[Mount],
    view: &View,
    has_materialization: bool,
) -> BTreeSet<PathBuf> {
    let mut directories = BTreeSet::new();
    for mount in runtime {
        add_destination_directories(&mut directories, mount);
    }
    for bind in &view.binds {
        if bind.directory {
            directories.insert(bind.destination.clone());
        }
        add_parents(&mut directories, &bind.destination);
    }
    if has_materialization {
        directories.insert(PathBuf::from("/crucible/manifest"));
        add_parents(&mut directories, Path::new("/crucible/manifest"));
    }
    for mask in &view.masks {
        add_parents(&mut directories, &mask.destination);
    }
    for socket in &view.sockets {
        add_parents(&mut directories, socket.destination());
    }
    for fixed in [
        Path::new("/dev"),
        Path::new("/proc"),
        Path::new("/tmp"),
        Path::new("/crucible-home"),
        Path::new("/run/crucible"),
    ] {
        directories.insert(fixed.to_path_buf());
    }

    directories
}

fn append_environment(
    process: &mut Command,
    command: &SandboxCommand,
    network: Option<&super::super::network::Mediator>,
) {
    // The environment travels in the backend's own, already cleared, process
    // environment, which it hands on unchanged to the command. It must not travel
    // as arguments: a process's argument list is readable by every local user
    // through /proc for as long as it runs, and this map can carry credentials.
    process.args(["--cap-drop", "ALL"]);
    for (name, value) in command.environment().iter() {
        if !matches!(name, "HOME" | "TMPDIR" | "SSH_AUTH_SOCK" | "GPG_AGENT_INFO") {
            process.env(name, value);
        }
    }
    process.env("HOME", "/crucible-home").env("TMPDIR", "/tmp");
    if let Some(network) = network {
        process.envs(network.environment(super::network::PROXY_ADDRESS));
    }
}

fn append_resource_limits(process: &mut Command, limits: crucible_core::SandboxResourceLimits) {
    for (name, value) in [
        ("--limit-cpu-seconds", limits.cpu_seconds),
        ("--limit-memory-bytes", limits.memory_bytes),
        ("--limit-open-files", limits.open_files),
        ("--limit-processes", limits.processes),
    ] {
        if let Some(value) = value {
            process.arg(name).arg(value.to_string());
        }
    }
}

fn append_projection_plan(process: &mut Command, projection: Option<&Projection>) {
    let Some(projection) = projection else {
        return;
    };
    for destination in projection.destinations() {
        process.arg("--project-root").arg(destination);
    }
    for destination in projection.excluded_destinations() {
        process.arg("--project-exclude").arg(destination);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mount {
    source: PathBuf,
    destination: PathBuf,
    read_only: bool,
}

impl Mount {
    fn read_only(path: &Path) -> Self {
        Self {
            source: path.to_path_buf(),
            destination: path.to_path_buf(),
            read_only: true,
        }
    }

    fn read_write(path: &Path) -> Self {
        Self {
            source: path.to_path_buf(),
            destination: path.to_path_buf(),
            read_only: false,
        }
    }
}

fn mount_order(left: &Mount, right: &Mount) -> std::cmp::Ordering {
    left.destination
        .components()
        .count()
        .cmp(&right.destination.components().count())
        .then_with(|| left.destination.cmp(&right.destination))
}

fn runtime_mounts() -> Vec<Mount> {
    RUNTIME_DIRECTORIES
        .iter()
        .chain(RUNTIME_FILES)
        .map(Path::new)
        .filter(|path| path.exists())
        .map(Mount::read_only)
        .collect()
}

fn add_destination_directories(directories: &mut BTreeSet<PathBuf>, mount: &Mount) {
    if fs::metadata(&mount.source).is_ok_and(|metadata| metadata.is_dir()) {
        directories.insert(mount.destination.clone());
    }
    add_parents(directories, &mount.destination);
}

fn add_parents(directories: &mut BTreeSet<PathBuf>, path: &Path) {
    let mut parents = Vec::new();
    let mut current = path.parent();
    while let Some(parent) = current {
        if parent == Path::new("/") {
            break;
        }
        parents.push(parent.to_path_buf());
        current = parent.parent();
    }
    for parent in parents.into_iter().rev() {
        directories.insert(parent);
    }
}

fn validate_granted_tree(
    root: &Path,
    protects_metadata: bool,
    sockets: &[PathBuf],
) -> Result<Vec<PathBuf>, SandboxError> {
    let root_device = fs::metadata(root)
        .map_err(|source| materialization("workspace root could not be inspected", Some(source)))?
        .dev();
    let mut protected = Vec::new();
    let mut hard_links: BTreeMap<(u64, u64), (u64, usize, bool, bool)> = BTreeMap::new();
    let mut pending = VecDeque::from([(root.to_path_buf(), 0_usize)]);
    let mut inspected = 0_usize;
    while let Some((directory, depth)) = pending.pop_front() {
        let entries = fs::read_dir(&directory).map_err(|source| SandboxError::Materialization {
            problem: "workspace metadata scan failed".into(),
            source: Some(source),
        })?;
        for entry in entries {
            inspected = inspected.saturating_add(1);
            if inspected > MAX_WORKSPACE_SCAN_ENTRIES {
                return Err(SandboxError::Materialization {
                    problem: "workspace validation scan exceeded its bound".into(),
                    source: None,
                });
            }
            let entry = entry.map_err(|source| SandboxError::Materialization {
                problem: "workspace metadata entry could not be inspected".into(),
                source: Some(source),
            })?;
            let name = entry.file_name();
            let path = entry.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|source| SandboxError::Materialization {
                    problem: "workspace metadata path changed during preparation".into(),
                    source: Some(source),
                })?;
            if metadata.file_type().is_symlink() {
                if protects_metadata && protected_name(&name) {
                    return Err(SandboxError::Materialization {
                        problem: "protected workspace metadata is a symbolic link".into(),
                        source: None,
                    });
                }
                continue;
            }
            if metadata.dev() != root_device {
                return Err(materialization(
                    "workspace tree crosses a filesystem boundary",
                    None,
                ));
            }
            if metadata.is_file() {
                if metadata.nlink() > 1 {
                    let is_protected = protects_metadata
                        && path.strip_prefix(root).is_ok_and(|relative| {
                            relative
                                .components()
                                .any(|component| protected_name(component.as_os_str()))
                        });
                    let observation = hard_links
                        .entry((metadata.dev(), metadata.ino()))
                        .or_insert((metadata.nlink(), 0, false, false));
                    if observation.0 != metadata.nlink() {
                        return Err(materialization(
                            "workspace hard-link identity changed during preparation",
                            None,
                        ));
                    }
                    observation.1 = observation.1.saturating_add(1);
                    observation.2 |= is_protected;
                    observation.3 |= !is_protected;
                }
                if protects_metadata && protected_name(&name) {
                    protected.push(path);
                }
                continue;
            }
            if metadata.is_dir() {
                if protects_metadata && protected_name(&name) {
                    protected.push(path.clone());
                }
                if depth >= MAX_PROTECTED_SCAN_DEPTH {
                    return Err(materialization(
                        "workspace validation scan exceeded its depth bound",
                        None,
                    ));
                }
                pending.push_back((path, depth.saturating_add(1)));
                continue;
            }
            if metadata.file_type().is_socket() && sockets.contains(&path) {
                continue;
            }
            return Err(materialization(
                "workspace tree contains a special file",
                None,
            ));
        }
    }
    if hard_links
        .values()
        .any(|(links, observed, protected, ordinary)| {
            usize::try_from(*links) != Ok(*observed) || (*protected && *ordinary)
        })
    {
        return Err(materialization(
            "workspace hard-link group escapes its projected authority",
            None,
        ));
    }
    Ok(protected)
}

fn expand_unreadable_patterns(
    patterns: &[SandboxUnreadablePattern],
) -> Result<Vec<PathBuf>, SandboxError> {
    let mut grouped: BTreeMap<PathBuf, Vec<&SandboxUnreadablePattern>> = BTreeMap::new();
    for pattern in patterns {
        grouped
            .entry(pattern.scan_root().to_path_buf())
            .or_default()
            .push(pattern);
    }

    let mut matched = Vec::new();
    let mut inspected = 0_usize;
    for (root, patterns) in grouped {
        let named = fs::symlink_metadata(&root).map_err(|source| {
            materialization("unreadable pattern scan root is unavailable", Some(source))
        })?;
        if named.file_type().is_symlink() || !named.is_dir() {
            return Err(materialization(
                "unreadable pattern scan root is not a real directory",
                None,
            ));
        }
        let canonical = root.canonicalize().map_err(|source| {
            materialization(
                "unreadable pattern scan root could not be canonicalized",
                Some(source),
            )
        })?;
        if canonical != root {
            return Err(materialization(
                "unreadable pattern scan root changed after policy resolution",
                None,
            ));
        }
        let device = named.dev();
        let mut pending = VecDeque::from([(root, 0_usize)]);
        while let Some((directory, depth)) = pending.pop_front() {
            let entries = fs::read_dir(&directory).map_err(|source| {
                materialization("unreadable pattern scan failed", Some(source))
            })?;
            let mut entries = entries.collect::<Result<Vec<_>, _>>().map_err(|source| {
                materialization("unreadable pattern entry could not be read", Some(source))
            })?;
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                inspected = inspected.saturating_add(1);
                if inspected > MAX_WORKSPACE_SCAN_ENTRIES {
                    return Err(materialization(
                        "unreadable pattern expansion exceeded its bound",
                        None,
                    ));
                }
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).map_err(|source| {
                    materialization(
                        "unreadable pattern path changed during expansion",
                        Some(source),
                    )
                })?;
                let selected = patterns.iter().any(|pattern| pattern.matches(&path));
                if metadata.file_type().is_symlink() {
                    if selected {
                        return Err(materialization(
                            "unreadable pattern selected a symbolic link",
                            None,
                        ));
                    }
                    continue;
                }
                if metadata.dev() != device {
                    return Err(materialization(
                        "unreadable pattern scan crossed a filesystem boundary",
                        None,
                    ));
                }
                if !metadata.is_dir() && !metadata.is_file() {
                    return Err(materialization(
                        "unreadable pattern scan encountered a special file",
                        None,
                    ));
                }
                if metadata.is_file() && metadata.nlink() != 1 {
                    return Err(materialization(
                        "unreadable pattern scan encountered a hard-linked file",
                        None,
                    ));
                }
                if selected {
                    matched.push(path.clone());
                }
                if metadata.is_dir() {
                    if depth >= MAX_PROTECTED_SCAN_DEPTH {
                        return Err(materialization(
                            "unreadable pattern expansion exceeded its depth bound",
                            None,
                        ));
                    }
                    pending.push_back((path, depth.saturating_add(1)));
                }
            }
        }
    }
    matched.sort();
    matched.dedup();
    Ok(matched)
}

fn protected_name(name: &OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(".git" | ".agents" | ".codex" | ".crucible")
    )
}

fn host_mount_points() -> Result<Vec<PathBuf>, SandboxError> {
    let bytes = fs::read("/proc/self/mountinfo")
        .map_err(|source| materialization("host mount table could not be read", Some(source)))?;
    if bytes.len() > MAX_MOUNTINFO_BYTES {
        return Err(materialization("host mount table exceeds its bound", None));
    }
    parse_mount_points(&bytes)
}

fn parse_mount_points(bytes: &[u8]) -> Result<Vec<PathBuf>, SandboxError> {
    let mut points = Vec::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        if points.len() >= MAX_MOUNT_POINTS {
            return Err(materialization(
                "host mount table has too many entries",
                None,
            ));
        }
        let encoded = line
            .split(|byte| *byte == b' ')
            .nth(4)
            .ok_or_else(|| materialization("host mount table is malformed", None))?;
        let mut decoded = Vec::with_capacity(encoded.len());
        let mut index = 0_usize;
        while let Some(&byte) = encoded.get(index) {
            if byte != b'\\' {
                decoded.push(byte);
                index = index.saturating_add(1);
                continue;
            }
            let digits = encoded
                .get(index.saturating_add(1)..index.saturating_add(4))
                .ok_or_else(|| materialization("host mount table escape is malformed", None))?;
            let [first, second, third] = digits else {
                return Err(materialization(
                    "host mount table escape is malformed",
                    None,
                ));
            };
            if digits.iter().any(|digit| !(b'0'..=b'7').contains(digit)) {
                return Err(materialization(
                    "host mount table escape is malformed",
                    None,
                ));
            }
            let value = u16::from(*first - b'0') * 64
                + u16::from(*second - b'0') * 8
                + u16::from(*third - b'0');
            let value = u8::try_from(value)
                .map_err(|_| materialization("host mount table escape is malformed", None))?;
            if value == 0 {
                return Err(materialization(
                    "host mount table contains a null path",
                    None,
                ));
            }
            decoded.push(value);
            index = index.saturating_add(4);
        }
        let path = PathBuf::from(OsString::from_vec(decoded));
        if !path.is_absolute() {
            return Err(materialization(
                "host mount table path is not absolute",
                None,
            ));
        }
        points.push(path);
    }
    Ok(points)
}

fn linked_worktree_mounts(path: &Path) -> Result<Vec<Mount>, SandboxError> {
    if path.file_name() != Some(OsStr::new(".git"))
        || !fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
    {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).map_err(|source| SandboxError::Materialization {
        problem: "linked-worktree metadata could not be read".into(),
        source: Some(source),
    })?;
    if text.len() > 4096 {
        return Err(SandboxError::Materialization {
            problem: "linked-worktree metadata exceeds its bound".into(),
            source: None,
        });
    }
    let Some(target) = text.trim().strip_prefix("gitdir: ") else {
        return Ok(Vec::new());
    };
    let requested_target = Path::new(target);
    if !requested_target.is_absolute() {
        return Err(SandboxError::Materialization {
            problem: "linked-worktree metadata does not name an absolute git directory".into(),
            source: None,
        });
    }
    let target =
        requested_target
            .canonicalize()
            .map_err(|source| SandboxError::Materialization {
                problem: "linked-worktree git directory is unavailable".into(),
                source: Some(source),
            })?;
    if target != requested_target || !fs::metadata(&target).is_ok_and(|metadata| metadata.is_dir())
    {
        return Err(materialization(
            "linked-worktree git directory is not a canonical directory",
            None,
        ));
    }
    let back_reference = target.join("gitdir");
    let back_reference = read_bounded_metadata_file(
        &back_reference,
        "linked-worktree back-reference is unavailable",
    )?;
    let back_reference = Path::new(back_reference.trim());
    if !back_reference.is_absolute() || back_reference != path {
        return Err(materialization(
            "linked-worktree git directory does not refer back to this workspace",
            None,
        ));
    }
    let mut mounts = vec![Mount::read_only(&target)];
    let common = target.join("commondir");
    match fs::symlink_metadata(&common) {
        Ok(_) => {
            let relative = read_bounded_metadata_file(
                &common,
                "linked-worktree common directory is unavailable",
            )?;
            let common = target
                .join(relative.trim())
                .canonicalize()
                .map_err(|source| SandboxError::Materialization {
                    problem: "linked-worktree common directory is unavailable".into(),
                    source: Some(source),
                })?;
            if common == Path::new("/")
                || !fs::metadata(&common).is_ok_and(|metadata| metadata.is_dir())
            {
                return Err(materialization(
                    "linked-worktree common directory is invalid",
                    None,
                ));
            }
            mounts.push(Mount::read_only(&common));
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(materialization(
                "linked-worktree common directory could not be inspected",
                Some(source),
            ));
        }
    }
    Ok(mounts)
}

fn read_bounded_metadata_file(path: &Path, problem: &'static str) -> Result<String, SandboxError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| materialization(problem, Some(source)))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.nlink() != 1 {
        return Err(materialization(problem, None));
    }
    let text = fs::read_to_string(path).map_err(|source| materialization(problem, Some(source)))?;
    if text.is_empty() || text.len() > 4096 {
        return Err(materialization(problem, None));
    }
    Ok(text)
}

fn linux_host() -> LinuxHost {
    fs::read_to_string("/proc/version").map_or(LinuxHost::Native, |version| {
        linux_host_from_version(&version)
    })
}

fn linux_host_from_version(version: &str) -> LinuxHost {
    let version = version.to_ascii_lowercase();
    let mut remaining = version.as_str();
    while let Some(marker) = remaining.find("wsl") {
        let version_start = marker.saturating_add("wsl".len());
        let digits: String = remaining
            .get(version_start..)
            .unwrap_or_default()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(number) = digits.parse::<u32>() {
            return if number == 1 {
                LinuxHost::Wsl1
            } else {
                LinuxHost::Wsl2
            };
        }
        remaining = remaining.get(version_start..).unwrap_or_default();
    }
    if version.contains("microsoft") && !version.contains("microsoft-standard") {
        LinuxHost::Wsl1
    } else if version.contains("microsoft") {
        LinuxHost::Wsl2
    } else {
        LinuxHost::Native
    }
}

fn unavailable(reason: &'static str) -> SandboxError {
    SandboxError::BackendUnavailable {
        reason: reason.into(),
    }
}

fn materialization(problem: &'static str, source: Option<std::io::Error>) -> SandboxError {
    SandboxError::Materialization {
        problem: problem.into(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    use crucible_core::{
        Ancestry, SandboxId, SandboxManifest, SandboxPolicy, SandboxRequest, ToolId,
    };

    use crate::sample::Sample;

    #[test]
    fn protected_names_are_exact_not_suffix_or_prefix_matches() {
        assert!(protected_name(OsStr::new(".git")));
        assert!(protected_name(OsStr::new(".agents")));
        assert!(protected_name(OsStr::new(".crucible")));
        assert!(!protected_name(OsStr::new(".github")));
        assert!(!protected_name(OsStr::new("repo.git")));
    }

    #[test]
    fn runtime_mounts_never_include_home_or_the_host_root() {
        let mounts = runtime_mounts();
        assert!(mounts.iter().all(|mount| mount.source != Path::new("/")));
        assert!(
            mounts
                .iter()
                .all(|mount| !mount.source.starts_with("/home"))
        );
    }

    #[test]
    fn a_confined_command_carries_the_standard_ceilings_to_the_broker() {
        // The policy stating a ceiling and the broker being told about it are
        // two different things, and only the second one bounds anything. This
        // reads the whole assembled plan rather than the one helper that
        // spells the flags, so removing the call is as red as mistranslating
        // it.
        let sample = Sample::new("sandbox-standard-ceilings");
        let read_only = SandboxPolicy::new(
            true,
            [SandboxFilesystemRule::new(
                sample.root().clone(),
                SandboxFilesystemAccess::ReadOnly,
                crucible_core::SandboxFilesystemProvenance::Workspace,
            )
            .expect("read-only rule")],
            sample.root().clone(),
            SandboxNetworkPolicy::Closed,
            crucible_core::SandboxResourceLimits {
                // Stated by this caller rather than by the confinement, so the
                // plan must carry it too; the broker lowers its own ceiling to
                // match and never raises it to.
                processes: Some(64),
                ..crucible_core::SandboxResourceLimits::confining()
            },
        )
        .expect("a confining read-only policy");
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("sandbox"),
            read_only,
            SandboxManifest::empty(),
        );
        let view = prepare(&request).expect("a view of the sample");
        let command = SandboxCommand::new(
            "/bin/sh",
            [
                std::ffi::OsString::from("-c"),
                std::ffi::OsString::from(":"),
            ],
            crucible_core::SandboxEnvironment::empty(),
        )
        .expect("a command to confine");
        let backend = Bwrap::unspawned(PathBuf::from("/nonexistent/bwrap")).expect("a backend");
        let image = File::open("/dev/null").expect("a file to stand in for the broker");
        let broker = Broker::unexecuted(PathBuf::from("/nonexistent/broker"), image);
        let status = File::open("/dev/null").expect("a file to stand in for the status pipe");

        let process = build(Plan {
            network: None,
            proxy_socket: None,
            backend: &backend,
            broker: &broker,
            request: &request,
            command: &command,
            view: &view,
            materialization: None,
            projection: None,
            status_descriptor: status.as_raw_fd(),
        })
        .expect("a plan for a read-only confined command");

        let argv: Vec<_> = process
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        for (flag, value) in [
            ("--limit-cpu-seconds", "3600"),
            ("--limit-open-files", "4096"),
            ("--limit-processes", "64"),
        ] {
            let at = argv
                .iter()
                .position(|argument| argument == flag)
                .unwrap_or_else(|| panic!("{flag} is missing from {argv:?}"));
            assert_eq!(argv.get(at + 1).map(String::as_str), Some(value), "{flag}");
        }

        // Stated nowhere, so asked for nowhere: a ceiling the broker cannot
        // apply would be refused before it ever got this far.
        assert!(
            !argv
                .iter()
                .any(|argument| argument == "--limit-memory-bytes"),
            "{argv:?}"
        );
    }

    #[test]
    fn preparation_refuses_workspace_hard_links_to_outside_inodes() {
        let sample = Sample::new("sandbox-workspace-hard-link");
        let outside = PathBuf::from(sample.outside("secret.txt", "secret"));
        std::fs::hard_link(outside, sample.root().join("alias.txt")).expect("hard link");
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("sandbox"),
            SandboxPolicy::standard(&sample.workspace()).expect("policy"),
            SandboxManifest::empty(),
        );

        assert!(prepare(&request).is_err());
    }

    #[test]
    fn preparation_refuses_workspace_special_files() {
        let sample = Sample::new("sandbox-workspace-special-file");
        let _socket = std::os::unix::net::UnixListener::bind(sample.root().join("host.sock"))
            .expect("host socket");
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("sandbox"),
            SandboxPolicy::standard(&sample.workspace()).expect("policy"),
            SandboxManifest::empty(),
        );

        assert!(prepare(&request).is_err());
    }

    #[test]
    fn preparation_also_refuses_hard_links_and_special_files_in_read_only_trees() {
        let hard_linked = Sample::new("sandbox-read-only-hard-link");
        let outside = PathBuf::from(hard_linked.outside("secret.txt", "secret"));
        std::fs::hard_link(outside, hard_linked.root().join("alias.txt")).expect("hard link");
        let read_only = SandboxPolicy::new(
            true,
            [SandboxFilesystemRule::new(
                hard_linked.root().clone(),
                SandboxFilesystemAccess::ReadOnly,
                crucible_core::SandboxFilesystemProvenance::Workspace,
            )
            .expect("read-only rule")],
            hard_linked.root().clone(),
            SandboxNetworkPolicy::Closed,
            crucible_core::SandboxResourceLimits::default(),
        )
        .expect("read-only policy");
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("sandbox"),
            read_only,
            SandboxManifest::empty(),
        );
        assert!(prepare(&request).is_err());

        let special = Sample::new("sandbox-read-only-special");
        let _socket = std::os::unix::net::UnixListener::bind(special.root().join("host.sock"))
            .expect("host socket");
        let read_only = SandboxPolicy::new(
            true,
            [SandboxFilesystemRule::new(
                special.root().clone(),
                SandboxFilesystemAccess::ReadOnly,
                crucible_core::SandboxFilesystemProvenance::Workspace,
            )
            .expect("read-only rule")],
            special.root().clone(),
            SandboxNetworkPolicy::Closed,
            crucible_core::SandboxResourceLimits::default(),
        )
        .expect("read-only policy");
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("sandbox"),
            read_only,
            SandboxManifest::empty(),
        );
        assert!(prepare(&request).is_err());
    }

    #[test]
    fn mount_table_paths_are_decoded_without_prefix_confusion() {
        let points = parse_mount_points(
            b"36 25 0:32 / / rw,relatime - tmpfs tmpfs rw\n\
              37 36 0:33 / /tmp/workspace\\040name rw - tmpfs tmpfs rw\n",
        )
        .expect("mount table");

        assert_eq!(
            points,
            [PathBuf::from("/"), PathBuf::from("/tmp/workspace name")]
        );
    }

    #[test]
    fn a_git_file_cannot_name_an_unrelated_host_directory() {
        let sample = Sample::new("sandbox-unrelated-gitdir");
        let outside = PathBuf::from(sample.outside("secret.txt", "secret"));
        let unrelated = outside.parent().expect("outside directory");
        sample.write(".git", &format!("gitdir: {}\n", unrelated.display()));
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("sandbox"),
            SandboxPolicy::standard(&sample.workspace()).expect("policy"),
            SandboxManifest::empty(),
        );

        assert!(prepare(&request).is_err());
    }

    #[test]
    fn a_mutually_linked_worktree_git_directory_is_mounted_read_only() {
        let sample = Sample::new("sandbox-linked-worktree");
        let outside = PathBuf::from(sample.outside("anchor.txt", "anchor"));
        let common = outside
            .parent()
            .expect("outside directory")
            .join("common.git");
        let target = common.join("worktrees/fixture");
        std::fs::create_dir_all(&target).expect("linked git directory");
        sample.write(".git", &format!("gitdir: {}\n", target.display()));
        std::fs::write(
            target.join("gitdir"),
            sample.root().join(".git").display().to_string(),
        )
        .expect("back-reference");
        std::fs::write(target.join("commondir"), "../..\n").expect("common reference");
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("sandbox"),
            SandboxPolicy::standard(&sample.workspace()).expect("policy"),
            SandboxManifest::empty(),
        );

        let view = prepare(&request).expect("linked worktree view");
        assert!(
            view.binds
                .iter()
                .any(|bind| { bind.destination == target && bind.read_only })
        );
        assert!(
            view.binds
                .iter()
                .any(|bind| { bind.destination == common && bind.read_only })
        );
    }

    #[test]
    fn kernel_versions_distinguish_wsl1_wsl2_and_native_linux() {
        for version in [
            "Linux version 4.4.0-22621-Microsoft",
            "Linux version 5.15.0-microsoft-standard-WSL1",
            "Linux version 5.15.0-wsl-microsoft-standard-WSL1",
        ] {
            assert_eq!(
                linux_host_from_version(version),
                LinuxHost::Wsl1,
                "{version}"
            );
        }
        for version in [
            "Linux version 6.6.87.2-microsoft-standard-WSL2",
            "Linux version 4.19.104-microsoft-standard",
        ] {
            assert_eq!(
                linux_host_from_version(version),
                LinuxHost::Wsl2,
                "{version}"
            );
        }
        assert_eq!(
            linux_host_from_version("Linux version 6.8.0"),
            LinuxHost::Native
        );
    }

    #[test]
    fn wsl_drvfs_and_host_root_grants_fail_instead_of_widening() {
        let root = SandboxFilesystemRule::new(
            "/",
            SandboxFilesystemAccess::ReadOnly,
            crucible_core::SandboxFilesystemProvenance::Workspace,
        )
        .expect("root rule");
        assert!(validate_host_for(&[root], LinuxHost::Native).is_err());

        let drvfs = SandboxFilesystemRule::new(
            "/mnt/c/workspace",
            SandboxFilesystemAccess::ReadWrite,
            crucible_core::SandboxFilesystemProvenance::Workspace,
        )
        .expect("DrvFS rule");
        assert!(validate_host_for(&[drvfs], LinuxHost::Wsl2).is_err());
    }

    #[test]
    fn protected_symlinks_and_unreadable_missing_entries_fail_deterministically() {
        let sample = Sample::new("sandbox-host-entry-fixtures");
        crate::sample::symlink(
            sample.outside("target", "secret"),
            sample.root().join("linked"),
        );
        let linked = SandboxFilesystemRule::new(
            sample.root().join("linked"),
            SandboxFilesystemAccess::Protected,
            crucible_core::SandboxFilesystemProvenance::ProtectedMetadata,
        )
        .expect("linked rule");
        assert!(validate_host_for(&[linked], LinuxHost::Native).is_err());

        let missing = SandboxFilesystemRule::new(
            sample.root().join("missing"),
            SandboxFilesystemAccess::Unreadable,
            crucible_core::SandboxFilesystemProvenance::Descendant,
        )
        .expect("missing rule");
        assert!(validate_host_for(&[missing], LinuxHost::Native).is_err());

        let missing_protected = SandboxFilesystemRule::new(
            sample.root().join("missing-protected"),
            SandboxFilesystemAccess::Protected,
            crucible_core::SandboxFilesystemProvenance::ProtectedMetadata,
        )
        .expect("missing protected rule");
        assert!(validate_host_for(&[missing_protected], LinuxHost::Native).is_err());
    }
}
