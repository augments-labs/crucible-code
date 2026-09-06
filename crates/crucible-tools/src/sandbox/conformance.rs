//! What an adapter must answer before crucible will run a command through it.
//!
//! A backend states what it can hold, and the whole sandbox rests on that
//! statement being exact in both directions: a claim it cannot keep would put
//! untrusted code behind a fence that is not there, and a claim it withholds
//! would refuse work for no reason. Nothing in the type system can check
//! either, because both are agreements between a table of words and a kernel.
//!
//! So this suite asks. For every feature a policy can name it builds the
//! smallest policy that needs exactly that feature and offers it, then reads
//! the claim and the answer together: a claimed feature must be accepted, and
//! a disclaimed one must be refused by name before any session exists. The
//! features no policy can name — a terminal, direct file operations, resuming
//! somebody else's session — are reported as stated rather than exercised,
//! because saying a claim was tested when nothing could test it is the same
//! failure this module exists to catch.
//!
//! The answers are grouped into the families that are worth conforming in
//! separately. A backend that isolates a filesystem perfectly and cannot bound
//! a single byte of egress is not partially conformant; it holds one family and
//! not another, and an adapter is selected against the family a policy needs.
//!
//! This is published rather than kept in the test tree because the backends
//! that have to pass it are not all in this repository. A container, a remote
//! executor or another operating system's adapter can depend on this crate,
//! run [`Conformance::audit`] over a directory it owns, and get the same
//! verdicts against the same table.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use crucible_core::{
    Ancestry, SandboxBackendIdentity, SandboxCapabilities, SandboxCapability, SandboxError,
    SandboxFeature, SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
    SandboxId, SandboxManifest, SandboxManifestEntry, SandboxNetworkEndpoint, SandboxNetworkPolicy,
    SandboxNetworkProvenance, SandboxPolicy, SandboxRequest, SandboxResourceLimits, SandboxService,
    ToolId,
};

/// A family of claims a backend is conformant in, or is not, on its own.
///
/// The families are the ones a caller chooses a backend by. Refusing egress
/// entirely and constraining it to an exact host are different mechanisms with
/// different failure modes, so they are asked about separately even though both
/// are about the network; a ceiling on money is separate from a ceiling on
/// memory for the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SandboxClaim {
    /// The kernel boundary itself, including refusing the network outright.
    Isolation,
    /// Staging bounded inert data into the workspace before a command runs.
    Materialization,
    /// Reaching an exact endpoint, and accounting for what crosses.
    Network,
    /// Ceilings on time, memory, storage, processes, files and output.
    Resources,
    /// A terminal and direct file operations through the session.
    Terminal,
    /// Sessions, snapshots and resumption that outlive one command.
    Persistence,
    /// Retained lifecycle facts and measured usage.
    Accounting,
    /// A ceiling on what a backend is allowed to spend.
    Cost,
}

impl SandboxClaim {
    /// Every family, in the order a report prints them.
    pub const ALL: [Self; 8] = [
        Self::Isolation,
        Self::Materialization,
        Self::Network,
        Self::Resources,
        Self::Terminal,
        Self::Persistence,
        Self::Accounting,
        Self::Cost,
    ];

    /// Stable report and diagnostic spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Isolation => "isolation",
            Self::Materialization => "materialization",
            Self::Network => "network",
            Self::Resources => "resources",
            Self::Terminal => "terminal",
            Self::Persistence => "persistence",
            Self::Accounting => "accounting",
            Self::Cost => "cost",
        }
    }

    /// The one family `feature` is judged in.
    ///
    /// Total and single-valued on purpose: a feature counted in two families
    /// would let a backend fail one and pass the other on the same evidence.
    #[must_use]
    pub const fn of(feature: SandboxFeature) -> Self {
        match feature {
            SandboxFeature::Filesystem
            | SandboxFeature::NetworkDeny
            | SandboxFeature::DescriptorIsolation
            | SandboxFeature::ProcessIsolation
            | SandboxFeature::KernelSurface
            | SandboxFeature::PrivilegeIsolation => Self::Isolation,
            SandboxFeature::Materialization => Self::Materialization,
            SandboxFeature::NetworkAllowlist | SandboxFeature::OutboundByteLimit => Self::Network,
            SandboxFeature::CpuLimit
            | SandboxFeature::MemoryLimit
            | SandboxFeature::DiskLimit
            | SandboxFeature::ProcessLimit
            | SandboxFeature::OpenFileLimit
            | SandboxFeature::CommandTimeLimit
            | SandboxFeature::SessionTimeLimit
            | SandboxFeature::OutputLimit
            | SandboxFeature::ConcurrencyLimit => Self::Resources,
            SandboxFeature::Pty | SandboxFeature::FileOperations => Self::Terminal,
            SandboxFeature::Persistence | SandboxFeature::Snapshot | SandboxFeature::Resume => {
                Self::Persistence
            }
            SandboxFeature::Audit | SandboxFeature::Usage => Self::Accounting,
            SandboxFeature::CostLimit => Self::Cost,
        }
    }
}

