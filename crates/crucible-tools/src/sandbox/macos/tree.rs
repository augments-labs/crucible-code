//! Bounded validation of policy-visible workspace trees.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};

use crucible_core::{SandboxError, SandboxFilesystemAccess, SandboxNetworkPolicy, SandboxPolicy};

const MAX_ENTRIES: usize = 262_144;
const MAX_DEPTH: usize = 64;

pub(super) struct Validation {
    protected: Vec<PathBuf>,
    linked_metadata: Vec<PathBuf>,
}

impl Validation {
    pub(super) fn protected(&self) -> &[PathBuf] {
        &self.protected
    }

    pub(super) fn linked_metadata(&self) -> &[PathBuf] {
        &self.linked_metadata
    }
}

pub(super) fn validate(policy: &SandboxPolicy) -> Result<Validation, SandboxError> {
    let sockets = validate_granted_sockets(policy)?;
    let mut roots: BTreeMap<PathBuf, bool> = BTreeMap::new();
    for rule in policy.filesystem().iter().filter(|rule| {
        rule.access() != SandboxFilesystemAccess::Unreadable
            && fs::metadata(rule.path()).is_ok_and(|metadata| metadata.is_dir())
    }) {
        if let Some((_, protects_metadata)) = roots
            .iter_mut()
            .find(|(root, _)| rule.path().starts_with(root))
        {
            *protects_metadata |= rule.access() == SandboxFilesystemAccess::ReadWrite;
        } else {
            roots.insert(
                rule.path().to_path_buf(),
                rule.access() == SandboxFilesystemAccess::ReadWrite,
            );
        }
    }

    let mut protected = Vec::new();
    for (root, protects_metadata) in roots {
        protected.extend(validate_tree(&root, protects_metadata, &sockets)?);
    }
    protected.sort();
    protected.dedup();
    protected.retain(|path| {
        !policy
            .filesystem()
            .iter()
            .any(|rule| rule.path() == path && rule.access() == SandboxFilesystemAccess::Protected)
    });

    let mut git_files: Vec<_> = policy
        .filesystem()
        .iter()
        .filter(|rule| rule.access() == SandboxFilesystemAccess::Protected)
        .map(crucible_core::SandboxFilesystemRule::path)
        .chain(protected.iter().map(PathBuf::as_path))
        .filter(|path| path.file_name() == Some(OsStr::new(".git")))
        .filter(|path| fs::metadata(path).is_ok_and(|metadata| metadata.is_file()))
        .map(Path::to_path_buf)
        .collect();
    git_files.sort();
    git_files.dedup();
    let mut linked_metadata = Vec::new();
    for git_file in git_files {
        linked_metadata.extend(linked_worktree_metadata(&git_file)?);
    }
    linked_metadata.sort();
    linked_metadata.dedup();

    Ok(Validation {
        protected,
        linked_metadata,
    })
}

