//! Verified system-Bubblewrap Linux backend.

mod command;
mod fd;
mod materialize;
mod probe;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crucible_core::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxCommand, SandboxCommandStage, SandboxError,
    SandboxFilesystemAccess, SandboxGuardrailDecision, SandboxInspection, SandboxProcess,
    SandboxRequest, SandboxSession,
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
    let view = command::prepare(&request)?;

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
        view,
        materialization: None,
        materialized: false,
    }))
}

struct LinuxSession {
    request: SandboxRequest,
    backend: probe::Bwrap,
    inspection: SandboxInspection,
    reservation: Option<Reservation>,
    view: command::View,
    materialization: Option<materialize::Materialization>,
    materialized: bool,
}

impl SandboxSession for LinuxSession {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn materialize(&mut self) -> Result<(), SandboxError> {
        if self.materialized {
            return Ok(());
        }
        self.materialization = materialize::commit(&self.request)?;
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
        if self
            .request
            .policy()
            .commands()
            .evaluate(&command, SandboxCommandStage::Requested)
            != SandboxGuardrailDecision::Allowed
        {
            return Err(SandboxError::Guardrail);
        }
        let process = command::build(
            &self.backend,
            &self.request,
            &command,
            &self.view,
            self.materialization.as_ref(),
        )?;
        let reservation = self.reservation.take().ok_or(SandboxError::Concurrency)?;
        let mut mount_sources = self.view.sources();
        let (stage, materialization_sources) = self
            .materialization
            .take()
            .map(materialize::Materialization::split)
            .map_or((None, Vec::new()), |(stage, sources)| {
                (Some(stage), sources)
            });
        mount_sources.extend(materialization_sources);
        let spawned = super::process::spawn(
            process,
            self.inspection.clone(),
            reservation,
            stage,
            self.request.policy().limits(),
        );
        drop(mount_sources);
        spawned
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
