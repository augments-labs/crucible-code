//! The redacted sandbox records a journal and a checkpoint store.
//!
//! A sandbox lifecycle is observed by the backend that confines it, and what
//! it observes is a value: a negotiated capability matrix, an effective plan
//! reduced to digests and counts, a bounded usage reading. That value is also
//! what a journal line and a checkpoint file hold, and a store must be able to
//! read one back without linking the runtime that produced it — so the whole
//! vocabulary lives here, beside the ports that keep it, and `crucible-sandbox`
//! re-exports it and builds it.
//!
//! What moved is the *record*, not the judgement. A backend still negotiates,
//! still refuses a policy it cannot enforce, and still reduces its policy and
//! manifest to these summaries; that work is `crucible-sandbox`'s, and the
//! constructors below take the values it reduced them to. What the stored
//! record judges is the claim it can see about itself: an inspection that
//! calls itself an enforcing boundary has to hold the capability claims that
//! back it, because the journal line and the checkpoint file that keep it are
//! read back by a build that never sees the policy.
//!
//! Everything here is already redacted — a digest stays a digest, a reach stays
//! a domain-separated identity, a capability claim stays a claim — and the words
//! a record is written with are owned here, beside the types they name.

use std::fmt;
use std::time::Duration;

use crucible_types::SandboxId;

/// Maximum entries in each domain or Unix-socket list.
pub const MAX_SANDBOX_NETWORK_RULES: usize = 64;

/// Maximum bytes in a stable backend identifier.
pub const MAX_SANDBOX_BACKEND_ID_BYTES: usize = 64;

/// Maximum bytes in a backend version or provenance label.
pub const MAX_SANDBOX_BACKEND_WORD_BYTES: usize = 128;

/// A stable, bounded backend implementation name.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SandboxBackendId(Box<str>);

impl SandboxBackendId {
    /// Validates a source-qualified backend name.
    ///
    /// # Errors
    ///
    /// Empty, oversized, or non-symbolic names are rejected.
    pub fn new(value: impl Into<Box<str>>) -> Result<Self, SandboxCapabilityError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_SANDBOX_BACKEND_ID_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            return Err(SandboxCapabilityError::InvalidBackendId);
        }
        Ok(Self(value))
    }

    /// The stable backend name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SandboxBackendId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SandboxBackendId").field(&self.0).finish()
    }
}

impl fmt::Display for SandboxBackendId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where the enforcing backend came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackendProvenance {
    /// A canonical executable outside every writable sandbox root.
    System,
    /// A release-pinned executable shipped with Crucible.
    Bundled,
    /// An authenticated remote executor.
    Remote,
    /// An explicitly selected non-confining compatibility implementation.
    Compatibility,
}

impl SandboxBackendProvenance {
    /// Stable inspection/persistence spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Bundled => "bundled",
            Self::Remote => "remote",
            Self::Compatibility => "compatibility",
        }
    }
}

/// Bounded backend identity retained in audit and inspection records.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxBackendIdentity {
    id: SandboxBackendId,
    version: Box<str>,
    provenance: SandboxBackendProvenance,
    digest: Option<[u8; 32]>,
}

impl SandboxBackendIdentity {
    /// Builds an identity without retaining an executable path.
    ///
    /// # Errors
    ///
    /// An empty or oversized version is rejected.
    pub fn new(
        id: SandboxBackendId,
        version: impl Into<Box<str>>,
        provenance: SandboxBackendProvenance,
        digest: Option<[u8; 32]>,
    ) -> Result<Self, SandboxCapabilityError> {
        let version = version.into();
        if version.is_empty() || version.len() > MAX_SANDBOX_BACKEND_WORD_BYTES {
            return Err(SandboxCapabilityError::InvalidBackendVersion);
        }
        Ok(Self {
            id,
            version,
            provenance,
            digest,
        })
    }

    /// Stable implementation identity.
    #[must_use]
    pub const fn id(&self) -> &SandboxBackendId {
        &self.id
    }

    /// Bounded implementation version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// How the executable or service was supplied.
    #[must_use]
    pub const fn provenance(&self) -> SandboxBackendProvenance {
        self.provenance
    }

    /// Digest of the backend artifact, where one is locally verifiable.
    #[must_use]
    pub const fn digest(&self) -> Option<[u8; 32]> {
        self.digest
    }
}

impl fmt::Debug for SandboxBackendIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SandboxBackendIdentity")
            .field("id", &self.id)
            .field("version", &self.version)
            .field("provenance", &self.provenance)
            .field("digest", &self.digest.map(|_| "[sha256]"))
            .finish()
    }
}

/// Strength of one backend claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SandboxCapability {
    /// The backend cannot provide the feature.
    Unsupported,
    /// The backend can measure or report it but cannot make it a hard ceiling.
    Observed,
    /// The backend enforces it before untrusted code can bypass it.
    Enforced,
}

impl SandboxCapability {
    /// Whether a required hard policy can rely on this claim.
    #[must_use]
    pub const fn is_enforced(self) -> bool {
        matches!(self, Self::Enforced)
    }

    /// Stable capability-matrix spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Observed => "observed",
            Self::Enforced => "enforced",
        }
    }
}

