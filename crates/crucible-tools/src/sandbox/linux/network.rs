//! Descriptor-pinned Unix endpoints exposed to the private network namespace.

use std::fs;
use std::os::fd::{AsRawFd as _, OwnedFd, RawFd};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};

use crucible_core::SandboxError;

pub(super) const PROXY_PATH: &str = "/run/crucible/network.sock";
pub(super) const PROXY_ADDRESS: std::net::SocketAddr =
    std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 31337);

/// `O_PATH` pins the socket inode without connecting or opening a directory that
/// could reveal another endpoint after preparation. The caller retains it until
/// Bubblewrap has consumed the inherited descriptor.
pub(super) struct SocketMount {
    source: OwnedFd,
    destination: PathBuf,
}
impl SocketMount {
    pub(super) fn open(source: &Path, destination: &Path) -> Result<Self, SandboxError> {
        let open = || -> std::io::Result<Self> {
            let named = fs::symlink_metadata(source)?;
            if !named.file_type().is_socket()
                || named.nlink() != 1
                || source.canonicalize()? != source
            {
                return Err(std::io::Error::other(
                    "sandbox Unix endpoint is not a canonical socket",
                ));
            }
            let descriptor = rustix::fs::open(
                source,
                rustix::fs::OFlags::PATH
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )?;
            let opened = rustix::fs::fstat(&descriptor)?;
            if opened.st_dev != named.dev()
                || opened.st_ino != named.ino()
                || rustix::fs::FileType::from_raw_mode(opened.st_mode)
                    != rustix::fs::FileType::Socket
            {
                return Err(std::io::Error::other(
                    "sandbox Unix endpoint changed during preparation",
                ));
            }
            Ok(Self {
                source: descriptor,
                destination: destination.to_owned(),
            })
        };
        open().map_err(|source| SandboxError::Materialization {
            problem: "sandbox Unix endpoint could not be pinned".into(),
            source: Some(source),
        })
    }
    pub(super) fn descriptor(&self) -> RawFd {
        self.source.as_raw_fd()
    }
    pub(super) fn destination(&self) -> &Path {
        &self.destination
    }
}
