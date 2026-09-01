//! Discovery, provenance, version, and functional backend probes.

use std::fs::File;
use std::io::{Read, Seek};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCapability, SandboxError, SandboxFeature,
};
use sha2::{Digest, Sha256};

/// Largest local backend artifact hashed during discovery.
const MAX_BACKEND_BYTES: u64 = 64 * 1024 * 1024;

/// Functional probes cannot hang startup indefinitely.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Required Bubblewrap command-line features.
const REQUIRED_OPTIONS: &[&str] = &[
    "--as-pid-1",
    "--bind",
    "--cap-drop",
    "--chdir",
    "--chmod",
    "--clearenv",
    "--dev",
    "--die-with-parent",
    "--dir",
    "--disable-userns",
    "--new-session",
    "--proc",
    "--remount-ro",
    "--ro-bind",
    "--setenv",
    "--tmpfs",
    "--unshare-ipc",
    "--unshare-net",
    "--unshare-pid",
    "--unshare-user",
    "--unshare-uts",
];

/// One verified system executable and its frozen claims.
#[derive(Debug, Clone)]
pub(super) struct Bwrap {
    path: PathBuf,
    identity: SandboxBackendIdentity,
    capabilities: SandboxCapabilities,
}

impl Bwrap {
    pub(super) fn find(excluded: &[&Path]) -> Result<Self, SandboxError> {
        let path = discover(excluded)?;
        let metadata = path
            .metadata()
            .map_err(|_| unavailable("could not inspect the system Bubblewrap executable"))?;
        if !metadata.is_file()
            || metadata.len() > MAX_BACKEND_BYTES
            || metadata.uid() != 0
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(unavailable(
                "system Bubblewrap is not a root-owned, non-writable regular file",
            ));
        }

        let help = Command::new(&path)
            .arg("--help")
            .env_clear()
            .output()
            .map_err(|_| unavailable("could not query system Bubblewrap capabilities"))?;
        let mut help_text = String::from_utf8_lossy(&help.stdout).into_owned();
        help_text.push_str(&String::from_utf8_lossy(&help.stderr));
        if !help.status.success()
            || REQUIRED_OPTIONS
                .iter()
                .any(|option| !help_text.contains(option))
        {
            return Err(unavailable(
                "system Bubblewrap does not expose the required confinement options",
            ));
        }

        let version = Command::new(&path)
            .arg("--version")
            .env_clear()
            .output()
            .map_err(|_| unavailable("could not query system Bubblewrap version"))?;
        let version = String::from_utf8_lossy(&version.stdout).trim().to_owned();
        if !version.starts_with("bubblewrap ") || version.len() > 128 {
            return Err(unavailable("system Bubblewrap returned an invalid version"));
        }

        functional_probe(&path)?;
        let digest = digest(&path, metadata.len())?;
        let id = SandboxBackendId::new("linux-bubblewrap")
            .map_err(|_| unavailable("invalid built-in Linux backend identity"))?;
        let identity = SandboxBackendIdentity::new(
            id,
            version,
            SandboxBackendProvenance::System,
            Some(digest),
        )
        .map_err(|_| unavailable("invalid system Bubblewrap identity"))?;

        Ok(Self {
            path,
            identity,
            capabilities: capabilities(),
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) const fn identity(&self) -> &SandboxBackendIdentity {
        &self.identity
    }

    pub(super) const fn capabilities(&self) -> &SandboxCapabilities {
        &self.capabilities
    }
}

fn discover(excluded: &[&Path]) -> Result<PathBuf, SandboxError> {
    let search = std::env::var_os("PATH")
        .ok_or_else(|| unavailable("PATH is unavailable while discovering system Bubblewrap"))?;
    let current = std::env::current_dir()
        .ok()
        .and_then(|path| path.canonicalize().ok());

    for directory in std::env::split_paths(&search).filter(|path| path.is_absolute()) {
        let candidate = directory.join("bwrap");
        let Ok(candidate) = candidate.canonicalize() else {
            continue;
        };
        if current
            .as_ref()
            .is_some_and(|root| candidate.starts_with(root))
            || excluded.iter().any(|root| candidate.starts_with(root))
        {
            continue;
        }
        return Ok(candidate);
    }

    Err(unavailable(
        "no suitable system Bubblewrap was found outside writable roots; bundled backend unavailable",
    ))
}

fn functional_probe(path: &Path) -> Result<(), SandboxError> {
    let mut child = Command::new(path)
        .args([
            "--die-with-parent",
            "--new-session",
            "--unshare-user",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-net",
            "--unshare-uts",
            "--disable-userns",
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--cap-drop",
            "ALL",
            "--clearenv",
            "--",
            "/bin/true",
        ])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| unavailable("could not start the system Bubblewrap probe"))?;

    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                return Err(unavailable(
                    "system Bubblewrap cannot create the required namespaces on this host",
                ));
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unavailable("system Bubblewrap probe timed out"));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unavailable("system Bubblewrap probe could not be reaped"));
            }
        }
    }
}

fn digest(path: &Path, length: u64) -> Result<[u8; 32], SandboxError> {
    if length > MAX_BACKEND_BYTES {
        return Err(unavailable(
            "system Bubblewrap executable is unexpectedly large",
        ));
    }
    let mut file = File::open(path)
        .map_err(|_| unavailable("could not open system Bubblewrap for provenance hashing"))?;
    file.rewind()
        .map_err(|_| unavailable("could not inspect system Bubblewrap provenance"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| unavailable("could not hash system Bubblewrap provenance"))?;
        if read == 0 {
            break;
        }
        let Some(bytes) = buffer.get(..read) else {
            return Err(unavailable("invalid Bubblewrap provenance read"));
        };
        digest.update(bytes);
    }
    Ok(digest.finalize().into())
}

fn capabilities() -> SandboxCapabilities {
    let enforced = SandboxCapability::Enforced;
    SandboxCapabilities::none()
        .with(SandboxFeature::Filesystem, enforced)
        .with(SandboxFeature::NetworkDeny, enforced)
        .with(SandboxFeature::DescriptorIsolation, enforced)
        .with(SandboxFeature::ProcessIsolation, enforced)
        .with(SandboxFeature::KernelSurface, enforced)
        .with(SandboxFeature::PrivilegeIsolation, enforced)
        .with(SandboxFeature::CommandTimeLimit, enforced)
        .with(SandboxFeature::OutputLimit, enforced)
        .with(SandboxFeature::ConcurrencyLimit, enforced)
        .with(SandboxFeature::Audit, enforced)
        .with(SandboxFeature::Usage, SandboxCapability::Observed)
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
    fn capability_snapshot_never_claims_unimplemented_limits_or_operations() {
        let capabilities = capabilities();
        assert_eq!(
            capabilities.claim(SandboxFeature::NetworkAllowlist),
            SandboxCapability::Unsupported
        );
        assert_eq!(
            capabilities.claim(SandboxFeature::MemoryLimit),
            SandboxCapability::Unsupported
        );
        assert_eq!(
            capabilities.claim(SandboxFeature::Snapshot),
            SandboxCapability::Unsupported
        );
        assert_eq!(
            capabilities.claim(SandboxFeature::Usage),
            SandboxCapability::Observed
        );
    }
}