/// Independently negotiated sandbox features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxFeature {
    /// Backend-enforced filesystem writes and protected carve-outs.
    ///
    /// Backends report their read-visibility boundary separately in inspection
    /// and platform documentation; native Windows retains the dedicated
    /// account's ordinary read access so the Win32 loader remains usable.
    Filesystem,
    /// A network policy that permits no egress.
    NetworkDeny,
    /// Exact host/port/DNS constrained egress.
    NetworkAllowlist,
    /// No host descriptors other than declared standard streams.
    DescriptorIsolation,
    /// Descendants inherit OS confinement and cannot reach undeclared host
    /// process, IPC, or network authority.
    ProcessIsolation,
    /// Kernel and device control surfaces are absent or policy-restricted.
    KernelSurface,
    /// A workload cannot acquire authority that bypasses its effective policy.
    PrivilegeIsolation,
    /// Bounded inert manifest staging.
    Materialization,
    /// CPU time ceiling.
    CpuLimit,
    /// Address-space or resident-memory ceiling.
    MemoryLimit,
    /// Ephemeral-storage ceiling.
    DiskLimit,
    /// Process-count ceiling.
    ProcessLimit,
    /// Open-file ceiling.
    OpenFileLimit,
    /// Command wall-time ceiling.
    CommandTimeLimit,
    /// Session wall-time ceiling.
    SessionTimeLimit,
    /// Outbound-byte ceiling.
    OutboundByteLimit,
    /// Captured-output ceiling.
    OutputLimit,
    /// Concurrent session/command ceiling.
    ConcurrencyLimit,
    /// Backend cost ceiling.
    CostLimit,
    /// Pseudo-terminal operation.
    Pty,
    /// Direct file operations through the service.
    FileOperations,
    /// Durable sessions.
    Persistence,
    /// Snapshots.
    Snapshot,
    /// Resuming a prior session.
    Resume,
    /// Bounded lifecycle audit facts.
    Audit,
    /// Bounded resource usage reporting.
    Usage,
}

/// One immutable capability snapshot returned before side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxCapabilities {
    claims: [SandboxCapability; SandboxFeature::COUNT],
}

impl SandboxCapabilities {
    /// A backend that claims nothing.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            claims: [SandboxCapability::Unsupported; SandboxFeature::COUNT],
        }
    }

    /// Returns a copy with one exact claim changed.
    #[must_use]
    pub fn with(mut self, feature: SandboxFeature, claim: SandboxCapability) -> Self {
        if let Some(slot) = self.claims.get_mut(feature.index()) {
            *slot = claim;
        }
        self
    }

    /// The backend's claim for `feature`.
    #[must_use]
    pub fn claim(&self, feature: SandboxFeature) -> SandboxCapability {
        match self.claims.get(feature.index()) {
            Some(claim) => *claim,
            None => SandboxCapability::Unsupported,
        }
    }

    /// Every feature and its exact claim in stable matrix order.
    pub fn iter(&self) -> impl Iterator<Item = (SandboxFeature, SandboxCapability)> + '_ {
        SandboxFeature::ALL
            .into_iter()
            .map(|feature| (feature, self.claim(feature)))
    }
}

impl Default for SandboxCapabilities {
    fn default() -> Self {
        Self::none()
    }
}

impl SandboxFeature {
    /// Number of independently negotiated features.
    pub const COUNT: usize = 26;

    /// Stable exhaustive capability-matrix order.
    pub const ALL: [Self; Self::COUNT] = [
        Self::Filesystem,
        Self::NetworkDeny,
        Self::NetworkAllowlist,
        Self::DescriptorIsolation,
        Self::ProcessIsolation,
        Self::KernelSurface,
        Self::PrivilegeIsolation,
        Self::Materialization,
        Self::CpuLimit,
        Self::MemoryLimit,
        Self::DiskLimit,
        Self::ProcessLimit,
        Self::OpenFileLimit,
        Self::CommandTimeLimit,
        Self::SessionTimeLimit,
        Self::OutboundByteLimit,
        Self::OutputLimit,
        Self::ConcurrencyLimit,
        Self::CostLimit,
        Self::Pty,
        Self::FileOperations,
        Self::Persistence,
        Self::Snapshot,
        Self::Resume,
        Self::Audit,
        Self::Usage,
    ];

    /// Stable capability-matrix spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Filesystem => "filesystem",
            Self::NetworkDeny => "network_deny",
            Self::NetworkAllowlist => "network_allowlist",
            Self::DescriptorIsolation => "descriptor_isolation",
            Self::ProcessIsolation => "process_isolation",
            Self::KernelSurface => "kernel_surface",
            Self::PrivilegeIsolation => "privilege_isolation",
            Self::Materialization => "materialization",
            Self::CpuLimit => "cpu_limit",
            Self::MemoryLimit => "memory_limit",
            Self::DiskLimit => "disk_limit",
            Self::ProcessLimit => "process_limit",
            Self::OpenFileLimit => "open_file_limit",
            Self::CommandTimeLimit => "command_time_limit",
            Self::SessionTimeLimit => "session_time_limit",
            Self::OutboundByteLimit => "outbound_byte_limit",
            Self::OutputLimit => "output_limit",
            Self::ConcurrencyLimit => "concurrency_limit",
            Self::CostLimit => "cost_limit",
            Self::Pty => "pty",
            Self::FileOperations => "file_operations",
            Self::Persistence => "persistence",
            Self::Snapshot => "snapshot",
            Self::Resume => "resume",
            Self::Audit => "audit",
            Self::Usage => "usage",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Filesystem => 0,
            Self::NetworkDeny => 1,
            Self::NetworkAllowlist => 2,
            Self::DescriptorIsolation => 3,
            Self::ProcessIsolation => 4,
            Self::KernelSurface => 5,
            Self::PrivilegeIsolation => 6,
            Self::Materialization => 7,
            Self::CpuLimit => 8,
            Self::MemoryLimit => 9,
            Self::DiskLimit => 10,
            Self::ProcessLimit => 11,
            Self::OpenFileLimit => 12,
            Self::CommandTimeLimit => 13,
            Self::SessionTimeLimit => 14,
            Self::OutboundByteLimit => 15,
            Self::OutputLimit => 16,
            Self::ConcurrencyLimit => 17,
            Self::CostLimit => 18,
            Self::Pty => 19,
            Self::FileOperations => 20,
            Self::Persistence => 21,
            Self::Snapshot => 22,
            Self::Resume => 23,
            Self::Audit => 24,
            Self::Usage => 25,
        }
    }
}

