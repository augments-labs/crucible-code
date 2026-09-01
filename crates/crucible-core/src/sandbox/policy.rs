//! Immutable authority and resource policy for one sandbox lifecycle.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::Workspace;
use sha2::{Digest, Sha256};

use super::guardrail::SandboxCommandPolicy;

/// Maximum filesystem rules in one effective policy.
pub const MAX_SANDBOX_FILESYSTEM_RULES: usize = 128;

/// Maximum bytes retained for one policy path.
pub const MAX_SANDBOX_PATH_BYTES: usize = 4096;

/// Maximum exact network endpoints in one policy.
pub const MAX_SANDBOX_NETWORK_ENDPOINTS: usize = 64;

/// Maximum bytes in one DNS name.
pub const MAX_SANDBOX_HOST_BYTES: usize = 253;

/// Whether the host must provide kernel confinement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    /// Refuse before materialization or spawn unless every requested hard
    /// feature has an enforcing backend.
    Required,
    /// Prefer confinement, but permit an explicitly user-selected and clearly
    /// reported compatibility backend when enforcement is unavailable.
    Degraded,
    /// Explicitly use the non-confining compatibility backend.
    Off,
}

impl SandboxMode {
    /// Whether `candidate` preserves or strengthens `parent`.
    #[must_use]
    pub const fn permits(self, candidate: Self) -> bool {
        candidate.strength() >= self.strength()
    }

    const fn strength(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Degraded => 1,
            Self::Required => 2,
        }
    }
}

/// Access granted to one exact filesystem subtree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxFilesystemAccess {
    /// The path must not be visible.
    Unreadable,
    /// Readable, but no mutation is allowed.
    ReadOnly,
    /// Readable and writable.
    ReadWrite,
    /// Readable but immutable even beneath a writable ancestor.
    Protected,
}

impl SandboxFilesystemAccess {
    const fn authority(self) -> u8 {
        match self {
            Self::Unreadable => 0,
            Self::ReadOnly | Self::Protected => 1,
            Self::ReadWrite => 2,
        }
    }
}

/// Why a filesystem rule is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxFilesystemProvenance {
    /// A root explicitly granted to the workspace.
    Workspace,
    /// A minimum host-owned runtime path.
    Runtime,
    /// A protected Crucible or repository metadata carve-out.
    ProtectedMetadata,
    /// A caller-requested narrowing.
    Descendant,
    /// An explicit manifest mount request.
    Manifest,
}

/// One absolute, bounded filesystem rule.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxFilesystemRule {
    path: PathBuf,
    access: SandboxFilesystemAccess,
    provenance: SandboxFilesystemProvenance,
}

impl SandboxFilesystemRule {
    /// Validates one already-resolved policy path.
    ///
    /// # Errors
    ///
    /// Relative, non-normalized, empty, or oversized paths are rejected.
    pub fn new(
        path: impl Into<PathBuf>,
        access: SandboxFilesystemAccess,
        provenance: SandboxFilesystemProvenance,
    ) -> Result<Self, SandboxPolicyError> {
        let path = path.into();
        validate_absolute_path(&path)?;
        Ok(Self {
            path,
            access,
            provenance,
        })
    }

    /// The normalized absolute path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Access at and beneath this path.
    #[must_use]
    pub const fn access(&self) -> SandboxFilesystemAccess {
        self.access
    }

    /// Authority source retained without a raw configuration document.
    #[must_use]
    pub const fn provenance(&self) -> SandboxFilesystemProvenance {
        self.provenance
    }
}

impl std::fmt::Debug for SandboxFilesystemRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxFilesystemRule")
            .field("path", &"[absolute path]")
            .field("access", &self.access)
            .field("provenance", &self.provenance)
            .finish()
    }
}

/// One exact outbound endpoint.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SandboxNetworkEndpoint {
    host: Box<str>,
    port: u16,
}

impl SandboxNetworkEndpoint {
    /// Validates a bounded DNS name or literal address and non-zero port.
    ///
    /// # Errors
    ///
    /// Empty/oversized/control-bearing hosts and port zero are rejected.
    pub fn new(host: impl Into<Box<str>>, port: u16) -> Result<Self, SandboxPolicyError> {
        let host = host.into();
        if host.is_empty()
            || host.len() > MAX_SANDBOX_HOST_BYTES
            || host.chars().any(char::is_control)
            || port == 0
        {
            return Err(SandboxPolicyError::InvalidEndpoint);
        }
        Ok(Self { host, port })
    }

