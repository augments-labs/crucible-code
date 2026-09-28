//! Reducing a negotiated boundary to the records a store keeps.
//!
//! The inspections, plans and lifecycle facts are stored values now: they
//! belong to `crucible-storage`, beside the journal and the checkpoint that
//! hold them, and `crucible-sandbox` re-exports them. What stays here is the
//! half that needs a policy: reducing a policy and a manifest to the redacted
//! summary a record keeps, and refusing an inspection that contradicts the
//! policy it claims to describe.
//!
//! These are functions rather than inherent constructors because a stored value
//! cannot carry an inherent impl in the crate that stores it.

use crucible_storage::{
    MAX_SANDBOX_BACKEND_WORD_BYTES, SandboxBackendIdentity, SandboxCapabilities, SandboxCleanup,
    SandboxInspection, SandboxNetworkInspection, SandboxPlanInspection, SandboxRootInspection,
};

use super::{SandboxError, SandboxManifest, SandboxPolicy, SandboxRequest};

/// The domain-separated identity of one reach's path.
///
/// A record keeps this instead of the path, so a stored inspection can be
/// compared after the machine it described is gone without naming anything on
/// it. What the digest separates is the label: the same bytes under two
/// labels are two identities, and the same bytes under one label are one. The
/// path goes in as the caller spelled it, so one reach spelled two ways
/// carries two identities for it.
fn path_identity(label: &[u8], path: &std::path::Path) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    digest.update(b"crucible-sandbox-inspection-path-v1\0");
    digest.update(label);
    digest.update([0]);
    digest.update(path.as_os_str().as_encoded_bytes());
    digest.finalize().into()
}