/// Why a capability identity was not safe to retain.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SandboxCapabilityError {
    /// The record is not internally consistent with the policy it describes.
    #[error("sandbox inspection is not internally consistent")]
    InvalidInspection,
    /// Backend identifiers are stable symbolic words.
    #[error(
        "sandbox backend id must be 1..={MAX_SANDBOX_BACKEND_ID_BYTES} ASCII letters, digits, '.', '-' or '_'"
    )]
    InvalidBackendId,
    /// Versions are short non-empty diagnostic values.
    #[error("sandbox backend version must be 1..={MAX_SANDBOX_BACKEND_WORD_BYTES} bytes")]
    InvalidBackendVersion,
}
/// Which immutable command image a guardrail is evaluating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxCommandStage {
    /// The host-selected invocation before a trusted adapter transformation.
    Requested,
    /// The invocation after every trusted program/argument transformation.
    Effective,
}

/// Redacted outcome retained by audit/events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxGuardrailDecision {
    /// Every independent filter admitted the command.
    Allowed,
    /// A deny matched or one filter's allow set did not match.
    Denied,
}
/// One redacted lifecycle fact, with the identity the surrounding record fixes.
///
/// A collector records one of these as the fact arrives, and it is what a
/// journal line, a display row and a replay all read: the sandbox's own
/// vocabulary never crosses into the log, and the log never asks the runtime
/// that produced the fact what it meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxFact {
    sandbox: SandboxId,
    kind: SandboxFactKind,
}

impl SandboxFact {
    /// Builds the record for one lifecycle fact.
    #[must_use]
    pub const fn new(sandbox: SandboxId, kind: SandboxFactKind) -> Self {
        Self { sandbox, kind }
    }

    /// Stable lifecycle identity.
    #[must_use]
    pub const fn sandbox(&self) -> SandboxId {
        self.sandbox
    }

    /// Redacted fact payload.
    #[must_use]
    pub const fn kind(&self) -> &SandboxFactKind {
        &self.kind
    }
}

/// A lifecycle transition that contains no backend-controlled prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxLifecycle {
    /// Effective policy and manifest identities were fixed.
    PolicyResolved,
    /// A session and its cleanup ownership were prepared.
    Prepared,
    /// The manifest transaction committed.
    Materialized,
    /// The exact command and release channel were fixed before `GO`.
    ReleaseIntent,
    /// The authenticated one-shot `GO` was sent or became ambiguous.
    CommandReleased,
    /// Application background ownership was durable before `GO`.
    OwnerTransferred,
    /// The governed command started.
    CommandStarted,
    /// The command leader exited and its descendant scope was emptied.
    CommandFinished,
    /// Terminal publication began after proved scope death.
    PublicationStarted,
    /// Valid workspace effects were durably published.
    Published,
    /// Private effects were discarded or publication was reversed.
    RolledBack,
    /// Preparation ended before any possible command release.
    Refused,
    /// Cleanup or recovery could not safely select publish or rollback.
    Quarantined,
}

/// Stable failure category retained without OS errors, paths, or command text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxFailureKind {
    /// A requested capability was unsupported.
    Unsupported,
    /// No suitable backend could be prepared.
    BackendUnavailable,
    /// A command guardrail denied an image.
    Guardrail,
    /// The concurrent reservation was exhausted.
    Concurrency,
    /// Manifest or filesystem preparation failed.
    Materialization,
    /// Process creation failed.
    Spawn,
    /// A step in a sandbox's life, from probing a backend to accepting a
    /// command's result, did not complete, including one dropped unanswered
    /// because it would have had to wait.
    Lifecycle,
    /// The bounded audit collector itself could not retain the fact.
    Audit,
    /// A command or environment record was structurally invalid.
    InvalidInput,
}

/// Lifecycle phase in which a typed failure occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxFailurePhase {
    /// Backend selection and capability negotiation.
    Prepare,
    /// Transactional manifest/workspace setup.
    Materialize,
    /// Guardrail evaluation and process creation.
    Start,
    /// Process control, accounting, or cleanup.
    Execute,
}