    /// Requested host spelling.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Requested TCP port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }
}

/// Immutable outbound-network request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxNetworkPolicy {
    /// No host network namespace, inherited socket, loopback, Unix socket, DNS,
    /// metadata address, or forwarding authority.
    Closed,
    /// Exact proxy-enforced host/port endpoints. The initial Linux backend
    /// reports this unsupported rather than broadening it.
    Exact {
        /// Canonically sorted permitted endpoints.
        endpoints: Box<[SandboxNetworkEndpoint]>,
        /// Whether DNS resolution through the enforcing mechanism is allowed.
        dns: bool,
        /// Whether bounded inbound/outbound port forwarding is requested.
        forwarding: bool,
    },
}

impl SandboxNetworkPolicy {
    /// Validates, sorts, and deduplicates an exact network request.
    ///
    /// # Errors
    ///
    /// Empty or oversized endpoint sets are rejected.
    pub fn exact(
        endpoints: impl IntoIterator<Item = SandboxNetworkEndpoint>,
        dns: bool,
        forwarding: bool,
    ) -> Result<Self, SandboxPolicyError> {
        let mut endpoints: Vec<_> = endpoints.into_iter().collect();
        endpoints.sort();
        endpoints.dedup();
        if endpoints.is_empty() || endpoints.len() > MAX_SANDBOX_NETWORK_ENDPOINTS {
            return Err(SandboxPolicyError::InvalidEndpointCount);
        }
        Ok(Self::Exact {
            endpoints: endpoints.into_boxed_slice(),
            dns,
            forwarding,
        })
    }

    fn is_no_wider_than(&self, parent: &Self) -> bool {
        match (self, parent) {
            (Self::Closed, _) => true,
            (Self::Exact { .. }, Self::Closed) => false,
            (
                Self::Exact {
                    endpoints,
                    dns,
                    forwarding,
                },
                Self::Exact {
                    endpoints: parent_endpoints,
                    dns: parent_dns,
                    forwarding: parent_forwarding,
                },
            ) => {
                (!dns || *parent_dns)
                    && (!forwarding || *parent_forwarding)
                    && endpoints
                        .iter()
                        .all(|endpoint| parent_endpoints.contains(endpoint))
            }
        }
    }
}

/// Optional resource ceilings for one command/session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SandboxResourceLimits {
    /// CPU seconds.
    pub cpu_seconds: Option<u64>,
    /// Memory bytes.
    pub memory_bytes: Option<u64>,
    /// Ephemeral-storage bytes.
    pub disk_bytes: Option<u64>,
    /// Processes/PIDs.
    pub processes: Option<u64>,
    /// Open files.
    pub open_files: Option<u64>,
    /// Outbound bytes.
    pub outbound_bytes: Option<u64>,
    /// Captured output bytes.
    pub output_bytes: Option<u64>,
    /// Concurrent commands within this service.
    pub concurrent_commands: Option<u64>,
    /// Command wall time.
    pub command_time: Option<Duration>,
    /// Session wall time.
    pub session_time: Option<Duration>,
    /// Backend cost in caller-defined micros.
    pub cost_micros: Option<u64>,
}

impl SandboxResourceLimits {
    fn is_no_wider_than(self, parent: Self) -> bool {
        no_larger(self.cpu_seconds, parent.cpu_seconds)
            && no_larger(self.memory_bytes, parent.memory_bytes)
            && no_larger(self.disk_bytes, parent.disk_bytes)
            && no_larger(self.processes, parent.processes)
            && no_larger(self.open_files, parent.open_files)
            && no_larger(self.outbound_bytes, parent.outbound_bytes)
            && no_larger(self.output_bytes, parent.output_bytes)
            && no_larger(self.concurrent_commands, parent.concurrent_commands)
            && no_larger(self.command_time, parent.command_time)
            && no_larger(self.session_time, parent.session_time)
            && no_larger(self.cost_micros, parent.cost_micros)
    }

    fn valid(self) -> bool {
        [
            self.cpu_seconds,
            self.memory_bytes,
            self.disk_bytes,
            self.processes,
            self.open_files,
            self.outbound_bytes,
            self.output_bytes,
            self.concurrent_commands,
            self.cost_micros,
        ]
        .into_iter()
        .flatten()
        .all(|value| value > 0)
            && [self.command_time, self.session_time]
                .into_iter()
                .flatten()
                .all(|value| !value.is_zero())
    }
}

