//! Native Windows confinement through the packaged account/token broker.

mod broker;
use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crucible_core::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxCapability, SandboxCleanup, SandboxCommand,
    SandboxCommandStage, SandboxError, SandboxFactKind, SandboxFailureKind, SandboxFailurePhase,
    SandboxFeature, SandboxFilesystemAccess, SandboxGuardrailDecision, SandboxInspection,
    SandboxInvocationMode, SandboxLaunch, SandboxLifecycle, SandboxNetworkPolicy, SandboxProcess,
    SandboxRequest, SandboxSession,
};

use super::process::{
    MAX_LOCAL_COMMANDS, Reservation, Stage, cleanup_prepared_owners, cleanup_unspawned,
};

pub(super) fn probe() -> Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError> {
    let broker = broker::Broker::find(&[])?;
    crucible_sandbox_broker::probe_windows_sandbox().map_err(|_| unavailable(
        "native Windows sandbox setup is missing, incomplete, or no longer matches machine policy",
    ))?;
    Ok((broker.identity().clone(), declared_capabilities()))
}

pub(super) fn declared_capabilities() -> SandboxCapabilities {
    let enforced = SandboxCapability::Enforced;
    SandboxCapabilities::none()
        .with(SandboxFeature::Filesystem, enforced)
        .with(SandboxFeature::NetworkDeny, enforced)
        .with(SandboxFeature::DescriptorIsolation, enforced)
        .with(SandboxFeature::ProcessIsolation, enforced)
        .with(SandboxFeature::KernelSurface, enforced)
        .with(SandboxFeature::PrivilegeIsolation, enforced)
        .with(SandboxFeature::CpuLimit, enforced)
        .with(SandboxFeature::CommandTimeLimit, enforced)
        .with(SandboxFeature::OutputLimit, enforced)
        .with(SandboxFeature::ConcurrencyLimit, enforced)
        .with(SandboxFeature::Audit, enforced)
        .with(SandboxFeature::Usage, SandboxCapability::Observed)
}

pub(super) fn prepare(
    request: SandboxRequest,
    active: Arc<AtomicUsize>,
) -> Result<Box<dyn SandboxSession>, SandboxError> {
    if !matches!(request.policy().network(), SandboxNetworkPolicy::Closed) {
        return Err(SandboxError::Unsupported {
            feature: SandboxFeature::NetworkAllowlist,
        });
    }
    if !request.policy().unreadable_patterns().is_empty()
        || request
            .policy()
            .filesystem()
            .iter()
            .any(|rule| rule.access() == SandboxFilesystemAccess::Unreadable)
    {
        return Err(SandboxError::Unsupported {
            feature: SandboxFeature::Filesystem,
        });
    }
    let excluded: Vec<_> = request
        .policy()
        .filesystem()
        .iter()
        .filter(|rule| rule.access() == SandboxFilesystemAccess::ReadWrite)
        .map(crucible_core::SandboxFilesystemRule::path)
        .collect();
    let broker = broker::Broker::find(&excluded)?;
    crucible_sandbox_broker::probe_windows_sandbox().map_err(|_| unavailable(
        "native Windows sandbox setup is missing, incomplete, or no longer matches machine policy",
    ))?;
    let capabilities = declared_capabilities();
    request.negotiate(&capabilities)?;
    let inspection =
        SandboxInspection::confined_for_request(broker.identity().clone(), capabilities, &request)?;
    request.audit().record(
        request.id(),
        SandboxFactKind::Negotiated(Box::new(inspection.clone())),
    )?;
    let maximum = request
        .policy()
        .limits()
        .concurrent_commands
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(MAX_LOCAL_COMMANDS);
    let scratch_root = scratch_path(&request)?;
    let mut reservation = Some(Reservation::take(active, maximum)?);
    let mut scratch = None;
    let setup = (|| {
        create_scratch(&scratch_root, &mut scratch)?;
        request.audit().record(
            request.id(),
            SandboxFactKind::Lifecycle(SandboxLifecycle::Prepared),
        )?;
        Ok(())
    })();
    if let Err(problem) = setup {
        let cleanup = cleanup_prepared_owners(&mut scratch, &mut reservation);
        return Err(preparation_failure(problem, cleanup));
    }
    let scratch = scratch.ok_or_else(|| {
        SandboxError::Lifecycle(std::io::Error::other(
            "prepared Windows scratch owner is unavailable",
        ))
    })?;
    let reservation = reservation.ok_or_else(|| {
        SandboxError::Lifecycle(std::io::Error::other(
            "prepared Windows admission owner is unavailable",
        ))
    })?;
    Ok(Box::new(WindowsSession {
        request,
        broker,
        inspection,
        scratch: Some(scratch),
        reservation: Some(reservation),
        materialized: false,
        transferred: false,
    }))
}

struct WindowsSession {
    request: SandboxRequest,
    broker: broker::Broker,
    inspection: SandboxInspection,
    scratch: Option<Stage>,
    reservation: Option<Reservation>,
    materialized: bool,
    transferred: bool,
}