/// One redacted lifecycle fact. The surrounding record supplies attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxFactKind {
    /// A deterministic lifecycle transition.
    Lifecycle(SandboxLifecycle),
    /// The immutable negotiated inspection snapshot.
    Negotiated(Box<SandboxInspection>),
    /// One requested/effective command-filter decision.
    Guardrail {
        /// Image evaluated.
        stage: SandboxCommandStage,
        /// Redacted allow/deny outcome.
        decision: SandboxGuardrailDecision,
    },
    /// A hard resource ceiling was crossed.
    Violation(SandboxViolation),
    /// Bounded current/final usage.
    Usage(SandboxUsage),
    /// Terminal cleanup state.
    Cleanup(SandboxCleanup),
    /// A typed phase failed without retaining its diagnostic text.
    Failed {
        /// Phase that could not complete.
        phase: SandboxFailurePhase,
        /// Stable failure class.
        kind: SandboxFailureKind,
    },
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
    ///
    /// Filesystem-equivalent spellings name the same protected object. A
    /// backend must keep every such spelling protected even when a
    /// case-preserving filesystem lets the spelling of its directory entry
    /// change without replacing the object.
    Protected,
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
    /// A filesystem grant or restriction in the user's configuration.
    UserConfiguration,
    /// A restriction in the checked-in project configuration.
    ProjectConfiguration,
    /// A restriction in the project-local configuration.
    ProjectLocalConfiguration,
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

/// CPU seconds a confining backend applies where the platform can enforce one.
///
/// Darwin delivers SIGXCPU but does not make the hard value an uncatchable
/// ceiling, so macOS states this as `None` and never advertises it as enforced.
pub const MAX_SANDBOX_CONFINING_CPU_SECONDS: u64 = 60 * 60;

/// Files one confined process may hold open at once.
///
/// Four times the soft limit a Linux shell usually starts with, so nothing that
/// works outside the sandbox stops working inside it, and far below the point
/// where a descriptor leak reaches the rest of the machine.
#[cfg(not(target_os = "windows"))]
pub const MAX_SANDBOX_CONFINING_OPEN_FILES: u64 = 4096;

impl SandboxResourceLimits {
    /// The ceilings a confining backend puts on a command it starts.
    ///
    /// Generous on purpose. These are not a budget anybody is meant to work
    /// within — a build is allowed to be slow and to open a great many files.
    /// They are the point past which a command has stopped being a command and
    /// become a runaway, and past which the machine crucible is running on is
    /// the thing at risk.
    ///
    /// Only the ceilings a confining backend actually applies are here, because
    /// a limit that nothing enforces is worse than none: it reads, to the next
    /// person, like the question was settled. What is deliberately absent:
    ///
    /// - Memory. The knob a confining backend has is the address space a
    ///   process may map, which is not the memory it uses. Runtimes that
    ///   reserve enormously and touch little — Go, a JVM, anything built under
    ///   a sanitiser — would be refused by a ceiling low enough to catch
    ///   anything real.
    /// - Processes. Not because nothing bounds them: the broker caps the scope
    ///   it is PID 1 of whether or not a policy says so. Stating a number here
    ///   would instead refuse every command on a kernel older than 5.14, where
    ///   the count is the person's whole machine rather than this namespace,
    ///   and a busy desktop would be turned away for reasons nothing here could
    ///   explain.
    /// - Disk, outbound bytes and cost. Nothing in this tree enforces them yet,
    ///   and a backend negotiation in `crucible-sandbox` refuses a policy
    ///   asking for a ceiling it cannot apply.
    ///
    /// Wall time, captured output and concurrency are the caller's: they belong
    /// to one command rather than to the confinement, and the policy in
    /// `crucible-sandbox` carries whatever that caller narrows to.
    #[must_use]
    pub const fn confining() -> Self {
        Self {
            #[cfg(not(target_os = "macos"))]
            cpu_seconds: Some(MAX_SANDBOX_CONFINING_CPU_SECONDS),
            // Darwin delivers SIGXCPU but does not make the hard value an
            // uncatchable ceiling. A handler can continue past it, so macOS
            // must not state or advertise this as enforced.
            #[cfg(target_os = "macos")]
            cpu_seconds: None,
            // Windows Job Objects have no per-process handle-count ceiling.
            // Claiming the Unix descriptor limit there would make the native
            // backend either lie or reject every standard policy.
            #[cfg(not(target_os = "windows"))]
            open_files: Some(MAX_SANDBOX_CONFINING_OPEN_FILES),
            #[cfg(target_os = "windows")]
            open_files: None,
            memory_bytes: None,
            disk_bytes: None,
            processes: None,
            outbound_bytes: None,
            output_bytes: None,
            concurrent_commands: None,
            command_time: None,
            session_time: None,
            cost_micros: None,
        }
    }
}

/// One redacted effective filesystem reach in an inspection report.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxRootInspection {
    identity: [u8; 32],
    access: SandboxFilesystemAccess,
    provenance: SandboxFilesystemProvenance,
}

impl SandboxRootInspection {
    /// Builds one redacted reach from a domain-separated identity.
    #[must_use]
    pub const fn new(
        identity: [u8; 32],
        access: SandboxFilesystemAccess,
        provenance: SandboxFilesystemProvenance,
    ) -> Self {
        Self {
            identity,
            access,
            provenance,
        }
    }

    /// Domain-separated identity of the reach, never the path itself.
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
    /// Domain/local policy counts; private spellings remain behind the digest.
    Domains {
        /// Number of allowed host patterns.
        allowed: usize,
        /// Number of overriding denied patterns.
        denied: usize,
        /// Whether the workload may bind local listeners.
        local_binding: bool,
        /// Number of exact host Unix sockets.
        unix_sockets: usize,
    },
}

impl SandboxFilesystemAccess {
    /// Stable persisted spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::ReadOnly => "read_only",
            Self::ReadWrite => "read_write",
            Self::Protected => "protected",
        }
    }
}

