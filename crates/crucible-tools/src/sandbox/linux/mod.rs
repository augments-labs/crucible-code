//! Verified system-Bubblewrap Linux backend.

mod broker;
mod command;
mod fd;
mod materialize;
mod probe;
mod projection;
mod transaction;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crucible_core::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxCleanup, SandboxCommand,
    SandboxCommandStage, SandboxError, SandboxFactKind, SandboxFailureKind, SandboxFailurePhase,
    SandboxFilesystemAccess, SandboxGuardrailDecision, SandboxInspection, SandboxInvocationMode,
    SandboxLaunch, SandboxLifecycle, SandboxProcess, SandboxRequest, SandboxSession,
};

use super::process::{MAX_LOCAL_COMMANDS, Reservation};

pub(super) fn probe(
    excluded: &[&Path],
) -> Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError> {
    let backend = probe::Bwrap::find(excluded)?;
    let _broker = broker::Broker::find(excluded)?;
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
    let broker = broker::Broker::find(&excluded)?;
    request.negotiate(backend.capabilities())?;
    let view = command::prepare(&request)?;

    let inspection = SandboxInspection::confined_for_request(
        backend.identity().clone(),
        backend.capabilities().clone(),
        &request,
    )?;
    request.audit().record(
        request.id(),
        SandboxFactKind::Negotiated(Box::new(inspection.clone())),
    )?;
    let transaction = transaction::Lease::acquire(&request)?;

    let maximum = request
        .policy()
        .limits()
        .concurrent_commands
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(MAX_LOCAL_COMMANDS);
    let reservation = Reservation::take(active, maximum)?;
    request.audit().record(
        request.id(),
        SandboxFactKind::Lifecycle(SandboxLifecycle::Prepared),
    )?;

    Ok(Box::new(LinuxSession {
        request,
        backend,
        broker,
        inspection,
        reservation: Some(reservation),
        view,
        materialization: None,
        materialized: false,
        transferred: false,
        transaction,
    }))
}

struct LinuxSession {
    request: SandboxRequest,
    backend: probe::Bwrap,
    broker: broker::Broker,
    inspection: SandboxInspection,
    reservation: Option<Reservation>,
    view: command::View,
    materialization: Option<materialize::Materialization>,
    materialized: bool,
    transferred: bool,
    transaction: Option<transaction::Lease>,
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

    fn stage(
        mut self: Box<Self>,
        command: SandboxCommand,
    ) -> Result<Box<dyn SandboxLaunch>, SandboxError> {
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
        let projection = match projection::Projection::prepare(
            &self.request,
            &self.view,
            self.materialization.as_ref(),
            self.transaction.take(),
        ) {
            Ok(projection) => projection,
            Err(problem) => {
                self.record_start_failure(&problem)?;
                return Err(problem);
            }
        };
        let mut status_channel = broker::StatusChannel::pair().map_err(SandboxError::Spawn)?;
        let status_descriptor = status_channel.descriptor().map_err(SandboxError::Spawn)?;
        let process = match command::build(command::Plan {
            backend: &self.backend,
            broker: &self.broker,
            request: &self.request,
            command: &command,
            view: &self.view,
            materialization: self.materialization.as_ref(),
            projection: projection.as_ref(),
            status_descriptor,
        }) {
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
        let (stage, materialization_sources) = self
            .materialization
            .take()
            .map(materialize::Materialization::split)
            .map_or((None, Vec::new()), |(stage, sources)| {
                (Some(stage), sources)
            });
        let spawned = super::process::spawn(
            process,
            super::process::SpawnPlan {
                inspection: self.inspection.clone(),
                reservation,
                stage,
                limits: self.request.policy().limits(),
                audit: self.request.audit().clone(),
                sandbox: self.request.id(),
                audit_started: false,
                invocation: self.request.invocation_mode(),
                call_result_key: self.request.call_result_key(),
            },
        );
        status_channel.close_writer();
        drop(materialization_sources);
        let process = match spawned {
            Ok(process) => process,
            Err(problem) => {
                self.record_start_failure(&problem)?;
                return Err(problem);
            }
        };
        let launch = LinuxLaunch {
            process: Some(process),
            projection,
            status_channel: Some(status_channel),
            inspection: self.inspection.clone(),
            audit: self.request.audit().clone(),
            sandbox: self.request.id(),
            invocation: self.request.invocation_mode(),
            call_result_key: self.request.call_result_key(),
            owner_transferred: false,
            released: false,
        };
        self.transferred = true;
        Ok(Box::new(launch))
    }
}

struct LinuxLaunch {
    process: Option<Box<dyn SandboxProcess>>,
    projection: Option<projection::Projection>,
    status_channel: Option<broker::StatusChannel>,
    inspection: SandboxInspection,
    audit: crucible_core::SandboxAudit,
    sandbox: crucible_core::SandboxId,
    invocation: SandboxInvocationMode,
    call_result_key: Option<crucible_core::CallResultKey>,
    owner_transferred: bool,
    released: bool,
}

impl SandboxLaunch for LinuxLaunch {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn transfer_owner(&mut self) -> Result<(), SandboxError> {
        if self.invocation != SandboxInvocationMode::Background || self.owner_transferred {
            return Err(SandboxError::Lifecycle(std::io::Error::other(
                "sandbox background ownership transfer is invalid",
            )));
        }
        self.audit.record(
            self.sandbox,
            SandboxFactKind::Lifecycle(SandboxLifecycle::OwnerTransferred),
        )?;
        self.owner_transferred = true;
        Ok(())
    }

