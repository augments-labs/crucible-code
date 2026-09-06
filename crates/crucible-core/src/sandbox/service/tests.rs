//! What a sandbox command and its policy promise before anything spawns.

use super::*;
use crate::sandbox::policy::{SandboxFilesystemProvenance, SandboxFilesystemRule};
use crate::{
    SandboxBackendId, SandboxBackendProvenance, SandboxDomainPattern, SandboxDomainPolicy,
    SandboxFilesystemAccess, SandboxNetworkProvenance,
};

fn policy() -> SandboxPolicy {
    SandboxPolicy::new(
        true,
        [SandboxFilesystemRule::new(
            "/workspace",
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("rule")],
        "/workspace",
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("policy")
}

#[test]
fn required_negotiation_refuses_observed_or_missing_features() {
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("call"),
        policy(),
        SandboxManifest::empty(),
    );
    let capabilities =
        SandboxCapabilities::none().with(SandboxFeature::Filesystem, SandboxCapability::Observed);

    assert!(matches!(
        request.negotiate(&capabilities),
        Err(SandboxError::Unsupported {
            feature: SandboxFeature::Filesystem
        })
    ));
}

#[test]
fn unconfined_backend_cannot_label_itself_confined() {
    let identity = SandboxBackendIdentity::new(
        SandboxBackendId::new("compatibility").expect("id"),
        "1",
        SandboxBackendProvenance::Compatibility,
        None,
    )
    .expect("identity");
    let policy = policy();
    let manifest = SandboxManifest::empty();
    assert!(
        SandboxInspection::new(
            SandboxId::new(),
            identity.clone(),
            SandboxCapabilities::none(),
            &policy,
            &manifest,
            true,
            None::<Box<str>>,
            SandboxCleanup::Pending,
        )
        .is_err()
    );
    assert!(
        SandboxInspection::new(
            SandboxId::new(),
            identity.clone(),
            SandboxCapabilities::none(),
            &policy,
            &manifest,
            false,
            None::<Box<str>>,
            SandboxCleanup::Pending,
        )
        .is_err(),
        "an unconfined report requires an explicit disabled_reason reason"
    );
    assert!(
        SandboxInspection::new(
            SandboxId::new(),
            identity,
            SandboxCapabilities::none(),
            &policy,
            &manifest,
            true,
            Some("contradictory disabled_reason"),
            SandboxCleanup::Pending,
        )
        .is_err(),
        "a confined report cannot also claim disabled_reason"
    );
}

#[test]
fn enabled_policy_cannot_be_reported_as_unconfined() {
    let identity = SandboxBackendIdentity::new(
        SandboxBackendId::new("compatibility").unwrap(),
        "1",
        SandboxBackendProvenance::Compatibility,
        None,
    )
    .unwrap();
    assert!(
        SandboxInspection::new(
            SandboxId::new(),
            identity,
            SandboxCapabilities::none(),
            &policy(),
            &SandboxManifest::empty(),
            false,
            Some("sandbox disabled"),
            SandboxCleanup::Pending,
        )
        .is_err(),
        "enabled has no degraded fallback, including SDK inspection"
    );
}

#[test]
fn confined_inspection_reports_the_domain_network_feature_and_redacts_reach() {
    let network = SandboxNetworkPolicy::Domains(
        SandboxDomainPolicy::new(
            [SandboxDomainPattern::new("private.example").unwrap()],
            [],
            false,
            [],
            SandboxNetworkProvenance::User,
        )
        .unwrap(),
    );
    let policy = SandboxPolicy::new(
        true,
        [SandboxFilesystemRule::new(
            "/secret-workspace",
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::Workspace,
        )
        .unwrap()],
        "/secret-workspace",
        network,
        SandboxResourceLimits::default(),
    )
    .unwrap();
    let capabilities = [
        SandboxFeature::Filesystem,
        SandboxFeature::NetworkAllowlist,
        SandboxFeature::DescriptorIsolation,
        SandboxFeature::ProcessIsolation,
        SandboxFeature::KernelSurface,
        SandboxFeature::PrivilegeIsolation,
    ]
    .into_iter()
    .fold(SandboxCapabilities::none(), |claims, feature| {
        claims.with(feature, SandboxCapability::Enforced)
    });
    let identity = SandboxBackendIdentity::new(
        SandboxBackendId::new("domain-proxy").unwrap(),
        "1",
        SandboxBackendProvenance::Remote,
        None,
    )
    .unwrap();
    let inspection = SandboxInspection::new(
        SandboxId::new(),
        identity,
        capabilities,
        &policy,
        &SandboxManifest::empty(),
        true,
        None::<Box<str>>,
        SandboxCleanup::Pending,
    )
    .expect("domain network capability is sufficient");

    assert_eq!(
        inspection.plan().network(),
        SandboxNetworkInspection::Domains {
            allowed: 1,
            denied: 0,
            local_binding: false,
            unix_sockets: 0,
        }
    );
    let shown = format!("{inspection:?}");
    assert!(!shown.contains("secret-workspace"), "{shown}");
    assert!(!shown.contains("private.example"), "{shown}");
}

#[test]
fn environment_is_sorted_bounded_and_redacted() {
    let environment = SandboxEnvironment::new([
        ("Z", OsStr::new("last-secret")),
        ("A", OsStr::new("first-secret")),
    ])
    .expect("environment");
    let names: Vec<_> = environment.iter().map(|(name, _)| name).collect();
    assert_eq!(names, ["A", "Z"]);
    let shown = format!("{environment:?}");
    assert!(!shown.contains("secret"));
}

#[test]
fn credential_projections_are_typed_bounded_and_fully_redacted() {
    let handle =
        SandboxCredentialHandle::new("provider/openai/default", SandboxCredentialProvenance::User)
            .expect("credential handle");
    let credential = SandboxCredentialProjection::new(
        handle.clone(),
        "OPENAI_API_KEY",
        OsStr::new("secret-provider-value"),
    )
    .expect("credential projection");
    let environment =
        SandboxEnvironment::with_credentials([("LANG", OsStr::new("C"))], [credential])
            .expect("projected environment");

    assert_eq!(
        environment.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        ["LANG", "OPENAI_API_KEY"]
    );
    assert_eq!(environment.credentials().collect::<Vec<_>>(), [&handle]);
    let shown = format!("{environment:?} {handle:?}");
    assert!(!shown.contains("secret-provider-value"));
    assert!(!shown.contains("provider/openai/default"));
}

#[test]
fn credential_names_cannot_collide_with_literal_environment_entries() {
    let handle = SandboxCredentialHandle::new(
        "provider/openai/default",
        SandboxCredentialProvenance::Account,
    )
    .expect("credential handle");
    let credential = SandboxCredentialProjection::new(handle, "TOKEN", OsStr::new("credential"))
        .expect("credential projection");

    assert!(matches!(
        SandboxEnvironment::with_credentials([("TOKEN", OsStr::new("literal"))], [credential]),
        Err(SandboxError::InvalidEnvironment)
    ));
}

#[test]
fn command_images_reject_interior_nul_bytes() {
    assert!(matches!(
        SandboxCommand::new(
            "/bin/sh",
            [OsString::from("bad\0argument")],
            SandboxEnvironment::empty(),
        ),
        Err(SandboxError::InvalidCommand)
    ));
    assert!(matches!(
        SandboxCommand::new(
            OsString::from("/bin/bad\0program"),
            std::iter::empty(),
            SandboxEnvironment::empty(),
        ),
        Err(SandboxError::InvalidCommand)
    ));
}

#[test]
fn a_command_is_not_spoken_to_unless_it_asks_to_be() {
    let command = SandboxCommand::new("/bin/sh", std::iter::empty(), SandboxEnvironment::empty())
        .expect("a command");

    assert_eq!(
        command.speech(),
        SandboxSpeech::Closed,
        "every command crucible already builds must keep a closed input"
    );
    assert_eq!(command.spoken_to().speech(), SandboxSpeech::Held);
}

/// A trusted transformation replaces the image, not what the command is.
/// Losing the choice here would spawn a peer nobody can speak to, which
/// looks like an extension that never answered.
#[test]
fn transforming_a_command_keeps_whether_crucible_speaks_to_it() {
    let command = SandboxCommand::new("/bin/sh", std::iter::empty(), SandboxEnvironment::empty())
        .expect("a command")
        .spoken_to()
        .transformed("/usr/bin/env", [OsString::from("sh")])
        .expect("a transformed command");

    assert_eq!(command.speech(), SandboxSpeech::Held);
}

#[test]
fn environment_values_reject_interior_nul_bytes() {
    assert!(matches!(
        SandboxEnvironment::new([("TOKEN", OsStr::new("bad\0value"))]),
        Err(SandboxError::InvalidEnvironment)
    ));
}

#[test]
fn disabled_confinement_still_refuses_explicit_features_the_backend_cannot_enforce() {
    let policy = policy()
        .with_enabled(false)
        .with_limits(SandboxResourceLimits {
            memory_bytes: Some(1_024),
            ..SandboxResourceLimits::default()
        })
        .expect("bounded policy");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("call"),
        policy,
        SandboxManifest::empty(),
    );
    let capabilities = SandboxCapabilities::none()
        .with(SandboxFeature::Audit, SandboxCapability::Enforced)
        .with(SandboxFeature::Usage, SandboxCapability::Observed);

    assert!(matches!(
        request.negotiate(&capabilities),
        Err(SandboxError::Unsupported {
            feature: SandboxFeature::MemoryLimit
        })
    ));
}