impl SandboxFilesystemProvenance {
    /// Stable persisted spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Runtime => "runtime",
            Self::ProtectedMetadata => "protected_metadata",
            Self::Descendant => "descendant",
            Self::Manifest => "manifest",
            Self::UserConfiguration => "user_configuration",
            Self::ProjectConfiguration => "project_configuration",
            Self::ProjectLocalConfiguration => "project_local_configuration",
        }
    }
}

impl SandboxNetworkInspection {
    /// Stable network-state spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Domains { .. } => "domains",
        }
    }
}

/// Bounded redacted summary of the immutable effective policy and manifest.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxPlanInspection {
    enabled: bool,
    roots: Box<[SandboxRootInspection]>,
    working_directory: [u8; 32],
    network: SandboxNetworkInspection,
    limits: SandboxResourceLimits,
    command_policy: [u8; 32],
    unreadable_patterns: usize,
    persistent: bool,
    snapshots: bool,
    manifest_entries: usize,
}

impl SandboxPlanInspection {
    /// Builds the redacted plan summary from the values a policy and a
    /// manifest were reduced to.
    ///
    /// Nothing is refused here, and nothing should be: every field is already
    /// a digest, a count or a bounded enumeration, and the values behind them
    /// are checked where the policy and the manifest are built — the only place
    /// a ceiling or a reach can still be refused. `crucible-sandbox` reduces
    /// them and calls this; a caller that has no policy to reduce has nothing
    /// to check against, and the line a record writes stays bounded either way.
    // Identity, reach, network, ceilings and the three retained flags are
    // independent fields, so grouping them would add no invariant.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        enabled: bool,
        roots: Box<[SandboxRootInspection]>,
        working_directory: [u8; 32],
        network: SandboxNetworkInspection,
        limits: SandboxResourceLimits,
        command_policy: [u8; 32],
        unreadable_patterns: usize,
        persistent: bool,
        snapshots: bool,
        manifest_entries: usize,
    ) -> Self {
        Self {
            enabled,
            roots,
            working_directory,
            network,
            limits,
            command_policy,
            unreadable_patterns,
            persistent,
            snapshots,
            manifest_entries,
        }
    }

    /// Whether the effective policy requires verified confinement.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Reaches of this plan, represented only by domain-separated identities.
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

    /// Number of bounded unreadable wildcard patterns.
    #[must_use]
    pub const fn unreadable_patterns(&self) -> usize {
        self.unreadable_patterns
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
            .field("enabled", &self.enabled)
            .field("roots", &self.roots)
            .field("working_directory", &"[sha256]")
            .field("network", &self.network)
            .field("limits", &self.limits)
            .field("command_policy", &"[sha256]")
            .field("unreadable_patterns", &self.unreadable_patterns)
            .field("persistent", &self.persistent)
            .field("snapshots", &self.snapshots)
            .field("manifest_entries", &self.manifest_entries)
            .finish()
    }
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

/// Redacted immutable inspection snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxInspection {
    id: SandboxId,
    backend: SandboxBackendIdentity,
    capabilities: SandboxCapabilities,
    requested_plan: SandboxPlanInspection,
    plan: SandboxPlanInspection,
    requested_policy_digest: [u8; 32],
    policy_digest: [u8; 32],
    manifest_digest: [u8; 32],
    confined: bool,
    disabled_reason: Option<Box<str>>,
    cleanup: SandboxCleanup,
}

/// Whether a record that claims an enforcing kernel boundary enforces it.
///
/// The six boundaries a confinement depends on, and which network boundary one
/// of them is, are both facts the record holds about itself: the negotiated
/// capability claims and the effective plan's network shape. Stated once for
/// the two constructors that refuse a dishonest claim, because a report and the
/// checkpoint that remembers one are the same claim and two lists could come to
/// disagree about which shape demands which boundary.
fn confines_as_claimed(
    confined: bool,
    network: SandboxNetworkInspection,
    capabilities: &SandboxCapabilities,
) -> bool {
    if !confined {
        return true;
    }
    let network_boundary = match network {
        SandboxNetworkInspection::Closed => SandboxFeature::NetworkDeny,
        SandboxNetworkInspection::Domains { .. } => SandboxFeature::NetworkAllowlist,
    };
    [
        SandboxFeature::Filesystem,
        network_boundary,
        SandboxFeature::DescriptorIsolation,
        SandboxFeature::ProcessIsolation,
        SandboxFeature::KernelSurface,
        SandboxFeature::PrivilegeIsolation,
    ]
    .into_iter()
    .all(|feature| capabilities.claim(feature).is_enforced())
}