fn no_larger<T: PartialOrd>(candidate: Option<T>, parent: Option<T>) -> bool {
    match (candidate, parent) {
        (Some(candidate), Some(parent)) => candidate <= parent,
        (Some(_), None) | (None, None) => true,
        (None, Some(_)) => false,
    }
}

/// One fully resolved immutable policy.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxPolicy {
    mode: SandboxMode,
    filesystem: Box<[SandboxFilesystemRule]>,
    working_directory: PathBuf,
    network: SandboxNetworkPolicy,
    limits: SandboxResourceLimits,
    commands: SandboxCommandPolicy,
    persistent: bool,
    snapshots: bool,
}

impl SandboxPolicy {
    /// The standard coding policy for a workspace.
    ///
    /// Every explicitly reached root is writable. Repository and Crucible
    /// metadata beneath it is re-declared protected so a broad root mount
    /// cannot make those control planes writable.
    ///
    /// # Errors
    ///
    /// A host path outside the contract's bounds is rejected.
    pub fn standard(workspace: &Workspace) -> Result<Self, SandboxPolicyError> {
        let mut filesystem = Vec::new();
        for root in workspace.roots() {
            filesystem.push(SandboxFilesystemRule::new(
                root,
                SandboxFilesystemAccess::ReadWrite,
                SandboxFilesystemProvenance::Workspace,
            )?);
            for protected in [".git", ".agents", ".codex"] {
                let path = root.join(protected);
                if std::fs::symlink_metadata(&path).is_ok() {
                    filesystem.push(SandboxFilesystemRule::new(
                        path,
                        SandboxFilesystemAccess::Protected,
                        SandboxFilesystemProvenance::ProtectedMetadata,
                    )?);
                }
            }
        }
        Self::new(
            SandboxMode::Required,
            filesystem,
            workspace.root(),
            SandboxNetworkPolicy::Closed,
            SandboxResourceLimits::default(),
        )
    }

    /// Validates an effective policy assembled by the host.
    ///
    /// # Errors
    ///
    /// Invalid paths, duplicate/conflicting rules, an out-of-policy working
    /// directory, oversized rule sets, or zero resource ceilings are refused.
    pub fn new(
        mode: SandboxMode,
        filesystem: impl IntoIterator<Item = SandboxFilesystemRule>,
        working_directory: impl Into<PathBuf>,
        network: SandboxNetworkPolicy,
        limits: SandboxResourceLimits,
    ) -> Result<Self, SandboxPolicyError> {
        let mut filesystem: Vec<_> = filesystem.into_iter().collect();
        if filesystem.is_empty() || filesystem.len() > MAX_SANDBOX_FILESYSTEM_RULES {
            return Err(SandboxPolicyError::InvalidFilesystemRuleCount);
        }
        filesystem.sort_by(|left, right| left.path.cmp(&right.path));
        if filesystem.windows(2).any(|pair| {
            pair.first().is_some_and(|left| {
                pair.get(1)
                    .is_some_and(|right| left.path == right.path && left.access != right.access)
            })
        }) {
            return Err(SandboxPolicyError::ConflictingFilesystemRule);
        }
        filesystem.dedup();

        let working_directory = working_directory.into();
        validate_absolute_path(&working_directory)?;
        if !readable_by(&filesystem, &working_directory) {
            return Err(SandboxPolicyError::WorkingDirectoryOutsidePolicy);
        }
        if !limits.valid() {
            return Err(SandboxPolicyError::InvalidResourceLimit);
        }

        Ok(Self {
            mode,
            filesystem: filesystem.into_boxed_slice(),
            working_directory,
            network,
            limits,
            commands: SandboxCommandPolicy::allow_all(),
            persistent: false,
            snapshots: false,
        })
    }

    /// Refuses a descendant policy that widens any parent authority or ceiling.
    ///
    /// # Errors
    ///
    /// A weaker mode, new/wider filesystem grant, broader network policy,
    /// relaxed resource ceiling, or new persistence authority is rejected.
    pub fn restrict(parent: &Self, mut candidate: Self) -> Result<Self, SandboxPolicyError> {
        if !parent.mode.permits(candidate.mode) {
            return Err(SandboxPolicyError::WeakerMode);
        }
        if !candidate
            .filesystem
            .iter()
            .all(|rule| filesystem_rule_allowed(&parent.filesystem, rule))
        {
            return Err(SandboxPolicyError::FilesystemWidening);
        }
        if !candidate.network.is_no_wider_than(&parent.network) {
            return Err(SandboxPolicyError::NetworkWidening);
        }
        if !candidate.limits.is_no_wider_than(parent.limits) {
            return Err(SandboxPolicyError::ResourceWidening);
        }
        candidate.commands = SandboxCommandPolicy::intersect(&parent.commands, &candidate.commands)
            .map_err(|_| SandboxPolicyError::InvalidCommandPolicy)?;
        if (candidate.persistent && !parent.persistent)
            || (candidate.snapshots && !parent.snapshots)
        {
            return Err(SandboxPolicyError::SessionWidening);
        }
        Ok(candidate)
    }

