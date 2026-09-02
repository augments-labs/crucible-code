//! Reusable capability/refusal conformance fixtures for local sandbox adapters.
//!
//! A future local adapter belongs in this module's matrix before it can be
//! selected by [`super::LocalSandbox`]. The checks deliberately enumerate the
//! unsupported cells as well as the implemented ones: absence is part of the
//! backend contract, not an invitation to fall back to an ordinary process.

use std::time::Duration;

use crucible_core::{
    Ancestry, SandboxBackendProvenance, SandboxCapabilities, SandboxCapability, SandboxError,
    SandboxFeature, SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
    SandboxId, SandboxManifest, SandboxManifestEntry, SandboxMode, SandboxNetworkEndpoint,
    SandboxNetworkPolicy, SandboxNetworkProvenance, SandboxPolicy, SandboxRequest,
    SandboxResourceLimits, SandboxService, ToolId,
};

use super::LocalSandbox;
use crate::sample::Sample;

const LINUX_ENFORCED: &[SandboxFeature] = &[
    SandboxFeature::Filesystem,
    SandboxFeature::NetworkDeny,
    SandboxFeature::DescriptorIsolation,
    SandboxFeature::ProcessIsolation,
    SandboxFeature::KernelSurface,
    SandboxFeature::PrivilegeIsolation,
    SandboxFeature::Materialization,
    SandboxFeature::CpuLimit,
    SandboxFeature::MemoryLimit,
    SandboxFeature::OpenFileLimit,
    SandboxFeature::CommandTimeLimit,
    SandboxFeature::OutputLimit,
    SandboxFeature::ConcurrencyLimit,
    SandboxFeature::Audit,
];

const COMPATIBILITY_ENFORCED: &[SandboxFeature] = &[
    SandboxFeature::CommandTimeLimit,
    SandboxFeature::OutputLimit,
    SandboxFeature::ConcurrencyLimit,
    SandboxFeature::Audit,
];

const OBSERVED: &[SandboxFeature] = &[SandboxFeature::Usage];

fn assert_exact_matrix(
    capabilities: &SandboxCapabilities,
    enforced: &[SandboxFeature],
    observed: &[SandboxFeature],
) {
    assert_eq!(capabilities.iter().count(), SandboxFeature::COUNT);
    for feature in SandboxFeature::ALL {
        let expected = expected(feature, enforced, observed);
        assert_eq!(
            capabilities.claim(feature),
            expected,
            "inaccurate {} capability",
            feature.as_str()
        );
    }
}

#[test]
fn compatibility_matrix_is_complete_and_never_claims_kernel_confinement() {
    let (identity, capabilities) = super::local::compatibility_capabilities().expect("identity");
    assert_eq!(
        identity.provenance(),
        SandboxBackendProvenance::Compatibility
    );
    assert_exact_matrix(&capabilities, COMPATIBILITY_ENFORCED, OBSERVED);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_matrix_is_complete_and_claims_only_implemented_boundaries() {
    let capabilities = super::linux::declared_capabilities();
    assert_exact_matrix(&capabilities, LINUX_ENFORCED, OBSERVED);
}

#[test]
fn published_capability_matrix_matches_both_declared_backends() {
    let document = include_str!("../../../../docs/security/sandboxing.md");
    for feature in SandboxFeature::ALL {
        let linux = expected(feature, LINUX_ENFORCED, OBSERVED).as_str();
        let compatibility = expected(feature, COMPATIBILITY_ENFORCED, OBSERVED).as_str();
        let row = format!("| `{}` | {linux} | {compatibility} |", feature.as_str());
        assert_eq!(
            document.matches(&row).count(),
            1,
            "missing or duplicate documented capability row: {row}"
        );
    }
}

fn expected(
    feature: SandboxFeature,
    enforced: &[SandboxFeature],
    observed: &[SandboxFeature],
) -> SandboxCapability {
    if enforced.contains(&feature) {
        SandboxCapability::Enforced
    } else if observed.contains(&feature) {
        SandboxCapability::Observed
    } else {
        SandboxCapability::Unsupported
    }
}

#[test]
fn compatibility_refuses_every_explicit_unsupported_policy_before_a_session_exists() {
    let sample = Sample::new("sandbox-compatibility-refusal-matrix");
    let mut requests = unsupported_requests(&sample);
    let service = LocalSandbox::new();

    for (feature, policy, manifest) in requests.drain(..) {
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new(format!("refuse-{}", feature.as_str())),
            policy,
            manifest,
        );
        let Err(problem) = service.prepare(request) else {
            panic!("compatibility accepted unsupported {}", feature.as_str());
        };
        assert!(
            matches!(problem, SandboxError::Unsupported { feature: refused } if refused == feature),
            "{} produced {problem:?}",
            feature.as_str()
        );
    }
}