impl SandboxSession for WindowsSession {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn materialize(&mut self) -> Result<(), SandboxError> {
        if !self.materialized {
            self.materialized = true;
            self.request.audit().record(
                self.request.id(),
                SandboxFactKind::Lifecycle(SandboxLifecycle::Materialized),
            )?;
        }
        Ok(())
    }

    fn stage(
        mut self: Box<Self>,
        command: SandboxCommand,
    ) -> Result<Box<dyn SandboxLaunch>, SandboxError> {
        if !self.materialized {
            self.record_start_failure(SandboxFailureKind::Materialization)?;
            return Err(materialization(
                "session was not materialized before start",
                None,
            ));
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
                self.record_start_failure(SandboxFailureKind::Guardrail)?;
                return Err(SandboxError::Guardrail);
            }
        }
        let scratch = self.scratch.as_ref().map(Stage::root).ok_or_else(|| {
            SandboxError::Lifecycle(std::io::Error::other(
                "Windows sandbox scratch owner is unavailable",
            ))
        })?;
        let startup_input = match launch_frame(&self.request, &self.broker, &command, scratch) {
            Ok(frame) => frame,
            Err(problem) => {
                self.record_start_failure(problem.failure_kind())?;
                return Err(problem);
            }
        };
        let mut process = Command::new(self.broker.path());
        process
            .arg(crucible_sandbox_broker::WINDOWS_LAUNCH_MODE)
            .current_dir(self.request.policy().working_directory());
        let reservation = self.reservation.take().ok_or(SandboxError::Concurrency)?;
        let stage = self.scratch.take().ok_or_else(|| {
            SandboxError::Lifecycle(std::io::Error::other(
                "Windows sandbox scratch owner is unavailable",
            ))
        })?;
        let launch = WindowsLaunch {
            process: Some(process),
            plan: Some(super::process::SpawnPlan {
                inspection: self.inspection.clone(),
                reservation,
                stage: Some(stage),
                limits: self.request.policy().limits(),
                audit: self.request.audit().clone(),
                sandbox: self.request.id(),
                audit_started: true,
                audit_cleanup: true,
                invocation: self.request.invocation_mode(),
                call_result_key: self.request.call_result_key(),
                canceller: None,
                speech: command.speech(),
                startup_input: Some(startup_input),
            }),
            inspection: self.inspection.clone(),
            audit: self.request.audit().clone(),
            sandbox: self.request.id(),
            invocation: self.request.invocation_mode(),
            owner_transferred: false,
            released: false,
        };
        self.transferred = true;
        Ok(Box::new(launch))
    }
}

impl WindowsSession {
    fn record_start_failure(&self, kind: SandboxFailureKind) -> Result<(), SandboxError> {
        self.request.audit().record(
            self.request.id(),
            SandboxFactKind::Failed {
                phase: SandboxFailurePhase::Start,
                kind,
            },
        )?;
        Ok(())
    }
}

impl Drop for WindowsSession {
    fn drop(&mut self) {
        if !self.transferred {
            let cleanup = cleanup_prepared_owners(&mut self.scratch, &mut self.reservation);
            let _ = self
                .request
                .audit()
                .record(self.request.id(), SandboxFactKind::Cleanup(cleanup));
        }
    }
}

struct WindowsLaunch {
    process: Option<Command>,
    plan: Option<super::process::SpawnPlan>,
    inspection: SandboxInspection,
    audit: crucible_core::SandboxAudit,
    sandbox: crucible_core::SandboxId,
    invocation: SandboxInvocationMode,
    owner_transferred: bool,
    released: bool,
}

impl SandboxLaunch for WindowsLaunch {
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
        if self.invocation != SandboxInvocationMode::Foreground && !self.owner_transferred {
            return Err(SandboxError::Lifecycle(std::io::Error::other(
                "background sandbox has no application cleanup owner",
            )));
        }
        let process = self.process.take().ok_or_else(|| {
            SandboxError::Spawn(std::io::Error::other(
                "Windows sandbox command was already released",
            ))
        })?;
        let plan = self.plan.take().ok_or_else(|| {
            SandboxError::Spawn(std::io::Error::other(
                "Windows sandbox launch plan is unavailable",
            ))
        })?;
        self.released = true;
        let spawned = super::process::spawn(process, plan);
        if let Err(problem) = &spawned {
            let cleanup = if matches!(problem, SandboxError::Lifecycle(_)) {
                SandboxCleanup::Failed
            } else {
                SandboxCleanup::Complete
            };
            let _ = self.audit.record(
                self.sandbox,
                SandboxFactKind::Failed {
                    phase: SandboxFailurePhase::Start,
                    kind: problem.failure_kind(),
                },
            );
            let _ = self
                .audit
                .record(self.sandbox, SandboxFactKind::Cleanup(cleanup));
        }
        spawned
    }
}

