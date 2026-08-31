//! Prompt-cache authority, defaults, and narrowing.

use std::fmt;
use std::str::FromStr;

use super::{PromptCacheMechanism, PromptCacheRetentionClass};

/// Largest user-controlled namespace retained in policy or identity metadata.
pub const MAX_PROMPT_CACHE_NAMESPACE_BYTES: usize = 64;

/// Largest explicit cache-retention ceiling Crucible accepts: one year.
pub const MAX_PROMPT_CACHE_RETENTION_SECONDS: u32 = 365 * 24 * 60 * 60;

/// Version of the policy semantics compiled into the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCachePolicyVersion(&'static str);

impl PromptCachePolicyVersion {
    /// Current merge, narrowing, and selection contract.
    pub const CURRENT: Self = Self("prompt-cache-policy-v1");

    /// Stable version label carried into facts and journals.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Where one effective policy ceiling came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCachePolicySource {
    /// No layer stated a value.
    Default,
    /// A file outside the workspace, owned by the user.
    User,
    /// A project or project-local layer, which may only narrow.
    Workspace,
    /// A run/SDK choice, also held under the inherited ceiling.
    Run,
}

/// Whether Crucible adds cache controls to provider requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheMode {
    /// Add no Crucible cache control, but retain reported provider usage.
    ObserveOnly,
    /// Select the first eligible reviewed mechanism and otherwise fall back.
    Prefer,
    /// Require an eligible reviewed mechanism before sending.
    Require,
    /// Require a real documented provider opt-out before sending.
    Prohibit,
}

impl PromptCacheMode {
    /// Canonical configuration spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ObserveOnly => "observeOnly",
            Self::Prefer => "prefer",
            Self::Require => "require",
            Self::Prohibit => "prohibit",
        }
    }
}

impl FromStr for PromptCacheMode {
    type Err = PromptCachePolicyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "observeOnly" => Ok(Self::ObserveOnly),
            "prefer" => Ok(Self::Prefer),
            "require" => Ok(Self::Require),
            "prohibit" => Ok(Self::Prohibit),
            _ => Err(PromptCachePolicyError::UnknownMode),
        }
    }
}

/// The broadest identity scope a selected cache may share across.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PromptCacheIsolation {
    /// One execution only.
    Run,
    /// One conversation session.
    Session,
    /// One trusted workspace.
    Workspace,
    /// One user/tenant identity.
    User,
}

impl PromptCacheIsolation {
    /// Canonical configuration spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Session => "session",
            Self::Workspace => "workspace",
            Self::User => "user",
        }
    }
}

impl FromStr for PromptCacheIsolation {
    type Err = PromptCachePolicyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "run" => Ok(Self::Run),
            "session" => Ok(Self::Session),
            "workspace" => Ok(Self::Workspace),
            "user" => Ok(Self::User),
            _ => Err(PromptCachePolicyError::UnknownIsolation),
        }
    }
}

/// Authority over separately managed remote cached-content resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCachePersistentMode {
    /// Neither resolve nor create a persistent resource.
    Forbid,
    /// Reuse an already authorized matching resource, but never create one.
    Reuse,
    /// Create, reuse, or renew within the other ceilings.
    Create,
    /// Carry creation authority and fail when no resource can be ready.
    Require,
}

impl PromptCachePersistentMode {
    /// Canonical configuration spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Forbid => "forbid",
            Self::Reuse => "reuse",
            Self::Create => "create",
            Self::Require => "require",
        }
    }

    const fn authority(self) -> u8 {
        match self {
            Self::Forbid => 0,
            Self::Reuse => 1,
            Self::Create | Self::Require => 2,
        }
    }
}

impl FromStr for PromptCachePersistentMode {
    type Err = PromptCachePolicyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "forbid" => Ok(Self::Forbid),
            "reuse" => Ok(Self::Reuse),
            "create" => Ok(Self::Create),
            "require" => Ok(Self::Require),
            _ => Err(PromptCachePolicyError::UnknownPersistentMode),
        }
    }
}