impl SandboxInspection {
    /// Builds the redacted inspection a record keeps.
    ///
    /// # Errors
    ///
    /// A disable reason must be bounded, a report may not call itself confined
    /// while also saying why confinement is off, an enforcing report must
    /// enforce the boundaries a confinement depends on, and its effective plan
    /// must agree with that claim.
    ///
    /// Each of those is a fact the record holds about itself, so the constructor
    /// refuses it here rather than trusting every caller to have checked first.
    /// `crucible-sandbox` reduces an effective plan and hands the record to this
    /// constructor, so a live build refuses a reason that is not bounded, a
    /// claim that contradicts its own reason, and an effective plan that
    /// contradicts the claim before it gets here, and reaches the boundary
    /// refusal here without restating it; a journal line or a checkpoint file
    /// is read back by a build that never sees the policy at all: a record
    /// that cannot be written dishonest is worth more here than a rule a
    /// caller has to remember.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: SandboxId,
        backend: SandboxBackendIdentity,
        capabilities: SandboxCapabilities,
        requested_plan: SandboxPlanInspection,
        plan: SandboxPlanInspection,
        requested_policy_digest: [u8; 32],
        policy_digest: [u8; 32],
        manifest_digest: [u8; 32],
        confined: bool,
        disabled_reason: Option<Box<str>>,
        cleanup: SandboxCleanup,
    ) -> Result<Self, SandboxCapabilityError> {
        if disabled_reason
            .as_ref()
            .is_some_and(|text| text.is_empty() || text.len() > MAX_SANDBOX_BACKEND_WORD_BYTES)
            || confined == disabled_reason.is_some()
            || plan.enabled() != confined
            || !confines_as_claimed(confined, plan.network(), &capabilities)
        {
            return Err(SandboxCapabilityError::InvalidInspection);
        }
        Ok(Self {
            id,
            backend,
            capabilities,
            requested_plan,
            plan,
            requested_policy_digest,
            policy_digest,
            manifest_digest,
            confined,
            disabled_reason,
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

    /// Bounded redacted plan submitted before parent restrictions were inherited.
    #[must_use]
    pub const fn requested_plan(&self) -> &SandboxPlanInspection {
        &self.requested_plan
    }

    /// Bounded redacted effective plan.
    #[must_use]
    pub const fn plan(&self) -> &SandboxPlanInspection {
        &self.plan
    }

    /// Requested policy identity before parent restrictions were inherited.
    #[must_use]
    pub const fn requested_policy_digest(&self) -> [u8; 32] {
        self.requested_policy_digest
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

    /// Why the host deliberately disabled confinement.
    #[must_use]
    pub fn disabled_reason(&self) -> Option<&str> {
        self.disabled_reason.as_deref()
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

impl std::fmt::Debug for SandboxCheckpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxCheckpoint")
            .field("id", &self.id)
            .field("backend", &self.backend)
            .field("capabilities", &self.capabilities)
            .field("enabled", &self.enabled)
            .field("network", &self.network)
            .field("policy_digest", &"[sha256]")
            .field("manifest_digest", &"[sha256]")
            .field("confined", &self.confined)
            .finish()
    }
}

/// Minimal redacted sandbox identity retained by an execution checkpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxCheckpoint {
    id: SandboxId,
    backend: SandboxBackendIdentity,
    capabilities: SandboxCapabilities,
    enabled: bool,
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
            enabled: inspection.plan.enabled,
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
    /// A record that calls itself confined without its required network and
    /// kernel capabilities is refused.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        id: SandboxId,
        backend: SandboxBackendIdentity,
        capabilities: SandboxCapabilities,
        enabled: bool,
        network: SandboxNetworkInspection,
        policy_digest: [u8; 32],
        manifest_digest: [u8; 32],
        confined: bool,
    ) -> Result<Self, SandboxCheckpointError> {
        if enabled != confined
            || matches!(network, SandboxNetworkInspection::Domains { allowed, denied, unix_sockets, .. }
                if [allowed, denied, unix_sockets].into_iter().any(|count| count > MAX_SANDBOX_NETWORK_RULES))
            || !confines_as_claimed(confined, network, &capabilities)
        {
            return Err(SandboxCheckpointError::InvalidInspection);
        }
        Ok(Self {
            id,
            backend,
            capabilities,
            enabled,
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

    /// Whether confinement was required before interruption.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
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
            && self.enabled == live.enabled
            && self.network == live.network
            && self.policy_digest == live.policy_digest
            && self.manifest_digest == live.manifest_digest
            && self.confined == live.confined
            && SandboxFeature::ALL
                .into_iter()
                .all(|feature| live.capabilities.claim(feature) >= self.capabilities.claim(feature))
    }
}

impl SandboxLifecycle {
    /// Stable persisted spelling.
    ///
    /// The session log writes this word, so the record owns it: a build that
    /// renames the variant renames the word in the same place.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PolicyResolved => "policy_resolved",
            Self::Prepared => "prepared",
            Self::Materialized => "materialized",
            Self::ReleaseIntent => "release_intent",
            Self::CommandReleased => "command_released",
            Self::OwnerTransferred => "owner_transferred",
            Self::CommandStarted => "command_started",
            Self::CommandFinished => "command_finished",
            Self::PublicationStarted => "publication_started",
            Self::Published => "published",
            Self::RolledBack => "rolled_back",
            Self::Refused => "refused",
            Self::Quarantined => "quarantined",
        }
    }
}

impl SandboxFailureKind {
    /// Stable persisted spelling.
    ///
    /// The session log writes this word, so the record owns it: a build that
    /// renames the variant renames the word in the same place.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::BackendUnavailable => "backend_unavailable",
            Self::Guardrail => "guardrail",
            Self::Concurrency => "concurrency",
            Self::Materialization => "materialization",
            Self::Spawn => "spawn",
            Self::Lifecycle => "lifecycle",
            Self::Audit => "audit",
            Self::InvalidInput => "invalid_input",
        }
    }
}

impl SandboxFailurePhase {
    /// Stable persisted spelling.
    ///
    /// The session log writes this word, so the record owns it: a build that
    /// renames the variant renames the word in the same place.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Materialize => "materialize",
            Self::Start => "start",
            Self::Execute => "execute",
        }
    }
}

impl SandboxCommandStage {
    /// Stable persisted spelling.
    ///
    /// The session log writes this word, so the record owns it: a build that
    /// renames the variant renames the word in the same place.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Effective => "effective",
        }
    }
}

