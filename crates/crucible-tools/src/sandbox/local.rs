//! Selection between an enforcing native backend and compatibility execution.

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crucible_core::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCapability, SandboxCleanup, SandboxCommand, SandboxCommandStage, SandboxError,
    SandboxFactKind, SandboxFailurePhase, SandboxFeature, SandboxGuardrailDecision,
    SandboxInspection, SandboxLaunch, SandboxLifecycle, SandboxMode, SandboxProcess,
    SandboxRequest, SandboxService, SandboxSession,
};

use super::process::{MAX_LOCAL_COMMANDS, Reservation};

/// Host-owned local sandbox service.
///
/// `required` requests use a verified native backend on Linux and macOS.
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
            super::linux::probe(&[])
        }
        #[cfg(target_os = "macos")]
        {
            super::macos::probe()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(SandboxError::BackendUnavailable {
                reason: "no enforcing local sandbox backend for this operating system".into(),
            })
        }
    }

    fn prepare(&self, request: SandboxRequest) -> Result<Box<dyn SandboxSession>, SandboxError> {
        let audit = request.audit().clone();
        let id = request.id();
        audit.record(
            id,
            SandboxFactKind::Lifecycle(SandboxLifecycle::PolicyResolved),
        )?;
        let prepared = match request.policy().mode() {
            SandboxMode::Off => compatibility(
                request,
                Arc::clone(&self.active),
                "sandbox disabled by effective policy",
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
                #[cfg(target_os = "macos")]
                {
                    match super::macos::prepare(request.clone(), Arc::clone(&self.active)) {
                        Ok(session) => Ok(session),
                        Err(SandboxError::BackendUnavailable { .. }) => compatibility(
                            request,
                            Arc::clone(&self.active),
                            "enforcing macOS sandbox unavailable; user selected degraded mode",
                        ),
                        Err(error) => Err(error),
                    }
                }
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                {
                    compatibility(
                        request,
                        Arc::clone(&self.active),
                        "no enforcing backend for this operating system; user selected degraded mode",
                    )
                }
            }
        };
        if let Err(problem) = &prepared {
            audit.record(
                id,
                SandboxFactKind::Failed {
                    phase: SandboxFailurePhase::Prepare,
                    kind: problem.failure_kind(),
                },
            )?;
            audit.record(id, SandboxFactKind::Cleanup(SandboxCleanup::Complete))?;
        }
        prepared
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
    #[cfg(target_os = "macos")]
    {
        super::macos::prepare(request, active)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
    let (backend, capabilities) = compatibility_capabilities()?;
    request.negotiate(&capabilities)?;
    let maximum = request
        .policy()
        .limits()
        .concurrent_commands
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(MAX_LOCAL_COMMANDS);
    let reservation = Reservation::take(active, maximum)?;
    let inspection =
        SandboxInspection::unconfined_for_request(backend, capabilities, &request, degradation)?;
    request.audit().record(
        request.id(),
        SandboxFactKind::Negotiated(Box::new(inspection.clone())),
    )?;
    request.audit().record(
        request.id(),
        SandboxFactKind::Lifecycle(SandboxLifecycle::Prepared),
    )?;
    Ok(Box::new(CompatibilitySession {
        request,
        inspection,
        reservation: Some(reservation),
        materialized: false,
        transferred: false,
    }))
}

pub(super) fn compatibility_capabilities()
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
    transferred: bool,
}