/// What one feature's claim and the backend's answer said together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Claimed, and a policy needing exactly it was prepared.
    Held,
    /// Disclaimed, and a policy needing it was refused by that name.
    Refused,
    /// Disclaimed, and a policy needing it was prepared anyway.
    Overclaimed,
    /// Claimed, and a policy needing no more than it was refused.
    Withheld,
    /// Refused for a feature the offered policy did not ask for.
    Misnamed {
        /// The feature the backend named instead.
        instead: SandboxFeature,
    },
    /// The offer never reached the claim: the backend failed for its own
    /// reasons, which says nothing about this feature either way.
    Unreached,
    /// Claimed, with nothing a policy can say to exercise it.
    Stated,
    /// Disclaimed, with nothing a policy can say to exercise it.
    Absent,
}

impl Verdict {
    /// Whether the claim and the answer contradict each other.
    ///
    /// An unreached offer is not a fault. It is a report about the host, and
    /// treating it as a failed claim would make a machine without a backend
    /// look like a backend that lies.
    #[must_use]
    pub const fn is_fault(self) -> bool {
        matches!(
            self,
            Self::Overclaimed | Self::Withheld | Self::Misnamed { .. }
        )
    }

    /// Stable report and diagnostic spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::Refused => "refused",
            Self::Overclaimed => "overclaimed",
            Self::Withheld => "withheld",
            Self::Misnamed { .. } => "misnamed",
            Self::Unreached => "unreached",
            Self::Stated => "stated",
            Self::Absent => "absent",
        }
    }
}

/// One feature, the claim it carries, and what the backend did with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finding {
    claim: SandboxClaim,
    feature: SandboxFeature,
    held: SandboxCapability,
    verdict: Verdict,
}

impl Finding {
    /// The family this finding is judged in.
    #[must_use]
    pub const fn claim(&self) -> SandboxClaim {
        self.claim
    }

    /// The feature that was asked about.
    #[must_use]
    pub const fn feature(&self) -> SandboxFeature {
        self.feature
    }

    /// What the backend's capability table said before anything was offered.
    #[must_use]
    pub const fn held(&self) -> SandboxCapability {
        self.held
    }

    /// What the claim and the answer said together.
    #[must_use]
    pub const fn verdict(&self) -> Verdict {
        self.verdict
    }
}

/// One backend's answers to the whole table.
#[derive(Debug, Clone)]
pub struct Conformance {
    backend: SandboxBackendIdentity,
    capabilities: SandboxCapabilities,
    findings: Vec<Finding>,
}

impl Conformance {
    /// Probes `service` and offers it one policy per nameable feature.
    ///
    /// `at` is a directory the caller owns and the policies are written
    /// against. Nothing is materialized and no command is started: each session
    /// is prepared to see whether it can be, and dropped. A backend that stages
    /// or spawns anything during preparation is failing a different contract
    /// than this one, and would be caught by its own lifecycle tests.
    ///
    /// # Errors
    ///
    /// The probe's own failure is returned as it stands. There is no backend to
    /// report on, and an empty matrix would read as one that holds nothing.
    pub fn audit(service: &dyn SandboxService, at: &Path) -> Result<Self, SandboxError> {
        let (backend, capabilities) = service.probe()?;
        // Each offer selects the backend that was just probed. An enabled
        // offer reaches the enforcing backend; a disabled offer reaches
        // compatibility. Mixing them would test another backend's claims.
        let enabled = capabilities.claim(SandboxFeature::Filesystem).is_enforced();
        // One offer covers the whole isolation family, because no policy field
        // names a PID namespace on its own; requiring confinement is the only
        // way to ask for any of them, and it asks for all of them at once.
        let confinement = offered(service, at, SandboxFeature::Filesystem, enabled);

        let mut findings = Vec::with_capacity(SandboxFeature::COUNT);
        for feature in SandboxFeature::ALL {
            let claim = SandboxClaim::of(feature);
            let held = capabilities.claim(feature);
            let alone;
            let answered = if claim == SandboxClaim::Isolation {
                confinement.as_ref()
            } else {
                alone = offered(service, at, feature, enabled);
                alone.as_ref()
            };
            // A confining offer carries the whole isolation family whatever
            // else it asks for, so a refusal naming one of them is honest and
            // leaves this feature untested rather than contradicted.
            let mut scope = Vec::with_capacity(CONFINEMENT.len() + 1);
            if enabled {
                scope.extend_from_slice(CONFINEMENT);
            }
            if !scope.contains(&feature) {
                scope.push(feature);
            }
            findings.push(Finding {
                claim,
                feature,
                held,
                verdict: judge(held, feature, &scope, answered),
            });
        }

        Ok(Self {
            backend,
            capabilities,
            findings,
        })
    }

