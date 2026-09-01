//! Exact Bubblewrap filesystem/namespace command planning.

use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crucible_core::{
    SandboxCommand, SandboxError, SandboxFeature, SandboxFilesystemAccess, SandboxFilesystemRule,
    SandboxNetworkPolicy, SandboxRequest,
};

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

/// Bounded metadata scan for nested repositories/control directories.
const MAX_PROTECTED_SCAN_ENTRIES: usize = 8192;

/// Maximum nested directory depth considered part of one workspace tree.
const MAX_PROTECTED_SCAN_DEPTH: usize = 64;

pub(super) fn validate(request: &SandboxRequest) -> Result<(), SandboxError> {
    if !request.manifest().is_empty() {
        return Err(SandboxError::Unsupported {
            feature: SandboxFeature::Materialization,
        });
    }
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
    Ok(())
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
) -> Result<Command, SandboxError> {
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

    grants.sort_by(|left, right| {
        left.destination
            .components()
            .count()
            .cmp(&right.destination.components().count())
            .then_with(|| left.destination.cmp(&right.destination))
    });
    grants.dedup_by(|left, right| {
        left.source == right.source
            && left.destination == right.destination
            && left.read_only == right.read_only
    });

    let runtime = runtime_mounts();
    let mut directories = BTreeSet::new();
    for mount in runtime.iter().chain(&grants) {
        add_destination_directories(&mut directories, mount);
    }
    for path in &masks {
        add_parents(&mut directories, path);
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

    for mount in runtime.iter().chain(&grants) {
        process
            .arg(if mount.read_only {
                "--ro-bind"
            } else {
                "--bind"
            })
            .arg(&mount.source)
            .arg(&mount.destination);
    }
    for mask in masks {
        let directory = fs::metadata(&mask).is_ok_and(|metadata| metadata.is_dir());
        if directory {
            process
                .arg("--tmpfs")
                .arg(&mask)
                .arg("--remount-ro")
                .arg(&mask);
        } else {
            process.arg("--ro-bind").arg("/dev/null").arg(&mask);
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
            if inspected > MAX_PROTECTED_SCAN_ENTRIES {
                return Err(SandboxError::Materialization {
                    problem: "workspace metadata scan exceeded its bound".into(),
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
            if protected_name(&name) {
                if metadata.file_type().is_symlink() {
                    return Err(SandboxError::Materialization {
                        problem: "protected workspace metadata is a symbolic link".into(),
                        source: None,
                    });
                }
                protected.push(path);
            } else if metadata.is_dir() && depth < MAX_PROTECTED_SCAN_DEPTH {
                pending.push_back((path, depth.saturating_add(1)));
            }
        }
    }
    Ok(protected)
}

fn protected_name(name: &OsStr) -> bool {
    matches!(name.to_str(), Some(".git" | ".agents" | ".codex"))
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
    let target = Path::new(target);
    if !target.is_absolute() {
        return Err(SandboxError::Materialization {
            problem: "linked-worktree metadata does not name an absolute git directory".into(),
            source: None,
        });
    }
    let target = target
        .canonicalize()
        .map_err(|source| SandboxError::Materialization {
            problem: "linked-worktree git directory is unavailable".into(),
            source: Some(source),
        })?;
    let mut mounts = vec![Mount::read_only(&target)];
    let common = target.join("commondir");
    if let Ok(relative) = fs::read_to_string(&common) {
        if relative.len() > 4096 {
            return Err(SandboxError::Materialization {
                problem: "linked-worktree common directory exceeds its bound".into(),
                source: None,
            });
        }
        let common = target
            .join(relative.trim())
            .canonicalize()
            .map_err(|source| SandboxError::Materialization {
                problem: "linked-worktree common directory is unavailable".into(),
                source: Some(source),
            })?;
        mounts.push(Mount::read_only(&common));
    }
    Ok(mounts)
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
