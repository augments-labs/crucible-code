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

/// A hostile or accidental PATH cannot make backend discovery unbounded.
const MAX_BWRAP_CANDIDATES: usize = 128;

/// Required Bubblewrap command-line features.
const REQUIRED_OPTIONS: &[&str] = &[
    "--as-pid-1",
    "--bind",
    "--bind-fd",
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
    "--ro-bind-fd",
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
        let search = std::env::var_os("PATH").ok_or_else(|| {
            unavailable("PATH is unavailable while discovering system Bubblewrap")
        })?;
        let current = std::env::current_dir()
            .ok()
            .and_then(|path| path.canonicalize().ok());
        let candidates =
            discover_candidates(std::env::split_paths(&search), current.as_deref(), excluded);
        if candidates.is_empty() {
            return Err(unavailable(
                "no suitable system Bubblewrap was found outside writable roots; bundled backend unavailable",
            ));
        }
        first_verified(candidates, |path| Self::verify(path).ok()).ok_or_else(|| {
            unavailable(
                "discovered system Bubblewrap candidates failed provenance, feature, version, or namespace verification; bundled backend unavailable",
            )
        })
    }

    fn verify(path: PathBuf) -> Result<Self, SandboxError> {
        let metadata = path
            .metadata()
            .map_err(|_| unavailable("could not inspect the system Bubblewrap executable"))?;
        if !metadata.is_file()
            || metadata.len() > MAX_BACKEND_BYTES
            || metadata.uid() != 0
            || metadata.permissions().mode() & 0o022 != 0
            || !trusted_parent_chain(&path)
        {
            return Err(unavailable(
                "system Bubblewrap or its parent path is not root-owned and non-writable",
            ));
        }

        let help = Command::new(&path)
            .arg("--help")
            .env_clear()
            .output()
            .map_err(|_| unavailable("could not query system Bubblewrap capabilities"))?;
        let mut help_text = String::from_utf8_lossy(&help.stdout).into_owned();
        help_text.push_str(&String::from_utf8_lossy(&help.stderr));
        if !help.status.success() || !help_supports_required_options(&help_text) {
            return Err(unavailable(
                "system Bubblewrap does not expose the required confinement options",
            ));
        }

        let version = Command::new(&path)
            .arg("--version")
            .env_clear()
            .output()
            .map_err(|_| unavailable("could not query system Bubblewrap version"))?;
        let version_text = String::from_utf8_lossy(&version.stdout);
        let Some(version) = parse_version(&version_text) else {
            return Err(unavailable("system Bubblewrap returned an invalid version"));
        };

        functional_probe(&path)?;
        let digest = digest(&path, metadata.len())?;
        let id = SandboxBackendId::new("linux-bubblewrap")
            .map_err(|_| unavailable("invalid built-in Linux backend identity"))?;
        let identity = SandboxBackendIdentity::new(
            id,
            version.to_owned(),
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

fn discover_candidates(
    search: impl IntoIterator<Item = PathBuf>,
    current: Option<&Path>,
    excluded: &[&Path],
) -> Vec<PathBuf> {
    let hidden_current = current.filter(|root| root.parent().is_some());
    let mut candidates = Vec::new();
    for directory in search
        .into_iter()
        .filter(|path| path.is_absolute())
        .take(MAX_BWRAP_CANDIDATES)
    {
        let candidate = directory.join("bwrap");
        let Ok(candidate) = candidate.canonicalize() else {
            continue;
        };
        if hidden_current.is_some_and(|root| candidate.starts_with(root))
            || excluded.iter().any(|root| candidate.starts_with(root))
        {
            continue;
        }
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

fn first_verified<T>(
    candidates: impl IntoIterator<Item = PathBuf>,
    verify: impl FnMut(PathBuf) -> Option<T>,
) -> Option<T> {
    candidates.into_iter().find_map(verify)
}

fn help_supports_required_options(help: &str) -> bool {
    REQUIRED_OPTIONS
        .iter()
        .all(|option| help.split_ascii_whitespace().any(|token| token == *option))
}

fn parse_version(version: &str) -> Option<&str> {
    let version = version.trim();
    let number = version.strip_prefix("bubblewrap ")?;
    let mut components = number.split('.');
    let valid = number.len() <= 64
        && components.by_ref().take(3).all(|component| {
            !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_digit())
        })
        && components.next().is_none()
        && number.matches('.').count() == 2;
    valid.then_some(version)
}

fn trusted_parent_chain(path: &Path) -> bool {
    trusted_parent_chain_with(path, |directory| {
        let metadata = directory.symlink_metadata().ok()?;
        Some((
            metadata.is_dir(),
            metadata.uid(),
            metadata.permissions().mode(),
        ))
    })
}

fn trusted_parent_chain_with(
    path: &Path,
    mut inspect: impl FnMut(&Path) -> Option<(bool, u32, u32)>,
) -> bool {
    path.parent().is_some_and(|parent| {
        parent.ancestors().all(|directory| {
            inspect(directory).is_some_and(|(is_directory, uid, mode)| {
                is_directory && uid == 0 && mode & 0o022 == 0
            })
        })
    })
}

fn functional_probe(path: &Path) -> Result<(), SandboxError> {
    let mut command = Command::new(path);
    command
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
        .stderr(Stdio::null());
    super::fd::inherit(&mut command, &[]).map_err(|_| {
        unavailable("could not prepare descriptor isolation for the Bubblewrap probe")
    })?;
    let mut child = command
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

pub(super) fn capabilities() -> SandboxCapabilities {
    let enforced = SandboxCapability::Enforced;
    SandboxCapabilities::none()
        .with(SandboxFeature::Filesystem, enforced)
        .with(SandboxFeature::NetworkDeny, enforced)
        .with(SandboxFeature::DescriptorIsolation, enforced)
        .with(SandboxFeature::ProcessIsolation, enforced)
        .with(SandboxFeature::KernelSurface, enforced)
        .with(SandboxFeature::PrivilegeIsolation, enforced)
        .with(SandboxFeature::Materialization, enforced)
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
    use std::cell::Cell;

    use crate::sample::Sample;

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

    #[test]
    fn candidate_discovery_skips_workspace_and_excluded_roots_but_not_root_cwd() {
        let sample = Sample::new("sandbox-bwrap-candidates");
        let workspace_bin = sample.root().join("bin");
        let excluded = sample.root().join("excluded");
        let system = sample.root().join("system");
        for directory in [&workspace_bin, &excluded, &system] {
            std::fs::create_dir_all(directory).expect("candidate directory");
            std::fs::write(directory.join("bwrap"), "fixture").expect("candidate");
        }

        let candidates = discover_candidates(
            [workspace_bin, excluded.clone(), system.clone()],
            Some(sample.root()),
            &[excluded.as_path()],
        );
        assert!(candidates.is_empty(), "every candidate is below cwd");

        let candidates = discover_candidates([system.clone()], Some(Path::new("/")), &[]);
        assert_eq!(candidates, [system.join("bwrap").canonicalize().unwrap()]);
    }

    #[test]
    fn verification_falls_through_an_unsuitable_earlier_candidate() {
        let attempts = Cell::new(0_usize);
        let selected = first_verified([PathBuf::from("old"), PathBuf::from("suitable")], |path| {
            attempts.set(attempts.get().saturating_add(1));
            (path == Path::new("suitable")).then_some(path)
        });

        assert_eq!(selected, Some(PathBuf::from("suitable")));
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn older_bubblewrap_help_and_malformed_versions_are_refused() {
        let complete = REQUIRED_OPTIONS.join("\n");
        assert!(help_supports_required_options(&complete));
        assert!(!help_supports_required_options("--ro-bind\n--unshare-user"));
        assert!(!help_supports_required_options(
            &complete.replace("--bind\n", "--bind-fd-only\n")
        ));
        assert_eq!(
            parse_version("bubblewrap 0.11.1\n"),
            Some("bubblewrap 0.11.1")
        );
        assert_eq!(parse_version("bwrap 0.11.1"), None);
        assert_eq!(parse_version("bubblewrap 0.11"), None);
        assert_eq!(parse_version("bubblewrap 0.11.x"), None);
        assert_eq!(parse_version("bubblewrap 0.11.1 extra"), None);
        assert_eq!(
            parse_version(&format!("bubblewrap {}", "x".repeat(129))),
            None
        );
    }

    #[test]
    fn every_backend_parent_must_be_a_root_owned_non_writable_directory() {
        let executable = Path::new("/usr/local/bin/bwrap");
        assert!(trusted_parent_chain_with(executable, |_| Some((
            true, 0, 0o755
        ))));
        assert!(!trusted_parent_chain_with(executable, |directory| {
            Some((
                true,
                u32::from(directory == Path::new("/usr/local")) * 1000,
                0o755,
            ))
        }));
        assert!(!trusted_parent_chain_with(executable, |directory| {
            Some((
                true,
                0,
                if directory == Path::new("/usr/local") {
                    0o775
                } else {
                    0o755
                },
            ))
        }));
        assert!(!trusted_parent_chain_with(executable, |directory| {
            (directory != Path::new("/usr/local")).then_some((true, 0, 0o755))
        }));
    }
}
