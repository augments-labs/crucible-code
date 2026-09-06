//! Canonical native-Windows launch plan and path-scoped capability identities.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::windows::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

use crate::WindowsLaunchRequest;

const CAPABILITY_DOMAIN: &[u8] = b"crucible/windows-sandbox/path-capability/v1\0";
const DESKTOP_DOMAIN: &[u8] = b"crucible/windows-sandbox/desktop-capability/v1\0";

#[derive(Clone, Copy)]
pub(super) enum Access {
    Read,
    Write,
    DenyWrite,
}

impl Access {
    const fn label(self) -> &'static [u8] {
        match self {
            Self::Read => b"read",
            Self::Write => b"write",
            Self::DenyWrite => b"deny-write",
        }
    }
}

pub(super) struct LaunchPlan {
    request: WindowsLaunchRequest,
}

impl LaunchPlan {
    pub(super) fn resolve(request: &WindowsLaunchRequest) -> io::Result<Self> {
        let working_directory = canonical(request.working_directory(), true)?;
        let program = canonical(request.program(), false)?;
        let readable = canonical_roots(request.readable_roots())?;
        let writable = canonical_roots(request.writable_roots())?;
        let protected = canonical_roots(request.protected_roots())?;
        if same_path_in(&writable, &protected) {
            return Err(invalid("Windows sandbox roots have conflicting access"));
        }
        if !readable
            .iter()
            .chain(&writable)
            .chain(&protected)
            .any(|root| working_directory.starts_with(root))
        {
            return Err(invalid(
                "Windows sandbox working directory is outside readable roots",
            ));
        }
        // The approved executable is itself authority to load that image.
        // Windows WRITE_RESTRICTED tokens deliberately retain the dedicated
        // account's ordinary read access so the system loader can reach
        // protected KnownDlls objects. The request grants and protects the
        // selected executable itself before this plan reaches the broker.
        let mut environment = request.environment().to_vec();
        environment.sort_by_key(|left| lowercase(&left.0));
        if environment
            .iter()
            .any(|(name, _)| name.contains(&u16::from(b'=')))
            || environment.windows(2).any(|pair| {
                pair.first()
                    .zip(pair.get(1))
                    .is_some_and(|(left, right)| lowercase(&left.0) == lowercase(&right.0))
            })
        {
            return Err(invalid("Windows sandbox environment is ambiguous"));
        }
        let request = WindowsLaunchRequest::new(
            wide(working_directory.as_os_str()),
            wide(program.as_os_str()),
            request.arguments().to_vec(),
            environment,
            wide_roots(&readable),
            wide_roots(&writable),
            wide_roots(&protected),
        )?;
        Ok(Self { request })
    }

    /// The owner-side broker already canonicalized and authenticated this
    /// request before writing the child-only pipe.
    pub(super) fn from_host(request: &WindowsLaunchRequest) -> Self {
        Self {
            request: request.clone(),
        }
    }

    pub(super) const fn request(&self) -> &WindowsLaunchRequest {
        &self.request
    }

    pub(super) fn protects_broker(&self, broker: &Path) -> bool {
        let broker = lowercase(&wide(broker.as_os_str()));
        self.request
            .readable_roots()
            .iter()
            .any(|root| lowercase(root) == broker)
            && self
                .request
                .protected_roots()
                .iter()
                .any(|root| lowercase(root) == broker)
    }

    pub(super) fn capability_sids(&self, account_sid: &[u8]) -> Vec<Vec<u8>> {
        self.request
            .readable_roots()
            .iter()
            .map(|root| capability_sid(account_sid, Access::Read, root))
            .chain(
                self.request
                    .writable_roots()
                    .iter()
                    .map(|root| capability_sid(account_sid, Access::Write, root)),
            )
            .chain(
                self.request
                    .protected_roots()
                    .iter()
                    .map(|root| capability_sid(account_sid, Access::DenyWrite, root)),
            )
            .collect()
    }
}