/// A hard upper bound on provider retention behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCacheRetention {
    class: PromptCacheRetentionClass,
    maximum_seconds: Option<u32>,
}

impl PromptCacheRetention {
    /// No provider retention override.
    #[must_use]
    pub const fn provider_default() -> Self {
        Self {
            class: PromptCacheRetentionClass::ProviderDefault,
            maximum_seconds: None,
        }
    }

    /// A bounded short-lived class.
    ///
    /// # Errors
    ///
    /// Returns an error when `maximum_seconds` is zero or exceeds the global
    /// retention ceiling.
    pub fn ephemeral(maximum_seconds: u32) -> Result<Self, PromptCachePolicyError> {
        Self::bounded(PromptCacheRetentionClass::Ephemeral, maximum_seconds)
    }

    /// A bounded longer-lived class, which configuration restricts to user origin.
    ///
    /// # Errors
    ///
    /// Returns an error when `maximum_seconds` is zero or exceeds the global
    /// retention ceiling.
    pub fn extended(maximum_seconds: u32) -> Result<Self, PromptCachePolicyError> {
        Self::bounded(PromptCacheRetentionClass::Extended, maximum_seconds)
    }

    fn bounded(
        class: PromptCacheRetentionClass,
        maximum_seconds: u32,
    ) -> Result<Self, PromptCachePolicyError> {
        if maximum_seconds == 0 || maximum_seconds > MAX_PROMPT_CACHE_RETENTION_SECONDS {
            return Err(PromptCachePolicyError::RetentionOutOfRange);
        }
        Ok(Self {
            class,
            maximum_seconds: Some(maximum_seconds),
        })
    }

    /// Requested provider-neutral retention class.
    #[must_use]
    pub const fn class(self) -> PromptCacheRetentionClass {
        self.class
    }

    /// Hard maximum, absent only for the unchanged provider default.
    #[must_use]
    pub const fn maximum_seconds(self) -> Option<u32> {
        self.maximum_seconds
    }

    fn narrowed(self, wanted: Self) -> Self {
        let class = if retention_rank(self.class) <= retention_rank(wanted.class) {
            self.class
        } else {
            wanted.class
        };
        let maximum_seconds = match (self.maximum_seconds, wanted.maximum_seconds) {
            (Some(held), Some(wanted)) => Some(held.min(wanted)),
            (bound, None) | (None, bound) => bound,
        };

        if class == PromptCacheRetentionClass::ProviderDefault {
            Self::provider_default()
        } else {
            Self {
                class,
                maximum_seconds,
            }
        }
    }
}

const fn retention_rank(class: PromptCacheRetentionClass) -> u8 {
    match class {
        PromptCacheRetentionClass::ProviderDefault => 0,
        PromptCacheRetentionClass::Ephemeral => 1,
        PromptCacheRetentionClass::Extended => 2,
    }
}

/// A compact allowlist over the four neutral mechanism kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCacheMechanisms(u8);

impl PromptCacheMechanisms {
    /// No mechanism is permitted.
    pub const NONE: Self = Self(0);
    /// Every neutral mechanism is permitted.
    pub const ALL: Self = Self(0b1111);

    /// A set containing one mechanism.
    #[must_use]
    pub const fn one(mechanism: PromptCacheMechanism) -> Self {
        Self(bit(mechanism))
    }

    /// Adds one mechanism.
    #[must_use]
    pub const fn with(self, mechanism: PromptCacheMechanism) -> Self {
        Self(self.0 | bit(mechanism))
    }

    /// The intersection of two allowlists.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Whether this set permits a mechanism.
    #[must_use]
    pub const fn contains(self, mechanism: PromptCacheMechanism) -> bool {
        self.0 & bit(mechanism) != 0
    }