fn validate_tree(
    root: &Path,
    protects_metadata: bool,
    sockets: &BTreeMap<PathBuf, SocketIdentity>,
) -> Result<Vec<PathBuf>, SandboxError> {
    let root_device = fs::metadata(root)
        .map_err(|source| failed("workspace root could not be inspected", source))?
        .dev();
    let mut protected = Vec::new();
    let mut hard_links: BTreeMap<(u64, u64), (u64, usize, bool, bool)> = BTreeMap::new();
    let mut pending = VecDeque::from([(root.to_path_buf(), 0_usize)]);
    let mut inspected = 0_usize;
    while let Some((directory, depth)) = pending.pop_front() {
        let entries = fs::read_dir(&directory)
            .map_err(|source| failed("workspace metadata scan failed", source))?;
        let entries =
            super::super::directory_entries(entries, MAX_ENTRIES.saturating_sub(inspected))
                .map_err(|source| {
                    failed("workspace metadata entry could not be inspected", source)
                })?;
        for entry in entries {
            inspected = inspected.saturating_add(1);
            if inspected > MAX_ENTRIES {
                return Err(refused("workspace validation scan exceeded its bound"));
            }
            let name = entry.file_name();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| {
                failed("workspace metadata path changed during preparation", source)
            })?;
            if metadata.file_type().is_symlink() {
                if protects_metadata && protected_name(&name) {
                    return Err(refused("protected workspace metadata is a symbolic link"));
                }
                continue;
            }
            if metadata.dev() != root_device {
                return Err(refused("workspace tree crosses a filesystem boundary"));
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
                        return Err(refused(
                            "workspace hard-link identity changed during preparation",
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
                if depth >= MAX_DEPTH {
                    return Err(refused(
                        "workspace validation scan exceeded its depth bound",
                    ));
                }
                pending.push_back((path, depth.saturating_add(1)));
                continue;
            }
            if (!protects_metadata || !protected_name(&name))
                && let Some(expected) = sockets.get(&path)
            {
                if expected.matches(&metadata) {
                    continue;
                }
                return Err(refused(
                    "granted Unix endpoint changed during workspace validation",
                ));
            }
            return Err(refused("workspace tree contains a special file"));
        }
    }
    if hard_links
        .values()
        .any(|(links, observed, protected, ordinary)| {
            usize::try_from(*links) != Ok(*observed) || (*protected && *ordinary)
        })
    {
        return Err(refused(
            "workspace hard-link group escapes its declared authority",
        ));
    }
    Ok(protected)
}

#[derive(Clone, Copy)]
struct SocketIdentity {
    device: u64,
    inode: u64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl SocketIdentity {
    fn from(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }

    fn matches(self, metadata: &fs::Metadata) -> bool {
        self.device == metadata.dev()
            && self.inode == metadata.ino()
            && self.changed_seconds == metadata.ctime()
            && self.changed_nanoseconds == metadata.ctime_nsec()
            && metadata.nlink() == 1
            && metadata.file_type().is_socket()
    }
}

fn validate_granted_sockets(
    policy: &SandboxPolicy,
) -> Result<BTreeMap<PathBuf, SocketIdentity>, SandboxError> {
    let SandboxNetworkPolicy::Domains(domains) = policy.network() else {
        return Ok(BTreeMap::new());
    };
    let mut sockets = BTreeMap::new();
    for path in domains.unix_sockets() {
        let inspect = || {
            fs::symlink_metadata(path)
                .map_err(|source| failed("granted Unix endpoint could not be inspected", source))
        };
        let named = inspect()?;
        if named.file_type().is_symlink() || !named.file_type().is_socket() || named.nlink() != 1 {
            return Err(refused("granted Unix endpoint is not a canonical socket"));
        }
        let canonical = path
            .canonicalize()
            .map_err(|source| failed("granted Unix endpoint could not be canonicalized", source))?;
        if canonical != path.as_path() {
            return Err(refused(
                "granted Unix endpoint contains a symbolic-link or non-canonical component",
            ));
        }
        let expected = SocketIdentity::from(&named);
        let confirmed = inspect()?;
        if !expected.matches(&confirmed) {
            return Err(refused(
                "granted Unix endpoint changed during workspace validation",
            ));
        }
        sockets.insert(path.clone(), expected);
    }
    Ok(sockets)
}

fn linked_worktree_metadata(path: &Path) -> Result<Vec<PathBuf>, SandboxError> {
    let text = fs::read_to_string(path)
        .map_err(|source| failed("linked-worktree metadata could not be read", source))?;
    if text.len() > 4096 {
        return Err(refused("linked-worktree metadata exceeds its bound"));
    }
    let Some(target) = text.trim().strip_prefix("gitdir: ") else {
        return Ok(Vec::new());
    };
    let requested_target = Path::new(target);
    if !requested_target.is_absolute() {
        return Err(refused(
            "linked-worktree metadata does not name an absolute git directory",
        ));
    }
    let target = requested_target
        .canonicalize()
        .map_err(|source| failed("linked-worktree git directory is unavailable", source))?;
    if target != requested_target || !fs::metadata(&target).is_ok_and(|metadata| metadata.is_dir())
    {
        return Err(refused(
            "linked-worktree git directory is not a canonical directory",
        ));
    }
    let back_reference = read_metadata_file(
        &target.join("gitdir"),
        "linked-worktree back-reference is unavailable",
    )?;
    let back_reference = Path::new(back_reference.trim());
    if !back_reference.is_absolute() || back_reference != path {
        return Err(refused(
            "linked-worktree git directory does not refer back to this workspace",
        ));
    }
    let mut linked = vec![target.clone()];
    let common_file = target.join("commondir");
    match fs::symlink_metadata(&common_file) {
        Ok(_) => {
            let relative = read_metadata_file(
                &common_file,
                "linked-worktree common directory is unavailable",
            )?;
            let common = target
                .join(relative.trim())
                .canonicalize()
                .map_err(|source| {
                    failed("linked-worktree common directory is unavailable", source)
                })?;
            if common == Path::new("/")
                || !fs::metadata(&common).is_ok_and(|metadata| metadata.is_dir())
            {
                return Err(refused("linked-worktree common directory is invalid"));
            }
            linked.push(common);
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(failed(
                "linked-worktree common directory could not be inspected",
                source,
            ));
        }
    }
    Ok(linked)
}

fn read_metadata_file(path: &Path, problem: &'static str) -> Result<String, SandboxError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| failed(problem, source))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.nlink() != 1 {
        return Err(refused(problem));
    }
    let text = fs::read_to_string(path).map_err(|source| failed(problem, source))?;
    if text.is_empty() || text.len() > 4096 {
        return Err(refused(problem));
    }
    Ok(text)
}

fn protected_name(name: &OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(".git" | ".agents" | ".codex" | ".crucible")
    )
}

