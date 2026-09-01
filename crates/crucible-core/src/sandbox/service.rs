//! Host-owned sandbox lifecycle and process interfaces.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use crate::{Ancestry, SandboxId, ToolId};
use sha2::{Digest as _, Sha256};

use super::audit::SandboxAudit;
use super::capability::{
    MAX_SANDBOX_BACKEND_WORD_BYTES, SandboxBackendIdentity, SandboxCapabilities, SandboxCapability,
    SandboxFeature,
};
use super::guardrail::SandboxCommandStage;
use super::manifest::SandboxManifest;
use super::policy::{
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxMode, SandboxNetworkPolicy,
    SandboxPolicy, SandboxResourceLimits,
};

/// Maximum environment entries given to one command.
pub const MAX_SANDBOX_ENVIRONMENT_ENTRIES: usize = 128;

/// Maximum bytes retained in one environment name.
pub const MAX_SANDBOX_ENVIRONMENT_NAME_BYTES: usize = 128;

/// Maximum aggregate bytes in explicit environment values.
pub const MAX_SANDBOX_ENVIRONMENT_BYTES: usize = 128 * 1024;

/// Maximum command arguments passed through one launch.
pub const MAX_SANDBOX_COMMAND_ARGUMENTS: usize = 512;

/// Maximum aggregate encoded bytes in a command program and arguments.
pub const MAX_SANDBOX_COMMAND_BYTES: usize = 128 * 1024;

/// Minimal explicit command environment.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxEnvironment {
    entries: Box<[(Box<str>, OsString)]>,
}

impl SandboxEnvironment {
    /// Validates, sorts, and deduplicates explicit variables.
    ///
    /// # Errors
    ///
    /// Invalid/duplicate names, too many entries, or oversized retained values
    /// are rejected. The host environment is never consulted here.
    pub fn new<'a>(
        entries: impl IntoIterator<Item = (&'a str, &'a OsStr)>,
    ) -> Result<Self, SandboxError> {
        let mut entries: Vec<_> = entries
            .into_iter()
            .map(|(name, value)| (Box::<str>::from(name), value.to_owned()))
            .collect();
        if entries.len() > MAX_SANDBOX_ENVIRONMENT_ENTRIES {
            return Err(SandboxError::InvalidEnvironment);
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut previous: Option<&str> = None;
        let mut bytes = 0_usize;
        for (name, value) in &entries {
            if name.is_empty()
                || name.len() > MAX_SANDBOX_ENVIRONMENT_NAME_BYTES
                || name.contains('=')
                || name.chars().any(char::is_control)
                || previous == Some(name)
                || value.as_encoded_bytes().contains(&0)
            {
                return Err(SandboxError::InvalidEnvironment);
            }
            bytes = bytes
                .saturating_add(name.len())
                .saturating_add(value.as_encoded_bytes().len());
            previous = Some(name);
        }
        if bytes > MAX_SANDBOX_ENVIRONMENT_BYTES {
            return Err(SandboxError::InvalidEnvironment);
        }
        Ok(Self {
            entries: entries.into_boxed_slice(),
        })
    }

    /// An empty environment.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            entries: Box::new([]),
        }
    }

    /// Explicit variables, in deterministic name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &OsStr)> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_ref(), value.as_os_str()))
    }

    /// Whether no variable is projected.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for SandboxEnvironment {
    fn default() -> Self {
        Self::empty()
    }
}

impl std::fmt::Debug for SandboxEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.entries.iter().map(|(name, _)| (name, "[redacted]")))
            .finish()
    }
}

/// One command to start inside an already negotiated session.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxCommand {
    requested_program: PathBuf,
    requested_arguments: Box<[OsString]>,
    program: PathBuf,
    arguments: Box<[OsString]>,
    environment: SandboxEnvironment,
}

impl SandboxCommand {
    /// Validates an absolute program and bounded argument vector.
    ///
    /// The working directory comes from the immutable policy so a command
    /// cannot replace it after capability negotiation.
    ///
    /// # Errors
    ///
    /// Relative/non-normalized programs or oversized argument sets are
    /// rejected before a backend is called.
    pub fn new(
        program: impl Into<PathBuf>,
        arguments: impl IntoIterator<Item = OsString>,
        environment: SandboxEnvironment,
    ) -> Result<Self, SandboxError> {
        let (program, arguments) = command_image(program.into(), arguments)?;
        Ok(Self {
            requested_program: program.clone(),
            requested_arguments: arguments.clone(),
            program,
            arguments,
            environment,
        })
    }

