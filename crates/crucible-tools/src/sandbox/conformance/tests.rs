//! What the published suite asks, and what the local backends answer.

use std::time::Duration;

use crucible_core::{
    Ancestry, SandboxBackendProvenance, SandboxCapabilities, SandboxCapability, SandboxError,
    SandboxFeature, SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
    SandboxId, SandboxManifest, SandboxManifestEntry, SandboxMode, SandboxNetworkEndpoint,
    SandboxNetworkPolicy, SandboxNetworkProvenance, SandboxPolicy, SandboxRequest,
    SandboxResourceLimits, SandboxService, ToolId,
};

use super::{CONFINEMENT, Conformance, SandboxClaim, Verdict, asking, judge};
use crate::sample::{REQUIRE_ENFORCING_SANDBOX, Sample, skipped_without_enforcement};
use crate::sandbox::LocalSandbox;

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

const MACOS_ENFORCED: &[SandboxFeature] = &[
    SandboxFeature::Filesystem,
    SandboxFeature::NetworkDeny,
    SandboxFeature::DescriptorIsolation,
    SandboxFeature::ProcessIsolation,
    SandboxFeature::KernelSurface,
    SandboxFeature::PrivilegeIsolation,
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
    let (identity, capabilities) =
        crate::sandbox::local::compatibility_capabilities().expect("identity");
    assert_eq!(
        identity.provenance(),
        SandboxBackendProvenance::Compatibility
    );
    assert_exact_matrix(&capabilities, COMPATIBILITY_ENFORCED, OBSERVED);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_matrix_is_complete_and_claims_only_implemented_boundaries() {
    let capabilities = crate::sandbox::linux::declared_capabilities();
    // The process ceiling is the one cell this host decides rather than this
    // tree: below Linux 5.14 the count it is checked against is the person's
    // whole machine, so the backend declines to call it a sandbox boundary.
    // Either answer is exact, and neither may be the third thing a claim must
    // never be, which is absent from the matrix.
    let mut enforced = LINUX_ENFORCED.to_vec();
    if capabilities.claim(SandboxFeature::ProcessLimit) == SandboxCapability::Enforced {
        enforced.push(SandboxFeature::ProcessLimit);
    }
    assert_exact_matrix(&capabilities, &enforced, OBSERVED);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_matrix_is_complete_and_claims_only_implemented_boundaries() {
    assert_exact_matrix(
        &crate::sandbox::macos::declared_capabilities(),
        MACOS_ENFORCED,
        OBSERVED,
    );
}

#[test]
fn published_capability_matrix_matches_declared_backends() {
    let document = include_str!("../../../../../docs/security/sandboxing.md");
    for feature in SandboxFeature::ALL {
        // The one cell no single word states, because the answer belongs to the
        // reader's kernel rather than to this tree. It is still one row and
        // still exact; it is checked just below.
        if feature == SandboxFeature::ProcessLimit {
            continue;
        }
        let linux = expected(feature, LINUX_ENFORCED, OBSERVED).as_str();
        let macos = expected(feature, MACOS_ENFORCED, OBSERVED).as_str();
        let compatibility = expected(feature, COMPATIBILITY_ENFORCED, OBSERVED).as_str();
        let row = format!(
            "| `{}` | {linux} | {macos} | {compatibility} |",
            feature.as_str()
        );
        assert_eq!(
            document.matches(&row).count(),
            1,
            "missing or duplicate documented capability row: {row}"
        );
    }
    let compatibility = expected(
        SandboxFeature::ProcessLimit,
        COMPATIBILITY_ENFORCED,
        OBSERVED,
    )
    .as_str();
    let row = format!(
        "| `{}` | enforced on Linux 5.14 or newer | unsupported | {compatibility} |",
        SandboxFeature::ProcessLimit.as_str()
    );
    assert_eq!(
        document.matches(&row).count(),
        1,
        "missing or duplicate documented capability row: {row}"
    );
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

#[test]
fn every_feature_is_judged_in_exactly_one_family() {
    // Written out rather than counted. A family is what an adapter is selected
    // by, so a feature quietly moved between two of them would let one piece of
    // evidence pass a family it never covered, and a count would still add up.
    let expected: [(SandboxClaim, &[SandboxFeature]); 8] = [
        (
            SandboxClaim::Isolation,
            &[
                SandboxFeature::Filesystem,
                SandboxFeature::NetworkDeny,
                SandboxFeature::DescriptorIsolation,
                SandboxFeature::ProcessIsolation,
                SandboxFeature::KernelSurface,
                SandboxFeature::PrivilegeIsolation,
            ],
        ),
        (
            SandboxClaim::Materialization,
            &[SandboxFeature::Materialization],
        ),
        (
            SandboxClaim::Network,
            &[
                SandboxFeature::NetworkAllowlist,
                SandboxFeature::OutboundByteLimit,
            ],
        ),
        (
            SandboxClaim::Resources,
            &[
                SandboxFeature::CpuLimit,
                SandboxFeature::MemoryLimit,
                SandboxFeature::DiskLimit,
                SandboxFeature::ProcessLimit,
                SandboxFeature::OpenFileLimit,
                SandboxFeature::CommandTimeLimit,
                SandboxFeature::SessionTimeLimit,
                SandboxFeature::OutputLimit,
                SandboxFeature::ConcurrencyLimit,
            ],
        ),
        (
            SandboxClaim::Terminal,
            &[SandboxFeature::Pty, SandboxFeature::FileOperations],
        ),
        (
            SandboxClaim::Persistence,
            &[
                SandboxFeature::Persistence,
                SandboxFeature::Snapshot,
                SandboxFeature::Resume,
            ],
        ),
        (
            SandboxClaim::Accounting,
            &[SandboxFeature::Audit, SandboxFeature::Usage],
        ),
        (SandboxClaim::Cost, &[SandboxFeature::CostLimit]),
    ];

    let mut counted = 0;
    for (claim, members) in expected {
        let held: Vec<_> = SandboxFeature::ALL
            .into_iter()
            .filter(|feature| SandboxClaim::of(*feature) == claim)
            .collect();
        assert_eq!(held, members, "{}", claim.as_str());
        assert!(!members.is_empty(), "{}", claim.as_str());
        counted += members.len();
    }
    // And nothing is judged outside the eight, so no claim goes unread.
    assert_eq!(counted, SandboxFeature::COUNT);
    assert_eq!(SandboxClaim::ALL.len(), expected.len());
}

#[test]
fn the_local_backend_contradicts_no_claim_it_states() {
    let sample = Sample::new("sandbox-conformance-local");
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let audited = Conformance::audit(&service, sample.root()).expect("a probe");

    let faults: Vec<_> = audited
        .faults()
        .map(|finding| {
            format!(
                "{} {} {}",
                finding.feature().as_str(),
                finding.held().as_str(),
                finding.verdict().as_str()
            )
        })
        .collect();
    assert!(faults.is_empty(), "{faults:?}\n{}", audited.report());
    for claim in SandboxClaim::ALL {
        assert!(audited.holds(claim), "{}", claim.as_str());
    }
    // Whichever backend this host selected, the table is answered whole. A
    // suite that quietly skipped rows would say a backend conforms on the
    // strength of the questions it happened to ask.
    assert_eq!(audited.findings().len(), SandboxFeature::COUNT);
}

#[test]
fn a_claim_no_policy_can_reach_is_reported_as_untested_rather_than_kept() {
    let sample = Sample::new("sandbox-conformance-unreachable");
    // A terminal, direct file operations and resumption are asked of a session,
    // not written into a policy. Offering a bare policy in their name would be
    // accepted by every backend, and reading that as five claims kept is the
    // exact hollow pass this suite exists to refuse.
    for feature in [
        SandboxFeature::Pty,
        SandboxFeature::FileOperations,
        SandboxFeature::Resume,
        SandboxFeature::Audit,
        SandboxFeature::Usage,
    ] {
        assert!(
            asking(sample.root(), feature, SandboxMode::Required).is_none(),
            "{} was offered a policy that cannot require it",
            feature.as_str()
        );
        assert_eq!(
            judge(SandboxCapability::Enforced, feature, &[feature], None),
            Verdict::Stated
        );
        assert_eq!(
            judge(SandboxCapability::Unsupported, feature, &[feature], None),
            Verdict::Absent
        );
    }

    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let audited = Conformance::audit(&service, sample.root()).expect("a probe");
    let pty = audited
        .findings()
        .iter()
        .find(|finding| finding.feature() == SandboxFeature::Pty)
        .expect("a terminal row");
    assert!(matches!(pty.verdict(), Verdict::Stated | Verdict::Absent));
}

#[test]
fn a_claim_and_the_answer_to_it_are_read_together_or_not_at_all() {
    let held = SandboxCapability::Enforced;
    let gone = SandboxCapability::Unsupported;
    let refused = |feature| Err(SandboxError::Unsupported { feature });
    let one = &[SandboxFeature::CostLimit];

    // The two contradictions, which are the only reason the suite exists.
    assert_eq!(
        judge(gone, SandboxFeature::CostLimit, one, Some(&Ok(()))),
        Verdict::Overclaimed
    );
    assert_eq!(
        judge(
            held,
            SandboxFeature::CostLimit,
            one,
            Some(&refused(SandboxFeature::CostLimit))
        ),
        Verdict::Withheld
    );
    // And the two agreements.
    assert_eq!(
        judge(held, SandboxFeature::CostLimit, one, Some(&Ok(()))),
        Verdict::Held
    );
    assert_eq!(
        judge(
            gone,
            SandboxFeature::CostLimit,
            one,
            Some(&refused(SandboxFeature::CostLimit))
        ),
        Verdict::Refused
    );

    // A refusal naming something the offer never asked for is a fault of its
    // own: the backend answered a question nobody put to it, and reading that
    // as this feature's answer would credit or blame the wrong claim.
    assert_eq!(
        judge(
            gone,
            SandboxFeature::CostLimit,
            one,
            Some(&refused(SandboxFeature::DiskLimit))
        ),
        Verdict::Misnamed {
            instead: SandboxFeature::DiskLimit
        }
    );

    // Within the confinement offer a refusal may honestly name any member,
    // because one policy asks for all six. The siblings are then untested
    // rather than withheld — nothing was ever said about them.
    assert_eq!(
        judge(
            held,
            SandboxFeature::Filesystem,
            CONFINEMENT,
            Some(&refused(SandboxFeature::ProcessIsolation))
        ),
        Verdict::Unreached
    );
    assert_eq!(
        judge(
            held,
            SandboxFeature::ProcessIsolation,
            CONFINEMENT,
            Some(&refused(SandboxFeature::ProcessIsolation))
        ),
        Verdict::Withheld
    );

    // A backend that failed for its own reasons has said nothing about any
    // claim, and a host without one must not read as a host that is lied to.
    assert_eq!(
        judge(
            held,
            SandboxFeature::CostLimit,
            one,
            Some(&Err(SandboxError::BackendUnavailable {
                reason: "nothing was installed".into()
            }))
        ),
        Verdict::Unreached
    );
    assert!(!Verdict::Unreached.is_fault());
    assert!(Verdict::Overclaimed.is_fault());
    assert!(Verdict::Withheld.is_fault());
}

#[test]
fn the_report_separates_the_families_and_names_the_backend_they_belong_to() {
    let sample = Sample::new("sandbox-conformance-report");
    let service = LocalSandbox::new();
    if skipped_without_enforcement(&service) {
        return;
    }
    let audited = Conformance::audit(&service, sample.root()).expect("a probe");
    let said = audited.report();

    assert!(said.starts_with(audited.backend().id().as_str()), "{said}");
    for claim in SandboxClaim::ALL {
        assert!(said.contains(claim.as_str()), "{claim:?} missing: {said}");
    }
    for feature in SandboxFeature::ALL {
        assert!(
            said.contains(feature.as_str()),
            "{} missing: {said}",
            feature.as_str()
        );
    }
    // The directory the suite ran over is the caller's and is not the backend's
    // answer to anything, so it stays out of a report meant to be pasted
    // somewhere.
    assert!(!said.contains("sandbox-conformance-report"), "{said}");
}

#[test]
fn the_name_a_job_requires_a_backend_by_is_the_one_spelled_outside_this_crate() {
    // Two files outside this module spell it and neither can import it: the
    // workflow that sets it, and the integration test that reads it through a
    // crate boundary. Renaming the variable is fine; renaming it in one place
    // is what turns a required backend into a silent skip.
    assert_eq!(
        REQUIRE_ENFORCING_SANDBOX,
        "CRUCIBLE_TEST_REQUIRE_ENFORCING_SANDBOX"
    );
}
