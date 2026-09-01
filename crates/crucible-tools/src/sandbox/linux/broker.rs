//! Descriptor-pinned broker discovery and authenticated wait-status channel.

use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use crucible_core::SandboxError;
use crucible_sandbox_broker::{
    BROKER_FAILURE_STATUS, GO_FRAME, READY_FRAME, WAIT_STATUS_BYTES, decode_wait_status,
};

const MAX_BROKER_BYTES: u64 = 16 * 1024 * 1024;

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
                || metadata.permissions().mode() & 0o022 != 0
            {
                continue;
            }
            let Ok(image) = File::open(&path) else {
                continue;
            };
            let Ok(opened) = image.metadata() else {
                continue;
            };
            if opened.len() != metadata.len() || opened.permissions().mode() & 0o022 != 0 {
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

    pub(super) fn release(&mut self) -> io::Result<()> {
        let mut ready = [0_u8; READY_FRAME.len()];
        self.reader.read_exact(&mut ready)?;
        if ready != READY_FRAME {
            return Err(io::Error::other(
                "sandbox broker did not attest readiness before release",
            ));
        }
        self.reader.write_all(&GO_FRAME)?;
        self.reader.flush()
    }

    pub(super) fn wait_status(&mut self) -> io::Result<ExitStatus> {
        let mut frame = [0_u8; WAIT_STATUS_BYTES];
        self.reader.read_exact(&mut frame)?;
        let raw = decode_wait_status(frame);
        if raw == BROKER_FAILURE_STATUS {
            return Err(io::Error::other(
                "sandbox broker could not create or reap the workload",
            ));
        }
        let mut trailing = [0_u8; 1];
        if self.reader.read(&mut trailing)? != 0 {
            return Err(io::Error::other(
                "sandbox broker emitted more than one wait-status frame",
            ));
        }
        Ok(ExitStatus::from_raw(raw))
    }
}

fn unavailable(reason: &'static str) -> SandboxError {
    SandboxError::BackendUnavailable {
        reason: reason.into(),
    }
}