impl SandboxGuardrailDecision {
    /// Stable persisted spelling.
    ///
    /// The session log writes this word, so the record owns it: a build that
    /// renames the variant renames the word in the same place.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Denied => "denied",
        }
    }
}

impl SandboxViolation {
    /// Stable persisted spelling.
    ///
    /// The session log writes this word, so the record owns it: a build that
    /// renames the variant renames the word in the same place.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CommandTime => "command_time",
            Self::Output => "output",
        }
    }
}

impl SandboxCleanup {
    /// Stable persisted spelling.
    ///
    /// The session log writes this word, so the record owns it: a build that
    /// renames the variant renames the word in the same place.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Complete => "complete",
            Self::Failed => "failed",
        }
    }
}

/// Why a stored checkpoint record refused what it was handed.
///
/// The record's other constructors report through
/// [`SandboxCapabilityError`], because a backend identity is a capability
/// question. This one is about the record disagreeing with itself, which is a
/// different mistake with a different owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SandboxCheckpointError {
    /// The record is not internally consistent with itself.
    #[error("sandbox checkpoint record is not internally consistent")]
    InvalidInspection,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend() -> SandboxBackendIdentity {
        SandboxBackendIdentity::new(
            SandboxBackendId::new("linux-bubblewrap").expect("a bounded backend name"),
            "0.11.1",
            SandboxBackendProvenance::System,
            Some([7; 32]),
        )
        .expect("a bounded backend identity")
    }

    fn confined() -> SandboxCapabilities {
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

    fn plan() -> SandboxPlanInspection {
        SandboxPlanInspection::new(
            true,
            vec![SandboxRootInspection::new(
                [4; 32],
                SandboxFilesystemAccess::ReadWrite,
                SandboxFilesystemProvenance::Workspace,
            )]
            .into_boxed_slice(),
            [5; 32],
            SandboxNetworkInspection::Closed,
            SandboxResourceLimits::default(),
            [6; 32],
            7,
            true,
            false,
            8,
        )
    }

    fn inspection() -> SandboxInspection {
        SandboxInspection::new(
            SandboxId::new(),
            backend(),
            confined(),
            plan(),
            plan(),
            [1; 32],
            [2; 32],
            [3; 32],
            true,
            None,
            SandboxCleanup::Pending,
        )
        .expect("a consistent inspection")
    }

    #[test]
    fn backend_identity_is_bounded_and_does_not_debug_its_digest() {
        assert!(SandboxBackendId::new("linux-bubblewrap").is_ok());
        assert!(SandboxBackendId::new("").is_err());
        assert!(SandboxBackendId::new("x".repeat(MAX_SANDBOX_BACKEND_ID_BYTES + 1)).is_err());

        let identity = backend();
        let shown = format!("{identity:?}");
        assert!(shown.contains("[sha256]"));
        assert!(!shown.contains("7, 7"));
    }

    #[test]
    fn capability_claims_distinguish_observation_from_enforcement() {
        let capabilities = SandboxCapabilities::none()
            .with(SandboxFeature::MemoryLimit, SandboxCapability::Observed)
            .with(SandboxFeature::Filesystem, SandboxCapability::Enforced);

        assert_eq!(
            capabilities.claim(SandboxFeature::MemoryLimit),
            SandboxCapability::Observed
        );
        assert!(
            !capabilities
                .claim(SandboxFeature::MemoryLimit)
                .is_enforced()
        );
        assert!(capabilities.claim(SandboxFeature::Filesystem).is_enforced());
    }

    #[test]
    fn a_stored_inspection_redacts_the_digests_its_fields_are_named_for() {
        let shown = format!("{:?}", inspection());
        assert!(shown.contains("[sha256]"), "{shown}");
        assert!(!shown.contains("4, 4, 4"), "{shown}");
        assert!(!shown.contains("7, 7"), "{shown}");

        let checkpoint = SandboxCheckpoint::from_inspection(&inspection());
        let shown = format!("{checkpoint:?}");
        assert!(shown.contains("[sha256]"), "{shown}");
        assert!(!shown.contains("1, 1, 1"), "{shown}");
    }

    #[test]
    fn a_checkpoint_record_refuses_what_a_confinement_claim_cannot_support() {
        assert_eq!(
            SandboxCheckpoint::restore(
                SandboxId::new(),
                backend(),
                SandboxCapabilities::none(),
                true,
                SandboxNetworkInspection::Closed,
                [1; 32],
                [2; 32],
                true,
            ),
            Err(SandboxCheckpointError::InvalidInspection)
        );
        assert_eq!(
            SandboxCheckpoint::restore(
                SandboxId::new(),
                backend(),
                confined(),
                false,
                SandboxNetworkInspection::Domains {
                    allowed: MAX_SANDBOX_NETWORK_RULES + 1,
                    denied: 0,
                    local_binding: false,
                    unix_sockets: 0,
                },
                [1; 32],
                [2; 32],
                false,
            ),
            Err(SandboxCheckpointError::InvalidInspection)
        );
        assert_eq!(
            SandboxInspection::new(
                SandboxId::new(),
                backend(),
                confined(),
                plan(),
                plan(),
                [1; 32],
                [2; 32],
                [3; 32],
                true,
                Some("because".into()),
                SandboxCleanup::Pending,
            ),
            Err(SandboxCapabilityError::InvalidInspection)
        );
    }

    /// The boundaries a claim of an enforcing kernel boundary rests on.
    ///
    /// Stated apart from the record's own list on purpose: a test that read the
    /// list it is checking would agree with a wrong one.
    const ESSENTIAL: [SandboxFeature; 6] = [
        SandboxFeature::Filesystem,
        SandboxFeature::NetworkDeny,
        SandboxFeature::DescriptorIsolation,
        SandboxFeature::ProcessIsolation,
        SandboxFeature::KernelSurface,
        SandboxFeature::PrivilegeIsolation,
    ];

    #[test]
    fn a_report_claiming_confinement_is_refused_when_its_own_fields_cannot_support_it() {
        // The reduction `crucible-sandbox` performs before it hands the values
        // over, reduced here to the two fields the claim is judged against.
        let reduced = |enabled: bool, network: SandboxNetworkInspection| {
            SandboxPlanInspection::new(
                enabled,
                vec![SandboxRootInspection::new(
                    [4; 32],
                    SandboxFilesystemAccess::ReadWrite,
                    SandboxFilesystemProvenance::Workspace,
                )]
                .into_boxed_slice(),
                [5; 32],
                network,
                SandboxResourceLimits::default(),
                [6; 32],
                7,
                true,
                false,
                8,
            )
        };
        let domains = || {
            reduced(
                true,
                SandboxNetworkInspection::Domains {
                    allowed: 1,
                    denied: 0,
                    local_binding: false,
                    unix_sockets: 0,
                },
            )
        };
        let report = |capabilities: SandboxCapabilities,
                      plan: SandboxPlanInspection,
                      confined: bool,
                      disabled_reason: Option<&str>| {
            SandboxInspection::new(
                SandboxId::new(),
                backend(),
                capabilities,
                plan.clone(),
                plan,
                [1; 32],
                [2; 32],
                [3; 32],
                confined,
                disabled_reason.map(Into::into),
                SandboxCleanup::Pending,
            )
        };
        let closed = || reduced(true, SandboxNetworkInspection::Closed);

        // A backend that negotiated nothing cannot be an enforcing boundary,
        // however the report names itself.
        assert_eq!(
            report(SandboxCapabilities::none(), closed(), true, None),
            Err(SandboxCapabilityError::InvalidInspection)
        );
        // And one that is the same backend, watching a boundary it claims to
        // enforce: exactly the six are required, and nothing else is, so
        // weakening a ceiling or an audit claim does not make a report a lie.
        for feature in SandboxFeature::ALL {
            let observed = confined().with(feature, SandboxCapability::Observed);
            assert_eq!(
                report(observed, closed(), true, None).is_err(),
                ESSENTIAL.contains(&feature),
                "{feature:?}"
            );
        }
        // The same six enforced is a record, through the same constructor.
        assert!(report(confined(), closed(), true, None).is_ok());

        // Which network boundary is required follows the effective plan's shape,
        // so a mediated grant is not satisfied by a closed-network claim.
        let mediated = || {
            confined()
                .with(SandboxFeature::NetworkDeny, SandboxCapability::Observed)
                .with(
                    SandboxFeature::NetworkAllowlist,
                    SandboxCapability::Enforced,
                )
        };
        assert_eq!(
            report(mediated(), closed(), true, None),
            Err(SandboxCapabilityError::InvalidInspection),
            "a closed plan requires the deny boundary, and watching it is not enforcing it"
        );
        assert!(report(mediated(), domains(), true, None).is_ok());
        assert_eq!(
            report(
                mediated().with(
                    SandboxFeature::NetworkAllowlist,
                    SandboxCapability::Observed
                ),
                domains(),
                true,
                None
            ),
            Err(SandboxCapabilityError::InvalidInspection)
        );

        // An enforcing claim over a plan that says confinement is off, and a
        // compatibility report that enforces nothing and says why.
        assert_eq!(
            report(
                confined(),
                reduced(false, SandboxNetworkInspection::Closed),
                true,
                None
            ),
            Err(SandboxCapabilityError::InvalidInspection)
        );
        assert!(
            report(
                SandboxCapabilities::none(),
                reduced(false, SandboxNetworkInspection::Closed),
                false,
                Some("the host offers no kernel boundary")
            )
            .is_ok()
        );
    }

    #[test]
    fn a_live_boundary_weaker_than_the_one_a_checkpoint_kept_is_not_compatible() {
        let saved = SandboxCheckpoint::from_inspection(&inspection());
        assert!(saved.is_compatible_with(&SandboxCheckpoint::from_inspection(&inspection())));

        let weaker = SandboxCheckpoint::restore(
            saved.id(),
            backend(),
            confined().with(SandboxFeature::Filesystem, SandboxCapability::Observed),
            true,
            SandboxNetworkInspection::Closed,
            saved.policy_digest(),
            saved.manifest_digest(),
            true,
        );
        assert_eq!(weaker, Err(SandboxCheckpointError::InvalidInspection));
    }

    #[test]
    fn the_matrix_is_one_order_with_one_name_each() {
        let mut names = Vec::with_capacity(SandboxFeature::COUNT);
        for (index, feature) in SandboxFeature::ALL.into_iter().enumerate() {
            names.push(feature.as_str());
            assert_eq!(feature.index(), index, "the matrix order is the index");
        }
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "a feature is spelled once");
        assert_eq!(
            names.len(),
            SandboxCapabilities::none().iter().count(),
            "every feature is readable back in that order"
        );
    }
}
