//! Immutable authority and resource policy for one sandbox lifecycle.

use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};

use crucible_workspace::Workspace;
use sha2::{Digest, Sha256};

use crucible_storage::{
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxResourceLimits,
};

use super::domains::SandboxDomainPolicy;
use super::guardrail::SandboxCommandPolicy;

/// Maximum filesystem rules in one effective policy.
pub const MAX_SANDBOX_FILESYSTEM_RULES: usize = 128;

/// Maximum bytes retained for one policy path.
pub const MAX_SANDBOX_PATH_BYTES: usize = 4096;

/// Maximum bytes in one DNS name.
pub const MAX_SANDBOX_HOST_BYTES: usize = 253;

/// Maximum unreadable wildcard patterns in one effective plan.
pub const MAX_SANDBOX_UNREADABLE_PATTERNS: usize = 64;

/// Maximum wildcard/literal components after one pattern's fixed scan root.
pub const MAX_SANDBOX_PATTERN_COMPONENTS: usize = 64;

/// How much authority one access level is, for the narrowing rules.
///
/// Stable, and its own function rather than a method, because the level is a
/// judgement about two reaches rather than a fact about one.
fn authority(access: SandboxFilesystemAccess) -> u8 {
    match access {
        SandboxFilesystemAccess::Unreadable => 0,
        SandboxFilesystemAccess::ReadOnly | SandboxFilesystemAccess::Protected => 1,
        SandboxFilesystemAccess::ReadWrite => 2,
    }
}

/// Whether a reach at `provenance` may be granted `candidate` authority.
fn permits(
    provenance: SandboxFilesystemProvenance,
    candidate: SandboxFilesystemProvenance,
) -> bool {
    provenance as u8 == candidate as u8
        || matches!(candidate, SandboxFilesystemProvenance::Descendant)
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

/// Authority that introduced an exact network endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SandboxNetworkProvenance {
    /// Direct user authority.
    User,
    /// A user-admitted project request.
    Project,
    /// A nested extension, tool, agent, or other descendant narrowing.
    Descendant,
}

/// One exact outbound endpoint.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SandboxNetworkEndpoint {
    host: Box<str>,
    port: u16,
    provenance: SandboxNetworkProvenance,
}

impl SandboxNetworkEndpoint {
    /// Validates a bounded DNS name or literal address and non-zero port.
    ///
    /// # Errors
    ///
    /// Empty/oversized/control-bearing hosts and port zero are rejected.
    pub fn new(
        host: impl Into<Box<str>>,
        port: u16,
        provenance: SandboxNetworkProvenance,
    ) -> Result<Self, SandboxPolicyError> {
        let host = host.into();
        if host.is_empty() || host.len() > MAX_SANDBOX_HOST_BYTES || port == 0 {
            return Err(SandboxPolicyError::InvalidEndpoint);
        }
        if let Ok(address) = host.parse::<IpAddr>() {
            return Ok(Self {
                host: address.to_string().into(),
                port,
                provenance,
            });
        }

        let host = host.strip_suffix('.').unwrap_or(&host);
        let looks_like_invalid_ipv4 = host
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.');
        let valid_dns_name = !host.is_empty()
            && host.is_ascii()
            && !looks_like_invalid_ipv4
            && host.split('.').all(|label| {
                (1..=63).contains(&label.len())
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                    && label
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_alphanumeric)
                    && label
                        .as_bytes()
                        .last()
                        .is_some_and(u8::is_ascii_alphanumeric)
            });
        if !valid_dns_name {
            return Err(SandboxPolicyError::InvalidEndpoint);
        }
        Ok(Self {
            host: host.to_ascii_lowercase().into(),
            port,
            provenance,
        })
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

    /// Authority source for this exact host/port request.
    #[must_use]
    pub const fn provenance(&self) -> SandboxNetworkProvenance {
        self.provenance
    }
}

impl std::fmt::Debug for SandboxNetworkEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxNetworkEndpoint")
            .field("host", &"[host]")
            .field("port", &self.port)
            .field("provenance", &self.provenance)
            .finish()
    }
}

/// One bounded unreadable path pattern expanded by a trusted backend.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxUnreadablePattern {
    pattern: PathBuf,
    scan_root: PathBuf,
    components: Box<[Box<[u8]>]>,
    provenance: SandboxFilesystemProvenance,
}

