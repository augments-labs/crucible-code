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
    SandboxBackendIdentity, SandboxCapabilities, SandboxCleanup, SandboxCommand,
    SandboxCommandStage, SandboxError, SandboxFactKind, SandboxFailureKind, SandboxFailurePhase,
    SandboxFilesystemAccess, SandboxGuardrailDecision, SandboxInspection, SandboxLifecycle,
    SandboxProcess, SandboxRequest, SandboxSession,
};

use super::process::{MAX_LOCAL_COMMANDS, Reservation};

pub(super) fn probe(
    excluded: &[&Path],
) -> Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError> {
    let backend = probe::Bwrap::find(excluded)?;
    Ok((backend.identity().clone(), backend.capabilities().clone()))
}

#[cfg(test)]
pub(super) fn declared_capabilities() -> SandboxCapabilities {
    probe::capabilities()
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
        request.policy(),
        request.manifest(),
        true,
        None::<Box<str>>,
        crucible_core::SandboxCleanup::Pending,
    )?;
    request.audit().record(
        request.id(),
        SandboxFactKind::Negotiated(Box::new(inspection.clone())),
    )?;
    request.audit().record(
        request.id(),
        SandboxFactKind::Lifecycle(SandboxLifecycle::Prepared),
    )?;

    Ok(Box::new(LinuxSession {
        request,
        backend,
        inspection,
        reservation: Some(reservation),
        view,
        materialization: None,
        materialized: false,
        transferred: false,
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
    transferred: bool,
}

impl SandboxSession for LinuxSession {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn materialize(&mut self) -> Result<(), SandboxError> {
        if self.materialized {
            return Ok(());
        }
        self.materialization = match materialize::commit(&self.request) {
            Ok(materialization) => materialization,
            Err(problem) => {
                self.request.audit().record(
                    self.request.id(),
                    SandboxFactKind::Failed {
                        phase: SandboxFailurePhase::Materialize,
                        kind: problem.failure_kind(),
                    },
                )?;
                return Err(problem);
            }
        };
        self.materialized = true;
        self.request.audit().record(
            self.request.id(),
            SandboxFactKind::Lifecycle(SandboxLifecycle::Materialized),
        )?;
        Ok(())
    }

    fn start(
        mut self: Box<Self>,
        command: SandboxCommand,
    ) -> Result<Box<dyn SandboxProcess>, SandboxError> {
        if !self.materialized {
            self.request.audit().record(
                self.request.id(),
                SandboxFactKind::Failed {
                    phase: SandboxFailurePhase::Start,
                    kind: SandboxFailureKind::Materialization,
                },
            )?;
            return Err(SandboxError::Materialization {
                problem: "session was not materialized before start".into(),
                source: None,
            });
        }
        for stage in [
            SandboxCommandStage::Requested,
            SandboxCommandStage::Effective,
        ] {
            let decision = self.request.policy().commands().evaluate(&command, stage);
            self.request.audit().record(
                self.request.id(),
                SandboxFactKind::Guardrail { stage, decision },
            )?;
            if decision != SandboxGuardrailDecision::Allowed {
                self.request.audit().record(
                    self.request.id(),
                    SandboxFactKind::Failed {
                        phase: SandboxFailurePhase::Start,
                        kind: SandboxFailureKind::Guardrail,
                    },
                )?;
                return Err(SandboxError::Guardrail);
            }
        }
        let process = match command::build(
            &self.backend,
            &self.request,
            &command,
            &self.view,
            self.materialization.as_ref(),
        ) {
            Ok(process) => process,
            Err(problem) => {
                self.record_start_failure(&problem)?;
                return Err(problem);
            }
        };
        let Some(reservation) = self.reservation.take() else {
            let problem = SandboxError::Concurrency;
            self.record_start_failure(&problem)?;
            return Err(problem);
        };
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
            super::process::SpawnPlan {
                inspection: self.inspection.clone(),
                reservation,
                stage,
                limits: self.request.policy().limits(),
                audit: self.request.audit().clone(),
                sandbox: self.request.id(),
            },
        );
        drop(mount_sources);
        match spawned {
            Ok(process) => {
                self.transferred = true;
                Ok(process)
            }
            Err(problem) => {
                self.record_start_failure(&problem)?;
                Err(problem)
            }
        }
    }
}

impl LinuxSession {
    fn record_start_failure(&self, problem: &SandboxError) -> Result<(), SandboxError> {
        self.request.audit().record(
            self.request.id(),
            SandboxFactKind::Failed {
                phase: SandboxFailurePhase::Start,
                kind: problem.failure_kind(),
            },
        )?;
        Ok(())
    }
}

impl Drop for LinuxSession {
    fn drop(&mut self) {
        if !self.transferred {
            let _ = self.request.audit().record(
                self.request.id(),
                SandboxFactKind::Cleanup(SandboxCleanup::Complete),
            );
        }
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
