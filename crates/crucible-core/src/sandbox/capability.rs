//! Exact backend identity and capability claims.

use std::fmt;

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
}

/// Independently negotiated sandbox features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxFeature {
    /// Read/write filesystem view and protected carve-outs.
    Filesystem,
    /// A network policy that permits no egress.
    NetworkDeny,
    /// Exact host/port/DNS constrained egress.
    NetworkAllowlist,
    /// No host descriptors other than declared standard streams.
    DescriptorIsolation,
    /// User, PID, IPC, and network namespaces or an equivalent isolation set.
    ProcessIsolation,
    /// Minimal procfs and device view.
    KernelSurface,
    /// No-new-privileges and dropped process capabilities.
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
    claims: [SandboxCapability; 26],
}

impl SandboxCapabilities {
    /// A backend that claims nothing.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            claims: [SandboxCapability::Unsupported; 26],
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
}

impl Default for SandboxCapabilities {
    fn default() -> Self {
        Self::none()
    }
}

impl SandboxFeature {
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
    /// Backend identifiers are stable symbolic words.
    #[error(
        "sandbox backend id must be 1..={MAX_SANDBOX_BACKEND_ID_BYTES} ASCII letters, digits, '.', '-' or '_'"
    )]
    InvalidBackendId,
    /// Versions are short non-empty diagnostic values.
    #[error("sandbox backend version must be 1..={MAX_SANDBOX_BACKEND_WORD_BYTES} bytes")]
    InvalidBackendVersion,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_identity_is_bounded_and_does_not_debug_its_digest() {
        assert!(SandboxBackendId::new("linux-bubblewrap").is_ok());
        assert!(SandboxBackendId::new("").is_err());
        assert!(SandboxBackendId::new("x".repeat(MAX_SANDBOX_BACKEND_ID_BYTES + 1)).is_err());

        let identity = SandboxBackendIdentity::new(
            SandboxBackendId::new("linux-bubblewrap").expect("valid id"),
            "0.11.1",
            SandboxBackendProvenance::System,
            Some([7; 32]),
        )
        .expect("valid identity");
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
}
