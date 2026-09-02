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

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crucible_core::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxCleanup, SandboxCommand,
    SandboxCommandStage, SandboxError, SandboxFactKind, SandboxFailureKind, SandboxFailurePhase,
    SandboxFilesystemAccess, SandboxGuardrailDecision, SandboxInspection, SandboxInvocationMode,
    SandboxLaunch, SandboxLifecycle, SandboxProcess, SandboxRead, SandboxRequest, SandboxSession,
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
    let registry = transaction::RegistryLease::acquire(&request)?;
    transaction::RegistryLease::reconcile(&registry).map_err(|source| {
        SandboxError::BackendUnavailable {
            reason: format!(
                "stale sandbox lifecycle requires recovery or quarantine review: {source}"
            )
            .into(),
        }
    })?;
    drop(registry);
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
            projection: Some(&projection),
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
                audit_cleanup: false,
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
            projection: Some(projection),
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
        if self.invocation == SandboxInvocationMode::Foreground || self.owner_transferred {
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
        if self.process.is_none() {
            return Err(SandboxError::Lifecycle(std::io::Error::other(
                "sandbox process scope is unavailable before release",
            )));
        }
        if self.status_channel.is_none() {
            return Err(SandboxError::Lifecycle(std::io::Error::other(
                "sandbox release channel is unavailable",
            )));
        }
        let Some(mut process) = self.process.take() else {
            return Err(SandboxError::Lifecycle(std::io::Error::other(
                "sandbox process scope is unavailable before release",
            )));
        };
        if let Some(projection) = self.projection.as_mut()
            && let Err(source) = projection.record(transaction::Record::ReleaseIntent)
        {
            self.refuse_and_cleanup(process.as_mut());
            return Err(SandboxError::Lifecycle(source));
        }
        if let Err(source) = self.audit.record(
            self.sandbox,
            SandboxFactKind::Lifecycle(SandboxLifecycle::ReleaseIntent),
        ) {
            self.refuse_and_cleanup(process.as_mut());
            return Err(SandboxError::Audit(source));
        }
        if self.invocation != SandboxInvocationMode::Foreground && !self.owner_transferred {
            self.refuse_and_cleanup(process.as_mut());
            return Err(SandboxError::Lifecycle(std::io::Error::other(
                "background sandbox has no application cleanup owner",
            )));
        }
        if self.invocation != SandboxInvocationMode::Foreground
            && let Some(projection) = self.projection.as_mut()
            && let Err(source) = projection.record(transaction::Record::OwnerTransferred)
        {
            self.refuse_and_cleanup(process.as_mut());
            return Err(SandboxError::Lifecycle(source));
        }
        let ready = self.status_channel.as_mut().map_or_else(
            || Err(io::Error::other("sandbox release channel is unavailable")),
            broker::StatusChannel::attest_ready,
        );
        if let Err(source) = ready {
            self.refuse_and_cleanup(process.as_mut());
            let said = drain_launcher_stderr(process.as_mut());
            let problem = SandboxError::Lifecycle(explain_launch_failure(source, &said));
            let _ = self.audit.record(
                self.sandbox,
                SandboxFactKind::Failed {
                    phase: SandboxFailurePhase::Start,
                    kind: problem.failure_kind(),
                },
            );
            return Err(problem);
        }
        if let Some(projection) = self.projection.as_mut()
            && let Err(source) = projection.record(transaction::Record::GoSentOrAmbiguous)
        {
            self.refuse_and_cleanup(process.as_mut());
            return Err(SandboxError::Lifecycle(source));
        }
        let released = self.status_channel.as_mut().map_or_else(
            || Err(io::Error::other("sandbox release channel is unavailable")),
            broker::StatusChannel::send_go,
        );
        if let Err(source) = released {
            self.rollback_and_cleanup(process.as_mut());
            return Err(SandboxError::Lifecycle(source));
        }
        for lifecycle in [
            SandboxLifecycle::CommandReleased,
            SandboxLifecycle::CommandStarted,
        ] {
            if let Err(source) = self
                .audit
                .record(self.sandbox, SandboxFactKind::Lifecycle(lifecycle))
            {
                self.rollback_and_cleanup(process.as_mut());
                return Err(SandboxError::Audit(source));
            }
        }
        let Some(status_channel) = self.status_channel.take() else {
            self.rollback_and_cleanup(process.as_mut());
            return Err(SandboxError::Lifecycle(io::Error::other(
                "sandbox release channel is unavailable",
            )));
        };
        self.released = true;
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
        if let Some(mut process) = self.process.take() {
            self.refuse_and_cleanup(process.as_mut());
        } else {
            self.status_channel.take();
            self.finish_cleanup(false);
        }
    }
}