impl SandboxUnreadablePattern {
    /// Validates a small absolute wildcard grammar.
    ///
    /// `*` matches bytes within one path component and `**` is accepted only
    /// as a complete component. `?` and character classes are deliberately
    /// unsupported. At least one non-root literal component must precede the
    /// first wildcard so expansion can never scan the host root.
    ///
    /// # Errors
    ///
    /// Relative, root-wide, traversal-bearing, unsupported, wildcard-free,
    /// oversized, or excessively deep patterns are rejected.
    pub fn new(
        pattern: impl Into<PathBuf>,
        provenance: SandboxFilesystemProvenance,
    ) -> Result<Self, SandboxPolicyError> {
        let pattern = pattern.into();
        let encoded = pattern.as_os_str().as_encoded_bytes();
        if !pattern.is_absolute()
            || encoded.len() > MAX_SANDBOX_PATH_BYTES
            || encoded.contains(&0)
            || pattern
                .components()
                .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        {
            return Err(SandboxPolicyError::InvalidUnreadablePattern);
        }

        let mut scan_root = PathBuf::new();
        let mut components = Vec::new();
        let mut wildcard = false;
        let mut recursive = false;
        for part in pattern.components() {
            match part {
                Component::RootDir | Component::Prefix(_) if !wildcard => scan_root.push(part),
                Component::Normal(name) => {
                    let bytes = name.as_encoded_bytes();
                    if bytes.contains(&b'?') || bytes.contains(&b'[') || bytes.contains(&b']') {
                        return Err(SandboxPolicyError::InvalidUnreadablePattern);
                    }
                    if bytes == b"**" {
                        if recursive {
                            return Err(SandboxPolicyError::InvalidUnreadablePattern);
                        }
                        recursive = true;
                    }
                    if !wildcard && !bytes.contains(&b'*') {
                        scan_root.push(name);
                    } else {
                        wildcard = true;
                        components.push(Box::<[u8]>::from(bytes));
                    }
                }
                Component::RootDir
                | Component::Prefix(_)
                | Component::CurDir
                | Component::ParentDir => {
                    return Err(SandboxPolicyError::InvalidUnreadablePattern);
                }
            }
        }
        if !wildcard
            || scan_root.parent().is_none()
            || components.is_empty()
            || components.len() > MAX_SANDBOX_PATTERN_COMPONENTS
        {
            return Err(SandboxPolicyError::InvalidUnreadablePattern);
        }
        Ok(Self {
            pattern,
            scan_root,
            components: components.into_boxed_slice(),
            provenance,
        })
    }

    /// Fixed non-wildcard directory under which expansion is bounded.
    #[must_use]
    pub fn scan_root(&self) -> &Path {
        &self.scan_root
    }

    /// Original validated absolute pattern.
    #[must_use]
    pub fn pattern(&self) -> &Path {
        &self.pattern
    }

    /// Whether one absolute candidate has this exact wildcard shape.
    #[must_use]
    pub fn matches(&self, candidate: &Path) -> bool {
        if validate_absolute_path(candidate).is_err() {
            return false;
        }
        let Ok(relative) = candidate.strip_prefix(&self.scan_root) else {
            return false;
        };
        let target = relative
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(name.as_encoded_bytes()),
                _ => None,
            })
            .collect::<Vec<_>>();
        target.len() <= MAX_SANDBOX_PATTERN_COMPONENTS
            && path_components_match(&self.components, &target)
    }

    /// Authority source for this removal of visibility.
    #[must_use]
    pub const fn provenance(&self) -> SandboxFilesystemProvenance {
        self.provenance
    }
}

impl std::fmt::Debug for SandboxUnreadablePattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxUnreadablePattern")
            .field("pattern", &"[absolute pattern]")
            .field("scan_root", &"[absolute path]")
            .field("provenance", &self.provenance)
            .finish_non_exhaustive()
    }
}

fn path_components_match(pattern: &[Box<[u8]>], target: &[&[u8]]) -> bool {
    let Some((head, tail)) = pattern.split_first() else {
        return target.is_empty();
    };
    if head.as_ref() == b"**" {
        return path_components_match(tail, target)
            || target
                .split_first()
                .is_some_and(|(_, rest)| path_components_match(pattern, rest));
    }
    target.split_first().is_some_and(|(candidate, rest)| {
        component_matches(head, candidate) && path_components_match(tail, rest)
    })
}

fn component_matches(pattern: &[u8], candidate: &[u8]) -> bool {
    let mut pattern_at = 0_usize;
    let mut candidate_at = 0_usize;
    let mut star = None;
    let mut retry_at = 0_usize;
    while let Some(&candidate_byte) = candidate.get(candidate_at) {
        match pattern.get(pattern_at) {
            Some(&pattern_byte) if pattern_byte == candidate_byte => {
                pattern_at = pattern_at.saturating_add(1);
                candidate_at = candidate_at.saturating_add(1);
            }
            Some(b'*') => {
                star = Some(pattern_at);
                pattern_at = pattern_at.saturating_add(1);
                retry_at = candidate_at;
            }
            _ if star.is_some() => {
                retry_at = retry_at.saturating_add(1);
                candidate_at = retry_at;
                pattern_at = star.unwrap_or_default().saturating_add(1);
            }
            _ => return false,
        }
    }
    pattern
        .get(pattern_at..)
        .is_some_and(|tail| tail.iter().all(|byte| *byte == b'*'))
}

/// Immutable outbound-network request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxNetworkPolicy {
    /// No host network namespace, inherited socket, loopback, Unix socket, DNS,
    /// metadata address, or forwarding authority.
    Closed,
    /// Domain mediation with overriding denies and explicit local connections.
    Domains(SandboxDomainPolicy),
}

impl SandboxNetworkPolicy {
    fn is_no_wider_than(&self, parent: &Self) -> bool {
        match (self, parent) {
            (Self::Closed, _) => true,
            (Self::Domains(child), Self::Domains(parent)) => child.is_no_wider_than(parent),
            (Self::Domains(child), Self::Closed) => child.is_closed(),
        }
    }
}