    /// Whether every mechanism has been filtered out.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

const fn bit(mechanism: PromptCacheMechanism) -> u8 {
    match mechanism {
        PromptCacheMechanism::ProviderManagedUsageOnly => 1,
        PromptCacheMechanism::AutomaticPrefix => 2,
        PromptCacheMechanism::ExplicitBreakpoints => 4,
        PromptCacheMechanism::PersistentContent => 8,
    }
}

/// A bounded opaque label contributing to cache scope identity.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PromptCacheNamespace {
    bytes: [u8; MAX_PROMPT_CACHE_NAMESPACE_BYTES],
    length: u8,
}

impl PromptCacheNamespace {
    /// Accepts only a short portable label, never an arbitrary provider key.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, oversized, or non-portable label.
    pub fn new(value: impl AsRef<str>) -> Result<Self, PromptCachePolicyError> {
        let value = value.as_ref().as_bytes();
        if value.is_empty()
            || value.len() > MAX_PROMPT_CACHE_NAMESPACE_BYTES
            || !value
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(PromptCachePolicyError::InvalidNamespace);
        }

        let mut bytes = [0; MAX_PROMPT_CACHE_NAMESPACE_BYTES];
        let Some(target) = bytes.get_mut(..value.len()) else {
            return Err(PromptCachePolicyError::InvalidNamespace);
        };
        target.copy_from_slice(value);
        Ok(Self {
            bytes,
            length: u8::try_from(value.len())
                .map_err(|_| PromptCachePolicyError::InvalidNamespace)?,
        })
    }

    /// The validated label.
    #[must_use]
    pub fn as_str(&self) -> &str {
        let bytes = self
            .bytes
            .get(..usize::from(self.length))
            .unwrap_or_default();
        std::str::from_utf8(bytes).unwrap_or_default()
    }
}

impl fmt::Debug for PromptCacheNamespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PromptCacheNamespace")
            .field(&"[redacted]")
            .finish()
    }
}

/// A contradictory inherited policy that preparation must report, not erase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCachePolicyConflict {
    /// A user requirement met a descendant prohibition.
    RequiredAndProhibited,
    /// Narrowing removed every mechanism while caching remained required.
    RequiredWithoutMechanism,
}

/// Invalid policy construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PromptCachePolicyError {
    /// `PromptCacheMode` received no canonical word.
    #[error("unknown prompt-cache mode")]
    UnknownMode,
    /// `PromptCacheIsolation` received no canonical word.
    #[error("unknown prompt-cache isolation scope")]
    UnknownIsolation,
    /// `PromptCachePersistentMode` received no canonical word.
    #[error("unknown prompt-cache persistent-resource mode")]
    UnknownPersistentMode,
    /// The namespace was empty, too long, or not a portable opaque label.
    #[error(
        "prompt-cache namespace must be 1..={MAX_PROMPT_CACHE_NAMESPACE_BYTES} ASCII letters, digits, '.', '-' or '_'"
    )]
    InvalidNamespace,
    /// The retention bound was zero or exceeded the hard maximum.
    #[error("prompt-cache retention must be 1..={MAX_PROMPT_CACHE_RETENTION_SECONDS} seconds")]
    RetentionOutOfRange,
    /// Privacy prohibition cannot coexist with remote-resource authority.
    #[error("prompt-cache prohibit mode cannot authorize persistent resources")]
    ProhibitWithPersistentResource,
}

/// The source of every independently narrowed policy field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCachePolicySources {
    mode: PromptCachePolicySource,
    mechanisms: PromptCachePolicySource,
    isolation: PromptCachePolicySource,
    retention: PromptCachePolicySource,
    persistent_resources: PromptCachePolicySource,
    namespace: PromptCachePolicySource,
}

impl PromptCachePolicySources {
    const DEFAULT: Self = Self {
        mode: PromptCachePolicySource::Default,
        mechanisms: PromptCachePolicySource::Default,
        isolation: PromptCachePolicySource::Default,
        retention: PromptCachePolicySource::Default,
        persistent_resources: PromptCachePolicySource::Default,
        namespace: PromptCachePolicySource::Default,
    };

