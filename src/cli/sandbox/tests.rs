//! What the confinement report says, and what it must never say.

use crucible_core::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCapability, SandboxCleanup, SandboxError, SandboxFeature, SandboxId, SandboxInspection,
    SandboxManifest, SandboxMode, SandboxPolicy, SandboxResourceLimits,
};

use super::{Probe, report};
use crate::cli::sample::Sample;

/// A backend that holds everything a confined report has to rest on.
///
/// Built up from nothing rather than from a preset, so a test that wants one
/// claim weakened weakens exactly that claim and nothing travels with it.
fn holding() -> SandboxCapabilities {
    [
        SandboxFeature::Filesystem,
        SandboxFeature::NetworkDeny,
        SandboxFeature::DescriptorIsolation,
        SandboxFeature::ProcessIsolation,
        SandboxFeature::KernelSurface,
        SandboxFeature::PrivilegeIsolation,
    ]
    .into_iter()
    .fold(SandboxCapabilities::none(), |claims, feature| {
        claims.with(feature, SandboxCapability::Enforced)
    })
}

/// Somebody's build of something, named the way a real probe would name it.
fn backend() -> SandboxBackendIdentity {
    SandboxBackendIdentity::new(
        SandboxBackendId::new("a-backend").expect("a name"),
        "1.2.3",
        SandboxBackendProvenance::System,
        Some([0x5a; 32]),
    )
    .expect("an identity")
}

/// One report over `policy`, as the flag would have written it.
fn written(sample: &Sample, policy: &SandboxPolicy, capabilities: SandboxCapabilities) -> String {
    let inspection = SandboxInspection::new(
        SandboxId::new(),
        backend(),
        capabilities,
        policy,
        &SandboxManifest::empty(),
        true,
        None::<&str>,
        SandboxCleanup::Pending,
    )
    .expect("a report");

    report(&sample.root(), policy.mode(), &Probe::Prepared(&inspection))
}

#[test]
fn a_ceiling_is_never_printed_without_the_claim_it_rests_on() {
    let sample = Sample::new("sandbox-report-ceilings");
    let workspace = sample.workspace();
    let standard = SandboxPolicy::standard(&workspace).expect("policy");
    // Narrowed from what the workspace already states rather than assembled
    // beside it, because a policy may only ever be tightened and dropping a
    // ceiling this one holds would be the widening it refuses.
    let limits = SandboxResourceLimits {
        cpu_seconds: Some(120),
        output_bytes: Some(1 << 20),
        // Deliberately not a whole number of anything. A ceiling is a number
        // somebody wrote down, and one printed as "1 MiB" would be a different
        // number from the one that was configured.
        memory_bytes: Some(1_500_000),
        command_time: Some(std::time::Duration::from_secs(90)),
        ..standard.limits()
    };
    let policy = standard.with_limits(limits).expect("limits");

    // One backend that enforces the first, only watches the second, and cannot
    // do the third at all. All three ceilings are the same kind of number in
    // the policy, and the difference between them is the whole answer.
    let capabilities = holding()
        .with(SandboxFeature::CpuLimit, SandboxCapability::Enforced)
        .with(SandboxFeature::OutputLimit, SandboxCapability::Observed);
    let said = written(&sample, &policy, capabilities);

    let ends_with = |stated: &str, claimed: &str| {
        let line = said
            .lines()
            .find(|line| line.trim_start().starts_with(stated))
            .unwrap_or_else(|| panic!("no ceiling line for {stated}: {said}"));
        assert!(line.ends_with(claimed), "{line}");
    };

    // The one that is genuinely a ceiling carries no excuse after it, so the
    // qualified lines are the ones that stand out rather than the plain one.
    ends_with("cpu 2m", "enforced");
    ends_with(
        "1 MiB captured",
        "observed, so it is recorded rather than imposed",
    );
    ends_with(
        "90s per command",
        "unsupported, so this number does nothing",
    );
    ends_with(
        "memory 1500000 bytes",
        "unsupported, so this number does nothing",
    );
}

#[test]
fn fractional_time_ceilings_are_reported_without_rounding() {
    let sample = Sample::new("sandbox-report-fractional-time");
    let workspace = sample.workspace();
    for (span, expected) in [
        (std::time::Duration::from_nanos(1), "0.000000001s"),
        (std::time::Duration::from_millis(1500), "1.5s"),
        (std::time::Duration::new(60, 1000), "60.000001s"),
        (std::time::Duration::from_mins(1), "1m"),
        (
            std::time::Duration::new(u64::MAX, 999_999_999),
            "18446744073709551615.999999999s",
        ),
    ] {
        let standard = SandboxPolicy::standard(&workspace).expect("policy");
        let limits = SandboxResourceLimits {
            command_time: Some(span),
            session_time: Some(span),
            ..standard.limits()
        };
        let policy = standard.with_limits(limits).expect("limits");
        let capabilities = holding()
            .with(
                SandboxFeature::CommandTimeLimit,
                SandboxCapability::Enforced,
            )
            .with(
                SandboxFeature::SessionTimeLimit,
                SandboxCapability::Observed,
            );
        let said = written(&sample, &policy, capabilities);
        for (scope, claim) in [
            ("per command", "enforced"),
            (
                "per session",
                "observed, so it is recorded rather than imposed",
            ),
        ] {
            let line = said
                .lines()
                .find(|line| line.contains(scope))
                .expect("time ceiling");
            assert!(
                line.trim_start()
                    .starts_with(&format!("{expected} {scope}")),
                "{line}"
            );
            assert!(line.ends_with(claim), "{line}");
        }
    }
}

