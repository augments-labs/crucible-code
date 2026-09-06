//! Native macOS confinement through the system Seatbelt launcher.

#[cfg(target_os = "macos")]
use std::ffi::OsStr;
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
    SandboxLaunch, SandboxLifecycle, SandboxNetworkPolicy, SandboxProcess, SandboxRequest,
    SandboxResourceLimits, SandboxSession,
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
    let scratch_root = scratch_path(&request)?;
    let profile = profile::Profile::build(
        request.policy(),
        protected.protected(),
        protected.linked_metadata(),
        &unreadable,
    )?
    .with_scratch(&scratch_root)?;
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
            "prepared macOS scratch owner is unavailable",
        ))
    })?;
    let reservation = reservation.ok_or_else(|| {
        SandboxError::Lifecycle(std::io::Error::other(
            "prepared macOS admission owner is unavailable",
        ))
    })?;
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
fn scratch_path(request: &SandboxRequest) -> Result<std::path::PathBuf, SandboxError> {
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

#[cfg(target_os = "macos")]
fn create_scratch(root: &std::path::Path, owner: &mut Option<Stage>) -> Result<(), SandboxError> {
    fs::create_dir(root).map_err(|source| {
        materialization(
            "the private sandbox temporary directory could not be created",
            Some(source),
        )
    })?;
    *owner = Some(Stage::new(root.to_path_buf()));
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).map_err(|source| {
        materialization(
            "the private sandbox temporary directory could not be protected",
            Some(source),
        )
    })
}

#[cfg(target_os = "macos")]
fn preparation_failure(problem: SandboxError, cleanup: SandboxCleanup) -> SandboxError {
    if cleanup == SandboxCleanup::Complete {
        problem
    } else {
        SandboxError::Lifecycle(std::io::Error::other(
            "macOS sandbox preparation failed and cleanup could not be confirmed",
        ))
    }
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

        if let Err(problem) = self.profile.validate_network() {
            self.record_start_failure(problem.failure_kind())?;
            return Err(problem);
        }
        let duration = self
            .request
            .policy()
            .limits()
            .command_time
            .unwrap_or(std::time::Duration::from_mins(20));
        let mut mediator = match self.request.policy().network() {
            SandboxNetworkPolicy::Domains(policy) if !policy.allowed().is_empty() => {
                match super::network::Mediator::tcp(policy.clone(), self.request.id(), duration) {
                    Ok(mediator) => Some(mediator),
                    Err(source) => {
                        let problem = SandboxError::Spawn(source);
                        self.record_start_failure(problem.failure_kind())?;
                        return Err(problem);
                    }
                }
            }
            SandboxNetworkPolicy::Domains(_) | SandboxNetworkPolicy::Closed => None,
        };
        let profile = match mediator.as_ref() {
            Some(network) => self.profile.with_proxy(network.address()),
            None => Ok(self.profile.clone()),
        };
        let profile = match profile {
            Ok(profile) => profile,
            Err(problem) => {
                self.record_start_failure(problem.failure_kind())?;
                return Err(problem);
            }
        };
        let limits = self.request.policy().limits();
        if let Err(problem) = validate_launch_arguments(&self.broker, &profile, &command, limits) {
            self.record_start_failure(problem.failure_kind())?;
            return Err(problem);
        }
        let mut process = Command::new(self.broker.path());
        process
            .arg(crucible_sandbox_broker::MACOS_LAUNCH_MODE)
            .arg("--cpu-seconds")
            .arg(limits.cpu_seconds.unwrap_or(0).to_string())
            .arg("--open-files")
            .arg(limits.open_files.unwrap_or(0).to_string())
            .arg("--profile")
            .arg(profile.policy());
        for definition in profile.definitions() {
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
        if let Some(network) = mediator.as_ref() {
            process.envs(network.environment(network.address()));
        }
        let reservation = self.reservation.take().ok_or(SandboxError::Concurrency)?;
        let stage = self.scratch.take().ok_or_else(|| {
            SandboxError::Lifecycle(std::io::Error::other(
                "sandbox scratch owner is unavailable",
            ))
        })?;
        let launch = MacLaunch {
            process: Some(process),
            profile,
            plan: Some(super::process::SpawnPlan {
                network: mediator.take(),
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
                startup_input: None,
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
fn validate_launch_arguments(
    broker: &broker::Broker,
    profile: &profile::Profile,
    command: &SandboxCommand,
    limits: SandboxResourceLimits,
) -> Result<(), SandboxError> {
    let cpu_seconds = limits.cpu_seconds.unwrap_or(0).to_string();
    let open_files = limits.open_files.unwrap_or(0).to_string();
    let mut bytes = [
        broker.path().as_os_str(),
        OsStr::new(crucible_sandbox_broker::MACOS_LAUNCH_MODE),
        OsStr::new("--cpu-seconds"),
        OsStr::new(&cpu_seconds),
        OsStr::new("--open-files"),
        OsStr::new(&open_files),
        OsStr::new("--profile"),
        OsStr::new(profile.policy()),
    ]
    .into_iter()
    .fold(0_usize, |total, argument| {
        total
            .saturating_add(argument.as_encoded_bytes().len())
            .saturating_add(1)
    });
    for definition in profile.definitions() {
        bytes = bytes
            .saturating_add("--definition".len())
            .saturating_add(1)
            .saturating_add(definition.as_encoded_bytes().len())
            .saturating_add(1);
    }
    bytes = bytes
        .saturating_add("--".len())
        .saturating_add(1)
        .saturating_add(command.program().as_os_str().as_encoded_bytes().len())
        .saturating_add(1);
    bytes = command.arguments().iter().fold(bytes, |total, argument| {
        total
            .saturating_add(argument.as_encoded_bytes().len())
            .saturating_add(1)
    });
    if bytes > crucible_sandbox_broker::MACOS_MAX_LAUNCH_ARGUMENT_BYTES {
        return Err(materialization(
            "macOS sandbox launcher arguments exceed the backend bound",
            None,
        ));
    }
    Ok(())
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
    profile: profile::Profile,
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
        if let Err(problem) = self.profile.validate_network() {
            self.audit.record(
                self.sandbox,
                SandboxFactKind::Failed {
                    phase: SandboxFailurePhase::Start,
                    kind: problem.failure_kind(),
                },
            )?;
            return Err(problem);
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