    /// The backend the answers belong to.
    #[must_use]
    pub const fn backend(&self) -> &SandboxBackendIdentity {
        &self.backend
    }

    /// The claims as the backend stated them, before anything was offered.
    #[must_use]
    pub const fn capabilities(&self) -> &SandboxCapabilities {
        &self.capabilities
    }

    /// Every feature's finding, in the capability table's own order.
    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// The findings belonging to one family.
    pub fn within(&self, claim: SandboxClaim) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(move |finding| finding.claim == claim)
    }

    /// Every claim the backend contradicted.
    pub fn faults(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.verdict.is_fault())
    }

    /// Whether one family may be relied on: nothing in it contradicted itself.
    ///
    /// This says the family is exact, not that it is supported. A backend that
    /// disclaims every network feature and refuses every network policy holds
    /// the family, and a caller reads the claims to learn it can do nothing.
    #[must_use]
    pub fn holds(&self, claim: SandboxClaim) -> bool {
        !self.within(claim).any(|finding| finding.verdict.is_fault())
    }

    /// The whole result as a table, for an adapter author to read or paste.
    #[must_use]
    pub fn report(&self) -> String {
        let mut said = String::new();
        let _ = writeln!(
            said,
            "{} {}, {}",
            self.backend.id(),
            self.backend.version(),
            self.backend.provenance().as_str()
        );
        for claim in SandboxClaim::ALL {
            let _ = writeln!(
                said,
                "\n{} — {}",
                claim.as_str(),
                if self.holds(claim) {
                    "exact"
                } else {
                    "contradicted"
                }
            );
            for finding in self.within(claim) {
                let _ = writeln!(
                    said,
                    "  {:<21} {:<12} {}",
                    finding.feature.as_str(),
                    finding.held.as_str(),
                    finding.verdict.as_str()
                );
            }
        }
        said
    }
}

/// The features one policy requiring confinement asks for at once.
///
/// No policy field names a PID namespace or a dropped capability set on its
/// own, so the whole family is offered together and a refusal may honestly name
/// any member of it.
const CONFINEMENT: &[SandboxFeature] = &[
    SandboxFeature::Filesystem,
    SandboxFeature::NetworkDeny,
    SandboxFeature::DescriptorIsolation,
    SandboxFeature::ProcessIsolation,
    SandboxFeature::KernelSurface,
    SandboxFeature::PrivilegeIsolation,
];

/// Reads a claim and the answer to the matching offer as one statement.
///
/// `scope` is what the offer actually asked for. A refusal naming a sibling in
/// that scope leaves this feature untested rather than contradicted: the offer
/// died before the backend had to say anything about it.
fn judge(
    held: SandboxCapability,
    feature: SandboxFeature,
    scope: &[SandboxFeature],
    answered: Option<&Result<(), SandboxError>>,
) -> Verdict {
    // Nothing to read against. Either the feature is one no policy can name, or
    // this module could not build the offer it meant to make; both leave the
    // claim untested, and neither is evidence about the backend.
    let Some(answered) = answered else {
        return match held {
            SandboxCapability::Unsupported => Verdict::Absent,
            _ => Verdict::Stated,
        };
    };
    let named = match answered {
        Ok(()) => {
            return match held {
                SandboxCapability::Unsupported => Verdict::Overclaimed,
                _ => Verdict::Held,
            };
        }
        Err(SandboxError::Unsupported { feature: named }) => *named,
        Err(_) => return Verdict::Unreached,
    };
    if !scope.contains(&named) {
        return Verdict::Misnamed { instead: named };
    }
    if named != feature {
        return Verdict::Unreached;
    }
    match held {
        SandboxCapability::Unsupported => Verdict::Refused,
        _ => Verdict::Withheld,
    }
}