    /// Returns a copy with command-scoped limits, preserving every other field.
    ///
    /// # Errors
    ///
    /// Zero ceilings are rejected.
    pub fn with_limits(
        mut self,
        limits: SandboxResourceLimits,
    ) -> Result<Self, SandboxPolicyError> {
        if !limits.valid() {
            return Err(SandboxPolicyError::InvalidResourceLimit);
        }
        self.limits = limits;
        Ok(self)
    }

    /// Returns a copy with one already-bounded top-level command filter.
    #[must_use]
    pub fn with_command_policy(mut self, commands: SandboxCommandPolicy) -> Self {
        self.commands = commands;
        self
    }

    /// Returns a copy under a host-authorized top-level mode.
    #[must_use]
    pub const fn with_mode(mut self, mode: SandboxMode) -> Self {
        self.mode = mode;
        self
    }

    /// Required/degraded/off selection.
    #[must_use]
    pub const fn mode(&self) -> SandboxMode {
        self.mode
    }

    /// Canonical ordered filesystem rules.
    #[must_use]
    pub fn filesystem(&self) -> &[SandboxFilesystemRule] {
        &self.filesystem
    }

    /// Working directory inside the granted view.
    #[must_use]
    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }

    /// Immutable network policy.
    #[must_use]
    pub const fn network(&self) -> &SandboxNetworkPolicy {
        &self.network
    }

    /// Resource ceilings.
    #[must_use]
    pub const fn limits(&self) -> SandboxResourceLimits {
        self.limits
    }

    /// Defense-in-depth command allow/deny filters.
    #[must_use]
    pub const fn commands(&self) -> &SandboxCommandPolicy {
        &self.commands
    }

    /// Whether durable session state was explicitly granted.
    #[must_use]
    pub const fn persistent(&self) -> bool {
        self.persistent
    }

    /// Whether snapshots were explicitly granted.
    #[must_use]
    pub const fn snapshots(&self) -> bool {
        self.snapshots
    }

    /// Whether this effective policy grants `access` throughout `path`.
    ///
    /// Manifest mounts and descendant requests use this to prove that an alias
    /// does not introduce a new host source or make an existing source more
    /// writable than its parent authority.
    #[must_use]
    pub fn permits_path(&self, path: &Path, access: SandboxFilesystemAccess) -> bool {
        validate_absolute_path(path).is_ok()
            && effective_rule(&self.filesystem, path).is_some_and(|granted| {
                access.authority() <= granted.access.authority()
                    && !(access == SandboxFilesystemAccess::ReadOnly
                        && granted.access == SandboxFilesystemAccess::Protected)
            })
            && self
                .filesystem
                .iter()
                .filter(|rule| rule.path.starts_with(path))
                .all(|rule| access.authority() <= rule.access.authority())
    }

    /// Domain-separated policy identity for bounded inspection/checkpoints.
    ///
    /// Paths and endpoint names contribute only through this digest; callers
    /// retain the typed effective policy in memory and do not need to persist
    /// its sensitive host spellings.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"crucible-sandbox-policy-v1\0");
        digest.update([self.mode as u8]);
        for rule in &self.filesystem {
            digest.update(rule.path.as_os_str().as_encoded_bytes());
            digest.update([0, rule.access as u8, rule.provenance as u8]);
        }
        digest.update(self.working_directory.as_os_str().as_encoded_bytes());
        match &self.network {
            SandboxNetworkPolicy::Closed => digest.update(b"\0closed"),
            SandboxNetworkPolicy::Exact {
                endpoints,
                dns,
                forwarding,
            } => {
                digest.update(b"\0exact");
                for endpoint in endpoints {
                    digest.update(endpoint.host.as_bytes());
                    digest.update([0]);
                    digest.update(endpoint.port.to_be_bytes());
                }
                digest.update([u8::from(*dns), u8::from(*forwarding)]);
            }
        }
        update_limits(&mut digest, self.limits);
        digest.update(self.commands.digest());
        digest.update([u8::from(self.persistent), u8::from(self.snapshots)]);
        digest.finalize().into()
    }
}