// The ceilings a policy narrows are a stored value now, so these three
// judgements — what a disabled backend may still promise, what a child may
// not widen, and what a usable set of ceilings is — live here beside the
// policy they are about rather than inside the value they read.
const fn unconfined(mut limits: SandboxResourceLimits) -> SandboxResourceLimits {
    limits.cpu_seconds = None;
    limits.memory_bytes = None;
    limits.open_files = None;
    limits
}
fn is_no_wider_than(limits: SandboxResourceLimits, parent: SandboxResourceLimits) -> bool {
    no_larger(limits.cpu_seconds, parent.cpu_seconds)
        && no_larger(limits.memory_bytes, parent.memory_bytes)
        && no_larger(limits.disk_bytes, parent.disk_bytes)
        && no_larger(limits.processes, parent.processes)
        && no_larger(limits.open_files, parent.open_files)
        && no_larger(limits.outbound_bytes, parent.outbound_bytes)
        && no_larger(limits.output_bytes, parent.output_bytes)
        && no_larger(limits.concurrent_commands, parent.concurrent_commands)
        && no_larger(limits.command_time, parent.command_time)
        && no_larger(limits.session_time, parent.session_time)
        && no_larger(limits.cost_micros, parent.cost_micros)
}
fn valid(limits: SandboxResourceLimits) -> bool {
    [
        limits.cpu_seconds,
        limits.memory_bytes,
        limits.disk_bytes,
        limits.processes,
        limits.open_files,
        limits.outbound_bytes,
        limits.output_bytes,
        limits.concurrent_commands,
        limits.cost_micros,
    ]
    .into_iter()
    .flatten()
    .all(|value| value > 0)
        && [limits.command_time, limits.session_time]
            .into_iter()
            .flatten()
            .all(|value| !value.is_zero())
}

fn no_larger<T: PartialOrd>(candidate: Option<T>, parent: Option<T>) -> bool {
    match (candidate, parent) {
        (Some(candidate), Some(parent)) => candidate <= parent,
        (Some(_) | None, None) => true,
        (None, Some(_)) => false,
    }
}