impl LinuxLaunch {
    fn record_refusal(&mut self, stopped: bool) -> io::Result<()> {
        let journaled = self
            .projection
            .as_mut()
            .map_or(Ok(()), |projection| projection.refuse(stopped));
        let lifecycle = if stopped && journaled.is_ok() {
            SandboxLifecycle::Refused
        } else {
            if let Some(projection) = self.projection.as_mut() {
                projection.retain_evidence();
            }
            SandboxLifecycle::Quarantined
        };
        let audited = self
            .audit
            .record(self.sandbox, SandboxFactKind::Lifecycle(lifecycle))
            .map_err(io::Error::other);
        journaled.and(audited)
    }

    fn refuse_and_cleanup(&mut self, process: &mut dyn SandboxProcess) {
        let _ = process.stop();
        let scope_reaped = process.inspection().cleanup() == SandboxCleanup::Complete;
        let _ = self.record_refusal(scope_reaped);
        self.finish_cleanup(scope_reaped);
    }

    fn rollback_and_cleanup(&mut self, process: &mut dyn SandboxProcess) {
        let _ = process.stop();
        let scope_reaped = process.inspection().cleanup() == SandboxCleanup::Complete;
        let rolled_back = self
            .projection
            .as_mut()
            .map_or(Ok(()), |projection| projection.abort(scope_reaped));
        let lifecycle = if scope_reaped && rolled_back.is_ok() {
            SandboxLifecycle::RolledBack
        } else {
            if let Some(projection) = self.projection.as_mut() {
                projection.retain_evidence();
            }
            SandboxLifecycle::Quarantined
        };
        let _ = self
            .audit
            .record(self.sandbox, SandboxFactKind::Lifecycle(lifecycle))
            .map_err(io::Error::other);
        self.finish_cleanup(scope_reaped);
    }

    fn finish_cleanup(&mut self, scope_reaped: bool) {
        self.status_channel.take();
        let projection_cleanup = self
            .projection
            .as_mut()
            .map_or(Ok(()), projection::Projection::cleanup);
        let projection_cleaned = projection_cleanup.is_ok();
        if projection_cleaned {
            self.projection.take();
        }
        let cleanup = if scope_reaped && projection_cleaned {
            SandboxCleanup::Complete
        } else {
            SandboxCleanup::Failed
        };
        let _ = self
            .audit
            .record(self.sandbox, SandboxFactKind::Cleanup(cleanup));
        self.released = true;
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
            let cleanup = self
                .materialization
                .as_mut()
                .map_or(Ok(()), materialize::Materialization::cleanup);
            self.transaction.take();
            self.reservation.take();
            if cleanup.is_ok() {
                self.materialization.take();
            }
            let cleanup = if cleanup.is_ok() {
                SandboxCleanup::Complete
            } else {
                SandboxCleanup::Failed
            };
            let _ = self
                .request
                .audit()
                .record(self.request.id(), SandboxFactKind::Cleanup(cleanup));
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

/// How much of the launcher's stderr a refused launch may quote.
const MAX_LAUNCHER_DIAGNOSTIC_BYTES: usize = 512;

/// Bubblewrap and the broker explain a refused launch only on stderr, which
/// would otherwise die unread with the process. A status channel that closes
/// before READY is reported with that explanation, bounded and on one line, so
/// a system Bubblewrap that rejects an option names the option.
fn explain_launch_failure(source: io::Error, launcher_said: &[u8]) -> io::Error {
    let truncated = launcher_said.len() > MAX_LAUNCHER_DIAGNOSTIC_BYTES;
    let quoted = launcher_said
        .get(..MAX_LAUNCHER_DIAGNOSTIC_BYTES)
        .unwrap_or(launcher_said);
    let one_line = String::from_utf8_lossy(quoted)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if one_line.is_empty() {
        return source;
    }
    let suffix = if truncated { " (truncated)" } else { "" };
    io::Error::new(
        source.kind(),
        format!("{source}; the sandbox launcher said: {one_line}{suffix}"),
    )
}

/// What a stopped launcher left on stderr, up to the quotable bound.
fn drain_launcher_stderr(process: &mut dyn SandboxProcess) -> Vec<u8> {
    let Some(mut stderr) = process.take_stderr() else {
        return Vec::new();
    };
    let mut said = Vec::new();
    let mut buffer = [0_u8; MAX_LAUNCHER_DIAGNOSTIC_BYTES];
    while said.len() <= MAX_LAUNCHER_DIAGNOSTIC_BYTES {
        let count = match stderr.read_ready(&mut buffer) {
            Ok(
                SandboxRead::Bytes(count)
                | SandboxRead::Limited {
                    retained: count, ..
                },
            ) if count > 0 => count,
            _ => break,
        };
        said.extend_from_slice(buffer.get(..count).unwrap_or(&buffer));
    }
    said
}