impl std::fmt::Debug for SandboxPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxPolicy")
            .field("mode", &self.mode)
            .field("filesystem_rules", &self.filesystem.len())
            .field("working_directory", &"[absolute path]")
            .field("network", &self.network)
            .field("limits", &self.limits)
            .field("commands", &self.commands)
            .field("persistent", &self.persistent)
            .field("snapshots", &self.snapshots)
            .finish()
    }
}

fn validate_absolute_path(path: &Path) -> Result<(), SandboxPolicyError> {
    if !path.is_absolute()
        || path.as_os_str().is_empty()
        || path.as_os_str().to_string_lossy().len() > MAX_SANDBOX_PATH_BYTES
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir | Component::ParentDir | Component::Prefix(_)
            )
        })
    {
        return Err(SandboxPolicyError::InvalidPath);
    }
    Ok(())
}

fn update_limits(digest: &mut Sha256, limits: SandboxResourceLimits) {
    for value in [
        limits.cpu_seconds,
        limits.memory_bytes,
        limits.disk_bytes,
        limits.processes,
        limits.open_files,
        limits.outbound_bytes,
        limits.output_bytes,
        limits.concurrent_commands,
        limits.cost_micros,
    ] {
        digest.update(value.unwrap_or_default().to_be_bytes());
    }
    for value in [limits.command_time, limits.session_time] {
        let value = value.unwrap_or_default();
        digest.update(value.as_secs().to_be_bytes());
        digest.update(value.subsec_nanos().to_be_bytes());
    }
}

fn readable_by(rules: &[SandboxFilesystemRule], path: &Path) -> bool {
    effective_rule(rules, path)
        .is_some_and(|rule| !matches!(rule.access, SandboxFilesystemAccess::Unreadable))
}

fn effective_rule<'a>(
    rules: &'a [SandboxFilesystemRule],
    path: &Path,
) -> Option<&'a SandboxFilesystemRule> {
    rules
        .iter()
        .filter(|rule| path.starts_with(&rule.path))
        .max_by_key(|rule| rule.path.components().count())
}

fn filesystem_rule_allowed(
    parent: &[SandboxFilesystemRule],
    candidate: &SandboxFilesystemRule,
) -> bool {
    effective_rule(parent, &candidate.path).is_some_and(|granted| {
        candidate.access.authority() <= granted.access.authority()
            && !(candidate.access == SandboxFilesystemAccess::ReadOnly
                && granted.access == SandboxFilesystemAccess::Protected)
    })
}