/// The redacted summary one policy and manifest reduce to.
///
/// Every reach becomes a domain-separated identity rather than a path, the
/// network becomes a closed marker or three counts and a flag rather than a
/// host pattern, and the command filter becomes a digest: what a record keeps
/// is what can be compared after the process is gone without naming anything
/// on the machine.
#[must_use]
pub fn plan_inspection(
    policy: &SandboxPolicy,
    manifest: &SandboxManifest,
) -> SandboxPlanInspection {
    let roots = policy
        .filesystem()
        .iter()
        .map(|rule| {
            SandboxRootInspection::new(
                path_identity(b"root", rule.path()),
                rule.access(),
                rule.provenance(),
            )
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let network = match policy.network() {
        super::SandboxNetworkPolicy::Closed => SandboxNetworkInspection::Closed,
        super::SandboxNetworkPolicy::Domains(policy) => SandboxNetworkInspection::Domains {
            allowed: policy.allowed().len(),
            denied: policy.denied().len(),
            local_binding: policy.allow_local_binding(),
            unix_sockets: policy.unix_sockets().len(),
        },
    };
    SandboxPlanInspection::new(
        policy.enabled(),
        roots,
        path_identity(b"working-directory", policy.working_directory()),
        network,
        policy.limits(),
        policy.commands().digest(),
        policy.unreadable_patterns().len(),
        policy.persistent(),
        policy.snapshots(),
        manifest.entries().len(),
    )
}

/// A bounded inspection report for one already-negotiated boundary.
///
/// # Errors
///
/// Disable-reason text is bounded, and a report may call itself confined only
/// when the essential kernel boundaries are all enforced.
#[allow(clippy::too_many_arguments)]
pub fn inspection(
    id: crucible_types::SandboxId,
    backend: SandboxBackendIdentity,
    capabilities: SandboxCapabilities,
    policy: &SandboxPolicy,
    manifest: &SandboxManifest,
    confined: bool,
    disabled_reason: Option<impl Into<Box<str>>>,
    cleanup: SandboxCleanup,
) -> Result<SandboxInspection, SandboxError> {
    build(
        id,
        backend,
        capabilities,
        policy,
        policy,
        manifest,
        confined,
        disabled_reason,
        cleanup,
    )
}

/// A confined report for one request, preserving policy narrowing.
///
/// # Errors
///
/// The effective policy's essential kernel boundaries must all be enforced.
pub fn confined_inspection(
    backend: SandboxBackendIdentity,
    capabilities: SandboxCapabilities,
    request: &SandboxRequest,
) -> Result<SandboxInspection, SandboxError> {
    build(
        request.id(),
        backend,
        capabilities,
        request.requested_policy(),
        request.policy(),
        request.manifest(),
        true,
        None::<Box<str>>,
        SandboxCleanup::Pending,
    )
}

/// An explicitly unconfined compatibility report for one request.
///
/// # Errors
///
/// Confinement must be disabled and the reason must be non-empty and bounded.
pub fn unconfined_inspection(
    backend: SandboxBackendIdentity,
    capabilities: SandboxCapabilities,
    request: &SandboxRequest,
    disabled_reason: impl Into<Box<str>>,
) -> Result<SandboxInspection, SandboxError> {
    build(
        request.id(),
        backend,
        capabilities,
        request.requested_policy(),
        request.policy(),
        request.manifest(),
        false,
        Some(disabled_reason),
        SandboxCleanup::Pending,
    )
}

#[allow(clippy::too_many_arguments)]
fn build(
    id: crucible_types::SandboxId,
    backend: SandboxBackendIdentity,
    capabilities: SandboxCapabilities,
    requested_policy: &SandboxPolicy,
    policy: &SandboxPolicy,
    manifest: &SandboxManifest,
    confined: bool,
    disabled_reason: Option<impl Into<Box<str>>>,
    cleanup: SandboxCleanup,
) -> Result<SandboxInspection, SandboxError> {
    let disabled_reason = disabled_reason.map(Into::into);
    if disabled_reason
        .as_ref()
        .is_some_and(|text| text.is_empty() || text.len() > MAX_SANDBOX_BACKEND_WORD_BYTES)
        || confined == disabled_reason.is_some()
        || policy.enabled() != confined
    {
        return Err(SandboxError::InvalidInspection);
    }
    // Which boundaries a claim of confinement rests on is decided once, where
    // the record lives, and this function reaches that decision through the
    // constructor below instead of restating it: a second list here could come
    // to disagree with the one the record is refused by, and the test below is
    // what compares the two. The record's rule reads the network boundary from
    // the plan the record carries rather than from the policy above, so this
    // module could not restate it faithfully even if it wanted to.
    SandboxInspection::new(
        id,
        backend,
        capabilities,
        plan_inspection(requested_policy, manifest),
        plan_inspection(policy, manifest),
        requested_policy.digest(),
        policy.digest(),
        manifest.digest(),
        confined,
        disabled_reason,
        cleanup,
    )
    .map_err(|_| SandboxError::InvalidInspection)
}

#[cfg(test)]
mod tests {
    //! The boundaries a claim of confinement rests on, as this crate's own path
    //! meets them.

    use super::*;

    // The vocabulary of the rule this module obeys rather than restates, which
    // is why the import list at the top of this file no longer names it.
    use crucible_storage::{SandboxCapabilities, SandboxCapability, SandboxFeature};
    use crucible_types::{Ancestry, SandboxId, ToolId};

    use crate::policy::SandboxFilesystemRule;
    use crate::{
        SandboxBackendId, SandboxBackendProvenance, SandboxDomainPattern, SandboxDomainPolicy,
        SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxNetworkPolicy,
        SandboxNetworkProvenance, SandboxResourceLimits,
    };

    /// The one readable root these fixtures grant, in the spelling of the target
    /// that compiles them.
    ///
    /// A rule is refused unless its path is absolute, and absolute is a leading
    /// slash on Unix and a drive on Windows, so one literal cannot serve both:
    /// the other platform refuses it in the shared fixture before either test
    /// reaches an assertion. Gating the module instead is the cheaper answer and
    /// the wrong one, because these two tests are the only witness for the rule
    /// that a report may call itself confined, and a Windows backend still
    /// negotiates against that rule. The spelling carries nothing either
    /// assertion depends on: what is checked is which capabilities a report may
    /// claim, not where the reach is.
    #[cfg(windows)]
    const WORKSPACE: &str = r"C:\workspace";
    #[cfg(not(windows))]
    const WORKSPACE: &str = "/workspace";

    /// The boundaries that do not depend on the network shape.
    ///
    /// Written out here rather than read from the rule this crate obeys,
    /// because a test that reads the list it is checking agrees with a wrong
    /// one. The sixth is whichever boundary the network shape names, so each
    /// shape below states its own.
    const FIXED: [SandboxFeature; 5] = [
        SandboxFeature::Filesystem,
        SandboxFeature::DescriptorIsolation,
        SandboxFeature::ProcessIsolation,
        SandboxFeature::KernelSurface,
        SandboxFeature::PrivilegeIsolation,
    ];

    /// One policy per network shape, beside the boundary that shape demands.
    fn shapes() -> [(&'static str, SandboxNetworkPolicy, SandboxFeature); 2] {
        let mediated = SandboxDomainPolicy::new(
            [SandboxDomainPattern::new("private.example").expect("domain pattern")],
            [],
            false,
            [],
            SandboxNetworkProvenance::User,
        )
        .expect("domain policy");
        [
            (
                "a closed network",
                SandboxNetworkPolicy::Closed,
                SandboxFeature::NetworkDeny,
            ),
            (
                "a mediated network",
                SandboxNetworkPolicy::Domains(mediated),
                SandboxFeature::NetworkAllowlist,
            ),
        ]
    }

    fn request(network: SandboxNetworkPolicy) -> SandboxRequest {
        SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("witness"),
            SandboxPolicy::new(
                true,
                [SandboxFilesystemRule::new(
                    WORKSPACE,
                    SandboxFilesystemAccess::ReadWrite,
                    SandboxFilesystemProvenance::Workspace,
                )
                .expect("rule")],
                WORKSPACE,
                network,
                SandboxResourceLimits::default(),
            )
            .expect("policy"),
            SandboxManifest::empty(),
        )
    }

    fn identity() -> SandboxBackendIdentity {
        SandboxBackendIdentity::new(
            SandboxBackendId::new("witness").expect("backend id"),
            "1",
            SandboxBackendProvenance::System,
            Some([1; 32]),
        )
        .expect("backend identity")
    }

    /// Every boundary a confinement depends on, so one can be watched instead.
    fn enforcing(network_boundary: SandboxFeature) -> SandboxCapabilities {
        FIXED
            .into_iter()
            .chain([network_boundary])
            .fold(SandboxCapabilities::none(), |claims, feature| {
                claims.with(feature, SandboxCapability::Enforced)
            })
    }

    #[test]
    fn a_confined_report_is_refused_when_one_boundary_it_claims_is_only_watched() {
        for (shape, network, network_boundary) in shapes() {
            let request = request(network);
            for watched in [network_boundary].into_iter().chain(FIXED) {
                let capabilities =
                    enforcing(network_boundary).with(watched, SandboxCapability::Observed);
                assert!(
                    matches!(
                        confined_inspection(identity(), capabilities, &request),
                        Err(SandboxError::InvalidInspection)
                    ),
                    "{shape} is accepted while {} is only watched",
                    watched.as_str()
                );
            }
        }
    }

    #[test]
    fn a_confined_report_is_written_when_every_boundary_it_claims_is_enforced() {
        for (shape, network, network_boundary) in shapes() {
            let report =
                confined_inspection(identity(), enforcing(network_boundary), &request(network))
                    .unwrap_or_else(|error| {
                        panic!("{shape} is refused with every boundary enforced: {error}")
                    });
            assert!(report.confined(), "{shape} is written as unconfined");
        }
    }
}