#[test]
fn disabled_confinement_requires_auditing_and_at_least_observed_usage() {
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("call"),
        policy().with_enabled(false),
        SandboxManifest::empty(),
    );
    let no_audit =
        SandboxCapabilities::none().with(SandboxFeature::Usage, SandboxCapability::Observed);
    assert!(matches!(
        request.negotiate(&no_audit),
        Err(SandboxError::Unsupported {
            feature: SandboxFeature::Audit
        })
    ));

    let observed_audit = SandboxCapabilities::none()
        .with(SandboxFeature::Audit, SandboxCapability::Observed)
        .with(SandboxFeature::Usage, SandboxCapability::Observed);
    assert!(matches!(
        request.negotiate(&observed_audit),
        Err(SandboxError::Unsupported {
            feature: SandboxFeature::Audit
        })
    ));

    let exact = SandboxCapabilities::none()
        .with(SandboxFeature::Audit, SandboxCapability::Enforced)
        .with(SandboxFeature::Usage, SandboxCapability::Observed);
    assert!(request.negotiate(&exact).is_ok());
}

#[test]
fn request_refuses_an_audit_collector_from_another_call() {
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("expected-call"),
        policy(),
        SandboxManifest::empty(),
    );
    let mismatched = SandboxAudit::new(Ancestry::new(), ToolId::new("other-call"));

    assert!(matches!(
        request.with_audit(mismatched),
        Err(SandboxError::Audit(
            crate::SandboxAuditError::AttributionMismatch
        ))
    ));
}

