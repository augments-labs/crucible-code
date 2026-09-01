//! Backend-neutral operating-system confinement contracts.
//!
//! Permission answers whether Crucible may invoke an operation. A sandbox is
//! the separate, host-owned boundary that limits what the resulting process
//! and every descendant can observe or change. The types here describe that
//! boundary without choosing Bubblewrap, a container runtime, or a remote
//! executor; concrete implementations live above core and are injected by the
//! binary composition root.
//!
//! Claims are exact. An implementation reports each capability as unsupported,
//! observed, or hard-enforced before materialization or spawn. A required
//! policy that cannot be hard-enforced is refused rather than translated into
//! an ordinary subprocess.

mod capability;
mod manifest;
mod policy;
mod service;

pub use capability::{
    MAX_SANDBOX_BACKEND_ID_BYTES, MAX_SANDBOX_BACKEND_WORD_BYTES, SandboxBackendId,
    SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities, SandboxCapability,
    SandboxCapabilityError, SandboxFeature,
};
pub use manifest::{
    MAX_SANDBOX_MANIFEST_BYTES, MAX_SANDBOX_MANIFEST_ENTRIES, MAX_SANDBOX_MANIFEST_FILE_BYTES,
    SandboxManifest, SandboxManifestEntry, SandboxManifestError,
};
pub use policy::{
    MAX_SANDBOX_FILESYSTEM_RULES, MAX_SANDBOX_HOST_BYTES, MAX_SANDBOX_NETWORK_ENDPOINTS,
    MAX_SANDBOX_PATH_BYTES, SandboxFilesystemAccess, SandboxFilesystemProvenance,
    SandboxFilesystemRule, SandboxMode, SandboxNetworkEndpoint, SandboxNetworkPolicy,
    SandboxPolicy, SandboxPolicyError, SandboxResourceLimits,
};
pub use service::{
    MAX_SANDBOX_COMMAND_ARGUMENTS, MAX_SANDBOX_COMMAND_BYTES, MAX_SANDBOX_ENVIRONMENT_BYTES,
    MAX_SANDBOX_ENVIRONMENT_ENTRIES, MAX_SANDBOX_ENVIRONMENT_NAME_BYTES, SandboxCleanup,
    SandboxCommand, SandboxEnvironment, SandboxError, SandboxInspection, SandboxOutput,
    SandboxProcess, SandboxRead, SandboxRequest, SandboxService, SandboxSession, SandboxUsage,
};