fn unsupported_requests(sample: &Sample) -> Vec<(SandboxFeature, SandboxPolicy, SandboxManifest)> {
    let empty = || SandboxManifest::empty();
    let mut requests = Vec::new();
    let endpoint = SandboxNetworkEndpoint::new("192.0.2.1", 443, SandboxNetworkProvenance::User)
        .expect("literal endpoint");
    requests.push((
        SandboxFeature::NetworkAllowlist,
        policy(
            sample,
            SandboxNetworkPolicy::exact([endpoint], false, false).expect("exact network"),
            SandboxResourceLimits::default(),
        ),
        empty(),
    ));
    requests.push((
        SandboxFeature::Materialization,
        policy(
            sample,
            SandboxNetworkPolicy::Closed,
            SandboxResourceLimits::default(),
        ),
        SandboxManifest::new([SandboxManifestEntry::file(
            "fixture.txt",
            Box::<[u8]>::from(&b"inert"[..]),
            SandboxFilesystemProvenance::Manifest,
        )
        .expect("manifest entry")])
        .expect("manifest"),
    ));

    for (feature, limits) in [
        (
            SandboxFeature::CpuLimit,
            SandboxResourceLimits {
                cpu_seconds: Some(1),
                ..SandboxResourceLimits::default()
            },
        ),
        (
            SandboxFeature::MemoryLimit,
            SandboxResourceLimits {
                memory_bytes: Some(1),
                ..SandboxResourceLimits::default()
            },
        ),
        (
            SandboxFeature::DiskLimit,
            SandboxResourceLimits {
                disk_bytes: Some(1),
                ..SandboxResourceLimits::default()
            },
        ),
        (
            SandboxFeature::ProcessLimit,
            SandboxResourceLimits {
                processes: Some(1),
                ..SandboxResourceLimits::default()
            },
        ),
        (
            SandboxFeature::OpenFileLimit,
            SandboxResourceLimits {
                open_files: Some(1),
                ..SandboxResourceLimits::default()
            },
        ),
        (
            SandboxFeature::SessionTimeLimit,
            SandboxResourceLimits {
                session_time: Some(Duration::from_secs(1)),
                ..SandboxResourceLimits::default()
            },
        ),
        (
            SandboxFeature::OutboundByteLimit,
            SandboxResourceLimits {
                outbound_bytes: Some(1),
                ..SandboxResourceLimits::default()
            },
        ),
        (
            SandboxFeature::CostLimit,
            SandboxResourceLimits {
                cost_micros: Some(1),
                ..SandboxResourceLimits::default()
            },
        ),
    ] {
        requests.push((
            feature,
            policy(sample, SandboxNetworkPolicy::Closed, limits),
            empty(),
        ));
    }

    requests.push((
        SandboxFeature::Persistence,
        policy(
            sample,
            SandboxNetworkPolicy::Closed,
            SandboxResourceLimits::default(),
        )
        .with_session_state(true, false),
        empty(),
    ));
    requests.push((
        SandboxFeature::Snapshot,
        policy(
            sample,
            SandboxNetworkPolicy::Closed,
            SandboxResourceLimits::default(),
        )
        .with_session_state(false, true),
        empty(),
    ));
    requests
}

fn policy(
    sample: &Sample,
    network: SandboxNetworkPolicy,
    limits: SandboxResourceLimits,
) -> SandboxPolicy {
    let root = sample.root().clone();
    SandboxPolicy::new(
        SandboxMode::Off,
        [SandboxFilesystemRule::new(
            &root,
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("workspace rule")],
        root,
        network,
        limits,
    )
    .expect("compatibility policy")
}
