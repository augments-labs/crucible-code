//! Provenance and functional probes for the native macOS backend.

use std::fs::File;
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCapability, SandboxError, SandboxFeature,
};
use sha2::{Digest as _, Sha256};

use super::broker::Broker;

const SEATBELT: &str = "/usr/bin/sandbox-exec";
const MAX_BACKEND_BYTES: u64 = 16 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub(super) struct Seatbelt {
    identity: SandboxBackendIdentity,
    capabilities: SandboxCapabilities,
}

impl Seatbelt {
    pub(super) fn find(broker: &Broker) -> Result<Self, SandboxError> {
        let path = PathBuf::from(SEATBELT);
        let metadata = path
            .metadata()
            .map_err(|_| unavailable("the system Seatbelt launcher is unavailable"))?;
        if !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > MAX_BACKEND_BYTES
            || metadata.uid() != 0
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(unavailable(
                "the system Seatbelt launcher is not root-owned and non-writable",
            ));
        }
        functional_probe(broker)?;
        let id = SandboxBackendId::new("macos-seatbelt")
            .map_err(|_| unavailable("invalid built-in macOS backend identity"))?;
        let identity = SandboxBackendIdentity::new(
            id,
            "seatbelt-v1",
            SandboxBackendProvenance::System,
            Some(digest(&path, metadata.len())?),
        )
        .map_err(|_| unavailable("invalid built-in macOS backend version"))?;
        Ok(Self {
            identity,
            capabilities: capabilities(),
        })
    }

    pub(super) const fn identity(&self) -> &SandboxBackendIdentity {
        &self.identity
    }

    pub(super) const fn capabilities(&self) -> &SandboxCapabilities {
        &self.capabilities
    }
}

pub(super) fn capabilities() -> SandboxCapabilities {
    let enforced = SandboxCapability::Enforced;
    SandboxCapabilities::none()
        .with(SandboxFeature::Filesystem, enforced)
        .with(SandboxFeature::NetworkDeny, enforced)
        .with(SandboxFeature::DescriptorIsolation, enforced)
        .with(SandboxFeature::ProcessIsolation, enforced)
        .with(SandboxFeature::KernelSurface, enforced)
        .with(SandboxFeature::PrivilegeIsolation, enforced)
        .with(SandboxFeature::OpenFileLimit, enforced)
        .with(SandboxFeature::CommandTimeLimit, enforced)
        .with(SandboxFeature::OutputLimit, enforced)
        .with(SandboxFeature::ConcurrencyLimit, enforced)
        .with(SandboxFeature::Audit, enforced)
        .with(SandboxFeature::Usage, SandboxCapability::Observed)
}

fn functional_probe(broker: &Broker) -> Result<(), SandboxError> {
    let mut child = Command::new(broker.path())
        .args([
            crucible_sandbox_broker::MACOS_LAUNCH_MODE,
            "--cpu-seconds",
            "0",
            "--open-files",
            "64",
            "--profile",
            "(version 1)\n(allow default)\n",
            "--",
            "/usr/bin/true",
        ])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| unavailable("the macOS sandbox functional probe could not start"))?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) | Err(_) => {
                return Err(unavailable("the macOS sandbox functional probe failed"));
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unavailable("the macOS sandbox functional probe timed out"));
            }
        }
    }
}

fn digest(path: &Path, length: u64) -> Result<[u8; 32], SandboxError> {
    let mut file = File::open(path)
        .map_err(|_| unavailable("the system Seatbelt launcher could not be opened"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut read = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| unavailable("the system Seatbelt launcher could not be hashed"))?;
        if count == 0 {
            break;
        }
        read = read.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
        if read > length || read > MAX_BACKEND_BYTES {
            return Err(unavailable(
                "the system Seatbelt launcher changed while it was inspected",
            ));
        }
        let chunk = buffer.get(..count).ok_or_else(|| {
            unavailable("the system Seatbelt launcher returned an invalid read length")
        })?;
        digest.update(chunk);
    }
    if read != length {
        return Err(unavailable(
            "the system Seatbelt launcher changed while it was inspected",
        ));
    }
    Ok(digest.finalize().into())
}

fn unavailable(reason: &'static str) -> SandboxError {
    SandboxError::BackendUnavailable {
        reason: reason.into(),
    }
}