    /// Origin of the selected mode ceiling.
    #[must_use]
    pub const fn mode(self) -> PromptCachePolicySource {
        self.mode
    }

    /// Origin of the mechanism allowlist ceiling.
    #[must_use]
    pub const fn mechanisms(self) -> PromptCachePolicySource {
        self.mechanisms
    }

    /// Origin of the isolation ceiling.
    #[must_use]
    pub const fn isolation(self) -> PromptCachePolicySource {
        self.isolation
    }

    /// Origin of the retention ceiling.
    #[must_use]
    pub const fn retention(self) -> PromptCachePolicySource {
        self.retention
    }

    /// Origin of persistent-resource authority.
    #[must_use]
    pub const fn persistent_resources(self) -> PromptCachePolicySource {
        self.persistent_resources
    }

    /// Origin of the optional namespace.
    #[must_use]
    pub const fn namespace(self) -> PromptCachePolicySource {
        self.namespace
    }
}

/// Fully resolved prompt-cache policy, compact enough to travel in `RunPolicy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCachePolicy {
    mode: PromptCacheMode,
    allowed_mechanisms: PromptCacheMechanisms,
    isolation: PromptCacheIsolation,
    retention: PromptCacheRetention,
    persistent_resources: PromptCachePersistentMode,
    namespace: Option<PromptCacheNamespace>,
    sources: PromptCachePolicySources,
    version: PromptCachePolicyVersion,
    conflict: Option<PromptCachePolicyConflict>,
}

impl Default for PromptCachePolicy {
    fn default() -> Self {
        Self {
            mode: PromptCacheMode::Prefer,
            allowed_mechanisms: PromptCacheMechanisms::ALL,
            isolation: PromptCacheIsolation::Session,
            retention: PromptCacheRetention::provider_default(),
            persistent_resources: PromptCachePersistentMode::Forbid,
            namespace: None,
            sources: PromptCachePolicySources::DEFAULT,
            version: PromptCachePolicyVersion::CURRENT,
            conflict: None,
        }
    }
}

impl PromptCachePolicy {
    /// Replaces the requested mode from one authority source.
    #[must_use]
    pub const fn with_mode(self, mode: PromptCacheMode) -> Self {
        self.with_mode_from(mode, PromptCachePolicySource::Run)
    }

    /// Replaces the requested mode and records its origin.
    #[must_use]
    pub const fn with_mode_from(
        mut self,
        mode: PromptCacheMode,
        source: PromptCachePolicySource,
    ) -> Self {
        self.mode = mode;
        self.sources.mode = source;
        self
    }

    /// Replaces the requested isolation from run/SDK construction.
    #[must_use]
    pub const fn with_isolation(self, isolation: PromptCacheIsolation) -> Self {
        self.with_isolation_from(isolation, PromptCachePolicySource::Run)
    }

    /// Replaces isolation and records its origin.
    #[must_use]
    pub const fn with_isolation_from(
        mut self,
        isolation: PromptCacheIsolation,
        source: PromptCachePolicySource,
    ) -> Self {
        self.isolation = isolation;
        self.sources.isolation = source;
        self
    }

    /// Replaces retention from run/SDK construction.
    #[must_use]
    pub const fn with_retention(self, retention: PromptCacheRetention) -> Self {
        self.with_retention_from(retention, PromptCachePolicySource::Run)
    }

    /// Replaces retention and records its origin.
    #[must_use]
    pub const fn with_retention_from(
        mut self,
        retention: PromptCacheRetention,
        source: PromptCachePolicySource,
    ) -> Self {
        self.retention = retention;
        self.sources.retention = source;
        self
    }

    /// Replaces persistent-resource authority from run/SDK construction.
    #[must_use]
    pub const fn with_persistent_resources(self, mode: PromptCachePersistentMode) -> Self {
        self.with_persistent_resources_from(mode, PromptCachePolicySource::Run)
    }

