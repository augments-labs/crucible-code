//! Verified system-Bubblewrap Linux backend.

mod command;
mod probe;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crucible_core::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxCommand, SandboxError,
    SandboxFilesystemAccess, SandboxInspection, SandboxProcess, SandboxRequest, SandboxSession,
};

use super::process::{MAX_LOCAL_COMMANDS, Reservation};

pub(super) fn probe(
    excluded: &[&Path],
) -> Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError> {
    let backend = probe::Bwrap::find(excluded)?;
    Ok((backend.identity().clone(), backend.capabilities().clone()))
}

pub(super) fn prepare(
    request: SandboxRequest,
    active: Arc<AtomicUsize>,
) -> Result<Box<dyn SandboxSession>, SandboxError> {
    let excluded: Vec<_> = request
        .policy()
        .filesystem()
        .iter()
        .filter(|rule| rule.access() == SandboxFilesystemAccess::ReadWrite)
        .map(crucible_core::SandboxFilesystemRule::path)
        .collect();
    let backend = probe::Bwrap::find(&excluded)?;
    request.negotiate(backend.capabilities())?;
    command::validate(&request)?;

    let maximum = request
        .policy()
        .limits()
        .concurrent_commands
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(MAX_LOCAL_COMMANDS);
    let reservation = Reservation::take(active, maximum)?;
    let inspection = SandboxInspection::new(
        request.id(),
        backend.identity().clone(),
        backend.capabilities().clone(),
        request.policy().digest(),
        request.manifest().digest(),
        true,
        None::<Box<str>>,
        crucible_core::SandboxCleanup::Pending,
    )?;

    Ok(Box::new(LinuxSession {
        request,
        backend,
        inspection,
        reservation: Some(reservation),
        materialized: false,
    }))
}

struct LinuxSession {
    request: SandboxRequest,
    backend: probe::Bwrap,
    inspection: SandboxInspection,
    reservation: Option<Reservation>,
    materialized: bool,
}

impl SandboxSession for LinuxSession {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn materialize(&mut self) -> Result<(), SandboxError> {
        if !self.request.manifest().is_empty() {
            return Err(SandboxError::Unsupported {
                feature: crucible_core::SandboxFeature::Materialization,
            });
        }
        self.materialized = true;
        Ok(())
    }

    fn start(
        mut self: Box<Self>,
        command: SandboxCommand,
    ) -> Result<Box<dyn SandboxProcess>, SandboxError> {
        if !self.materialized {
            return Err(SandboxError::Materialization {
                problem: "session was not materialized before start".into(),
                source: None,
            });
        }
        let process = command::build(&self.backend, &self.request, &command)?;
        let reservation = self.reservation.take().ok_or(SandboxError::Concurrency)?;
        super::process::spawn(process, self.inspection.clone(), reservation)
    }
}

impl std::fmt::Debug for LinuxSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinuxSession")
            .field("inspection", &self.inspection)
            .field("materialized", &self.materialized)
            .finish_non_exhaustive()
    }
}
