//! Descriptor-pinned broker discovery and authenticated wait-status channel.

use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crucible_core::SandboxError;
use crucible_sandbox_broker::{
    GO_FRAME, READY_FRAME, REFUSED_DESCRIPTOR_CLOSURE, REFUSED_FRAME, REFUSED_SCAN,
};

const MAX_BROKER_BYTES: u64 = 16 * 1024 * 1024;

/// Whether a broker image, or a directory above it, can be rewritten only by
/// root or by the user running Crucible.
///
/// A path under the user's home is the normal case, and a user private group
/// makes group-writable directories the default there, so the group bit is
/// accepted only for this process's own effective group. Anyone else's
/// directory, and anything world-writable such as `/tmp`, is refused.
fn trusted_owner(uid: u32, gid: u32, mode: u32) -> bool {
    (uid == 0 || uid == rustix::process::getuid().as_raw())
        && mode & 0o002 == 0
        && (mode & 0o020 == 0 || gid == rustix::process::getegid().as_raw())
}

/// One opened broker image whose descriptor is mounted into the namespace.
pub(super) struct Broker {
    path: PathBuf,
    image: File,
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
        Self::first_trusted(candidates, excluded)
    }

    fn first_trusted(candidates: Vec<PathBuf>, excluded: &[&Path]) -> Result<Self, SandboxError> {
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
            // The broker is namespace PID 1: it applies the resource limits,
            // ends the process tree and drives the scan that decides what is
            // published back. Like Bubblewrap it may only come from a path no
            // other unprivileged user can rewrite, though unlike a system
            // package it may belong to the user running Crucible.
            if !metadata.is_file()
                || metadata.len() == 0
                || metadata.len() > MAX_BROKER_BYTES
                || !trusted_owner(
                    metadata.uid(),
                    metadata.gid(),
                    metadata.permissions().mode(),
                )
                || !super::probe::trusted_parent_chain_owned_by(&path, trusted_owner)
            {
                continue;
            }
            let Ok(image) = File::open(&path) else {
                continue;
            };
            let Ok(opened) = image.metadata() else {
                continue;
            };
            if opened.len() != metadata.len()
                || opened.ino() != metadata.ino()
                || opened.dev() != metadata.dev()
                || !trusted_owner(opened.uid(), opened.gid(), opened.permissions().mode())
            {
                continue;
            }
            return Ok(Self { path, image });
        }
        Err(unavailable(
            "the descriptor-pinned crucible-sandbox-broker executable is unavailable",
        ))
    }

    pub(super) fn descriptor(&self) -> RawFd {
        self.image.as_raw_fd()
    }
}

impl std::fmt::Debug for Broker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Broker")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// One private socket pair; only the broker side crosses the namespace.
pub(super) struct StatusChannel {
    reader: UnixStream,
    writer: Option<UnixStream>,
}

impl StatusChannel {
    pub(super) fn pair() -> io::Result<Self> {
        let (reader, writer) = UnixStream::pair()?;
        Ok(Self {
            reader,
            writer: Some(writer),
        })
    }

    pub(super) fn descriptor(&self) -> io::Result<RawFd> {
        self.writer
            .as_ref()
            .map(AsRawFd::as_raw_fd)
            .ok_or_else(|| io::Error::other("broker status writer is already closed"))
    }

    pub(super) fn close_writer(&mut self) {
        self.writer.take();
    }

    pub(super) fn attest_ready(&mut self) -> io::Result<()> {
        let mut ready = [0_u8; READY_FRAME.len()];
        self.reader.read_exact(&mut ready)?;
        if ready == REFUSED_FRAME {
            let mut reason = [0_u8; 1];
            self.reader.read_exact(&mut reason)?;
            return Err(io::Error::other(match reason.first().copied() {
                Some(REFUSED_SCAN) => {
                    "sandbox broker refused the bounded pre-release semantic scan"
                }
                Some(REFUSED_DESCRIPTOR_CLOSURE) => {
                    "sandbox broker could not close undeclared descriptors before release"
                }
                _ => "sandbox broker returned an unknown pre-release refusal",
            }));
        }
        if ready != READY_FRAME {
            return Err(io::Error::other(
                "sandbox broker did not attest readiness before release",
            ));
        }
        Ok(())
    }

    pub(super) fn send_go(&mut self) -> io::Result<()> {
        self.reader.write_all(&GO_FRAME)?;
        self.reader.flush()
    }

    pub(super) fn into_stream(self) -> UnixStream {
        self.reader
    }
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
    fn a_broker_image_is_trusted_only_where_no_other_user_can_rewrite_it() {
        let me = rustix::process::getuid().as_raw();
        let my_group = rustix::process::getegid().as_raw();
        assert!(trusted_owner(0, 0, 0o755));
        assert!(trusted_owner(me, my_group, 0o755));
        assert!(trusted_owner(me, my_group, 0o775), "the user's own group");
        assert!(!trusted_owner(me, my_group.wrapping_add(1), 0o775));
        assert!(!trusted_owner(me, my_group, 0o777));
        assert!(
            !trusted_owner(0, 0, 0o1777),
            "a sticky world-writable directory"
        );
        assert!(!trusted_owner(me.wrapping_add(1), my_group, 0o755));
    }

    #[test]
    fn a_broker_image_under_a_world_writable_directory_is_refused() {
        let sample = crate::sample::Sample::new("sandbox-broker-untrusted-parent");
        let image = sample.root().join("crucible-sandbox-broker");
        std::fs::write(&image, b"#!/bin/sh\nexit 0\n").expect("broker fixture");
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o755))
            .expect("fixture mode");
        std::fs::set_permissions(sample.root(), std::fs::Permissions::from_mode(0o777))
            .expect("world-writable parent");
        let refused = Broker::first_trusted(vec![image], &[]);
        assert!(
            matches!(refused, Err(SandboxError::BackendUnavailable { .. })),
            "a broker anyone can replace was accepted: {refused:?}"
        );
    }
}
