//! Discovery and provenance of the packaged native-Windows broker.

use std::fs::File;
use std::io::Read as _;
use std::os::windows::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use crucible_core::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxError,
};
use sha2::{Digest as _, Sha256};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

const MAX_BROKER_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(super) struct Broker {
    path: PathBuf,
    identity: SandboxBackendIdentity,
}

impl Broker {
    pub(super) fn find(excluded: &[&Path]) -> Result<Self, SandboxError> {
        let executable = std::env::current_exe()
            .map_err(|_| unavailable("could not locate the Crucible executable"))?;
        let mut candidates = Vec::new();
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join("crucible-sandbox-broker.exe"));
            if let Some(build_root) = parent.parent() {
                candidates.push(build_root.join("crucible-sandbox-broker.exe"));
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
                || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            {
                continue;
            }
            let digest = digest(&path, metadata.len())?;
            let id = SandboxBackendId::new("windows-native")
                .map_err(|_| unavailable("invalid built-in Windows backend identity"))?;
            let identity = SandboxBackendIdentity::new(
                id,
                "account-wfp-token-v1",
                SandboxBackendProvenance::Bundled,
                Some(digest),
            )
            .map_err(|_| unavailable("invalid built-in Windows backend version"))?;
            return Ok(Self { path, identity });
        }
        Err(unavailable(
            "the packaged Windows sandbox broker is unavailable outside writable roots",
        ))
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) const fn identity(&self) -> &SandboxBackendIdentity {
        &self.identity
    }
}

fn digest(path: &Path, expected: u64) -> Result<[u8; 32], SandboxError> {
    let mut file = File::open(path)
        .map_err(|_| unavailable("the Windows sandbox broker could not be opened"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| unavailable("the Windows sandbox broker could not be hashed"))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if total > expected || total > MAX_BROKER_BYTES {
            return Err(unavailable(
                "the Windows sandbox broker changed while it was inspected",
            ));
        }
        let bytes = buffer.get(..read).ok_or_else(|| {
            unavailable("the Windows sandbox broker returned an invalid read length")
        })?;
        digest.update(bytes);
    }
    if total != expected {
        return Err(unavailable(
            "the Windows sandbox broker changed while it was inspected",
        ));
    }
    Ok(digest.finalize().into())
}

fn unavailable(reason: &'static str) -> SandboxError {
    SandboxError::BackendUnavailable {
        reason: reason.into(),
    }
}
