//! Native macOS confinement through the system Seatbelt launcher.

#[cfg(target_os = "macos")]
use std::fs;
#[cfg(target_os = "macos")]
use std::os::unix::fs::PermissionsExt as _;
#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::atomic::AtomicUsize;

#[cfg(target_os = "macos")]
use crucible_core::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxCleanup, SandboxCommand,
    SandboxCommandStage, SandboxError, SandboxFactKind, SandboxFailureKind, SandboxFailurePhase,
    SandboxFilesystemAccess, SandboxGuardrailDecision, SandboxInspection, SandboxInvocationMode,
    SandboxLaunch, SandboxLifecycle, SandboxProcess, SandboxRequest, SandboxSession,
};

#[cfg(target_os = "macos")]
use super::process::{
    MAX_LOCAL_COMMANDS, Reservation, Stage, cleanup_prepared_owners, cleanup_unspawned,
};

#[cfg(target_os = "macos")]
mod broker;
#[cfg(target_os = "macos")]
mod probe;
mod profile;
mod tree;
mod unreadable;

#[cfg(target_os = "macos")]
pub(super) fn probe() -> Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError> {
    let broker = broker::Broker::find(&[])?;
    let backend = probe::Seatbelt::find(&broker)?;
    Ok((backend.identity().clone(), backend.capabilities().clone()))
}

#[cfg(all(target_os = "macos", test))]
pub(super) fn declared_capabilities() -> SandboxCapabilities {
    probe::capabilities()
}

#[cfg(target_os = "macos")]
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
    let broker = broker::Broker::find(&excluded)?;
    let backend = probe::Seatbelt::find(&broker)?;
    request.negotiate(backend.capabilities())?;
    validate_roots(&request)?;

    let protected = tree::validate(request.policy())?;
    let unreadable = unreadable::expand(request.policy().unreadable_patterns())?;
    let scratch = create_scratch(&request)?;
    let profile = profile::Profile::build(
        request.policy(),
        protected.protected(),
        protected.linked_metadata(),
        &unreadable,
    )?
    .with_scratch(scratch.root())?;
    let inspection = SandboxInspection::confined_for_request(
        backend.identity().clone(),
        backend.capabilities().clone(),
        &request,
    )?;
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
    let reservation = Reservation::take(active, maximum)?;
    request.audit().record(
        request.id(),
        SandboxFactKind::Lifecycle(SandboxLifecycle::Prepared),
    )?;
    Ok(Box::new(MacSession {
        request,
        broker,
        inspection,
        profile,
        scratch: Some(scratch),
        reservation: Some(reservation),
        materialized: false,
        transferred: false,
    }))
}

#[cfg(target_os = "macos")]
fn validate_roots(request: &SandboxRequest) -> Result<(), SandboxError> {
    for rule in request.policy().filesystem() {
        let metadata = fs::symlink_metadata(rule.path()).map_err(|source| {
            materialization("sandbox policy path could not be inspected", Some(source))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(materialization(
                "sandbox policy root or protected path is a symbolic link",
                None,
            ));
        }
        let canonical = rule.path().canonicalize().map_err(|source| {
            materialization(
                "sandbox policy path could not be canonicalized",
                Some(source),
            )
        })?;
        if canonical != rule.path() {
            return Err(materialization(
                "sandbox policy path changed after resolution",
                None,
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn create_scratch(request: &SandboxRequest) -> Result<Stage, SandboxError> {
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
    fs::create_dir(&root).map_err(|source| {
        materialization(
            "the private sandbox temporary directory could not be created",
            Some(source),
        )
    })?;
    if let Err(source) = fs::set_permissions(&root, fs::Permissions::from_mode(0o700)) {
        let _ = fs::remove_dir(&root);
        return Err(materialization(
            "the private sandbox temporary directory could not be protected",
            Some(source),
        ));
    }
    Ok(Stage::new(root))
}

#[cfg(target_os = "macos")]
fn materialization(problem: &'static str, source: Option<std::io::Error>) -> SandboxError {
    SandboxError::Materialization {
        problem: problem.into(),
        source,
    }
}

#[cfg(target_os = "macos")]
struct MacSession {
    request: SandboxRequest,
    broker: broker::Broker,
    inspection: SandboxInspection,
    profile: profile::Profile,
    scratch: Option<Stage>,
    reservation: Option<Reservation>,
    materialized: bool,
    transferred: bool,
}

#[cfg(target_os = "macos")]
impl SandboxSession for MacSession {
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

        let limits = self.request.policy().limits();
        let mut process = Command::new(self.broker.path());
        process
            .arg(crucible_sandbox_broker::MACOS_LAUNCH_MODE)
            .arg("--cpu-seconds")
            .arg(limits.cpu_seconds.unwrap_or(0).to_string())
            .arg("--open-files")
            .arg(limits.open_files.unwrap_or(0).to_string())
            .arg("--profile")
            .arg(self.profile.policy());
        for definition in self.profile.definitions() {
            process.arg("--definition").arg(definition);
        }
        let scratch = self.scratch.as_ref().map(Stage::root).ok_or_else(|| {
            SandboxError::Lifecycle(std::io::Error::other(
                "sandbox scratch owner is unavailable",
            ))
        })?;
        process
            .arg("--")
            .arg(command.program())
            .args(command.arguments())
            .current_dir(self.request.policy().working_directory())
            .env_clear()
            .envs(command.environment().iter())
            .env("TMPDIR", scratch);
        let reservation = self.reservation.take().ok_or(SandboxError::Concurrency)?;
        let stage = self.scratch.take().ok_or_else(|| {
            SandboxError::Lifecycle(std::io::Error::other(
                "sandbox scratch owner is unavailable",
            ))
        })?;
        let launch = MacLaunch {
            process: Some(process),
            plan: Some(super::process::SpawnPlan {
                inspection: self.inspection.clone(),
                reservation,
                stage: Some(stage),
                limits,
                audit: self.request.audit().clone(),
                sandbox: self.request.id(),
                audit_started: true,
                audit_cleanup: true,
                invocation: self.request.invocation_mode(),
                call_result_key: self.request.call_result_key(),
                canceller: None,
                speech: command.speech(),
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

#[cfg(target_os = "macos")]
impl MacSession {
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

#[cfg(target_os = "macos")]
impl Drop for MacSession {
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

#[cfg(target_os = "macos")]
struct MacLaunch {
    process: Option<Command>,
    plan: Option<super::process::SpawnPlan>,
    inspection: SandboxInspection,
    audit: crucible_core::SandboxAudit,
    sandbox: crucible_core::SandboxId,
    invocation: SandboxInvocationMode,
    owner_transferred: bool,
    released: bool,
}

#[cfg(target_os = "macos")]
impl SandboxLaunch for MacLaunch {
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
            SandboxError::Spawn(std::io::Error::other("macOS command was already released"))
        })?;
        let plan = self.plan.take().ok_or_else(|| {
            SandboxError::Spawn(std::io::Error::other("macOS launch plan is unavailable"))
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

#[cfg(target_os = "macos")]
impl Drop for MacLaunch {
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

#[cfg(target_os = "macos")]
impl std::fmt::Debug for MacSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacSession")
            .field("inspection", &self.inspection)
            .field("materialized", &self.materialized)
            .finish_non_exhaustive()
    }
}