    /// Applies one trusted host transformation while retaining the requested
    /// image for the first guardrail decision.
    ///
    /// # Errors
    ///
    /// The transformed image has the same absolute-path and byte/count bounds
    /// as the requested image.
    pub fn transformed(
        mut self,
        program: impl Into<PathBuf>,
        arguments: impl IntoIterator<Item = OsString>,
    ) -> Result<Self, SandboxError> {
        let (program, arguments) = command_image(program.into(), arguments)?;
        self.program = program;
        self.arguments = arguments;
        Ok(self)
    }

    /// Absolute executable selected after trusted transformation.
    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Effective opaque argument vector.
    #[must_use]
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    /// Explicit command environment.
    #[must_use]
    pub const fn environment(&self) -> &SandboxEnvironment {
        &self.environment
    }

    pub(super) fn image(&self, stage: SandboxCommandStage) -> (&Path, &[OsString]) {
        match stage {
            SandboxCommandStage::Requested => (&self.requested_program, &self.requested_arguments),
            SandboxCommandStage::Effective => (&self.program, &self.arguments),
        }
    }
}

fn command_image(
    program: PathBuf,
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<(PathBuf, Box<[OsString]>), SandboxError> {
    if !program.is_absolute()
        || program.as_os_str().as_encoded_bytes().contains(&0)
        || program
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(SandboxError::InvalidCommand);
    }
    let arguments: Vec<_> = arguments.into_iter().collect();
    if arguments
        .iter()
        .any(|argument| argument.as_encoded_bytes().contains(&0))
    {
        return Err(SandboxError::InvalidCommand);
    }
    let bytes = arguments.iter().fold(
        program.as_os_str().as_encoded_bytes().len(),
        |total, argument| total.saturating_add(argument.as_encoded_bytes().len()),
    );
    if arguments.len() > MAX_SANDBOX_COMMAND_ARGUMENTS || bytes > MAX_SANDBOX_COMMAND_BYTES {
        return Err(SandboxError::InvalidCommand);
    }
    Ok((program, arguments.into_boxed_slice()))
}

impl std::fmt::Debug for SandboxCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxCommand")
            .field("program", &"[absolute executable]")
            .field(
                "arguments",
                &format_args!("[{} redacted]", self.arguments.len()),
            )
            .field("environment", &self.environment)
            .field(
                "transformed",
                &(self.requested_program != self.program
                    || self.requested_arguments != self.arguments),
            )
            .finish()
    }
}

/// One immutable prepare request with execution attribution.
#[derive(Debug, Clone)]
pub struct SandboxRequest {
    id: SandboxId,
    ancestry: Ancestry,
    call: ToolId,
    policy: SandboxPolicy,
    manifest: SandboxManifest,
    audit: SandboxAudit,
}

impl SandboxRequest {
    /// Creates a request. This performs no backend or filesystem side effect.
    #[must_use]
    pub fn new(
        id: SandboxId,
        ancestry: Ancestry,
        call: ToolId,
        policy: SandboxPolicy,
        manifest: SandboxManifest,
    ) -> Self {
        let audit = SandboxAudit::new(ancestry, call.clone());
        Self {
            id,
            ancestry,
            call,
            policy,
            manifest,
            audit,
        }
    }

    /// Uses the host-created fixed-attribution audit collector for this call.
    ///
    /// # Errors
    ///
    /// A collector minted for another ancestry or tool call is refused before
    /// a backend can emit a misattributed lifecycle fact.
    pub fn with_audit(mut self, audit: SandboxAudit) -> Result<Self, SandboxError> {
        if !audit.belongs_to(self.ancestry, &self.call) {
            return Err(super::audit::SandboxAuditError::AttributionMismatch.into());
        }
        self.audit = audit;
        Ok(self)
    }

    /// Stable lifecycle identity.
    #[must_use]
    pub const fn id(&self) -> SandboxId {
        self.id
    }

    /// Run ancestry for audit attribution.
    #[must_use]
    pub const fn ancestry(&self) -> Ancestry {
        self.ancestry
    }

    /// Tool call identity.
    #[must_use]
    pub const fn call(&self) -> &ToolId {
        &self.call
    }

    /// Immutable effective policy.
    #[must_use]
    pub const fn policy(&self) -> &SandboxPolicy {
        &self.policy
    }

    /// Bounded inert manifest.
    #[must_use]
    pub const fn manifest(&self) -> &SandboxManifest {
        &self.manifest
    }