impl Drop for WindowsLaunch {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.audit.record(
                self.sandbox,
                SandboxFactKind::Lifecycle(SandboxLifecycle::Refused),
            );
            let cleanup = self
                .plan
                .take()
                .map_or(SandboxCleanup::Complete, cleanup_unspawned);
            let _ = self
                .audit
                .record(self.sandbox, SandboxFactKind::Cleanup(cleanup));
        }
    }
}

fn launch_frame(
    request: &SandboxRequest,
    broker: &broker::Broker,
    command: &SandboxCommand,
    scratch: &Path,
) -> Result<Vec<u8>, SandboxError> {
    let mut readable = Vec::new();
    let mut writable = Vec::new();
    let mut protected = Vec::new();
    for rule in request.policy().filesystem() {
        match rule.access() {
            SandboxFilesystemAccess::ReadWrite => writable.push(rule.path().to_path_buf()),
            SandboxFilesystemAccess::ReadOnly | SandboxFilesystemAccess::Protected => {
                readable.push(rule.path().to_path_buf());
                protected.push(rule.path().to_path_buf());
            }
            SandboxFilesystemAccess::Unreadable => {
                return Err(SandboxError::Unsupported {
                    feature: SandboxFeature::Filesystem,
                });
            }
        }
    }
    readable.push(broker.path().to_path_buf());
    readable.push(command.program().to_path_buf());
    writable.push(scratch.to_path_buf());
    protected.push(broker.path().to_path_buf());
    protected.push(command.program().to_path_buf());
    normalize_roots(&mut readable);
    normalize_roots(&mut writable);
    normalize_roots(&mut protected);

    let mut environment: Vec<_> = command
        .environment()
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("TEMP") && !name.eq_ignore_ascii_case("TMP"))
        .map(|(name, value)| (wide(OsStr::new(name)), wide(value)))
        .collect();
    environment.push((wide(OsStr::new("TEMP")), wide(scratch.as_os_str())));
    environment.push((wide(OsStr::new("TMP")), wide(scratch.as_os_str())));

    let request = crucible_sandbox_broker::WindowsLaunchRequest::new(
        wide(request.policy().working_directory().as_os_str()),
        wide(command.program().as_os_str()),
        command
            .arguments()
            .iter()
            .map(|argument| wide(argument))
            .collect(),
        environment,
        wide_roots(&readable),
        wide_roots(&writable),
        wide_roots(&protected),
    )
    .map_err(|source| materialization("Windows sandbox launch plan is invalid", Some(source)))?;
    let mut frame = Vec::new();
    crucible_sandbox_broker::encode_windows_launch(&request, &mut frame).map_err(|source| {
        materialization(
            "Windows sandbox launch plan could not be encoded",
            Some(source),
        )
    })?;
    Ok(frame)
}

fn scratch_path(request: &SandboxRequest) -> Result<PathBuf, SandboxError> {
    let base = std::env::temp_dir().canonicalize().map_err(|source| {
        materialization(
            "the system temporary directory is unavailable",
            Some(source),
        )
    })?;
    let root = base.join(format!("crucible-sandbox-{}", request.id()));
    if request
        .policy()
        .filesystem()
        .iter()
        .any(|rule| root.starts_with(rule.path()))
    {
        return Err(materialization(
            "the private sandbox temporary directory overlaps a declared root",
            None,
        ));
    }
    Ok(root)
}

fn create_scratch(root: &Path, owner: &mut Option<Stage>) -> Result<(), SandboxError> {
    fs::create_dir(root).map_err(|source| {
        materialization(
            "the private sandbox temporary directory could not be created",
            Some(source),
        )
    })?;
    *owner = Some(Stage::new(root.to_path_buf()));
    crucible_privacy::directory(root).map_err(|source| {
        materialization(
            "the private sandbox temporary directory could not be protected",
            Some(source.into_io()),
        )
    })
}

fn preparation_failure(problem: SandboxError, cleanup: SandboxCleanup) -> SandboxError {
    if cleanup == SandboxCleanup::Complete {
        problem
    } else {
        SandboxError::Lifecycle(std::io::Error::other(
            "Windows sandbox preparation failed and cleanup could not be confirmed",
        ))
    }
}

fn normalize_roots(roots: &mut Vec<PathBuf>) {
    // Preserve Windows' complete native path representation. The privileged
    // broker canonicalizes these roots and rejects filesystem aliases before
    // it changes any ACL.
    roots.sort();
    roots.dedup();
}

fn wide_roots(roots: &[PathBuf]) -> Vec<Vec<u16>> {
    roots.iter().map(|root| wide(root.as_os_str())).collect()
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().collect()
}

fn unavailable(reason: &'static str) -> SandboxError {
    SandboxError::BackendUnavailable {
        reason: reason.into(),
    }
}

fn materialization(problem: &'static str, source: Option<std::io::Error>) -> SandboxError {
    SandboxError::Materialization {
        problem: problem.into(),
        source,
    }
}