/// One fully resolved immutable policy.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxPolicy {
    enabled: bool,
    filesystem: Box<[SandboxFilesystemRule]>,
    unreadable_patterns: Box<[SandboxUnreadablePattern]>,
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
            for protected in [".git", ".agents", ".codex", ".crucible"] {
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
            true,
            filesystem,
            workspace.root(),
            SandboxNetworkPolicy::Closed,
            SandboxResourceLimits::confining(),
        )
    }

    /// Validates an effective policy assembled by the host.
    ///
    /// # Errors
    ///
    /// Invalid paths, duplicate/conflicting rules, an out-of-policy working
    /// directory, oversized rule sets, or zero resource ceilings are refused.
    pub fn new(
        enabled: bool,
        filesystem: impl IntoIterator<Item = SandboxFilesystemRule>,
        working_directory: impl Into<PathBuf>,
        network: SandboxNetworkPolicy,
        limits: SandboxResourceLimits,
    ) -> Result<Self, SandboxPolicyError> {
        let mut filesystem: Vec<_> = filesystem
            .into_iter()
            .take(MAX_SANDBOX_FILESYSTEM_RULES + 1)
            .collect();
        if filesystem.is_empty() || filesystem.len() > MAX_SANDBOX_FILESYSTEM_RULES {
            return Err(SandboxPolicyError::InvalidFilesystemRuleCount);
        }
        filesystem.sort_by(|left, right| left.path.cmp(&right.path));
        if filesystem.windows(2).any(|pair| {
            pair.first().is_some_and(|left| {
                pair.get(1).is_some_and(|right| {
                    left.path == right.path
                        && (left.access != right.access || left.provenance != right.provenance)
                })
            })
        }) {
            return Err(SandboxPolicyError::ConflictingFilesystemRule);
        }
        filesystem.dedup();
        if filesystem.iter().any(|rule| {
            filesystem.iter().any(|ancestor| {
                ancestor.path != rule.path
                    && rule.path.starts_with(&ancestor.path)
                    && matches!(
                        ancestor.access,
                        SandboxFilesystemAccess::Protected | SandboxFilesystemAccess::Unreadable
                    )
                    && authority(rule.access) > authority(ancestor.access)
            })
        }) {
            return Err(SandboxPolicyError::FilesystemWidening);
        }

        let working_directory = working_directory.into();
        validate_absolute_path(&working_directory)?;
        if !readable_by(&filesystem, &working_directory) {
            return Err(SandboxPolicyError::WorkingDirectoryOutsidePolicy);
        }
        if !valid(limits) {
            return Err(SandboxPolicyError::InvalidResourceLimit);
        }

        Ok(Self {
            enabled,
            filesystem: filesystem.into_boxed_slice(),
            unreadable_patterns: Box::new([]),
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
    /// Disabled required confinement, a wider filesystem grant or network policy,
    /// relaxed resource ceiling, or new persistence authority is rejected.
    pub fn restrict(parent: &Self, mut candidate: Self) -> Result<Self, SandboxPolicyError> {
        if parent.enabled && !candidate.enabled {
            return Err(SandboxPolicyError::ConfinementDisabled);
        }
        if !candidate
            .filesystem
            .iter()
            .all(|rule| filesystem_rule_allowed(&parent.filesystem, rule))
            || parent.filesystem.iter().any(|parent_rule| {
                effective_rule(&candidate.filesystem, &parent_rule.path).is_some_and(|candidate| {
                    !filesystem_access_allowed(parent_rule.access, candidate.access)
                        || !permits(parent_rule.provenance, candidate.provenance)
                })
            })
        {
            return Err(SandboxPolicyError::FilesystemWidening);
        }
        if !candidate.network.is_no_wider_than(&parent.network) {
            return Err(SandboxPolicyError::NetworkWidening);
        }
        if !is_no_wider_than(candidate.limits, parent.limits) {
            return Err(SandboxPolicyError::ResourceWidening);
        }
        candidate.commands = SandboxCommandPolicy::intersect(&parent.commands, &candidate.commands)
            .map_err(|_| SandboxPolicyError::InvalidCommandPolicy)?;
        if (candidate.persistent && !parent.persistent)
            || (candidate.snapshots && !parent.snapshots)
        {
            return Err(SandboxPolicyError::SessionWidening);
        }
        if candidate.unreadable_patterns.iter().any(|pattern| {
            parent
                .unreadable_patterns
                .iter()
                .find(|inherited| inherited.pattern == pattern.pattern)
                .map_or(
                    pattern.provenance != SandboxFilesystemProvenance::Descendant,
                    |inherited| !permits(inherited.provenance, pattern.provenance),
                )
        }) {
            return Err(SandboxPolicyError::FilesystemWidening);
        }
        let mut unreadable_patterns = parent.unreadable_patterns.to_vec();
        for pattern in &candidate.unreadable_patterns {
            if !unreadable_patterns
                .iter()
                .any(|retained| retained.pattern == pattern.pattern)
            {
                unreadable_patterns.push(pattern.clone());
            }
        }
        if unreadable_patterns.len() > MAX_SANDBOX_UNREADABLE_PATTERNS {
            return Err(SandboxPolicyError::InvalidUnreadablePatternCount);
        }
        unreadable_patterns.sort_by(|left, right| left.pattern.cmp(&right.pattern));
        candidate.unreadable_patterns = unreadable_patterns.into_boxed_slice();
        Ok(candidate)
    }

    /// Returns a copy with command-scoped limits, preserving every other field.
    ///
    /// One command may narrow what the policy already states; it may not widen
    /// it, and dropping a ceiling is widening. Start from [`Self::limits`] and
    /// change the fields the command owns.
    ///
    /// # Errors
    ///
    /// Zero ceilings are rejected, as is any ceiling wider than the policy's
    /// own.
    pub fn with_limits(
        mut self,
        limits: SandboxResourceLimits,
    ) -> Result<Self, SandboxPolicyError> {
        if !valid(limits) {
            return Err(SandboxPolicyError::InvalidResourceLimit);
        }
        if !is_no_wider_than(limits, self.limits) {
            return Err(SandboxPolicyError::ResourceWidening);
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

    /// Returns a copy with a bounded deterministic unreadable-pattern set.
    ///
    /// # Errors
    ///
    /// Too many patterns, conflicting provenance for one spelling, or a scan
    /// root outside the effective filesystem view is rejected.
    pub fn with_unreadable_patterns(
        mut self,
        patterns: impl IntoIterator<Item = SandboxUnreadablePattern>,
    ) -> Result<Self, SandboxPolicyError> {
        let mut patterns: Vec<_> = patterns
            .into_iter()
            .take(MAX_SANDBOX_UNREADABLE_PATTERNS + 1)
            .collect();
        if patterns.len() > MAX_SANDBOX_UNREADABLE_PATTERNS {
            return Err(SandboxPolicyError::InvalidUnreadablePatternCount);
        }
        patterns.sort_by(|left, right| left.pattern.cmp(&right.pattern));
        if patterns.windows(2).any(|pair| {
            pair.first().is_some_and(|left| {
                pair.get(1).is_some_and(|right| {
                    left.pattern == right.pattern && left.provenance != right.provenance
                })
            })
        }) || patterns
            .iter()
            .any(|pattern| !readable_by(&self.filesystem, pattern.scan_root()))
        {
            return Err(SandboxPolicyError::InvalidUnreadablePattern);
        }
        patterns.dedup();
        self.unreadable_patterns = patterns.into_boxed_slice();
        Ok(self)
    }

    /// Applies the host-authorized opt-in choice.
    ///
    /// Disabling removes kernel-only ceilings; command limits remain active.
    /// Descendants must pass through [`Self::restrict`] and cannot disable
    /// confinement required by their parent.
    #[must_use]
    pub const fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        if !enabled {
            self.limits = unconfined(self.limits);
        }
        self
    }

    /// Returns a copy with host-authorized durable-session and snapshot
    /// operations. Descendants still pass through [`Self::restrict`], which
    /// may remove either grant but never add one their parent did not hold.
    #[must_use]
    pub const fn with_session_state(mut self, persistent: bool, snapshots: bool) -> Self {
        self.persistent = persistent;
        self.snapshots = snapshots;
        self
    }

    /// Whether commands require verified operating-system confinement.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Canonical ordered filesystem rules.
    #[must_use]
    pub fn filesystem(&self) -> &[SandboxFilesystemRule] {
        &self.filesystem
    }

    /// Canonical unreadable wildcard patterns.
    #[must_use]
    pub fn unreadable_patterns(&self) -> &[SandboxUnreadablePattern] {
        &self.unreadable_patterns
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
                authority(access) <= authority(granted.access)
                    && !(access == SandboxFilesystemAccess::ReadOnly
                        && granted.access == SandboxFilesystemAccess::Protected)
            })
            && self
                .filesystem
                .iter()
                .filter(|rule| rule.path.starts_with(path))
                .all(|rule| authority(access) <= authority(rule.access))
    }

    /// Domain-separated policy identity for bounded inspection/checkpoints.
    ///
    /// Paths and endpoint names contribute only through this digest; callers
    /// retain the typed effective policy in memory and do not need to persist
    /// its sensitive host spellings.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"crucible-sandbox-policy-v2\0");
        digest.update([u8::from(self.enabled)]);
        for rule in &self.filesystem {
            digest.update(rule.path.as_os_str().as_encoded_bytes());
            digest.update([0, rule.access as u8, rule.provenance as u8]);
        }
        for pattern in &self.unreadable_patterns {
            digest.update(pattern.pattern.as_os_str().as_encoded_bytes());
            digest.update([0, pattern.provenance as u8]);
        }
        digest.update(self.working_directory.as_os_str().as_encoded_bytes());
        match &self.network {
            SandboxNetworkPolicy::Closed => digest.update(b"\0closed"),
            SandboxNetworkPolicy::Domains(policy) => policy.update_digest(&mut digest),
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
            .field("enabled", &self.enabled)
            .field("filesystem_rules", &self.filesystem.len())
            .field("unreadable_patterns", &self.unreadable_patterns.len())
            .field("working_directory", &"[absolute path]")
            .field("network", &self.network)
            .field("limits", &self.limits)
            .field("commands", &self.commands)
            .field("persistent", &self.persistent)
            .field("snapshots", &self.snapshots)
            .finish()
    }
}

pub(super) fn validate_absolute_path(path: &Path) -> Result<(), SandboxPolicyError> {
    let encoded = path.as_os_str().as_encoded_bytes();
    if !path.is_absolute()
        || path.as_os_str().is_empty()
        || encoded.len() > MAX_SANDBOX_PATH_BYTES
        || encoded.contains(&0)
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
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
        filesystem_access_allowed(granted.access, candidate.access)
            && permits(granted.provenance, candidate.provenance)
    })
}