    fn release(mut self: Box<Self>) -> Result<Box<dyn SandboxProcess>, SandboxError> {
        let mut process = self.process.take().ok_or_else(|| {
            SandboxError::Lifecycle(std::io::Error::other(
                "sandbox process scope is unavailable before release",
            ))
        })?;
        let mut status_channel = self.status_channel.take().ok_or_else(|| {
            SandboxError::Lifecycle(std::io::Error::other(
                "sandbox release channel is unavailable",
            ))
        })?;
        if let Some(projection) = self.projection.as_mut()
            && let Err(source) = projection.record(transaction::Record::ReleaseIntent)
        {
            let stopped = process.stop().is_ok();
            self.record_refusal(stopped);
            self.released = true;
            return Err(SandboxError::Lifecycle(source));
        }
        if let Err(source) = self.audit.record(
            self.sandbox,
            SandboxFactKind::Lifecycle(SandboxLifecycle::ReleaseIntent),
        ) {
            let stopped = process.stop().is_ok();
            self.record_refusal(stopped);
            self.released = true;
            return Err(SandboxError::Audit(source));
        }
        if self.invocation == SandboxInvocationMode::Background && !self.owner_transferred {
            let stopped = process.stop().is_ok();
            self.record_refusal(stopped);
            self.released = true;
            return Err(SandboxError::Lifecycle(std::io::Error::other(
                "background sandbox has no application cleanup owner",
            )));
        }
        if self.invocation == SandboxInvocationMode::Background
            && let Some(projection) = self.projection.as_mut()
            && let Err(source) = projection.record(transaction::Record::OwnerTransferred)
        {
            let stopped = process.stop().is_ok();
            self.record_refusal(stopped);
            self.released = true;
            return Err(SandboxError::Lifecycle(source));
        }
        if let Err(source) = status_channel.attest_ready() {
            let stopped = process.stop().is_ok();
            self.record_refusal(stopped);
            let problem = SandboxError::Lifecycle(source);
            let _ = self.audit.record(
                self.sandbox,
                SandboxFactKind::Failed {
                    phase: SandboxFailurePhase::Start,
                    kind: problem.failure_kind(),
                },
            );
            self.released = true;
            return Err(problem);
        }
        if let Some(projection) = self.projection.as_mut()
            && let Err(source) = projection.record(transaction::Record::GoSentOrAmbiguous)
        {
            let stopped = process.stop().is_ok();
            self.record_refusal(stopped);
            self.released = true;
            return Err(SandboxError::Lifecycle(source));
        }
        if let Err(source) = status_channel.send_go() {
            let stopped = process.stop().is_ok();
            let rolled_back = self
                .projection
                .as_mut()
                .is_none_or(|projection| projection.abort(stopped).is_ok());
            let lifecycle = if stopped && rolled_back {
                SandboxLifecycle::RolledBack
            } else {
                if let Some(projection) = self.projection.as_mut() {
                    projection.retain_evidence();
                }
                SandboxLifecycle::Quarantined
            };
            let _ = self
                .audit
                .record(self.sandbox, SandboxFactKind::Lifecycle(lifecycle));
            self.released = true;
            return Err(SandboxError::Lifecycle(source));
        }
        self.released = true;
        for lifecycle in [
            SandboxLifecycle::CommandReleased,
            SandboxLifecycle::CommandStarted,
        ] {
            if let Err(source) = self
                .audit
                .record(self.sandbox, SandboxFactKind::Lifecycle(lifecycle))
            {
                let stopped = process.stop().is_ok();
                if let Some(projection) = self.projection.as_mut()
                    && (projection.abort(stopped).is_err() || !stopped)
                {
                    projection.retain_evidence();
                }
                return Err(SandboxError::Audit(source));
            }
        }
        let wrapped = projection::wrap(
            process,
            projection::ProcessPlan {
                projection: self.projection.take(),
                status_channel,
                audit: self.audit.clone(),
                sandbox: self.sandbox,
                invocation: self.invocation,
                call_result_key: self.call_result_key,
            },
        );
        wrapped.map_err(SandboxError::Lifecycle)
    }
}

impl Drop for LinuxLaunch {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let stopped = self
            .process
            .as_mut()
            .is_none_or(|process| process.stop().is_ok());
        self.record_refusal(stopped);
    }
}

impl LinuxLaunch {
    fn record_refusal(&mut self, stopped: bool) {
        let journaled = self
            .projection
            .as_mut()
            .is_none_or(|projection| projection.refuse(stopped).is_ok());
        let lifecycle = if stopped && journaled {
            SandboxLifecycle::Refused
        } else {
            if let Some(projection) = self.projection.as_mut() {
                projection.retain_evidence();
            }
            SandboxLifecycle::Quarantined
        };
        let _ = self
            .audit
            .record(self.sandbox, SandboxFactKind::Lifecycle(lifecycle));
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