/// Offers the smallest policy that needs `feature`, and drops what it gets.
///
/// `None` where no such policy can be written: the features a session offers
/// rather than a policy requires, and — because every fixture here is built
/// from constants — a bug in this module, which surfaces as an untested claim
/// rather than as a verdict nothing earned.
fn offered(
    service: &dyn SandboxService,
    at: &Path,
    feature: SandboxFeature,
    enabled: bool,
) -> Option<Result<(), SandboxError>> {
    let (policy, manifest) = asking(at, feature, enabled)?;
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new(format!("conformance-{}", feature.as_str())),
        policy,
        manifest,
    );
    Some(service.prepare(request).map(drop))
}

/// The smallest policy and manifest that require `feature` of a backend.
///
/// `None` where nothing a policy can say requires it. Those features are asked
/// of a session rather than of a policy, and a bare policy offered in their
/// name would be accepted by every backend, which would read as five claims
/// kept and prove nothing about any of them.
fn asking(
    at: &Path,
    feature: SandboxFeature,
    enabled: bool,
) -> Option<(SandboxPolicy, SandboxManifest)> {
    let mut manifest = SandboxManifest::empty();
    let mut network = SandboxNetworkPolicy::Closed;
    let mut limits = SandboxResourceLimits::default();
    let mut persistent = false;
    let mut snapshots = false;

    match feature {
        SandboxFeature::Filesystem
        | SandboxFeature::NetworkDeny
        | SandboxFeature::DescriptorIsolation
        | SandboxFeature::ProcessIsolation
        | SandboxFeature::KernelSurface
        | SandboxFeature::PrivilegeIsolation => {}
        SandboxFeature::Materialization => {
            let entry = SandboxManifestEntry::file(
                "conformance.txt",
                Box::<[u8]>::from(&b"inert"[..]),
                SandboxFilesystemProvenance::Manifest,
            )
            .ok()?;
            manifest = SandboxManifest::new([entry]).ok()?;
        }
        SandboxFeature::NetworkAllowlist => {
            let endpoint =
                SandboxNetworkEndpoint::new("192.0.2.1", 443, SandboxNetworkProvenance::User)
                    .ok()?;
            network = SandboxNetworkPolicy::exact([endpoint], false, false).ok()?;
        }
        SandboxFeature::CpuLimit => limits.cpu_seconds = Some(1),
        SandboxFeature::MemoryLimit => limits.memory_bytes = Some(1),
        SandboxFeature::DiskLimit => limits.disk_bytes = Some(1),
        SandboxFeature::ProcessLimit => limits.processes = Some(1),
        SandboxFeature::OpenFileLimit => limits.open_files = Some(1),
        SandboxFeature::CommandTimeLimit => limits.command_time = Some(Duration::from_secs(1)),
        SandboxFeature::SessionTimeLimit => limits.session_time = Some(Duration::from_secs(1)),
        SandboxFeature::OutboundByteLimit => limits.outbound_bytes = Some(1),
        SandboxFeature::OutputLimit => limits.output_bytes = Some(1),
        SandboxFeature::ConcurrencyLimit => limits.concurrent_commands = Some(1),
        SandboxFeature::CostLimit => limits.cost_micros = Some(1),
        SandboxFeature::Persistence => persistent = true,
        SandboxFeature::Snapshot => snapshots = true,
        SandboxFeature::Pty
        | SandboxFeature::FileOperations
        | SandboxFeature::Resume
        | SandboxFeature::Audit
        | SandboxFeature::Usage => return None,
    }

    let root = at.to_path_buf();
    let rule = SandboxFilesystemRule::new(
        &root,
        SandboxFilesystemAccess::ReadWrite,
        SandboxFilesystemProvenance::Workspace,
    )
    .ok()?;
    let policy = SandboxPolicy::new(enabled, [rule], root, network, limits)
        .ok()?
        .with_session_state(persistent, snapshots);
    Some((policy, manifest))
}

#[cfg(test)]
mod tests;
