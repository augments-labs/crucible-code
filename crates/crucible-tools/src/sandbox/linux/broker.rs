//! Descriptor-pinned broker discovery and authenticated wait-status channel.

use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crucible_core::SandboxError;
use crucible_sandbox_broker::{
    CANCEL_FRAME, GO_FRAME, READY_FRAME, REFUSED_DESCRIPTOR_CLOSURE, REFUSED_FRAME, REFUSED_SCAN,
};

use super::super::process::Canceller;

const MAX_BROKER_BYTES: u64 = 16 * 1024 * 1024;

/// Whether a broker image, or a directory above it, can be rewritten only by
/// root or by the user running Crucible.
///
/// Unlike a system package the image may belong to the user, since a path
/// under their home is the normal case. Group write is refused even for the
/// user's own group: nothing here can prove that group is private to them, and
/// a shared group would let any member rewrite namespace PID 1 in place.
fn trusted_owner(uid: u32, mode: u32) -> bool {
    (uid == 0 || uid == rustix::process::getuid().as_raw()) && mode & 0o022 == 0
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
            // other unprivileged user can rewrite.
            if !metadata.is_file()
                || metadata.len() == 0
                || metadata.len() > MAX_BROKER_BYTES
                || !trusted_owner(metadata.uid(), metadata.permissions().mode())
                || !super::probe::trusted_parent_chain_owned_by(&path, |uid, _, mode| {
                    trusted_owner(uid, mode)
                })
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
                || !trusted_owner(opened.uid(), opened.permissions().mode())
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

    /// A stop the supervisor hands the broker before it kills the launcher.
    ///
    /// Killing Bubblewrap alone does not end the PID namespace: the broker,
    /// started in its own session, outlives it together with the workload. The
    /// cancellation frame makes the broker kill the workload and write its wait
    /// status, and the wait keeps the launcher alive until that report has
    /// left, or until the budget ends and the kill proceeds regardless.
    pub(super) fn canceller(&self) -> io::Result<Canceller> {
        let channel = self.reader.try_clone()?;
        Ok(Box::new(move |leader| {
            (&channel).write_all(&CANCEL_FRAME)?;
            (&channel).flush()?;
            await_launcher_exit(leader)
        }))
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

/// How long a cancelled broker may take to end its workload and report.
///
/// The supervisor holds the process lifecycle lock meanwhile, so this also
/// bounds how long a wait on the process can stall behind a deadline or an
/// output ceiling.
const CANCEL_GRACE: Duration = Duration::from_secs(5);
/// How often the cancelling supervisor looks for the launcher's exit.
const CANCEL_POLL: Duration = Duration::from_millis(5);

/// Waits, within the cancellation budget, for the launcher to exit.
///
/// The exit is observed without reaping so the process owner still collects
/// the status. A launcher that outlives the budget is reported as timed out
/// and left to the kill.
fn await_launcher_exit(leader: u32) -> io::Result<()> {
    use rustix::process::{WaitId, WaitIdOptions};

    let raw = i32::try_from(leader)
        .map_err(|_| io::Error::other("launcher process id does not fit this platform"))?;
    let pid = rustix::process::Pid::from_raw(raw)
        .ok_or_else(|| io::Error::other("launcher process id cannot be observed"))?;
    let options = WaitIdOptions::NOHANG | WaitIdOptions::EXITED | WaitIdOptions::NOWAIT;
    let expired = Instant::now() + CANCEL_GRACE;
    while rustix::process::waitid(WaitId::Pid(pid), options)?.is_none() {
        if Instant::now() >= expired {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "sandbox broker did not report within the cancellation budget",
            ));
        }
        std::thread::sleep(CANCEL_POLL);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broker_image_is_trusted_only_where_no_other_user_can_rewrite_it() {
        let me = rustix::process::getuid().as_raw();
        assert!(trusted_owner(0, 0o755));
        assert!(trusted_owner(me, 0o755));
        assert!(trusted_owner(me, 0o700));
        assert!(
            !trusted_owner(me, 0o775),
            "the user's own group cannot be proven private"
        );
        assert!(
            !trusted_owner(0, 0o775),
            "root's file, but any group member"
        );
        assert!(!trusted_owner(me, 0o777));
        assert!(
            !trusted_owner(0, 0o1777),
            "a sticky world-writable directory"
        );
        assert!(!trusted_owner(me.wrapping_add(1), 0o755));
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
            "a broker anyone can replace was accepted"
        );
    }
}