    /// Replaces resource authority and records its origin.
    #[must_use]
    pub const fn with_persistent_resources_from(
        mut self,
        mode: PromptCachePersistentMode,
        source: PromptCachePolicySource,
    ) -> Self {
        self.persistent_resources = mode;
        self.sources.persistent_resources = source;
        self
    }

    /// Replaces the mechanism allowlist from run/SDK construction.
    #[must_use]
    pub const fn allowing(self, mechanisms: PromptCacheMechanisms) -> Self {
        self.allowing_from(mechanisms, PromptCachePolicySource::Run)
    }

    /// Replaces the allowlist and records its origin.
    #[must_use]
    pub const fn allowing_from(
        mut self,
        mechanisms: PromptCacheMechanisms,
        source: PromptCachePolicySource,
    ) -> Self {
        self.allowed_mechanisms = mechanisms;
        self.sources.mechanisms = source;
        self
    }

    /// Replaces the bounded namespace from run/SDK construction.
    #[must_use]
    pub const fn with_namespace(self, namespace: PromptCacheNamespace) -> Self {
        self.with_namespace_from(namespace, PromptCachePolicySource::Run)
    }

    /// Replaces the namespace and records its origin.
    #[must_use]
    pub const fn with_namespace_from(
        mut self,
        namespace: PromptCacheNamespace,
        source: PromptCachePolicySource,
    ) -> Self {
        self.namespace = Some(namespace);
        self.sources.namespace = source;
        self
    }

    /// Holds a descendant policy under every axis of this policy.
    #[must_use]
    pub fn narrowed(self, wanted: Self) -> Self {
        let (mode, mode_source, mode_conflict) = narrow_mode(self, wanted);
        let (isolation, isolation_source) = if self.isolation <= wanted.isolation {
            (self.isolation, self.sources.isolation)
        } else {
            (wanted.isolation, wanted.sources.isolation)
        };
        let retention = self.retention.narrowed(wanted.retention);
        let retention_source = if retention == self.retention {
            self.sources.retention
        } else {
            wanted.sources.retention
        };
        let persistent_resources =
            narrow_persistent(self.persistent_resources, wanted.persistent_resources);
        let persistent_source = if persistent_resources == self.persistent_resources {
            self.sources.persistent_resources
        } else {
            wanted.sources.persistent_resources
        };
        let allowed_mechanisms = self
            .allowed_mechanisms
            .intersection(wanted.allowed_mechanisms);
        let mechanism_source = if allowed_mechanisms == self.allowed_mechanisms {
            self.sources.mechanisms
        } else {
            wanted.sources.mechanisms
        };
        let conflict = mode_conflict
            .or(self.conflict)
            .or(wanted.conflict)
            .or_else(|| {
                (mode == PromptCacheMode::Require && allowed_mechanisms.is_empty())
                    .then_some(PromptCachePolicyConflict::RequiredWithoutMechanism)
            });

        Self {
            mode,
            allowed_mechanisms,
            isolation,
            retention,
            persistent_resources,
            namespace: self.namespace.or(wanted.namespace),
            sources: PromptCachePolicySources {
                mode: mode_source,
                mechanisms: mechanism_source,
                isolation: isolation_source,
                retention: retention_source,
                persistent_resources: persistent_source,
                namespace: if self.namespace.is_some() {
                    self.sources.namespace
                } else {
                    wanted.sources.namespace
                },
            },
            version: PromptCachePolicyVersion::CURRENT,
            conflict,
        }
    }

    /// Validates contradictions within one constructed policy.
    ///
    /// # Errors
    ///
    /// Returns an error when caching is prohibited while persistent resources
    /// are still authorized.
    pub fn validate(self) -> Result<Self, PromptCachePolicyError> {
        if self.mode == PromptCacheMode::Prohibit
            && self.persistent_resources != PromptCachePersistentMode::Forbid
        {
            return Err(PromptCachePolicyError::ProhibitWithPersistentResource);
        }
        Ok(self)
    }