    /// Bounded lifecycle fact collector.
    #[must_use]
    pub const fn audit(&self) -> &SandboxAudit {
        &self.audit
    }

    /// Verifies exact support before a service may materialize or spawn.
    ///
    /// # Errors
    ///
    /// Every explicit hard feature must be reported as enforced in every mode.
    /// `degraded` and `off` relax only the documented baseline kernel boundary;
    /// they do not turn requested limits or session semantics into telemetry.
    pub fn negotiate(&self, capabilities: &SandboxCapabilities) -> Result<(), SandboxError> {
        for (feature, minimum) in capability_requirements(&self.policy, &self.manifest) {
            if capabilities.claim(feature) < minimum {
                return Err(SandboxError::Unsupported { feature });
            }
        }
        Ok(())
    }
}

fn capability_requirements(
    policy: &SandboxPolicy,
    manifest: &SandboxManifest,
) -> Vec<(SandboxFeature, SandboxCapability)> {
    let enforced = SandboxCapability::Enforced;
    let mut features = Vec::with_capacity(SandboxFeature::COUNT);
    if policy.mode() == SandboxMode::Required {
        features.extend([
            (SandboxFeature::Filesystem, enforced),
            (SandboxFeature::DescriptorIsolation, enforced),
            (SandboxFeature::ProcessIsolation, enforced),
            (SandboxFeature::KernelSurface, enforced),
            (SandboxFeature::PrivilegeIsolation, enforced),
            (
                match policy.network() {
                    SandboxNetworkPolicy::Closed => SandboxFeature::NetworkDeny,
                    SandboxNetworkPolicy::Exact { .. } => SandboxFeature::NetworkAllowlist,
                },
                enforced,
            ),
        ]);
    } else if matches!(policy.network(), SandboxNetworkPolicy::Exact { .. }) {
        // Compatibility may deliberately expose the ordinary host network, but
        // it can never pretend that broad reach is an exact endpoint grant.
        features.push((SandboxFeature::NetworkAllowlist, enforced));
    }
    if !manifest.is_empty() {
        features.push((SandboxFeature::Materialization, enforced));
    }
    let limits = policy.limits();
    for (present, feature) in [
        (limits.cpu_seconds.is_some(), SandboxFeature::CpuLimit),
        (limits.memory_bytes.is_some(), SandboxFeature::MemoryLimit),
        (limits.disk_bytes.is_some(), SandboxFeature::DiskLimit),
        (limits.processes.is_some(), SandboxFeature::ProcessLimit),
        (limits.open_files.is_some(), SandboxFeature::OpenFileLimit),
        (
            limits.command_time.is_some(),
            SandboxFeature::CommandTimeLimit,
        ),
        (
            limits.session_time.is_some(),
            SandboxFeature::SessionTimeLimit,
        ),
        (
            limits.outbound_bytes.is_some(),
            SandboxFeature::OutboundByteLimit,
        ),
        (limits.output_bytes.is_some(), SandboxFeature::OutputLimit),
        (
            limits.concurrent_commands.is_some(),
            SandboxFeature::ConcurrencyLimit,
        ),
        (limits.cost_micros.is_some(), SandboxFeature::CostLimit),
    ] {
        if present {
            features.push((feature, enforced));
        }
    }
    if policy.persistent() {
        features.push((SandboxFeature::Persistence, enforced));
    }
    if policy.snapshots() {
        features.push((SandboxFeature::Snapshot, enforced));
    }
    features.push((SandboxFeature::Audit, enforced));
    features.push((SandboxFeature::Usage, SandboxCapability::Observed));
    features
}

/// One redacted effective filesystem reach in an inspection report.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxRootInspection {
    identity: [u8; 32],
    access: SandboxFilesystemAccess,
    provenance: SandboxFilesystemProvenance,
}

impl SandboxRootInspection {
    /// Domain-separated identity of the canonical path, never the path itself.
    #[must_use]
    pub const fn identity(&self) -> [u8; 32] {
        self.identity
    }

    /// Effective access granted at this reach.
    #[must_use]
    pub const fn access(&self) -> SandboxFilesystemAccess {
        self.access
    }

    /// Authority source for the reach.
    #[must_use]
    pub const fn provenance(&self) -> SandboxFilesystemProvenance {
        self.provenance
    }
}

impl std::fmt::Debug for SandboxRootInspection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxRootInspection")
            .field("identity", &"[sha256]")
            .field("access", &self.access)
            .field("provenance", &self.provenance)
            .finish()
    }
}