pub(super) fn capability_sid(account_sid: &[u8], access: Access, path: &[u16]) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(CAPABILITY_DOMAIN);
    hash.update(account_sid);
    hash.update([0]);
    hash.update(access.label());
    hash.update([0]);
    for unit in lowercase(path) {
        hash.update(unit.to_le_bytes());
    }
    sid_from_digest(hash.finalize().into())
}

pub(super) fn desktop_sid(account_sid: &[u8], desktop: &[u16]) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(DESKTOP_DOMAIN);
    hash.update(account_sid);
    hash.update([0]);
    for unit in lowercase(desktop) {
        hash.update(unit.to_le_bytes());
    }
    sid_from_digest(hash.finalize().into())
}

fn sid_from_digest(digest: [u8; 32]) -> Vec<u8> {
    let mut sid = Vec::with_capacity(28);
    sid.extend_from_slice(&[1, 5, 0, 0, 0, 0, 0, 5]);
    sid.extend_from_slice(&21_u32.to_le_bytes());
    for chunk in digest[..16].chunks_exact(4) {
        let mut subauthority = [0_u8; 4];
        subauthority.copy_from_slice(chunk);
        sid.extend_from_slice(&u32::from_le_bytes(subauthority).to_le_bytes());
    }
    sid
}

fn canonical_roots(roots: &[Vec<u16>]) -> io::Result<Vec<PathBuf>> {
    let mut canonical: Vec<_> = roots
        .iter()
        .map(|root| canonical(root, false))
        .collect::<io::Result<_>>()?;
    canonical.sort_by_key(|path| lowercase(&wide(path.as_os_str())));
    let unique: BTreeSet<_> = canonical
        .iter()
        .map(|path| lowercase(&wide(path.as_os_str())))
        .collect();
    if unique.len() != canonical.len() {
        return Err(invalid("Windows sandbox roots contain aliases"));
    }
    Ok(canonical)
}

fn canonical(units: &[u16], directory: bool) -> io::Result<PathBuf> {
    let path = PathBuf::from(OsString::from_wide(units));
    if !path.is_absolute() {
        return Err(invalid("Windows sandbox path is not absolute"));
    }
    for component in path.ancestors() {
        let metadata = std::fs::symlink_metadata(component).map_err(|source| {
            io::Error::new(
                source.kind(),
                format!("Windows sandbox path could not be inspected: {source}"),
            )
        })?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid("Windows sandbox path crosses a reparse point"));
        }
    }
    let canonical = path.canonicalize().map_err(|source| {
        io::Error::new(
            source.kind(),
            format!("Windows sandbox path could not be canonicalized: {source}"),
        )
    })?;
    let metadata = canonical.metadata()?;
    if (directory && !metadata.is_dir())
        || (!directory && !metadata.is_dir() && !metadata.is_file())
    {
        return Err(invalid("Windows sandbox path has the wrong object type"));
    }
    Ok(canonical)
}

fn same_path_in(left: &[PathBuf], right: &[PathBuf]) -> bool {
    left.iter().any(|left| {
        let left = lowercase(&wide(left.as_os_str()));
        right
            .iter()
            .any(|right| left == lowercase(&wide(right.as_os_str())))
    })
}

fn wide_roots(roots: &[PathBuf]) -> Vec<Vec<u16>> {
    roots.iter().map(|root| wide(root.as_os_str())).collect()
}

fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().collect()
}

fn lowercase(units: &[u16]) -> Vec<u16> {
    String::from_utf16_lossy(units)
        .to_lowercase()
        .encode_utf16()
        .collect()
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::desktop_sid;

    #[test]
    fn private_desktops_have_distinct_capabilities() {
        let account = [1_u8, 2, 3, 4];
        let first: Vec<_> = "CrucibleSandbox-00000000000000000000000000000001\\Default"
            .encode_utf16()
            .collect();
        let second: Vec<_> = "CrucibleSandbox-00000000000000000000000000000002\\Default"
            .encode_utf16()
            .collect();

        assert_ne!(
            desktop_sid(&account, &first),
            desktop_sid(&account, &second)
        );
        assert_eq!(desktop_sid(&account, &first), desktop_sid(&account, &first));
    }
}