    /// Selected mode.
    #[must_use]
    pub const fn mode(self) -> PromptCacheMode {
        self.mode
    }

    /// Permitted neutral mechanisms.
    #[must_use]
    pub const fn allowed_mechanisms(self) -> PromptCacheMechanisms {
        self.allowed_mechanisms
    }

    /// Broadest sharing scope.
    #[must_use]
    pub const fn isolation(self) -> PromptCacheIsolation {
        self.isolation
    }

    /// Retention ceiling.
    #[must_use]
    pub const fn retention(self) -> PromptCacheRetention {
        self.retention
    }

    /// Persistent-resource authority.
    #[must_use]
    pub const fn persistent_resources(self) -> PromptCachePersistentMode {
        self.persistent_resources
    }

    /// Optional opaque namespace.
    #[must_use]
    pub const fn namespace(self) -> Option<PromptCacheNamespace> {
        self.namespace
    }

    /// Source of each effective ceiling.
    #[must_use]
    pub const fn sources(self) -> PromptCachePolicySources {
        self.sources
    }

    /// Policy semantics version.
    #[must_use]
    pub const fn version(self) -> PromptCachePolicyVersion {
        self.version
    }

    /// A conflict carried to request preparation for an explicit failure.
    #[must_use]
    pub const fn conflict(self) -> Option<PromptCachePolicyConflict> {
        self.conflict
    }
}

fn narrow_mode(
    held: PromptCachePolicy,
    wanted: PromptCachePolicy,
) -> (
    PromptCacheMode,
    PromptCachePolicySource,
    Option<PromptCachePolicyConflict>,
) {
    use PromptCacheMode::{ObserveOnly, Prefer, Prohibit, Require};
    match (held.mode, wanted.mode) {
        (Require, Prohibit | ObserveOnly) => (
            Require,
            held.sources.mode,
            Some(PromptCachePolicyConflict::RequiredAndProhibited),
        ),
        (Require, _) => (Require, held.sources.mode, None),
        (Prohibit, _) => (Prohibit, held.sources.mode, None),
        (ObserveOnly, _) => (ObserveOnly, held.sources.mode, None),
        (Prefer, ObserveOnly | Prohibit) => (wanted.mode, wanted.sources.mode, None),
        (Prefer, Prefer | Require) => (Prefer, held.sources.mode, None),
    }
}