fn filesystem_access_allowed(
    parent: SandboxFilesystemAccess,
    candidate: SandboxFilesystemAccess,
) -> bool {
    authority(candidate) <= authority(parent)
        && !(candidate == SandboxFilesystemAccess::ReadOnly
            && parent == SandboxFilesystemAccess::Protected)
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
    /// Unreadable wildcard expansion has one small, absolute grammar.
    #[error("sandbox unreadable path pattern is invalid or outside the readable policy")]
    InvalidUnreadablePattern,
    /// The wildcard set is bounded independently of ordinary roots.
    #[error(
        "sandbox policy contains more than {MAX_SANDBOX_UNREADABLE_PATTERNS} unreadable patterns"
    )]
    InvalidUnreadablePatternCount,
    /// One path cannot carry two different effective modes at the same depth.
    #[error("sandbox filesystem policy contains conflicting rules for one path")]
    ConflictingFilesystemRule,
    /// Cwd must be visible through the policy itself.
    #[error("sandbox working directory is outside the readable policy")]
    WorkingDirectoryOutsidePolicy,
    /// Network endpoints are bounded and structurally valid.
    #[error("sandbox network endpoint is invalid")]
    InvalidEndpoint,
    /// A ceiling of zero has no useful meaning and often means unlimited to a kernel API.
    #[error("sandbox resource ceilings must be greater than zero")]
    InvalidResourceLimit,
    /// Descendants cannot opt out of a parent's confinement requirement.
    #[error("sandbox descendant disabled required confinement")]
    ConfinementDisabled,
    /// Descendants cannot add or widen filesystem reach.
    #[error("sandbox descendant requested wider filesystem authority")]
    FilesystemWidening,
    /// Domain and local socket lists have a fixed bound.
    #[error("sandbox network rule list exceeds its bound")]
    InvalidNetworkRuleCount,
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