/// Redacted network shape from the immutable effective plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxNetworkInspection {
    /// No network reach.
    Closed,
    /// Exact endpoint policy; endpoint spellings remain behind a digest.
    Exact {
        /// Number of canonical endpoints.
        endpoints: usize,
        /// Whether DNS was requested through the enforcing mechanism.
        dns: bool,
        /// Whether bounded forwarding was requested.
        forwarding: bool,
    },
}

impl SandboxNetworkInspection {
    /// Stable network-state spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Exact { .. } => "exact",
        }
    }

    /// Number of exact endpoint grants.
    #[must_use]
    pub const fn endpoints(self) -> usize {
        match self {
            Self::Closed => 0,
            Self::Exact { endpoints, .. } => endpoints,
        }
    }

    /// Whether DNS was requested.
    #[must_use]
    pub const fn dns(self) -> bool {
        match self {
            Self::Closed => false,
            Self::Exact { dns, .. } => dns,
        }
    }

    /// Whether forwarding was requested.
    #[must_use]
    pub const fn forwarding(self) -> bool {
        match self {
            Self::Closed => false,
            Self::Exact { forwarding, .. } => forwarding,
        }
    }
}

/// Bounded redacted summary of the immutable effective policy and manifest.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxPlanInspection {
    mode: SandboxMode,
    roots: Box<[SandboxRootInspection]>,
    working_directory: [u8; 32],
    network: SandboxNetworkInspection,
    limits: SandboxResourceLimits,
    command_policy: [u8; 32],
    persistent: bool,
    snapshots: bool,
    manifest_entries: usize,
}

impl SandboxPlanInspection {
    fn new(policy: &SandboxPolicy, manifest: &SandboxManifest) -> Self {
        let roots = policy
            .filesystem()
            .iter()
            .map(|rule| SandboxRootInspection {
                identity: path_identity(b"root", rule.path()),
                access: rule.access(),
                provenance: rule.provenance(),
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let network = match policy.network() {
            SandboxNetworkPolicy::Closed => SandboxNetworkInspection::Closed,
            SandboxNetworkPolicy::Exact { .. } => SandboxNetworkInspection::Exact {
                endpoints: policy.network().endpoints().len(),
                dns: policy.network().dns(),
                forwarding: policy.network().forwarding(),
            },
        };
        Self {
            mode: policy.mode(),
            roots,
            working_directory: path_identity(b"working-directory", policy.working_directory()),
            network,
            limits: policy.limits(),
            command_policy: policy.commands().digest(),
            persistent: policy.persistent(),
            snapshots: policy.snapshots(),
            manifest_entries: manifest.entries().len(),
        }
    }

    /// Required/degraded/off effective selection.
    #[must_use]
    pub const fn mode(&self) -> SandboxMode {
        self.mode
    }

    /// Canonical reaches, represented only by domain-separated identities.
    #[must_use]
    pub fn roots(&self) -> &[SandboxRootInspection] {
        &self.roots
    }

    /// Domain-separated identity of the effective working directory.
    #[must_use]
    pub const fn working_directory(&self) -> [u8; 32] {
        self.working_directory
    }

    /// Effective redacted network shape.
    #[must_use]
    pub const fn network(&self) -> SandboxNetworkInspection {
        self.network
    }

    /// Effective hard/observed ceiling requests.
    #[must_use]
    pub const fn limits(&self) -> SandboxResourceLimits {
        self.limits
    }

    /// Domain-separated command-filter identity.
    #[must_use]
    pub const fn command_policy(&self) -> [u8; 32] {
        self.command_policy
    }

    /// Whether persistent session state was requested.
    #[must_use]
    pub const fn persistent(&self) -> bool {
        self.persistent
    }

    /// Whether snapshots were requested.
    #[must_use]
    pub const fn snapshots(&self) -> bool {
        self.snapshots
    }

    /// Bounded manifest entry count.
    #[must_use]
    pub const fn manifest_entries(&self) -> usize {
        self.manifest_entries
    }
}

impl std::fmt::Debug for SandboxPlanInspection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxPlanInspection")
            .field("mode", &self.mode)
            .field("roots", &self.roots)
            .field("working_directory", &"[sha256]")
            .field("network", &self.network)
            .field("limits", &self.limits)
            .field("command_policy", &"[sha256]")
            .field("persistent", &self.persistent)
            .field("snapshots", &self.snapshots)
            .field("manifest_entries", &self.manifest_entries)
            .finish()
    }
}