const fn narrow_persistent(
    held: PromptCachePersistentMode,
    wanted: PromptCachePersistentMode,
) -> PromptCachePersistentMode {
    if held.authority() < wanted.authority() {
        held
    } else if wanted.authority() < held.authority() {
        wanted
    } else {
        // Equal create authority does not let a descendant turn Create into
        // Require or weaken an inherited Require into Create.
        held
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use super::*;

    #[test]
    fn clean_install_prefers_native_nonpersistent_session_caching() {
        let policy = PromptCachePolicy::default();

        assert_eq!(policy.mode(), PromptCacheMode::Prefer);
        assert_eq!(policy.isolation(), PromptCacheIsolation::Session);
        assert_eq!(policy.retention(), PromptCacheRetention::provider_default());
        assert_eq!(
            policy.persistent_resources(),
            PromptCachePersistentMode::Forbid
        );
        assert_eq!(policy.allowed_mechanisms(), PromptCacheMechanisms::ALL);
        assert_eq!(policy.namespace(), None);
        assert_eq!(policy.conflict(), None);
    }

    #[test]
    fn a_descendant_can_only_narrow_each_authority_axis() {
        let held = PromptCachePolicy::default()
            .with_mode(PromptCacheMode::Prefer)
            .with_isolation(PromptCacheIsolation::Workspace)
            .with_retention(PromptCacheRetention::extended(3_600).unwrap())
            .with_persistent_resources(PromptCachePersistentMode::Create)
            .allowing(PromptCacheMechanisms::ALL);
        let wanted = PromptCachePolicy::default()
            .with_mode(PromptCacheMode::ObserveOnly)
            .with_isolation(PromptCacheIsolation::Run)
            .with_retention(PromptCacheRetention::ephemeral(300).unwrap())
            .with_persistent_resources(PromptCachePersistentMode::Reuse)
            .allowing(PromptCacheMechanisms::one(
                PromptCacheMechanism::ExplicitBreakpoints,
            ));

        let effective = held.narrowed(wanted);

        assert_eq!(effective.mode(), PromptCacheMode::ObserveOnly);
        assert_eq!(effective.isolation(), PromptCacheIsolation::Run);
        assert_eq!(
            effective.retention(),
            PromptCacheRetention::ephemeral(300).unwrap()
        );
        assert_eq!(
            effective.persistent_resources(),
            PromptCachePersistentMode::Reuse
        );
        assert_eq!(
            effective.allowed_mechanisms(),
            PromptCacheMechanisms::one(PromptCacheMechanism::ExplicitBreakpoints)
        );
    }

    #[test]
    fn a_descendant_cannot_turn_caching_on_or_widen_sharing() {
        let held = PromptCachePolicy::default()
            .with_mode(PromptCacheMode::ObserveOnly)
            .with_isolation(PromptCacheIsolation::Run)
            .with_persistent_resources(PromptCachePersistentMode::Forbid);
        let wanted = PromptCachePolicy::default()
            .with_mode(PromptCacheMode::Require)
            .with_isolation(PromptCacheIsolation::User)
            .with_persistent_resources(PromptCachePersistentMode::Require);

        let effective = held.narrowed(wanted);

        assert_eq!(effective.mode(), PromptCacheMode::ObserveOnly);
        assert_eq!(effective.isolation(), PromptCacheIsolation::Run);
        assert_eq!(
            effective.persistent_resources(),
            PromptCachePersistentMode::Forbid
        );
    }

    #[test]
    fn narrowing_an_inherited_requirement_to_prohibit_is_an_explicit_conflict() {
        let held = PromptCachePolicy::default().with_mode(PromptCacheMode::Require);
        let wanted = PromptCachePolicy::default().with_mode(PromptCacheMode::Prohibit);

        let effective = held.narrowed(wanted);

        assert_eq!(effective.mode(), PromptCacheMode::Require);
        assert_eq!(
            effective.conflict(),
            Some(PromptCachePolicyConflict::RequiredAndProhibited)
        );
    }

    #[test]
    fn canonical_words_round_trip_and_aliases_are_rejected() {
        for word in ["observeOnly", "prefer", "require", "prohibit"] {
            let value = PromptCacheMode::from_str(word).unwrap();
            assert_eq!(value.as_str(), word);
        }
        assert!(PromptCacheMode::from_str("enabled").is_err());
        assert!(PromptCacheIsolation::from_str("tenant").is_err());
        assert!(PromptCachePersistentMode::from_str("always").is_err());
    }

    #[test]
    fn namespaces_and_retention_are_bounded_at_construction() {
        assert!(PromptCacheNamespace::new("team-a").is_ok());
        assert!(PromptCacheNamespace::new("").is_err());
        assert!(PromptCacheNamespace::new("contains space").is_err());
        assert!(
            PromptCacheNamespace::new("x".repeat(MAX_PROMPT_CACHE_NAMESPACE_BYTES + 1)).is_err()
        );
        assert!(PromptCacheRetention::ephemeral(0).is_err());
        assert!(PromptCacheRetention::extended(MAX_PROMPT_CACHE_RETENTION_SECONDS + 1).is_err());
    }

    #[test]
    fn prohibit_cannot_authorize_a_persistent_resource() {
        let policy = PromptCachePolicy::default()
            .with_mode(PromptCacheMode::Prohibit)
            .with_persistent_resources(PromptCachePersistentMode::Create);

        assert!(policy.validate().is_err());
    }
}
