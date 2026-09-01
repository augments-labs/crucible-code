//! Selection between enforcing Linux and explicit compatibility execution.

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crucible_core::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCapability, SandboxCleanup, SandboxCommand, SandboxError, SandboxFeature,
    SandboxInspection, SandboxMode, SandboxProcess, SandboxRequest, SandboxService, SandboxSession,
};

use super::process::{MAX_LOCAL_COMMANDS, Reservation};

/// Host-owned local sandbox service.
///
/// Linux `required` requests use only a verified system Bubblewrap backend.
/// `off`, and an explicitly selected `degraded` fallback, still pass through
/// this service so lifecycle and inspection cannot be bypassed or mislabeled.
#[derive(Debug, Clone, Default)]
pub struct LocalSandbox {
    active: Arc<AtomicUsize>,
}

impl LocalSandbox {
    /// A service with no active commands.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SandboxService for LocalSandbox {
    fn probe(&self) -> Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError> {
        #[cfg(target_os = "linux")]
        {
            return super::linux::probe(&[]);
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(SandboxError::BackendUnavailable {
                reason: "no enforcing local sandbox backend for this operating system".into(),
            })
        }
    }

    fn prepare(&self, request: SandboxRequest) -> Result<Box<dyn SandboxSession>, SandboxError> {
        match request.policy().mode() {
            SandboxMode::Off => compatibility(
                request,
                Arc::clone(&self.active),
                "sandbox explicitly disabled by user policy",
            ),
            SandboxMode::Required => enforcing(request, Arc::clone(&self.active)),
            SandboxMode::Degraded => {
                #[cfg(target_os = "linux")]
                {
                    match super::linux::prepare(request.clone(), Arc::clone(&self.active)) {
                        Ok(session) => Ok(session),
                        Err(SandboxError::BackendUnavailable { .. }) => compatibility(
                            request,
                            Arc::clone(&self.active),
                            "enforcing Linux sandbox unavailable; user selected degraded mode",
                        ),
                        Err(error) => Err(error),
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    compatibility(
                        request,
                        Arc::clone(&self.active),
                        "no enforcing backend for this operating system; user selected degraded mode",
                    )
                }
            }
        }
    }
}

fn enforcing(
    request: SandboxRequest,
    active: Arc<AtomicUsize>,
) -> Result<Box<dyn SandboxSession>, SandboxError> {
    #[cfg(target_os = "linux")]
    {
        super::linux::prepare(request, active)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (request, active);
        Err(SandboxError::BackendUnavailable {
            reason: "required confinement is unsupported on this operating system".into(),
        })
    }
}

fn compatibility(
    request: SandboxRequest,
    active: Arc<AtomicUsize>,
    degradation: &'static str,
) -> Result<Box<dyn SandboxSession>, SandboxError> {
    if !request.manifest().is_empty() {
        return Err(SandboxError::Unsupported {
            feature: SandboxFeature::Materialization,
        });
    }
    let (backend, capabilities) = compatibility_capabilities()?;
    let maximum = request
        .policy()
        .limits()
        .concurrent_commands
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(MAX_LOCAL_COMMANDS);
    let reservation = Reservation::take(active, maximum)?;
    let inspection = SandboxInspection::new(
        request.id(),
        backend,
        capabilities,
        request.policy().digest(),
        request.manifest().digest(),
        false,
        Some(degradation),
        SandboxCleanup::Pending,
    )?;
    Ok(Box::new(CompatibilitySession {
        request,
        inspection,
        reservation: Some(reservation),
        materialized: false,
    }))
}

fn compatibility_capabilities()
-> Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError> {
    let id = SandboxBackendId::new("local-compatibility").map_err(|_| {
        SandboxError::BackendUnavailable {
            reason: "invalid built-in compatibility backend identity".into(),
        }
    })?;
    let identity =
        SandboxBackendIdentity::new(id, "1", SandboxBackendProvenance::Compatibility, None)
            .map_err(|_| SandboxError::BackendUnavailable {
                reason: "invalid built-in compatibility backend version".into(),
            })?;
    let capabilities = SandboxCapabilities::none()
        .with(
            SandboxFeature::CommandTimeLimit,
            SandboxCapability::Enforced,
        )
        .with(SandboxFeature::OutputLimit, SandboxCapability::Enforced)
        .with(
            SandboxFeature::ConcurrencyLimit,
            SandboxCapability::Enforced,
        )
        .with(SandboxFeature::Audit, SandboxCapability::Enforced)
        .with(SandboxFeature::Usage, SandboxCapability::Observed);
    Ok((identity, capabilities))
}

struct CompatibilitySession {
    request: SandboxRequest,
    inspection: SandboxInspection,
    reservation: Option<Reservation>,
    materialized: bool,
}

impl SandboxSession for CompatibilitySession {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn materialize(&mut self) -> Result<(), SandboxError> {
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
        let mut process = Command::new(command.program());
        process
            .args(command.arguments())
            .current_dir(self.request.policy().working_directory())
            .env_clear()
            .envs(command.environment().iter());
        let reservation = self.reservation.take().ok_or(SandboxError::Concurrency)?;
        super::process::spawn(process, self.inspection.clone(), reservation)
    }
}

impl std::fmt::Debug for CompatibilitySession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompatibilitySession")
            .field("inspection", &self.inspection)
            .field("materialized", &self.materialized)
            .finish_non_exhaustive()
    }
}