fn refused(problem: &'static str) -> SandboxError {
    SandboxError::Materialization {
        problem: problem.into(),
        source: None,
    }
}

fn failed(problem: &'static str, source: std::io::Error) -> SandboxError {
    SandboxError::Materialization {
        problem: problem.into(),
        source: Some(source),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use crucible_core::{
        SandboxDomainPolicy, SandboxNetworkPolicy, SandboxNetworkProvenance, SandboxPolicy,
        SandboxResourceLimits,
    };

    #[test]
    fn nested_repository_metadata_is_discovered() {
        let sample = crate::sample::Sample::new("macos-nested-metadata");
        sample.write("nested/.git/config", "protected");
        let policy = SandboxPolicy::standard(&sample.workspace()).expect("policy");

        assert!(
            super::validate(&policy)
                .expect("validated tree")
                .protected()
                .contains(&policy.working_directory().join("nested/.git"))
        );
    }

    #[test]
    fn only_an_exact_granted_socket_is_accepted_in_the_workspace_tree() {
        let sample = crate::sample::Sample::socket("macos-granted-workspace-socket");
        let socket_path = sample.root().join("granted.sock");
        let _socket = UnixListener::bind(&socket_path).expect("Unix socket fixture");
        let standard = SandboxPolicy::standard(&sample.workspace()).expect("standard policy");
        let domains =
            SandboxDomainPolicy::new([], [], false, [socket_path], SandboxNetworkProvenance::User)
                .expect("domain policy");
        let policy = SandboxPolicy::new(
            true,
            standard.filesystem().iter().cloned(),
            standard.working_directory(),
            SandboxNetworkPolicy::Domains(domains),
            SandboxResourceLimits::confining(),
        )
        .expect("effective policy");

        super::validate(&policy).expect("the exact granted socket is valid tree content");

        let _ambient = UnixListener::bind(sample.root().join("ambient.sock"))
            .expect("ambient Unix socket fixture");
        let Err(problem) = super::validate(&policy) else {
            panic!("an ambient socket must be refused");
        };
        assert!(problem.to_string().contains("special file"));
    }

    #[test]
    fn a_hard_link_outside_the_workspace_is_refused() {
        let sample = crate::sample::Sample::new("macos-hard-link");
        let outside = sample
            .root()
            .parent()
            .expect("fixture parent")
            .join(format!("outside-hard-link-{}", std::process::id()));
        std::fs::write(&outside, "outside").expect("outside file");
        std::fs::hard_link(&outside, sample.root().join("linked")).expect("hard link");
        let policy = SandboxPolicy::standard(&sample.workspace()).expect("policy");

        assert!(matches!(
            super::validate(&policy),
            Err(crucible_core::SandboxError::Materialization { .. })
        ));
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn mutually_linked_worktree_metadata_is_read_only_runtime() {
        let sample = crate::sample::Sample::new("macos-linked-worktree");
        let common = sample
            .root()
            .parent()
            .expect("fixture parent")
            .join(format!("common-git-{}", std::process::id()));
        let target = common.join("worktrees/fixture");
        std::fs::create_dir_all(&target).expect("linked git directory");
        let common = common.canonicalize().expect("canonical common directory");
        let target = target
            .canonicalize()
            .expect("canonical linked git directory");
        sample.write(".git", &format!("gitdir: {}\n", target.display()));
        std::fs::write(
            target.join("gitdir"),
            sample
                .root()
                .canonicalize()
                .expect("canonical fixture root")
                .join(".git")
                .display()
                .to_string(),
        )
        .expect("back-reference");
        std::fs::write(target.join("commondir"), "../..\n").expect("common reference");
        let policy = SandboxPolicy::standard(&sample.workspace()).expect("policy");

        let validated = super::validate(&policy).expect("validated tree");
        assert!(validated.linked_metadata().contains(&target));
        assert!(validated.linked_metadata().contains(&common));
        let _ = std::fs::remove_dir_all(common);
    }
}