fn path_identity(label: &[u8], path: &Path) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"crucible-sandbox-inspection-path-v1\0");
    digest.update(label);
    digest.update([0]);
    digest.update(path.as_os_str().as_encoded_bytes());
    digest.finalize().into()
}

/// Cleanup state retained without backend error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxCleanup {
    /// A session or process still owns resources.
    Pending,
    /// Every process, pipe, mount, stage, proxy, and lease is gone.
    Complete,
    /// Cleanup was attempted but could not be fully confirmed.
    Failed,
}

/// Redacted immutable inspection snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxInspection {
    id: SandboxId,
    backend: SandboxBackendIdentity,
    capabilities: SandboxCapabilities,
    plan: SandboxPlanInspection,
    policy_digest: [u8; 32],
    manifest_digest: [u8; 32],
    confined: bool,
    degradation: Option<Box<str>>,
    cleanup: SandboxCleanup,
}

impl SandboxInspection {
    /// Builds a bounded inspection report.
    ///
    /// # Errors
    ///
    /// Degradation text is bounded and a report may call itself confined only
    /// when the essential kernel boundaries are all enforced.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: SandboxId,
        backend: SandboxBackendIdentity,
        capabilities: SandboxCapabilities,
        policy: &SandboxPolicy,
        manifest: &SandboxManifest,
        confined: bool,
        degradation: Option<impl Into<Box<str>>>,
        cleanup: SandboxCleanup,
    ) -> Result<Self, SandboxError> {
        let degradation = degradation.map(Into::into);
        if degradation
            .as_ref()
            .is_some_and(|text| text.is_empty() || text.len() > MAX_SANDBOX_BACKEND_WORD_BYTES)
        {
            return Err(SandboxError::InvalidInspection);
        }
        let network = match policy.network() {
            SandboxNetworkPolicy::Closed => SandboxFeature::NetworkDeny,
            SandboxNetworkPolicy::Exact { .. } => SandboxFeature::NetworkAllowlist,
        };
        let essential = [
            SandboxFeature::Filesystem,
            network,
            SandboxFeature::DescriptorIsolation,
            SandboxFeature::ProcessIsolation,
            SandboxFeature::KernelSurface,
            SandboxFeature::PrivilegeIsolation,
        ];
        if confined
            && essential
                .into_iter()
                .any(|feature| capabilities.claim(feature) != SandboxCapability::Enforced)
        {
            return Err(SandboxError::InvalidInspection);
        }
        Ok(Self {
            id,
            backend,
            capabilities,
            plan: SandboxPlanInspection::new(policy, manifest),
            policy_digest: policy.digest(),
            manifest_digest: manifest.digest(),
            confined,
            degradation,
            cleanup,
        })
    }

    /// Stable lifecycle identity.
    #[must_use]
    pub const fn id(&self) -> SandboxId {
        self.id
    }

    /// Enforcing or compatibility backend identity.
    #[must_use]
    pub const fn backend(&self) -> &SandboxBackendIdentity {
        &self.backend
    }

    /// Exact capability snapshot used for negotiation.
    #[must_use]
    pub const fn capabilities(&self) -> &SandboxCapabilities {
        &self.capabilities
    }

    /// Bounded redacted effective plan.
    #[must_use]
    pub const fn plan(&self) -> &SandboxPlanInspection {
        &self.plan
    }

    /// Effective policy identity.
    #[must_use]
    pub const fn policy_digest(&self) -> [u8; 32] {
        self.policy_digest
    }

    /// Materialization plan/content identity.
    #[must_use]
    pub const fn manifest_digest(&self) -> [u8; 32] {
        self.manifest_digest
    }

    /// Whether this is an enforcing kernel boundary rather than compatibility.
    #[must_use]
    pub const fn confined(&self) -> bool {
        self.confined
    }

    /// Explicit reason for a user-authorized degradation.
    #[must_use]
    pub fn degradation(&self) -> Option<&str> {
        self.degradation.as_deref()
    }

    /// Last known cleanup outcome.
    #[must_use]
    pub const fn cleanup(&self) -> SandboxCleanup {
        self.cleanup
    }

    /// Returns a copy with the terminal cleanup result.
    #[must_use]
    pub const fn cleaned(mut self, cleanup: SandboxCleanup) -> Self {
        self.cleanup = cleanup;
        self
    }
}

