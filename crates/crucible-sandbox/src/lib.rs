//! Backend-neutral operating-system confinement contracts.
//!
//! Permission answers whether Crucible may invoke an operation. A sandbox is
//! the separate, host-owned boundary that limits what the resulting process
//! and every descendant can observe or change. The types here describe that
//! boundary without choosing Bubblewrap, a container runtime, or a remote
//! executor; concrete implementations live above this crate and are injected
//! by the binary composition root.
//!
//! Claims are exact. An implementation reports each capability as unsupported,
//! observed, or hard-enforced before materialization or spawn. A required
//! policy that cannot be hard-enforced is refused rather than translated into
//! an ordinary subprocess.
//!
//! Nothing here spawns a process or reads a policy file. A backend that does
//! those things depends on this crate; this crate depends on no backend, which
//! is what lets one binary carry several.

mod audit;
mod domains;
mod enablement;
mod guardrail;
mod inspect;
mod manifest;
mod policy;
mod service;

pub use audit::{
    MAX_SANDBOX_AUDIT_FACTS, MAX_SANDBOX_AUDIT_LIFECYCLES, SandboxAudit, SandboxAuditError,
    SandboxAuditRecord, SandboxAuditRegistry,
};
pub use domains::{SandboxDomainPattern, SandboxDomainPolicy};
pub use enablement::SandboxEnablement;
pub use guardrail::{
    MAX_SANDBOX_GUARDRAIL_BYTES, MAX_SANDBOX_GUARDRAIL_LAYERS, MAX_SANDBOX_GUARDRAIL_RULES,
    MAX_SANDBOX_GUARDRAIL_WORDS, SandboxCommandPolicy, SandboxCommandRule, SandboxGuardrailEffect,
    SandboxGuardrailError,
};
pub use manifest::{
    MAX_SANDBOX_MANIFEST_BYTES, MAX_SANDBOX_MANIFEST_ENTRIES, MAX_SANDBOX_MANIFEST_FILE_BYTES,
    SandboxManifest, SandboxManifestEntry, SandboxManifestError,
};
pub use policy::{
    MAX_SANDBOX_FILESYSTEM_RULES, MAX_SANDBOX_HOST_BYTES, MAX_SANDBOX_PATH_BYTES,
    MAX_SANDBOX_PATTERN_COMPONENTS, MAX_SANDBOX_UNREADABLE_PATTERNS, SandboxFilesystemRule,
    SandboxNetworkEndpoint, SandboxNetworkPolicy, SandboxNetworkProvenance, SandboxPolicy,
    SandboxPolicyError, SandboxUnreadablePattern,
};
pub use service::{
    MAX_SANDBOX_COMMAND_ARGUMENTS, MAX_SANDBOX_COMMAND_BYTES, MAX_SANDBOX_CREDENTIAL_HANDLE_BYTES,
    MAX_SANDBOX_ENVIRONMENT_BYTES, MAX_SANDBOX_ENVIRONMENT_ENTRIES,
    MAX_SANDBOX_ENVIRONMENT_NAME_BYTES, SandboxCommand, SandboxCredentialHandle,
    SandboxCredentialProjection, SandboxCredentialProvenance, SandboxEnvironment, SandboxError,
    SandboxInput, SandboxInvocationMode, SandboxLaunch, SandboxOutput, SandboxProcess, SandboxRead,
    SandboxRequest, SandboxService, SandboxSession, SandboxSpeech,
};
// The redacted records a journal and a checkpoint file hold are owned by
// `crucible-storage`, beside the ports that keep them, and re-exported here
// because they are part of this crate's answer: a caller that negotiates a
// boundary or records a fact about it already names this crate, and the words
// it uses for the result should not change because the result moved.
pub use crucible_storage::{
    MAX_SANDBOX_BACKEND_ID_BYTES, MAX_SANDBOX_BACKEND_WORD_BYTES, MAX_SANDBOX_NETWORK_RULES,
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCapability, SandboxCapabilityError, SandboxCheckpoint, SandboxCheckpointError,
    SandboxCleanup, SandboxCommandStage, SandboxFact, SandboxFactKind, SandboxFailureKind,
    SandboxFailurePhase, SandboxFeature, SandboxFilesystemAccess, SandboxFilesystemProvenance,
    SandboxGuardrailDecision, SandboxInspection, SandboxLifecycle, SandboxNetworkInspection,
    SandboxPlanInspection, SandboxResourceLimits, SandboxRootInspection, SandboxUsage,
    SandboxViolation,
};
// The inspection is a stored value now; what builds it from a policy and a
// manifest is the half that is still this crate's, and it is a function rather
// than a constructor because a stored value cannot carry an inherent impl.
pub use inspect::{confined_inspection, inspection, plan_inspection, unconfined_inspection};
