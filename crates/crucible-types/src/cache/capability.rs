//! The words a cache capability, a cache policy and a cache fact share.
//!
//! Which mechanisms a route supports is a record `crucible-models` owns; the
//! names of those mechanisms, of retention classes and of reporting depth are
//! what a policy is written in and what a session log keeps, so they sit here.

/// Whether the exact adapter/endpoint/model combination is known to cache.
///
/// `Unknown` is deliberately not a permissive answer. It is used for custom
/// compatible endpoints and model revisions whose behavior has not been
/// verified; no cache-specific request field may be inferred from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheSupport {
    /// No reviewed record establishes either answer.
    Unknown,
    /// A reviewed record establishes that no supported mechanism exists.
    Unsupported,
    /// At least one reviewed adapter/model mechanism remains after intersection.
    Supported,
}

/// Provider-neutral cache mechanisms, in reviewed preference order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptCacheMechanism {
    /// The provider may cache automatically and only reports usage.
    ProviderManagedUsageOnly,
    /// The provider automatically reuses an eligible stable prefix.
    AutomaticPrefix,
    /// The request marks legal stable-prefix boundaries.
    ExplicitBreakpoints,
    /// The request references a separately managed cached-content resource.
    PersistentContent,
}

impl PromptCacheMechanism {
    /// Canonical configuration spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderManagedUsageOnly => "providerManagedUsageOnly",
            Self::AutomaticPrefix => "automaticPrefix",
            Self::ExplicitBreakpoints => "explicitBreakpoints",
            Self::PersistentContent => "persistentContent",
        }
    }
}

impl std::str::FromStr for PromptCacheMechanism {
    type Err = PromptCacheCapabilityWordError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "providerManagedUsageOnly" => Ok(Self::ProviderManagedUsageOnly),
            "automaticPrefix" => Ok(Self::AutomaticPrefix),
            "explicitBreakpoints" => Ok(Self::ExplicitBreakpoints),
            "persistentContent" => Ok(Self::PersistentContent),
            _ => Err(PromptCacheCapabilityWordError),
        }
    }
}

/// Retention classes policy may select without naming a vendor TTL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptCacheRetentionClass {
    /// Do not ask the provider to alter its ordinary retention.
    ProviderDefault,
    /// A verified short-lived cache class.
    Ephemeral,
    /// A verified longer-lived class requiring user authority.
    Extended,
}

impl PromptCacheRetentionClass {
    /// Canonical configuration spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderDefault => "providerDefault",
            Self::Ephemeral => "ephemeral",
            Self::Extended => "extended",
        }
    }
}

impl std::str::FromStr for PromptCacheRetentionClass {
    type Err = PromptCacheCapabilityWordError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "providerDefault" => Ok(Self::ProviderDefault),
            "ephemeral" => Ok(Self::Ephemeral),
            "extended" => Ok(Self::Extended),
            _ => Err(PromptCacheCapabilityWordError),
        }
    }
}

/// A non-canonical capability/policy word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unknown prompt-cache capability value")]
pub struct PromptCacheCapabilityWordError;

/// Which cache-specific input buckets a provider can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PromptCacheUsageReporting {
    /// No cache-specific usage is documented.
    None,
    /// Cache reads can be distinguished.
    ReadTokens,
    /// Cache reads and writes/creation can both be distinguished.
    ReadAndWriteTokens,
}