/// Why an immutable policy was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SandboxPolicyError {
    /// Paths are bounded, absolute, and normalized before entering a policy.
    #[error(
        "sandbox policy path must be normalized, absolute, and at most {MAX_SANDBOX_PATH_BYTES} bytes"
    )]
    InvalidPath,
    /// Every policy has a bounded non-empty filesystem rule set.
    #[error("sandbox filesystem policy must contain 1..={MAX_SANDBOX_FILESYSTEM_RULES} rules")]
    InvalidFilesystemRuleCount,
    /// One path cannot carry two different effective modes at the same depth.
    #[error("sandbox filesystem policy contains conflicting rules for one path")]
    ConflictingFilesystemRule,
    /// Cwd must be visible through the policy itself.
    #[error("sandbox working directory is outside the readable policy")]
    WorkingDirectoryOutsidePolicy,
    /// Network endpoints are bounded and structurally valid.
    #[error("sandbox network endpoint is invalid")]
    InvalidEndpoint,
    /// An exact request is non-empty and bounded.
    #[error("sandbox network policy must contain 1..={MAX_SANDBOX_NETWORK_ENDPOINTS} endpoints")]
    InvalidEndpointCount,
    /// A ceiling of zero has no useful meaning and often means unlimited to a kernel API.
    #[error("sandbox resource ceilings must be greater than zero")]
    InvalidResourceLimit,
    /// Descendants cannot opt out of a parent's confinement requirement.
    #[error("sandbox descendant requested a weaker confinement mode")]
    WeakerMode,
    /// Descendants cannot add or widen filesystem reach.
    #[error("sandbox descendant requested wider filesystem authority")]
    FilesystemWidening,
    /// Descendants cannot broaden network reach.
    #[error("sandbox descendant requested wider network authority")]
    NetworkWidening,
    /// Descendants cannot relax resource ceilings.
    #[error("sandbox descendant requested wider resource authority")]
    ResourceWidening,
    /// Descendants cannot add persistence or snapshot authority.
    #[error("sandbox descendant requested wider session authority")]
    SessionWidening,
    /// The bounded parent/descendant command-filter intersection overflowed.
    #[error("sandbox command policy intersection exceeds its bound")]
    InvalidCommandPolicy,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(path: &str, access: SandboxFilesystemAccess) -> SandboxFilesystemRule {
        SandboxFilesystemRule::new(path, access, SandboxFilesystemProvenance::Workspace)
            .expect("valid fixture rule")
    }

    fn policy(mode: SandboxMode, rules: Vec<SandboxFilesystemRule>) -> SandboxPolicy {
        let working_directory = rules
            .first()
            .map(SandboxFilesystemRule::path)
            .unwrap_or_else(|| Path::new("/workspace"))
            .to_path_buf();
        SandboxPolicy::new(
            mode,
            rules,
            working_directory,
            SandboxNetworkPolicy::Closed,
            SandboxResourceLimits::default(),
        )
        .expect("valid fixture policy")
    }

    #[test]
    fn descendants_may_narrow_but_never_widen_filesystem_or_mode() {
        let parent = policy(
            SandboxMode::Required,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        );
        let narrower = policy(
            SandboxMode::Required,
            vec![rule("/workspace/src", SandboxFilesystemAccess::ReadOnly)],
        );
        assert!(SandboxPolicy::restrict(&parent, narrower).is_ok());

        let weaker = policy(
            SandboxMode::Off,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        );
        assert_eq!(
            SandboxPolicy::restrict(&parent, weaker),
            Err(SandboxPolicyError::WeakerMode)
        );

        let wider = policy(
            SandboxMode::Required,
            vec![rule("/other", SandboxFilesystemAccess::ReadOnly)],
        );
        assert_eq!(
            SandboxPolicy::restrict(&parent, wider),
            Err(SandboxPolicyError::FilesystemWidening)
        );
    }

    #[test]
    fn exact_network_policy_is_canonical_and_closed_is_always_narrower() {
        let first = SandboxNetworkEndpoint::new("example.com", 443).expect("endpoint");
        let exact =
            SandboxNetworkPolicy::exact([first.clone(), first], true, false).expect("exact policy");
        let SandboxNetworkPolicy::Exact { endpoints, .. } = &exact else {
            panic!("expected exact policy");
        };
        assert_eq!(endpoints.len(), 1);
        assert!(SandboxNetworkPolicy::Closed.is_no_wider_than(&exact));
        assert!(!exact.is_no_wider_than(&SandboxNetworkPolicy::Closed));
    }

    #[test]
    fn resource_ceiling_cannot_disappear_or_grow_in_a_descendant() {
        let parent = SandboxResourceLimits {
            memory_bytes: Some(1024),
            command_time: Some(Duration::from_secs(10)),
            ..SandboxResourceLimits::default()
        };
        let narrow = SandboxResourceLimits {
            memory_bytes: Some(512),
            command_time: Some(Duration::from_secs(5)),
            ..SandboxResourceLimits::default()
        };
        assert!(narrow.is_no_wider_than(parent));
        assert!(!SandboxResourceLimits::default().is_no_wider_than(parent));
    }

    #[test]
    fn mount_aliases_cannot_bypass_narrower_descendant_rules() {
        let protected = policy(
            SandboxMode::Required,
            vec![
                rule("/workspace", SandboxFilesystemAccess::ReadWrite),
                rule("/workspace/.git", SandboxFilesystemAccess::Protected),
            ],
        );
        assert!(
            !protected.permits_path(Path::new("/workspace"), SandboxFilesystemAccess::ReadWrite)
        );
        assert!(protected.permits_path(Path::new("/workspace"), SandboxFilesystemAccess::ReadOnly));

        let unreadable = policy(
            SandboxMode::Required,
            vec![
                rule("/workspace", SandboxFilesystemAccess::ReadWrite),
                rule("/workspace/private", SandboxFilesystemAccess::Unreadable),
            ],
        );
        assert!(
            !unreadable.permits_path(Path::new("/workspace"), SandboxFilesystemAccess::ReadOnly)
        );
    }
}