/// Minimal redacted sandbox identity retained by an execution checkpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxCheckpoint {
    id: SandboxId,
    backend: SandboxBackendIdentity,
    capabilities: SandboxCapabilities,
    mode: SandboxMode,
    network: SandboxNetworkInspection,
    policy_digest: [u8; 32],
    manifest_digest: [u8; 32],
    confined: bool,
}

impl SandboxCheckpoint {
    /// Captures only bounded identity needed for resume revalidation.
    #[must_use]
    pub fn from_inspection(inspection: &SandboxInspection) -> Self {
        Self {
            id: inspection.id,
            backend: inspection.backend.clone(),
            capabilities: inspection.capabilities.clone(),
            mode: inspection.plan.mode,
            network: inspection.plan.network,
            policy_digest: inspection.policy_digest,
            manifest_digest: inspection.manifest_digest,
            confined: inspection.confined,
        }
    }

    /// Restores one typed checkpoint record from protected persistence.
    ///
    /// # Errors
    ///
    /// A record that calls itself confined without its exact essential network
    /// and kernel capabilities is refused.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        id: SandboxId,
        backend: SandboxBackendIdentity,
        capabilities: SandboxCapabilities,
        mode: SandboxMode,
        network: SandboxNetworkInspection,
        policy_digest: [u8; 32],
        manifest_digest: [u8; 32],
        confined: bool,
    ) -> Result<Self, SandboxError> {
        if matches!(
            network,
            SandboxNetworkInspection::Exact { endpoints: 0, .. }
        ) || network.endpoints() > super::policy::MAX_SANDBOX_NETWORK_ENDPOINTS
        {
            return Err(SandboxError::InvalidInspection);
        }
        let network_feature = match network {
            SandboxNetworkInspection::Closed => SandboxFeature::NetworkDeny,
            SandboxNetworkInspection::Exact { .. } => SandboxFeature::NetworkAllowlist,
        };
        if confined
            && [
                SandboxFeature::Filesystem,
                network_feature,
                SandboxFeature::DescriptorIsolation,
                SandboxFeature::ProcessIsolation,
                SandboxFeature::KernelSurface,
                SandboxFeature::PrivilegeIsolation,
            ]
            .into_iter()
            .any(|feature| capabilities.claim(feature) != SandboxCapability::Enforced)
        {
            return Err(SandboxError::InvalidInspection);
        }
        Ok(Self {
            id,
            backend,
            capabilities,
            mode,
            network,
            policy_digest,
            manifest_digest,
            confined,
        })
    }

    /// Original lifecycle identity.
    #[must_use]
    pub const fn id(&self) -> SandboxId {
        self.id
    }

    /// Exact backend identity used before interruption.
    #[must_use]
    pub const fn backend(&self) -> &SandboxBackendIdentity {
        &self.backend
    }

    /// Exact capability snapshot used before interruption.
    #[must_use]
    pub const fn capabilities(&self) -> &SandboxCapabilities {
        &self.capabilities
    }

    /// Effective mode before interruption.
    #[must_use]
    pub const fn mode(&self) -> SandboxMode {
        self.mode
    }

    /// Effective redacted network shape before interruption.
    #[must_use]
    pub const fn network(&self) -> SandboxNetworkInspection {
        self.network
    }

    /// Effective policy identity before interruption.
    #[must_use]
    pub const fn policy_digest(&self) -> [u8; 32] {
        self.policy_digest
    }

    /// Materialization identity before interruption.
    #[must_use]
    pub const fn manifest_digest(&self) -> [u8; 32] {
        self.manifest_digest
    }

    /// Whether the prior backend was an enforcing kernel boundary.
    #[must_use]
    pub const fn confined(&self) -> bool {
        self.confined
    }

    /// Whether fresh evidence preserves the exact backend/plan and every
    /// earlier capability claim.
    #[must_use]
    pub fn is_compatible_with(&self, live: &Self) -> bool {
        self.backend == live.backend
            && self.mode == live.mode
            && self.network == live.network
            && self.policy_digest == live.policy_digest
            && self.manifest_digest == live.manifest_digest
            && self.confined == live.confined
            && SandboxFeature::ALL
                .into_iter()
                .all(|feature| live.capabilities.claim(feature) >= self.capabilities.claim(feature))
    }
}

impl std::fmt::Debug for SandboxCheckpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxCheckpoint")
            .field("id", &self.id)
            .field("backend", &self.backend)
            .field("capabilities", &self.capabilities)
            .field("mode", &self.mode)
            .field("network", &self.network)
            .field("policy_digest", &"[sha256]")
            .field("manifest_digest", &"[sha256]")
            .field("confined", &self.confined)
            .finish()
    }
}