// These policy fixtures use POSIX paths. Native Windows path and enforcement
// coverage lives in the backend integration tests.
#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::time::Duration;

    use crucible_storage::{MAX_SANDBOX_CONFINING_CPU_SECONDS, MAX_SANDBOX_CONFINING_OPEN_FILES};

    use super::*;

    fn rule(path: &str, access: SandboxFilesystemAccess) -> SandboxFilesystemRule {
        SandboxFilesystemRule::new(path, access, SandboxFilesystemProvenance::Workspace)
            .expect("valid fixture rule")
    }

    fn policy(enabled: bool, rules: Vec<SandboxFilesystemRule>) -> SandboxPolicy {
        policy_with(enabled, rules, SandboxResourceLimits::default())
    }

    fn policy_with(
        enabled: bool,
        rules: Vec<SandboxFilesystemRule>,
        limits: SandboxResourceLimits,
    ) -> SandboxPolicy {
        let working_directory = rules
            .first()
            .map_or_else(|| Path::new("/workspace"), SandboxFilesystemRule::path)
            .to_path_buf();
        SandboxPolicy::new(
            enabled,
            rules,
            working_directory,
            SandboxNetworkPolicy::Closed,
            limits,
        )
        .expect("valid fixture policy")
    }

    #[test]
    fn a_confined_command_is_given_ceilings_a_runaway_reaches_and_a_build_does_not() {
        let workspace =
            Workspace::open(env!("CARGO_MANIFEST_DIR")).expect("this crate's own directory");
        let limits = SandboxPolicy::standard(&workspace)
            .expect("the standard policy for a workspace")
            .limits();

        #[cfg(not(target_os = "macos"))]
        assert_eq!(limits.cpu_seconds, Some(MAX_SANDBOX_CONFINING_CPU_SECONDS));
        #[cfg(target_os = "macos")]
        assert_eq!(limits.cpu_seconds, None);
        assert_eq!(limits.open_files, Some(MAX_SANDBOX_CONFINING_OPEN_FILES));

        // Stated only where a backend applies them. A ceiling nothing enforces
        // reads as a settled question and bounds nothing; see
        // `SandboxResourceLimits::confining`.
        assert_eq!(limits.memory_bytes, None);
        assert_eq!(limits.processes, None);
        assert_eq!(limits.disk_bytes, None);
        assert_eq!(limits.outbound_bytes, None);
    }

    #[test]
    fn policy_identity_versions_boolean_enablement() {
        let rules = vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)];
        let disabled = policy(false, rules.clone());
        let enabled = policy(true, rules);
        assert_ne!(disabled.digest(), enabled.digest());
        // Fixed vectors bind the persisted identity to the v2 domain and to
        // enablement even when all filesystem and resource limits are equal.
        for (policy, expected) in [
            (
                disabled,
                "842034099768d0182c014f30178b768be61d9d79d8c133e7ad09dd50acecb1d9",
            ),
            (
                enabled,
                "0c087a9983c6d5baeb62fa6791ded47fe999f8ef105d47a7d09a920091076f36",
            ),
        ] {
            let actual = policy.digest().map(|byte| format!("{byte:02x}")).concat();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn disabling_confinement_removes_only_its_kernel_ceilings() {
        // Memory is not one of the ceilings `confining` states, so a policy
        // that only used those could not tell whether `with_enabled` took it off
        // or whether it was never there. A host that did ask for it says both.
        let confining = policy_with(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
            SandboxResourceLimits {
                memory_bytes: Some(1 << 30),
                command_time: Some(Duration::from_secs(30)),
                ..SandboxResourceLimits::confining()
            },
        );
        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            confining.limits().cpu_seconds,
            Some(MAX_SANDBOX_CONFINING_CPU_SECONDS)
        );
        #[cfg(target_os = "macos")]
        assert_eq!(confining.limits().cpu_seconds, None);
        assert_eq!(confining.limits().memory_bytes, Some(1 << 30));

        // The compatibility backend applies no rlimit. Carrying the numbers
        // across would not bound the command; it would refuse it, because a
        // policy may not ask for a ceiling its backend cannot apply.
        let relaxed = confining.clone().with_enabled(false);
        assert_eq!(relaxed.limits().cpu_seconds, None);
        assert_eq!(relaxed.limits().open_files, None);
        assert_eq!(relaxed.limits().memory_bytes, None);

        // What the caller asked for over one command is not the
        // confinement's, and stays exactly as it was.
        assert_eq!(relaxed.limits().command_time, Some(Duration::from_secs(30)));
    }

    #[test]
    fn one_command_may_narrow_the_confinement_it_runs_under_but_never_shed_it() {
        let confining = policy_with(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
            SandboxResourceLimits::confining(),
        );

        let shed = confining
            .clone()
            .with_limits(SandboxResourceLimits {
                command_time: Some(Duration::from_secs(30)),
                ..SandboxResourceLimits::default()
            })
            .expect_err("a command that states no processor ceiling has widened past its policy");
        assert!(
            matches!(shed, SandboxPolicyError::ResourceWidening),
            "{shed:?}"
        );

        let narrowed = confining
            .with_limits(SandboxResourceLimits {
                command_time: Some(Duration::from_secs(30)),
                cpu_seconds: Some(MAX_SANDBOX_CONFINING_CPU_SECONDS / 2),
                ..SandboxResourceLimits::confining()
            })
            .expect("narrowing what the policy already states");
        assert_eq!(
            narrowed.limits().cpu_seconds,
            Some(MAX_SANDBOX_CONFINING_CPU_SECONDS / 2)
        );
        assert_eq!(
            narrowed.limits().open_files,
            Some(MAX_SANDBOX_CONFINING_OPEN_FILES)
        );
    }

    #[test]
    fn descendants_may_narrow_but_never_widen_filesystem_or_disable_confinement() {
        let parent = policy(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        );
        let narrower = policy(
            true,
            vec![rule("/workspace/src", SandboxFilesystemAccess::ReadOnly)],
        );
        assert!(SandboxPolicy::restrict(&parent, narrower).is_ok());

        let weaker = policy(
            false,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        );
        assert_eq!(
            SandboxPolicy::restrict(&parent, weaker),
            Err(SandboxPolicyError::ConfinementDisabled)
        );

        let wider = policy(
            true,
            vec![rule("/other", SandboxFilesystemAccess::ReadOnly)],
        );
        assert_eq!(
            SandboxPolicy::restrict(&parent, wider),
            Err(SandboxPolicyError::FilesystemWidening)
        );
        assert_eq!(
            SandboxFilesystemRule::new(
                "/workspace/nul\0path",
                SandboxFilesystemAccess::ReadOnly,
                SandboxFilesystemProvenance::Workspace,
            ),
            Err(SandboxPolicyError::InvalidPath)
        );
    }

    #[test]
    fn descendants_cannot_drop_parent_filesystem_carve_outs_or_relabel_authority() {
        let parent = policy(
            true,
            vec![
                rule("/workspace", SandboxFilesystemAccess::ReadWrite),
                SandboxFilesystemRule::new(
                    "/workspace/.git",
                    SandboxFilesystemAccess::Protected,
                    SandboxFilesystemProvenance::ProtectedMetadata,
                )
                .expect("protected rule"),
                SandboxFilesystemRule::new(
                    "/workspace/private",
                    SandboxFilesystemAccess::Unreadable,
                    SandboxFilesystemProvenance::Descendant,
                )
                .expect("unreadable rule"),
            ],
        );
        let dropped = policy(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        );
        assert_eq!(
            SandboxPolicy::restrict(&parent, dropped),
            Err(SandboxPolicyError::FilesystemWidening)
        );

        let relabeled = policy(
            true,
            vec![
                SandboxFilesystemRule::new(
                    "/workspace/src",
                    SandboxFilesystemAccess::ReadOnly,
                    SandboxFilesystemProvenance::Runtime,
                )
                .expect("relabeled rule"),
            ],
        );
        assert_eq!(
            SandboxPolicy::restrict(&parent, relabeled),
            Err(SandboxPolicyError::FilesystemWidening)
        );
    }

    #[test]
    fn endpoint_provenance_is_retained() {
        let child_endpoint =
            SandboxNetworkEndpoint::new("192.0.2.1", 443, SandboxNetworkProvenance::Descendant)
                .expect("child endpoint");
        assert_eq!(
            child_endpoint.provenance(),
            SandboxNetworkProvenance::Descendant
        );
    }

    #[test]
    fn endpoint_hosts_are_dns_names_or_literal_addresses_not_url_fragments() {
        for invalid in [
            "https://example.com",
            "example com",
            "-bad.example",
            "bad-.example",
            "bad..example",
            "exa_mple.com",
            "métadata.example",
            "127.0.0.999",
        ] {
            assert_eq!(
                SandboxNetworkEndpoint::new(invalid, 443, SandboxNetworkProvenance::User),
                Err(SandboxPolicyError::InvalidEndpoint),
                "{invalid}"
            );
        }
        assert_eq!(
            SandboxNetworkEndpoint::new("2001:0db8::1", 443, SandboxNetworkProvenance::User)
                .expect("IPv6 literal")
                .host(),
            "2001:db8::1"
        );
        assert_eq!(
            SandboxNetworkEndpoint::new("EXAMPLE.COM.", 443, SandboxNetworkProvenance::User)
                .expect("canonical hostname")
                .host(),
            "example.com"
        );

        let endpoint =
            SandboxNetworkEndpoint::new("private.example", 8443, SandboxNetworkProvenance::User)
                .expect("endpoint");
        let shown = format!("{endpoint:?}");
        assert!(!shown.contains("private.example"), "{shown}");
        assert!(shown.contains("[host]"), "{shown}");
    }

    #[test]
    fn unreadable_patterns_are_bounded_absolute_and_match_only_their_shape() {
        let pattern = SandboxUnreadablePattern::new(
            "/workspace/**/*.pem",
            SandboxFilesystemProvenance::Descendant,
        )
        .expect("pattern");
        assert_eq!(pattern.scan_root(), Path::new("/workspace"));
        assert!(pattern.matches(Path::new("/workspace/key.pem")));
        assert!(pattern.matches(Path::new("/workspace/nested/key.pem")));
        assert!(!pattern.matches(Path::new("/workspace/key.txt")));
        assert!(!pattern.matches(Path::new("/other/key.pem")));
        assert!(!pattern.matches(Path::new("/workspace/../workspace/key.pem")));

        for invalid in [
            "relative/*.pem",
            "/**/*.pem",
            "/workspace/no-wildcard",
            "/workspace/../*.pem",
            "/workspace/key?.pem",
            "/workspace/[ab].pem",
            "/workspace/**/nested/**/*.pem",
        ] {
            assert!(
                SandboxUnreadablePattern::new(invalid, SandboxFilesystemProvenance::Descendant,)
                    .is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn filesystem_rule_iterators_stop_at_the_count_bound() {
        let consumed = std::cell::Cell::new(0);
        let rules = (0..MAX_SANDBOX_FILESYSTEM_RULES + 8).map(|_| {
            consumed.set(consumed.get() + 1);
            rule("/workspace", SandboxFilesystemAccess::ReadWrite)
        });
        let result = SandboxPolicy::new(
            false,
            rules,
            "/workspace",
            SandboxNetworkPolicy::Closed,
            SandboxResourceLimits::default(),
        );
        assert!(matches!(
            result,
            Err(SandboxPolicyError::InvalidFilesystemRuleCount)
        ));
        assert_eq!(consumed.get(), MAX_SANDBOX_FILESYSTEM_RULES + 1);
    }

    #[test]
    fn unreadable_pattern_iterators_stop_at_the_count_bound() {
        let consumed = std::cell::Cell::new(0);
        let pattern = SandboxUnreadablePattern::new(
            "/workspace/**/*.pem",
            SandboxFilesystemProvenance::Workspace,
        )
        .unwrap();
        let patterns = (0..MAX_SANDBOX_UNREADABLE_PATTERNS + 8).map(|_| {
            consumed.set(consumed.get() + 1);
            pattern.clone()
        });
        let result = policy(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        )
        .with_unreadable_patterns(patterns);
        assert!(matches!(
            result,
            Err(SandboxPolicyError::InvalidUnreadablePatternCount)
        ));
        assert_eq!(consumed.get(), MAX_SANDBOX_UNREADABLE_PATTERNS + 1);
    }

    #[test]
    fn descendant_policy_cannot_drop_a_parent_unreadable_pattern() {
        let parent = policy(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        )
        .with_unreadable_patterns([SandboxUnreadablePattern::new(
            "/workspace/**/*.env",
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("parent pattern")])
        .expect("parent policy");
        let child = policy(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadOnly)],
        );

        let effective = SandboxPolicy::restrict(&parent, child).expect("effective policy");
        assert_eq!(effective.unreadable_patterns().len(), 1);
        assert!(
            effective
                .unreadable_patterns()
                .first()
                .expect("inherited pattern")
                .matches(Path::new("/workspace/nested/.env"))
        );
    }

    #[test]
    fn descendant_unreadable_patterns_cannot_claim_parent_provenance() {
        let parent = policy(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        );
        let relabeled = policy(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        )
        .with_unreadable_patterns([SandboxUnreadablePattern::new(
            "/workspace/**/*.key",
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("relabeled pattern")])
        .expect("candidate policy");
        assert_eq!(
            SandboxPolicy::restrict(&parent, relabeled),
            Err(SandboxPolicyError::FilesystemWidening)
        );

        let descendant = policy(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        )
        .with_unreadable_patterns([SandboxUnreadablePattern::new(
            "/workspace/**/*.key",
            SandboxFilesystemProvenance::Descendant,
        )
        .expect("descendant pattern")])
        .expect("candidate policy");
        assert!(SandboxPolicy::restrict(&parent, descendant).is_ok());
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
        assert!(is_no_wider_than(narrow, parent));
        assert!(!is_no_wider_than(SandboxResourceLimits::default(), parent));
    }

    #[test]
    fn session_state_authority_can_be_preserved_or_removed_but_not_added() {
        let parent = policy(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        )
        .with_session_state(true, true);
        let narrower = policy(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        )
        .with_session_state(false, false);
        assert!(SandboxPolicy::restrict(&parent, narrower).is_ok());

        let no_state = policy(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        );
        let added = policy(
            true,
            vec![rule("/workspace", SandboxFilesystemAccess::ReadWrite)],
        )
        .with_session_state(true, false);
        assert_eq!(
            SandboxPolicy::restrict(&no_state, added),
            Err(SandboxPolicyError::SessionWidening)
        );
    }

    #[test]
    fn mount_aliases_cannot_bypass_narrower_descendant_rules() {
        let protected = policy(
            true,
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
            true,
            vec![
                rule("/workspace", SandboxFilesystemAccess::ReadWrite),
                rule("/workspace/private", SandboxFilesystemAccess::Unreadable),
            ],
        );
        assert!(
            !unreadable.permits_path(Path::new("/workspace"), SandboxFilesystemAccess::ReadOnly)
        );
    }

    #[test]
    fn one_policy_cannot_reopen_a_protected_or_unreadable_subtree() {
        for protected in [
            SandboxFilesystemAccess::Protected,
            SandboxFilesystemAccess::Unreadable,
        ] {
            assert_eq!(
                SandboxPolicy::new(
                    true,
                    [
                        rule("/workspace", SandboxFilesystemAccess::ReadWrite),
                        rule("/workspace/private", protected),
                        rule(
                            "/workspace/private/reopened",
                            SandboxFilesystemAccess::ReadWrite,
                        ),
                    ],
                    "/workspace",
                    SandboxNetworkPolicy::Closed,
                    SandboxResourceLimits::default(),
                ),
                Err(SandboxPolicyError::FilesystemWidening)
            );
        }
    }

    #[test]
    fn one_filesystem_path_cannot_claim_two_authority_sources() {
        let workspace = rule("/workspace", SandboxFilesystemAccess::ReadWrite);
        let descendant = SandboxFilesystemRule::new(
            "/workspace",
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::Descendant,
        )
        .expect("descendant rule");
        assert_eq!(
            SandboxPolicy::new(
                true,
                [workspace, descendant],
                "/workspace",
                SandboxNetworkPolicy::Closed,
                SandboxResourceLimits::default(),
            ),
            Err(SandboxPolicyError::ConflictingFilesystemRule)
        );
    }
}
