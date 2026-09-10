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
//! Nothing here spawns a process, reads a policy file, or names an operating
//! system. A backend that does those things depends on this crate; this crate
//! depends on no backend, which is what lets one binary carry several.

mod audit;
mod capability;
mod domains;
mod enablement;
mod guardrail;
mod manifest;
mod policy;
mod service;

pub use audit::{
    MAX_SANDBOX_AUDIT_FACTS, MAX_SANDBOX_AUDIT_LIFECYCLES, SandboxAudit, SandboxAuditError,
    SandboxAuditRecord, SandboxAuditRegistry, SandboxFact, SandboxFactKind, SandboxFailureKind,
    SandboxFailurePhase, SandboxLifecycle,
};
pub use capability::{
    MAX_SANDBOX_BACKEND_ID_BYTES, MAX_SANDBOX_BACKEND_WORD_BYTES, SandboxBackendId,
    SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities, SandboxCapability,
    SandboxCapabilityError, SandboxFeature,
};
pub use domains::{MAX_SANDBOX_NETWORK_RULES, SandboxDomainPattern, SandboxDomainPolicy};
pub use enablement::SandboxEnablement;
pub use guardrail::{
    MAX_SANDBOX_GUARDRAIL_BYTES, MAX_SANDBOX_GUARDRAIL_LAYERS, MAX_SANDBOX_GUARDRAIL_RULES,
    MAX_SANDBOX_GUARDRAIL_WORDS, SandboxCommandPolicy, SandboxCommandRule, SandboxCommandStage,
    SandboxGuardrailDecision, SandboxGuardrailEffect, SandboxGuardrailError,
};
pub use manifest::{
    MAX_SANDBOX_MANIFEST_BYTES, MAX_SANDBOX_MANIFEST_ENTRIES, MAX_SANDBOX_MANIFEST_FILE_BYTES,
    SandboxManifest, SandboxManifestEntry, SandboxManifestError,
};
pub use policy::{
    MAX_SANDBOX_FILESYSTEM_RULES, MAX_SANDBOX_HOST_BYTES, MAX_SANDBOX_PATH_BYTES,
    MAX_SANDBOX_PATTERN_COMPONENTS, MAX_SANDBOX_UNREADABLE_PATTERNS, SandboxFilesystemAccess,
    SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxNetworkEndpoint,
    SandboxNetworkPolicy, SandboxNetworkProvenance, SandboxPolicy, SandboxPolicyError,
    SandboxResourceLimits, SandboxUnreadablePattern,
};
pub use service::{
    MAX_SANDBOX_COMMAND_ARGUMENTS, MAX_SANDBOX_COMMAND_BYTES, MAX_SANDBOX_CREDENTIAL_HANDLE_BYTES,
    MAX_SANDBOX_ENVIRONMENT_BYTES, MAX_SANDBOX_ENVIRONMENT_ENTRIES,
    MAX_SANDBOX_ENVIRONMENT_NAME_BYTES, SandboxCheckpoint, SandboxCleanup, SandboxCommand,
    SandboxCredentialHandle, SandboxCredentialProjection, SandboxCredentialProvenance,
    SandboxEnvironment, SandboxError, SandboxInspection, SandboxInvocationMode, SandboxLaunch,
    SandboxNetworkInspection, SandboxOutput, SandboxPlanInspection, SandboxProcess, SandboxRead,
    SandboxRequest, SandboxRootInspection, SandboxService, SandboxSession, SandboxSpeech,
    SandboxUsage, SandboxViolation,
};
