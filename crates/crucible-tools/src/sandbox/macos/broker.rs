//! Discovery of the packaged pre-Seatbelt launcher.

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use crucible_core::SandboxError;

const MAX_BROKER_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(super) struct Broker {
    path: PathBuf,
}

impl Broker {
    pub(super) fn find(excluded: &[&Path]) -> Result<Self, SandboxError> {
        let executable = std::env::current_exe()
            .map_err(|_| unavailable("could not locate the Crucible executable"))?;
        let mut candidates = Vec::new();
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join("crucible-sandbox-broker"));
            if let Some(build_root) = parent.parent() {
                candidates.push(build_root.join("crucible-sandbox-broker"));
            }
        }
        candidates.sort();
        candidates.dedup();

        for candidate in candidates {
            let Ok(path) = candidate.canonicalize() else {
                continue;
            };
            if excluded.iter().any(|root| path.starts_with(root)) {
                continue;
            }
            let Ok(metadata) = path.metadata() else {
                continue;
            };
            if !metadata.is_file()
                || metadata.len() == 0
                || metadata.len() > MAX_BROKER_BYTES
                || !trusted_owner(metadata.uid(), metadata.permissions().mode())
                || !trusted_parent_chain(&path)
            {
                continue;
            }
            return Ok(Self { path });
        }
        Err(unavailable(
            "the packaged crucible-sandbox-broker executable is unavailable or writable by another user",
        ))
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

fn trusted_owner(uid: u32, mode: u32) -> bool {
    (uid == 0 || uid == rustix::process::getuid().as_raw()) && mode & 0o022 == 0
}

fn trusted_parent_chain(path: &Path) -> bool {
    path.parent().is_some_and(|parent| {
        parent.ancestors().all(|directory| {
            directory.symlink_metadata().is_ok_and(|metadata| {
                metadata.is_dir() && trusted_owner(metadata.uid(), metadata.permissions().mode())
            })
        })
    })
}

fn unavailable(reason: &'static str) -> SandboxError {
    SandboxError::BackendUnavailable {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::trusted_owner;

    #[test]
    fn only_root_or_the_current_user_may_own_a_non_shared_launcher() {
        let me = rustix::process::getuid().as_raw();
        assert!(trusted_owner(0, 0o755));
        assert!(trusted_owner(me, 0o700));
        assert!(!trusted_owner(me, 0o775));
        assert!(!trusted_owner(me.wrapping_add(1), 0o755));
    }
}