#[test]
fn restricted_requests_inspect_requested_and_effective_policy_separately() {
    let parent_pattern = crate::SandboxUnreadablePattern::new(
        "/workspace/**/*.env",
        SandboxFilesystemProvenance::Workspace,
    )
    .expect("parent pattern");
    let parent = policy()
        .with_unreadable_patterns([parent_pattern])
        .expect("parent policy");
    let requested = policy();
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("restricted"),
        requested,
        SandboxManifest::empty(),
    )
    .restricted_to(&parent)
    .expect("restricted request");
    assert_eq!(request.requested_policy().unreadable_patterns().len(), 0);
    assert_eq!(request.policy().unreadable_patterns().len(), 1);

    let capabilities = [
        SandboxFeature::Filesystem,
        SandboxFeature::NetworkDeny,
        SandboxFeature::DescriptorIsolation,
        SandboxFeature::ProcessIsolation,
        SandboxFeature::KernelSurface,
        SandboxFeature::PrivilegeIsolation,
        SandboxFeature::Audit,
    ]
    .into_iter()
    .fold(SandboxCapabilities::none(), |claims, feature| {
        claims.with(feature, SandboxCapability::Enforced)
    })
    .with(SandboxFeature::Usage, SandboxCapability::Observed);
    let identity = SandboxBackendIdentity::new(
        SandboxBackendId::new("restricted").expect("backend id"),
        "1",
        SandboxBackendProvenance::System,
        Some([1; 32]),
    )
    .expect("backend identity");
    let inspection = SandboxInspection::confined_for_request(identity, capabilities, &request)
        .expect("inspection");
    assert_eq!(inspection.requested_plan().unreadable_patterns(), 0);
    assert_eq!(inspection.plan().unreadable_patterns(), 1);
    assert_ne!(
        inspection.requested_policy_digest(),
        inspection.policy_digest()
    );
}