/// Bounded per-command/session accounting without raw paths or command text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SandboxUsage {
    /// Wall time measured by the host supervisor.
    pub wall_time: Duration,
    /// CPU time where the backend can report it.
    pub cpu_time: Option<Duration>,
    /// Peak memory bytes where known.
    pub peak_memory_bytes: Option<u64>,
    /// Ephemeral-storage bytes where known.
    pub disk_bytes: Option<u64>,
    /// Outbound bytes where networking is supported.
    pub outbound_bytes: Option<u64>,
    /// Raw captured output bytes before retention elision.
    pub output_bytes: u64,
    /// Backend cost in caller-defined micros where applicable.
    pub cost_micros: Option<u64>,
}

/// A hard command ceiling crossed while the backend owned the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxViolation {
    /// The command outlived its wall-clock deadline.
    CommandTime,
    /// The command produced more captured output than its shared stream budget.
    Output,
}

/// Result of one non-blocking output read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxRead {
    /// Bytes were copied into the caller's buffer.
    Bytes(usize),
    /// Some bytes were retained and the rest were consumed past the hard
    /// aggregate stdout/stderr ceiling.
    Limited {
        /// Prefix bytes available in the caller's buffer.
        retained: usize,
        /// Raw bytes consumed but not retained.
        discarded: usize,
    },
    /// The writer is live but no bytes are ready.
    Pending,
    /// Every writer has closed this stream.
    End,
}

/// A backend-owned non-blocking output stream.
pub trait SandboxOutput: Send {
    /// Reads currently available bytes without waiting indefinitely.
    ///
    /// # Errors
    ///
    /// The backend stream could not be read or inspected.
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead>;
}

/// A running sandbox command, including its complete cleanup scope.
pub trait SandboxProcess: Send {
    /// Takes stdout once.
    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>>;

    /// Takes stderr once.
    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>>;

    /// Non-blocking process status.
    ///
    /// # Errors
    ///
    /// The backend process could not be inspected or reaped.
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;

    /// Stops and reaps the complete process tree. Idempotent.
    ///
    /// # Errors
    ///
    /// The backend could not confirm that the complete owned scope was reaped.
    fn stop(&mut self) -> io::Result<()>;

    /// Redacted inspection snapshot.
    fn inspection(&self) -> &SandboxInspection;

    /// Current bounded usage snapshot.
    fn usage(&self) -> SandboxUsage;

    /// First hard resource violation observed by the host supervisor.
    fn violation(&self) -> Option<SandboxViolation>;
}

/// A prepared session. Dropping one must clean any completed staging.
pub trait SandboxSession: Send {
    /// Redacted negotiated state.
    fn inspection(&self) -> &SandboxInspection;

    /// Transactionally materializes the bounded manifest.
    ///
    /// # Errors
    ///
    /// No command may start after a partial or failed materialization.
    fn materialize(&mut self) -> Result<(), SandboxError>;

    /// Starts one governed command and transfers all cleanup ownership into the
    /// returned process handle.
    ///
    /// # Errors
    ///
    /// Refusal or launch failure occurs before untrusted code runs.
    fn start(
        self: Box<Self>,
        command: SandboxCommand,
    ) -> Result<Box<dyn SandboxProcess>, SandboxError>;
}

/// Backend-neutral confinement service.
pub trait SandboxService: Send + Sync {
    /// Exact identity/capabilities, probed without materialization or spawn.
    ///
    /// # Errors
    ///
    /// Unavailable or unsuitable backends return a typed diagnostic.
    fn probe(&self) -> Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>;

    /// Negotiates and prepares one session before side effects.
    ///
    /// # Errors
    ///
    /// Unsupported required features and backend failures are refused before
    /// materialization or spawn.
    fn prepare(&self, request: SandboxRequest) -> Result<Box<dyn SandboxSession>, SandboxError>;
}

impl std::fmt::Debug for dyn SandboxService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SandboxService(..)")
    }
}

impl std::fmt::Debug for dyn SandboxSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxSession")
            .field("inspection", self.inspection())
            .finish()
    }
}

impl std::fmt::Debug for dyn SandboxProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxProcess")
            .field("inspection", self.inspection())
            .field("usage", &self.usage())
            .finish()
    }
}

