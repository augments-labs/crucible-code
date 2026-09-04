//! Admission is bounded before a session may materialize or launch anything.
//!
//! Compatibility always runs here. Enforcing preparations join the same probes
//! when a backend is available, and are mandatory on the enforcing CI job.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};

use crucible_core::{
    Ancestry, SandboxAudit, SandboxCleanup, SandboxError, SandboxFactKind, SandboxFilesystemAccess,
    SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxId, SandboxLifecycle,
    SandboxManifest, SandboxMode, SandboxNetworkPolicy, SandboxPolicy, SandboxRequest,
    SandboxResourceLimits, SandboxService, ToolId,
};
use crucible_tools::LocalSandbox;

struct Owned(PathBuf);

impl Owned {
    #[allow(clippy::expect_used)] // A fixture failure is not a backend result.
    fn new() -> Self {
        let at = std::env::temp_dir().join(format!("crucible-admission-{}", SandboxId::new()));
        fs::create_dir(&at).expect("temporary directory");
        Self(fs::canonicalize(at).expect("canonical directory"))
    }

    #[allow(clippy::expect_used)] // Every policy below names this fixture alone.
    fn request(&self, mode: SandboxMode, maximum: Option<u64>) -> SandboxRequest {
        let limits = SandboxResourceLimits {
            concurrent_commands: maximum,
            ..Default::default()
        };
        let rule = SandboxFilesystemRule::new(
            &self.0,
            SandboxFilesystemAccess::ReadOnly,
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("root");
        let policy =
            SandboxPolicy::new(mode, [rule], &self.0, SandboxNetworkPolicy::Closed, limits)
                .expect("policy");
        SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("admission"),
            policy,
            SandboxManifest::empty(),
        )
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The CI contract requires actual backend preparations, never silent skips.
#[allow(clippy::panic)] // An unexpected probe error is a test failure.
fn modes(service: &LocalSandbox) -> Vec<SandboxMode> {
    match service.probe() {
        Ok(_) => vec![SandboxMode::Off, SandboxMode::Required],
        Err(SandboxError::BackendUnavailable { reason }) => {
            assert!(
                std::env::var_os("CRUCIBLE_TEST_REQUIRE_ENFORCING_SANDBOX").is_none(),
                "enforcing admission tests require a backend: {reason}"
            );
            vec![SandboxMode::Off]
        }
        Err(other) => panic!("sandbox probe: {other}"),
    }
}

#[test]
#[allow(clippy::expect_used)] // Test assertions must retain the actual failure.
fn cloned_services_share_a_ceiling_before_materialization_and_release_it_on_drop() {
    let at = Owned::new();
    let service = LocalSandbox::new();
    for mode in modes(&service) {
        let first = service
            .prepare(at.request(mode, Some(2)))
            .expect("first slot");
        let second = service
            .clone()
            .prepare(at.request(mode, Some(2)))
            .expect("second slot");
        let request = at.request(mode, Some(2));
        let audit = SandboxAudit::new(request.ancestry(), request.call().clone());
        let request = request.with_audit(audit.clone()).expect("attribution");
        assert!(matches!(
            service.clone().prepare(request),
            Err(SandboxError::Concurrency)
        ));
        let records = audit.records().expect("audit");
        assert!(!records.iter().any(|record| matches!(
            record.fact().kind(),
            SandboxFactKind::Lifecycle(
                SandboxLifecycle::Prepared
                    | SandboxLifecycle::Materialized
                    | SandboxLifecycle::CommandStarted
            )
        )));
        assert!(records.iter().any(|record| matches!(
            record.fact().kind(),
            SandboxFactKind::Cleanup(SandboxCleanup::Complete)
        )));
        drop(first);
        let replacement = service
            .prepare(at.request(mode, Some(2)))
            .expect("released slot");
        assert!(matches!(
            service.prepare(at.request(mode, Some(2))),
            Err(SandboxError::Concurrency)
        ));
        drop(second);
        drop(replacement);
        assert!(service.prepare(at.request(mode, Some(1))).is_ok());
    }
}

#[test]
#[allow(clippy::expect_used)] // Worker panics and unexpected refusals are failures.
fn simultaneous_preparations_cannot_oversubscribe_the_requested_ceiling() {
    let at = Owned::new();
    let service = LocalSandbox::new();
    for mode in modes(&service) {
        let barrier = Arc::new(Barrier::new(8));
        let results = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| {
                    let service = service.clone();
                    let barrier = Arc::clone(&barrier);
                    let request = at.request(mode, Some(2));
                    scope.spawn(move || {
                        barrier.wait();
                        service.prepare(request)
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().expect("worker"))
                .collect::<Vec<_>>()
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 2);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(SandboxError::Concurrency)))
                .count(),
            6
        );
        drop(results);
        assert!(service.prepare(at.request(mode, Some(1))).is_ok());
    }
}

#[test]
#[allow(clippy::expect_used)] // Each of the sixteen advertised local slots must exist.
fn omitted_or_larger_policy_limits_cannot_bypass_the_absolute_service_ceiling() {
    let at = Owned::new();
    let service = LocalSandbox::new();
    for mode in modes(&service) {
        for maximum in [None, Some(100)] {
            let held: Vec<_> = (0..16)
                .map(|_| service.prepare(at.request(mode, maximum)).expect("slot"))
                .collect();
            assert!(matches!(
                service.prepare(at.request(mode, maximum)),
                Err(SandboxError::Concurrency)
            ));
            drop(held);
            assert!(service.prepare(at.request(mode, Some(1))).is_ok());
        }
    }
}
