//! Exact Bubblewrap filesystem/namespace command planning.

use std::collections::{BTreeSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crucible_core::{
    SandboxCommand, SandboxError, SandboxFeature, SandboxFilesystemAccess, SandboxFilesystemRule,
    SandboxNetworkPolicy, SandboxRequest,
};

use super::materialize::Materialization;
use super::probe::Bwrap;

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
}

impl View {
    pub(super) fn sources(self) -> Vec<OwnedFd> {
        self.binds.into_iter().map(|bind| bind.source).collect()
    }
}

impl std::fmt::Debug for View {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("View")
            .field("binds", &self.binds.len())
            .field("masks", &self.masks.len())
            .finish()
    }
}

struct Bind {
    source: OwnedFd,
    destination: PathBuf,
    read_only: bool,
    directory: bool,
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
        let source = fs::File::open(&canonical).map_err(|source| {
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
            destination: mount.destination,
            read_only: mount.read_only,
            directory: opened.is_dir(),
        })
    }

    fn descriptor(&self) -> RawFd {
        self.source.as_raw_fd()
    }
}

struct Mask {
    destination: PathBuf,
    directory: bool,
}

pub(super) fn prepare(request: &SandboxRequest) -> Result<View, SandboxError> {
    if !matches!(request.policy().network(), SandboxNetworkPolicy::Closed) {
        return Err(SandboxError::Unsupported {
            feature: SandboxFeature::NetworkAllowlist,
        });
    }
    let limits = request.policy().limits();
    for (present, feature) in [
        (limits.cpu_seconds.is_some(), SandboxFeature::CpuLimit),
        (limits.memory_bytes.is_some(), SandboxFeature::MemoryLimit),
        (limits.disk_bytes.is_some(), SandboxFeature::DiskLimit),
        (limits.processes.is_some(), SandboxFeature::ProcessLimit),
        (limits.open_files.is_some(), SandboxFeature::OpenFileLimit),
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
    for root in request
        .policy()
        .filesystem()
        .iter()
        .filter(|rule| rule.access() == SandboxFilesystemAccess::ReadWrite)
        .map(SandboxFilesystemRule::path)
    {
        for protected in nested_protected(root)? {
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
    Ok(View { binds, masks })
}

fn validate_host(rules: &[SandboxFilesystemRule]) -> Result<(), SandboxError> {
    if is_wsl1() {
        return Err(unavailable("Bubblewrap confinement is unsupported on WSL1"));
    }
    let wsl = is_wsl();
    for rule in rules {
        if rule.path() == Path::new("/") {
            return Err(SandboxError::Materialization {
                problem: "host root cannot be granted as a sandbox root".into(),
                source: None,
            });
        }
        if wsl && rule.path().starts_with("/mnt/") {
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
                if rule.access() != SandboxFilesystemAccess::Protected {
                    return Err(SandboxError::Materialization {
                        problem: "sandbox policy path disappeared before preparation".into(),
                        source: Some(source),
                    });
                }
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

pub(super) fn build(
    backend: &Bwrap,
    request: &SandboxRequest,
    command: &SandboxCommand,
    view: &View,
    materialization: Option<&Materialization>,
) -> Result<Command, SandboxError> {
    let runtime = runtime_mounts();
    let mut directories = BTreeSet::new();
    for mount in &runtime {
        add_destination_directories(&mut directories, mount);
    }
    for bind in &view.binds {
        if bind.directory {
            directories.insert(bind.destination.clone());
        }
        add_parents(&mut directories, &bind.destination);
    }
    if materialization.is_some() {
        directories.insert(PathBuf::from("/crucible/manifest"));
        add_parents(&mut directories, Path::new("/crucible/manifest"));
    }
    for mask in &view.masks {
        add_parents(&mut directories, &mask.destination);
    }
    for fixed in [
        Path::new("/dev"),
        Path::new("/proc"),
        Path::new("/tmp"),
        Path::new("/crucible-home"),
    ] {
        directories.insert(fixed.to_path_buf());
    }

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
    let mut inherited = Vec::new();
    for bind in &view.binds {
        inherited.push(bind.descriptor());
        process
            .arg(if bind.read_only {
                "--ro-bind-fd"
            } else {
                "--bind-fd"
            })
            .arg(bind.descriptor().to_string())
            .arg(&bind.destination);
    }
    if let Some(materialization) = materialization {
        inherited.push(materialization.descriptor());
        process
            .arg("--ro-bind-fd")
            .arg(materialization.descriptor().to_string())
            .arg("/crucible/manifest");
        for mount in materialization.mounts() {
            inherited.push(mount.descriptor());
            process
                .arg(match mount.access() {
                    SandboxFilesystemAccess::ReadOnly => "--ro-bind-fd",
                    SandboxFilesystemAccess::ReadWrite => "--bind-fd",
                    SandboxFilesystemAccess::Protected | SandboxFilesystemAccess::Unreadable => {
                        return Err(SandboxError::Unsupported {
                            feature: SandboxFeature::Materialization,
                        });
                    }
                })
                .arg(mount.descriptor().to_string())
                .arg(mount.destination());
        }
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

    process
        .args(["--cap-drop", "ALL", "--clearenv"])
        .args(["--setenv", "HOME", "/crucible-home"])
        .args(["--setenv", "TMPDIR", "/tmp"]);
    for (name, value) in command.environment().iter() {
        if !matches!(name, "HOME" | "TMPDIR" | "SSH_AUTH_SOCK" | "GPG_AGENT_INFO") {
            process.arg("--setenv").arg(name).arg(value);
        }
    }
    process
        .arg("--chdir")
        .arg(request.policy().working_directory())
        .arg("--")
        .arg(command.program())
        .args(command.arguments());
    super::fd::inherit(&mut process, &inherited)?;
    Ok(process)
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

fn nested_protected(root: &Path) -> Result<Vec<PathBuf>, SandboxError> {
    let root_device = fs::metadata(root)
        .map_err(|source| materialization("workspace root could not be inspected", Some(source)))?
        .dev();
    let mut protected = Vec::new();
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
                if protected_name(&name) {
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
                if metadata.nlink() != 1 {
                    return Err(materialization(
                        "workspace tree contains a hard-linked file",
                        None,
                    ));
                }
                if protected_name(&name) {
                    protected.push(path);
                }
                continue;
            }
            if metadata.is_dir() {
                if protected_name(&name) {
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
            return Err(materialization(
                "workspace tree contains a special file",
                None,
            ));
        }
    }
    Ok(protected)
}

fn protected_name(name: &OsStr) -> bool {
    matches!(name.to_str(), Some(".git" | ".agents" | ".codex"))
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
        while index < encoded.len() {
            if encoded[index] != b'\\' {
                decoded.push(encoded[index]);
                index = index.saturating_add(1);
                continue;
            }
            let digits = encoded
                .get(index.saturating_add(1)..index.saturating_add(4))
                .ok_or_else(|| materialization("host mount table escape is malformed", None))?;
            if digits.len() != 3 || digits.iter().any(|digit| !(b'0'..=b'7').contains(digit)) {
                return Err(materialization(
                    "host mount table escape is malformed",
                    None,
                ));
            }
            let value = u16::from(digits[0] - b'0') * 64
                + u16::from(digits[1] - b'0') * 8
                + u16::from(digits[2] - b'0');
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

fn is_wsl() -> bool {
    fs::read_to_string("/proc/version")
        .is_ok_and(|version| version.to_ascii_lowercase().contains("microsoft"))
}

fn is_wsl1() -> bool {
    fs::read_to_string("/proc/version").is_ok_and(|version| {
        let version = version.to_ascii_lowercase();
        version.contains("microsoft") && !version.contains("microsoft-standard")
    })
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
    use crucible_core::{
        Ancestry, SandboxId, SandboxManifest, SandboxPolicy, SandboxRequest, ToolId,
    };

    use crate::sample::Sample;

    #[test]
    fn protected_names_are_exact_not_suffix_or_prefix_matches() {
        assert!(protected_name(OsStr::new(".git")));
        assert!(protected_name(OsStr::new(".agents")));
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
}