impl SandboxSession for CompatibilitySession {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn materialize(&mut self) -> Result<(), SandboxError> {
        if self.materialized {
            return Ok(());
        }
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
                        kind: crucible_core::SandboxFailureKind::Guardrail,
                    },
                )?;
                return Err(SandboxError::Guardrail);
            }
        }
        let mut process = Command::new(command.program());
        process
            .args(command.arguments())
            .current_dir(self.request.policy().working_directory())
            .env_clear()
            .envs(command.environment().iter());
        let reservation = self.reservation.take().ok_or(SandboxError::Concurrency)?;
        let launch = CompatibilityLaunch {
            process: Some(process),
            plan: Some(super::process::SpawnPlan {
                inspection: self.inspection.clone(),
                reservation,
                stage: None,
                limits: self.request.policy().limits(),
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

struct CompatibilityLaunch {
    process: Option<Command>,
    plan: Option<super::process::SpawnPlan>,
    inspection: SandboxInspection,
    audit: crucible_core::SandboxAudit,
    sandbox: crucible_core::SandboxId,
    invocation: crucible_core::SandboxInvocationMode,
    owner_transferred: bool,
    released: bool,
}

impl SandboxLaunch for CompatibilityLaunch {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn transfer_owner(&mut self) -> Result<(), SandboxError> {
        if self.invocation == crucible_core::SandboxInvocationMode::Foreground
            || self.owner_transferred
        {
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
        if self.invocation != crucible_core::SandboxInvocationMode::Foreground
            && !self.owner_transferred
        {
            return Err(SandboxError::Lifecycle(std::io::Error::other(
                "background sandbox has no application cleanup owner",
            )));
        }
        let process = self.process.take().ok_or_else(|| {
            SandboxError::Spawn(std::io::Error::other(
                "compatibility command was already released",
            ))
        })?;
        let plan = self.plan.take().ok_or_else(|| {
            SandboxError::Spawn(std::io::Error::other(
                "compatibility launch plan is unavailable",
            ))
        })?;
        self.released = true;
        let spawned = super::process::spawn(process, plan);
        if let Err(problem) = &spawned {
            // Spawn returns Lifecycle when cleanup itself was unconfirmed.
            // Preserve that primary error even if the bounded audit is full.
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

impl Drop for CompatibilityLaunch {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.audit.record(
                self.sandbox,
                SandboxFactKind::Lifecycle(SandboxLifecycle::Refused),
            );
            let _ = self.audit.record(
                self.sandbox,
                SandboxFactKind::Cleanup(SandboxCleanup::Complete),
            );
        }
    }
}

impl Drop for CompatibilitySession {
    fn drop(&mut self) {
        if !self.transferred {
            let _ = self.request.audit().record(
                self.request.id(),
                SandboxFactKind::Cleanup(SandboxCleanup::Complete),
            );
        }
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

// Every test drives the compatibility backend through `/bin/sh`, which Windows
// has no equivalent of and does not even accept as an absolute path.
#[cfg(test)]
#[cfg(unix)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn startup_stage_cleanup_failure_is_audited_as_failed() -> std::io::Result<()> {
        use crate::sandbox::process::{Stage, testing_plan};

        let sample = crate::sample::Sample::new("compatibility-startup-cleanup");
        let root = sample.root().join("stage-file");
        std::fs::write(&root, "a regular file cannot be removed as a tree")?;
        let plan = testing_plan(
            crucible_core::SandboxSpeech::Closed,
            Some(Stage::new(root.clone())),
        )
        .map_err(std::io::Error::other)?;
        let audit = plan.audit.clone();
        let launch = super::CompatibilityLaunch {
            process: Some(std::process::Command::new(
                sample.root().join("absent-program"),
            )),
            inspection: plan.inspection.clone(),
            sandbox: plan.sandbox,
            audit: audit.clone(),
            invocation: plan.invocation,
            plan: Some(plan),
            owner_transferred: false,
            released: false,
        };
        let result = crucible_core::SandboxLaunch::release(Box::new(launch));
        let facts = audit.records().map_err(std::io::Error::other)?;
        assert!(
            facts.iter().any(|record| matches!(
                record.fact().kind(),
                crucible_core::SandboxFactKind::Cleanup(crucible_core::SandboxCleanup::Failed)
            )),
            "unconfirmed startup cleanup must be audited as failed: {facts:?}"
        );
        assert!(!facts.iter().any(|record| matches!(
            record.fact().kind(),
            crucible_core::SandboxFactKind::Cleanup(crucible_core::SandboxCleanup::Complete)
        )));
        assert!(matches!(
            result,
            Err(crucible_core::SandboxError::Lifecycle(_))
        ));
        assert!(root.exists());
        Ok(())
    }

    use std::ffi::OsString;
    use std::thread;
    use std::time::{Duration, Instant};

    use crucible_core::{
        Ancestry, SandboxAudit, SandboxCommandPolicy, SandboxCommandRule, SandboxEnvironment,
        SandboxFactKind, SandboxGuardrailEffect, SandboxId, SandboxLifecycle, SandboxManifest,
        SandboxPolicy, SandboxRequest, ToolId,
    };

    use super::*;
    use crate::sample::Sample;

    fn fact(facts: &[crucible_core::SandboxAuditRecord], index: usize) -> &SandboxFactKind {
        facts
            .get(index)
            .expect("expected ordered sandbox fact")
            .fact()
            .kind()
    }

    #[test]
    fn a_denied_command_is_refused_before_compatibility_spawn() {
        let sample = Sample::new("sandbox-command-guardrail");
        let ancestry = Ancestry::new();
        let call = ToolId::new("guardrail");
        let audit = SandboxAudit::new(ancestry, call.clone());
        let script = "printf denied > blocked.txt";
        let commands = SandboxCommandPolicy::new([SandboxCommandRule::exact(
            SandboxGuardrailEffect::Deny,
            ["/bin/sh", "-c", script],
        )
        .expect("rule")])
        .expect("command policy");
        let policy = SandboxPolicy::standard(&sample.workspace())
            .expect("policy")
            .with_mode(SandboxMode::Off)
            .with_command_policy(commands);
        let request = SandboxRequest::new(
            SandboxId::new(),
            ancestry,
            call,
            policy,
            SandboxManifest::empty(),
        )
        .with_audit(audit.clone())
        .expect("matching audit attribution");
        let service = LocalSandbox::new();
        let mut session = service.prepare(request).expect("session");
        session.materialize().expect("materialized");
        let command = SandboxCommand::new(
            "/bin/sh",
            [OsString::from("-c"), OsString::from(script)],
            SandboxEnvironment::empty(),
        )
        .expect("command");

        assert!(matches!(
            session.start(command),
            Err(SandboxError::Guardrail)
        ));
        assert!(!sample.root().join("blocked.txt").exists());
        let facts = audit.records().expect("facts");
        assert_eq!(facts.len(), 7, "{facts:#?}");
        assert!(matches!(
            fact(&facts, 4),
            SandboxFactKind::Guardrail {
                stage: SandboxCommandStage::Requested,
                decision: SandboxGuardrailDecision::Denied,
            }
        ));
        assert!(matches!(
            fact(&facts, 5),
            SandboxFactKind::Failed {
                phase: SandboxFailurePhase::Start,
                kind: crucible_core::SandboxFailureKind::Guardrail,
            }
        ));
        assert!(matches!(
            fact(&facts, 6),
            SandboxFactKind::Cleanup(SandboxCleanup::Complete)
        ));
    }

    #[test]
    fn a_successful_lifecycle_emits_one_ordered_redacted_fact_sequence() {
        let sample = Sample::new("sandbox-audit-lifecycle");
        let ancestry = Ancestry::new();
        let call = ToolId::new("audit-call");
        let audit = SandboxAudit::new(ancestry, call.clone());
        let policy = SandboxPolicy::standard(&sample.workspace())
            .expect("policy")
            .with_mode(SandboxMode::Off);
        let request = SandboxRequest::new(
            SandboxId::new(),
            ancestry,
            call,
            policy,
            SandboxManifest::empty(),
        )
        .with_audit(audit.clone())
        .expect("matching audit attribution");
        let service = LocalSandbox::new();
        let mut session = service.prepare(request).expect("session");
        session.materialize().expect("materialized");
        let command = SandboxCommand::new(
            "/bin/sh",
            [OsString::from("-c"), OsString::from("exit 0")],
            SandboxEnvironment::empty(),
        )
        .expect("command");
        let mut process = session.start(command).expect("process");
        let deadline = Instant::now() + Duration::from_secs(2);
        while process.try_wait().expect("wait").is_none() {
            assert!(Instant::now() < deadline, "command did not finish");
            thread::sleep(Duration::from_millis(5));
        }
        process.stop().expect("cleanup");
        process.stop().expect("idempotent cleanup");

        let facts = audit.records().expect("facts");
        assert_eq!(facts.len(), 10, "{facts:#?}");
        assert!(matches!(
            fact(&facts, 0),
            SandboxFactKind::Lifecycle(SandboxLifecycle::PolicyResolved)
        ));
        assert!(matches!(fact(&facts, 1), SandboxFactKind::Negotiated(_)));
        assert!(matches!(
            fact(&facts, 2),
            SandboxFactKind::Lifecycle(SandboxLifecycle::Prepared)
        ));
        assert!(matches!(
            fact(&facts, 3),
            SandboxFactKind::Lifecycle(SandboxLifecycle::Materialized)
        ));
        assert!(matches!(
            fact(&facts, 4),
            SandboxFactKind::Guardrail {
                stage: SandboxCommandStage::Requested,
                decision: SandboxGuardrailDecision::Allowed,
            }
        ));
        assert!(matches!(
            fact(&facts, 5),
            SandboxFactKind::Guardrail {
                stage: SandboxCommandStage::Effective,
                decision: SandboxGuardrailDecision::Allowed,
            }
        ));
        assert!(matches!(
            fact(&facts, 6),
            SandboxFactKind::Lifecycle(SandboxLifecycle::CommandStarted)
        ));
        assert!(matches!(
            fact(&facts, 7),
            SandboxFactKind::Lifecycle(SandboxLifecycle::CommandFinished)
        ));
        assert!(matches!(fact(&facts, 8), SandboxFactKind::Usage(_)));
        assert!(matches!(
            fact(&facts, 9),
            SandboxFactKind::Cleanup(SandboxCleanup::Complete)
        ));
        assert!(facts.iter().all(|record| record.ancestry() == ancestry));
    }

    #[test]
    fn a_trusted_argument_transformation_is_checked_again() {
        let sample = Sample::new("sandbox-transformed-command-guardrail");
        let requested = "printf requested > requested.txt";
        let transformed = "printf transformed > transformed.txt";
        let commands = SandboxCommandPolicy::new([SandboxCommandRule::exact(
            SandboxGuardrailEffect::Allow,
            ["/bin/sh", "-c", requested],
        )
        .expect("rule")])
        .expect("command policy");
        let policy = SandboxPolicy::standard(&sample.workspace())
            .expect("policy")
            .with_mode(SandboxMode::Off)
            .with_command_policy(commands);
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("guardrail"),
            policy,
            SandboxManifest::empty(),
        );
        let service = LocalSandbox::new();
        let mut session = service.prepare(request).expect("session");
        session.materialize().expect("materialized");
        let command = SandboxCommand::new(
            "/bin/sh",
            [OsString::from("-c"), OsString::from(requested)],
            SandboxEnvironment::empty(),
        )
        .expect("command")
        .transformed(
            "/bin/sh",
            [OsString::from("-c"), OsString::from(transformed)],
        )
        .expect("transformation");

        assert!(matches!(
            session.start(command),
            Err(SandboxError::Guardrail)
        ));
        assert!(!sample.root().join("requested.txt").exists());
        assert!(!sample.root().join("transformed.txt").exists());
    }

    #[test]
    fn a_command_deadline_is_enforced_without_a_bash_waiter() {
        let sample = Sample::new("sandbox-service-command-deadline");
        let ancestry = Ancestry::new();
        let call = ToolId::new("deadline");
        let audit = SandboxAudit::new(ancestry, call.clone());
        let limits = crucible_core::SandboxResourceLimits {
            command_time: Some(Duration::from_millis(100)),
            ..Default::default()
        };
        let policy = SandboxPolicy::standard(&sample.workspace())
            .expect("policy")
            .with_mode(SandboxMode::Off)
            .with_limits(limits)
            .expect("limits");
        let request = SandboxRequest::new(
            SandboxId::new(),
            ancestry,
            call,
            policy,
            SandboxManifest::empty(),
        )
        .with_audit(audit.clone())
        .expect("matching audit attribution");
        let service = LocalSandbox::new();
        let mut session = service.prepare(request).expect("session");
        session.materialize().expect("materialized");
        let command = SandboxCommand::new(
            "/bin/sh",
            [OsString::from("-c"), OsString::from("sleep 5")],
            SandboxEnvironment::empty(),
        )
        .expect("command");
        let mut process = session.start(command).expect("process");
        thread::sleep(Duration::from_millis(250));
        let status = process.try_wait().expect("wait");
        let violation = process.violation();
        process.stop().expect("cleanup");

        assert!(status.is_some(), "command survived its service deadline");
        assert_eq!(
            violation,
            Some(crucible_core::SandboxViolation::CommandTime)
        );
        let facts = audit.records().expect("facts");
        let kinds = facts
            .iter()
            .map(|record| record.fact().kind())
            .collect::<Vec<_>>();
        let violated = kinds
            .iter()
            .position(|kind| {
                matches!(
                    kind,
                    SandboxFactKind::Violation(crucible_core::SandboxViolation::CommandTime)
                )
            })
            .expect("deadline violation fact");
        let finished = kinds
            .iter()
            .position(|kind| {
                matches!(
                    kind,
                    SandboxFactKind::Lifecycle(SandboxLifecycle::CommandFinished)
                )
            })
            .expect("command finished fact");
        let cleaned = kinds
            .iter()
            .position(|kind| matches!(kind, SandboxFactKind::Cleanup(SandboxCleanup::Complete)))
            .expect("cleanup fact");
        assert!(violated < finished && finished < cleaned, "{facts:#?}");
    }

    #[test]
    fn process_streams_enforce_the_combined_output_ceiling() {
        let sample = Sample::new("sandbox-service-output-limit");
        let limits = crucible_core::SandboxResourceLimits {
            output_bytes: Some(32),
            ..Default::default()
        };
        let policy = SandboxPolicy::standard(&sample.workspace())
            .expect("policy")
            .with_mode(SandboxMode::Off)
            .with_limits(limits)
            .expect("limits");
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("output"),
            policy,
            SandboxManifest::empty(),
        );
        let service = LocalSandbox::new();
        let mut session = service.prepare(request).expect("session");
        session.materialize().expect("materialized");
        let command = SandboxCommand::new(
            "/bin/sh",
            [
                OsString::from("-c"),
                OsString::from("head -c 1024 /dev/zero"),
            ],
            SandboxEnvironment::empty(),
        )
        .expect("command");
        let mut process = session.start(command).expect("process");
        let mut output = process.take_stdout().expect("stdout");
        let mut retained = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut buffer = [0_u8; 128];
            match output.read_ready(&mut buffer).expect("read") {
                crucible_core::SandboxRead::Bytes(read)
                | crucible_core::SandboxRead::Limited { retained: read, .. } => {
                    retained.extend_from_slice(buffer.get(..read).expect("reported bytes"));
                }
                crucible_core::SandboxRead::Pending => {
                    assert!(Instant::now() < deadline, "output did not finish");
                    thread::sleep(Duration::from_millis(10));
                }
                crucible_core::SandboxRead::End => break,
            }
        }
        while process.try_wait().expect("wait").is_none() {
            assert!(Instant::now() < deadline, "command did not finish");
            thread::sleep(Duration::from_millis(10));
        }
        let usage = process.usage();
        process.stop().expect("cleanup");

        assert!(retained.len() <= 32, "retained {} bytes", retained.len());
        assert!(
            usage.output_bytes >= 1024,
            "observed {} bytes",
            usage.output_bytes
        );
    }

    #[test]
    fn stdout_and_stderr_share_one_output_budget() {
        let sample = Sample::new("sandbox-shared-output-limit");
        let limits = crucible_core::SandboxResourceLimits {
            output_bytes: Some(32),
            ..Default::default()
        };
        let policy = SandboxPolicy::standard(&sample.workspace())
            .expect("policy")
            .with_mode(SandboxMode::Off)
            .with_limits(limits)
            .expect("limits");
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("shared-output"),
            policy,
            SandboxManifest::empty(),
        );
        let service = LocalSandbox::new();
        let mut session = service.prepare(request).expect("session");
        session.materialize().expect("materialized");
        let command = SandboxCommand::new(
            "/bin/sh",
            [
                OsString::from("-c"),
                OsString::from("head -c 24 /dev/zero; head -c 24 /dev/zero >&2"),
            ],
            SandboxEnvironment::empty(),
        )
        .expect("command");
        let mut process = session.start(command).expect("process");
        let mut stdout = process.take_stdout();
        let mut stderr = process.take_stderr();
        let mut retained = 0_usize;
        let deadline = Instant::now() + Duration::from_secs(2);
        while stdout.is_some() || stderr.is_some() {
            for stream in [&mut stdout, &mut stderr] {
                let Some(output) = stream else {
                    continue;
                };
                let mut buffer = [0_u8; 64];
                match output.read_ready(&mut buffer).expect("read") {
                    crucible_core::SandboxRead::Bytes(read)
                    | crucible_core::SandboxRead::Limited { retained: read, .. } => {
                        retained = retained.saturating_add(read);
                    }
                    crucible_core::SandboxRead::Pending => {}
                    crucible_core::SandboxRead::End => *stream = None,
                }
            }
            assert!(Instant::now() < deadline, "streams did not finish");
            thread::sleep(Duration::from_millis(5));
        }
        while process.try_wait().expect("wait").is_none() {
            assert!(Instant::now() < deadline, "command did not finish");
            thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(retained, 32);
        assert!(process.usage().output_bytes >= 48);
        assert_eq!(
            process.violation(),
            Some(crucible_core::SandboxViolation::Output)
        );
        process.stop().expect("cleanup");
    }

    /// Runs a command writing 24 bytes to each stream under `ceiling`, drains
    /// both to their end, and returns what the caller kept beside what the
    /// accounting says was captured.
    fn drained_under(name: &str, ceiling: Option<u64>) -> (usize, crucible_core::SandboxUsage) {
        let sample = Sample::new(name);
        let limits = crucible_core::SandboxResourceLimits {
            output_bytes: ceiling,
            ..Default::default()
        };
        let policy = SandboxPolicy::standard(&sample.workspace())
            .expect("policy")
            .with_mode(SandboxMode::Off)
            .with_limits(limits)
            .expect("limits");
        let service = LocalSandbox::new();
        let mut session = service
            .prepare(SandboxRequest::new(
                SandboxId::new(),
                Ancestry::new(),
                ToolId::new("counted-output"),
                policy,
                SandboxManifest::empty(),
            ))
            .expect("session");
        session.materialize().expect("materialized");
        let mut process = session
            .start(
                SandboxCommand::new(
                    "/bin/sh",
                    [
                        OsString::from("-c"),
                        OsString::from("head -c 24 /dev/zero; head -c 24 /dev/zero >&2"),
                    ],
                    SandboxEnvironment::empty(),
                )
                .expect("command"),
            )
            .expect("process");
        let mut streams = [process.take_stdout(), process.take_stderr()];
        let mut finished = [false, false];
        let mut retained = 0_usize;
        let deadline = Instant::now() + Duration::from_secs(2);
        while finished.iter().any(|done| !done) {
            for (stream, done) in streams.iter_mut().zip(finished.iter_mut()) {
                let Some(output) = stream else {
                    *done = true;
                    continue;
                };
                if *done {
                    continue;
                }
                // Smaller than either stream, so the bytes arrive over several
                // reads and a counter that added on the wrong edge would show.
                let mut buffer = [0_u8; 16];
                match output.read_ready(&mut buffer).expect("read") {
                    crucible_core::SandboxRead::Bytes(read)
                    | crucible_core::SandboxRead::Limited { retained: read, .. } => {
                        retained = retained.saturating_add(read);
                    }
                    crucible_core::SandboxRead::Pending => {}
                    crucible_core::SandboxRead::End => *done = true,
                }
            }
            assert!(Instant::now() < deadline, "streams did not finish");
            thread::sleep(Duration::from_millis(5));
        }
        while process.try_wait().expect("wait").is_none() {
            assert!(Instant::now() < deadline, "command did not finish");
            thread::sleep(Duration::from_millis(5));
        }

        // Reading a stream that already ended is not another 48 bytes, and
        // asking for the usage twice is a reading rather than an addition.
        for stream in &mut streams {
            let Some(output) = stream else {
                continue;
            };
            let mut buffer = [0_u8; 16];
            assert!(matches!(
                output.read_ready(&mut buffer).expect("read past the end"),
                crucible_core::SandboxRead::End
            ));
        }
        let usage = process.usage();
        assert_eq!(
            usage.output_bytes,
            process.usage().output_bytes,
            "the count moved when it was read"
        );
        process.stop().expect("cleanup");
        (retained, usage)
    }

    #[test]
    fn every_captured_byte_is_counted_once_whether_it_was_kept_or_dropped() {
        // Nothing is dropped, so the two numbers are the same number, and it is
        // the number of bytes the command actually wrote.
        let (retained, usage) = drained_under("sandbox-output-counted-once", None);
        assert_eq!(retained, 48);
        assert_eq!(usage.output_bytes, 48);

        // Past the ceiling the caller keeps less, and the accounting still
        // counts each byte exactly once: it is raw capture, not what survived.
        let (retained, usage) = drained_under("sandbox-output-counted-past", Some(32));
        assert_eq!(retained, 32);
        assert_eq!(usage.output_bytes, 48);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn normal_leader_exit_stops_and_reaps_its_descendants() {
        let sample = Sample::new("sandbox-normal-exit-descendants");
        let policy = SandboxPolicy::standard(&sample.workspace())
            .expect("policy")
            .with_mode(SandboxMode::Off);
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("descendants"),
            policy,
            SandboxManifest::empty(),
        );
        let service = LocalSandbox::new();
        let mut session = service.prepare(request).expect("session");
        session.materialize().expect("materialized");
        let command = SandboxCommand::new(
            "/bin/sh",
            [
                OsString::from("-c"),
                OsString::from("sleep 30 & echo $! > descendant.pid; exit 0"),
            ],
            SandboxEnvironment::empty(),
        )
        .expect("command");
        let mut process = session.start(command).expect("process");
        let deadline = Instant::now() + Duration::from_secs(2);
        while process.try_wait().expect("wait").is_none() {
            assert!(Instant::now() < deadline, "leader did not exit");
            thread::sleep(Duration::from_millis(5));
        }
        let pid = read_pid(&sample.root().join("descendant.pid"));
        assert_no_live_process(pid, deadline);
        process.stop().expect("first cleanup");
        process.stop().expect("idempotent cleanup");
    }

    #[cfg(target_os = "linux")]
    fn read_pid(path: &std::path::Path) -> u32 {
        std::fs::read_to_string(path)
            .expect("pid file")
            .trim()
            .parse()
            .expect("numeric pid")
    }

    #[cfg(target_os = "linux")]
    fn assert_no_live_process(pid: u32, deadline: Instant) {
        let status = std::path::PathBuf::from(format!("/proc/{pid}/stat"));
        loop {
            match std::fs::read_to_string(&status) {
                Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => return,
                Ok(stat) if stat.split_whitespace().nth(2) == Some("Z") => return,
                Ok(_) => {}
                Err(problem) => panic!("could not inspect descendant: {problem}"),
            }
            assert!(Instant::now() < deadline, "descendant {pid} is still live");
            thread::sleep(Duration::from_millis(5));
        }
    }
}