/// Why sandbox preparation, launch, or lifecycle failed.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// A capability-claimed audit fact could not be retained.
    #[error(transparent)]
    Audit(#[from] super::audit::SandboxAuditError),
    /// A required hard feature is unavailable.
    #[error("sandbox backend cannot enforce required feature {feature:?}")]
    Unsupported {
        /// Exact unsupported feature.
        feature: SandboxFeature,
    },
    /// No suitable backend was found or its probe failed.
    #[error("sandbox backend unavailable: {reason}")]
    BackendUnavailable {
        /// Bounded redacted diagnostic.
        reason: Box<str>,
    },
    /// Program/argument shape crossed the command boundary.
    #[error("sandbox command is invalid or exceeds its bound")]
    InvalidCommand,
    /// Environment projection crossed its structural or byte bound.
    #[error("sandbox environment is invalid or exceeds its bound")]
    InvalidEnvironment,
    /// A backend attempted an inaccurate or unbounded inspection report.
    #[error("sandbox inspection report is invalid")]
    InvalidInspection,
    /// A command guardrail refused the transformed invocation.
    #[error("sandbox command guardrail refused the invocation")]
    Guardrail,
    /// The bounded concurrent-session/command reservation is exhausted.
    #[error("sandbox concurrency ceiling is reached")]
    Concurrency,
    /// Manifest staging did not commit.
    #[error("sandbox materialization failed: {problem}")]
    Materialization {
        /// Redacted stage description.
        problem: Box<str>,
        /// Operating-system cause where applicable.
        #[source]
        source: Option<io::Error>,
    },
    /// The enforcing command could not start.
    #[error("sandbox launch failed")]
    Spawn(#[source] io::Error),
    /// A running process could not be controlled or reaped.
    #[error("sandbox lifecycle failed")]
    Lifecycle(#[source] io::Error),
}

impl SandboxError {
    /// Stable redacted category suitable for audit and diagnostics.
    #[must_use]
    pub const fn failure_kind(&self) -> super::audit::SandboxFailureKind {
        use super::audit::SandboxFailureKind;

        match self {
            Self::Unsupported { .. } => SandboxFailureKind::Unsupported,
            Self::BackendUnavailable { .. } => SandboxFailureKind::BackendUnavailable,
            Self::InvalidCommand | Self::InvalidEnvironment | Self::InvalidInspection => {
                SandboxFailureKind::InvalidInput
            }
            Self::Guardrail => SandboxFailureKind::Guardrail,
            Self::Concurrency => SandboxFailureKind::Concurrency,
            Self::Materialization { .. } => SandboxFailureKind::Materialization,
            Self::Spawn(_) => SandboxFailureKind::Spawn,
            Self::Lifecycle(_) => SandboxFailureKind::Lifecycle,
            Self::Audit(_) => SandboxFailureKind::Audit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::policy::{SandboxFilesystemProvenance, SandboxFilesystemRule};
    use crate::{SandboxBackendId, SandboxBackendProvenance, SandboxFilesystemAccess};

    fn policy() -> SandboxPolicy {
        SandboxPolicy::new(
            SandboxMode::Required,
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
        let capabilities = SandboxCapabilities::none()
            .with(SandboxFeature::Filesystem, SandboxCapability::Observed);

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
                identity,
                SandboxCapabilities::none(),
                &policy,
                &manifest,
                true,
                None::<Box<str>>,
                SandboxCleanup::Pending,
            )
            .is_err()
        );
    }

    #[test]
    fn confined_inspection_reports_the_exact_network_feature_and_redacts_reach() {
        let network = SandboxNetworkPolicy::exact(
            [crate::SandboxNetworkEndpoint::new("private.example", 443).unwrap()],
            true,
            false,
        )
        .unwrap();
        let policy = SandboxPolicy::new(
            SandboxMode::Required,
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
            SandboxBackendId::new("exact-proxy").unwrap(),
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
        .expect("exact network capability is sufficient");

        assert_eq!(inspection.plan().network().endpoints(), 1);
        assert!(inspection.plan().network().dns());
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
    fn environment_values_reject_interior_nul_bytes() {
        assert!(matches!(
            SandboxEnvironment::new([("TOKEN", OsStr::new("bad\0value"))]),
            Err(SandboxError::InvalidEnvironment)
        ));
    }

    #[test]
    fn compatibility_modes_still_refuse_explicit_features_the_backend_cannot_enforce() {
        for mode in [SandboxMode::Degraded, SandboxMode::Off] {
            let policy = policy()
                .with_mode(mode)
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
    }

    #[test]
    fn every_mode_requires_auditing_and_at_least_observed_usage() {
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("call"),
            policy().with_mode(SandboxMode::Off),
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
}