#[test]
fn nothing_but_the_directory_that_was_asked_about_reaches_the_report() {
    // A component no digest could contain by accident and no fixed spelling in
    // the report could collide with.
    let sample = Sample::new("secret-tenant-zzqq");
    let workspace = sample.workspace();
    let policy = SandboxPolicy::standard(&workspace).expect("policy");
    let said = written(&sample, &policy, holding());

    // Once, on the line that says which directory the question was about. The
    // roots below it are the same paths and are named by digest, so a report
    // pasted into an issue carries the reach without carrying the tree.
    assert_eq!(
        said.matches("secret-tenant-zzqq").count(),
        1,
        "a path escaped the redaction: {said}"
    );
    let header = said.lines().next().expect("a first line");
    assert!(header.contains("secret-tenant-zzqq"), "{header}");
    assert!(
        said.lines()
            .skip(1)
            .all(|line| !line.contains("secret-tenant-zzqq")),
        "{said}"
    );
}

#[test]
fn a_backend_that_would_not_take_the_policy_still_says_what_it_can_hold() {
    let sample = Sample::new("sandbox-report-refused");
    let identity = backend();
    // The refusal a policy requiring confinement meets on a machine whose
    // kernel will not give crucible its own namespaces.
    let capabilities = holding().with(
        SandboxFeature::ProcessIsolation,
        SandboxCapability::Unsupported,
    );
    let why = SandboxError::Unsupported {
        feature: SandboxFeature::ProcessIsolation,
    };
    let said = report(
        &sample.root(),
        SandboxMode::Required,
        &Probe::Refused {
            backend: &identity,
            capabilities: &capabilities,
            why: &why,
        },
    );

    // The matrix is the point of printing anything at all here: a refusal on
    // its own repeats what the failing command already said, and the line that
    // explains it is the one feature this backend says it cannot hold.
    assert!(said.contains("a-backend 1.2.3, system"), "{said}");
    assert!(said.contains("process_isolation     unsupported"), "{said}");
    assert!(said.contains("no command could be run here"), "{said}");
    // And nothing is claimed about a session that was never negotiated.
    assert!(!said.contains("what a command would run under"), "{said}");
    assert!(!said.contains("confined"), "{said}");
}

#[test]
fn nothing_answering_is_said_as_that_rather_than_as_an_empty_report() {
    let sample = Sample::new("sandbox-report-absent");
    let why = SandboxError::BackendUnavailable {
        reason: "nothing was installed".into(),
    };
    let said = report(&sample.root(), SandboxMode::Required, &Probe::Absent(&why));

    assert!(said.contains("no sandbox backend answered"), "{said}");
    assert!(said.contains("nothing was installed"), "{said}");
    // No matrix, because there is nobody whose claims those would be. An empty
    // one would read as a backend that holds nothing, which is a different and
    // much worse thing to be told.
    assert!(!said.contains("what this backend can hold"), "{said}");
}

#[test]
fn what_was_given_up_is_printed_whether_or_not_anything_was() {
    let sample = Sample::new("sandbox-report-degraded");
    let workspace = sample.workspace();
    let policy = SandboxPolicy::standard(&workspace)
        .expect("policy")
        .with_mode(SandboxMode::Degraded);

    let confined = written(&sample, &policy, holding());
    assert!(confined.contains("confined  yes"), "{confined}");
    assert!(confined.contains("gave up   nothing"), "{confined}");

    // The same report from a backend that could not confine. The reason is the
    // backend's own sentence and is the only place the report says why the
    // answer above changed.
    let inspection = SandboxInspection::new(
        SandboxId::new(),
        backend(),
        SandboxCapabilities::none(),
        &policy,
        &SandboxManifest::empty(),
        false,
        Some("this kernel refuses user namespaces"),
        SandboxCleanup::Pending,
    )
    .expect("a report");
    let given = report(&sample.root(), policy.mode(), &Probe::Prepared(&inspection));

    assert!(given.contains("confined  no"), "{given}");
    assert!(
        given.contains("gave up   this kernel refuses user namespaces"),
        "{given}"
    );
}
