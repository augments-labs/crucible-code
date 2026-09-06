//! Domain types and the traits every other crucible crate implements.
//!
//! This crate is the bottom of the dependency graph and depends on no other
//! crucible crate. Providers, tools, the runner and the renderer all depend on
//! this crate and never on each other; cargo enforces that, so the arrangement
//! cannot rot.
//!
//! Two kinds of type live here, and the split is deliberate:
//!
//! - **Closed sets are enums.** Events, verdicts and errors are owned here, so
//!   adding a variant breaks every `match` and forces each site to decide.
//! - **Open sets are traits.** `Provider`, `Credential` and `Tool` are
//!   implemented in the crates above, so adding one must never edit this crate.
//!
//! Authentication is a separate axis from the wire protocol: a `Provider`
//! receives an already-resolved `Credential` and never learns what kind it is.

mod aside;
mod ask;
mod attachable;
mod cancel;
mod compaction;
mod context;
mod credential;
mod diff;
mod event;
mod extension;
mod ids;
mod interruption;
mod journal;
mod modality;
mod model;
mod permission;
mod prompt;
mod prompt_cache;
mod provider;
mod registry;
mod revealed;
mod run;
mod sandbox;
mod source;
mod steer;
mod tool;
mod toolset;
mod transcript;
mod version;
mod workspace;

pub use aside::Aside;
pub use ask::{Answer, Answered, Put, Question};
pub use attachable::{CEILING, KINDS, Kind, kind};
pub use cancel::Cancel;
pub use compaction::{Compacted, Compacting, RECAP, Room};
pub use context::{ContextError, ContextPatch, ContextSection, ContextSnapshot, Fragment, Seen};
pub use credential::{
    ApiKey, Credential, CredentialError, Header, HeaderKey, Outgoing, Redactions,
};
pub use diff::{Change, Diff, Line};
pub use event::{Event, EventEnvelope, Post, Reporter, TurnError};
pub use extension::{
    EXTENSION_ID_BYTES, EXTENSION_MANIFEST_BYTES, EXTENSION_REQUESTS, EXTENSION_TEXT_BYTES,
    ExtensionCapability, ExtensionContribution, ExtensionError, ExtensionIdentity,
    ExtensionManifest, ExtensionProtocol, ExtensionRequests, ExtensionUnhosted,
    calls::{Asked, CallError, EXTENSION_CALLS, Serving},
    conversation::{Broken, Conversation, Next},
    speaking::{Asking, Over, Speaking, Turn},
    spoken::{CallId, EXTENSION_SAID_BYTES, Malformed, Outcome, Spoken, SpokenError, Trouble},
    trust::{ExtensionDecision, ExtensionTrusted, ExtensionUntrusted},
    wire::{FRAME_BYTES, FrameError, Frames, Written},
};
pub use ids::{
    AgentId, CredentialScopeId, IdError, ProviderAttemptId, RunId, SandboxId, SessionId, ToolId,
    TurnId,
};
pub use interruption::{
    ActionId, ActionResolution, ApprovalDecision, CacheCheckpoint, CheckpointId, CheckpointStore,
    ExecutionCheckpoint, IdempotencyKey, InterruptionError, InvocationId, InvocationRecord,
    InvocationState, JournalEntryId, MAX_CHECKPOINT_INVOCATIONS, MAX_CHECKPOINT_SANDBOXES,
    MAX_CHECKPOINT_WORD_BYTES, MAX_HUMAN_INPUT_BYTES, MAX_PENDING_ACTIONS, PendingAction,
    PendingActions, PendingApproval, PendingExternalTool, PendingHumanInput, RecoveryAction,
    ResolutionChange, ResumeDigest, ResumeEvidence, ResumeScope, ResumedAction, ToolEffect,
    ValidatedResume,
};
pub use journal::{
    CallResultKey, CallResultReceipt, CallResultStoreError, CompactionRecord, CustomEntry,
    CustomProjector, JournalError, JournalStore, MAX_CUSTOM_DATA_BYTES, MAX_JOURNAL_WORD_BYTES,
    MAX_RUN_HISTORY_BYTES, MAX_RUN_ITEM_BYTES, MAX_RUN_ITEM_RETAINED_BYTES, MAX_RUN_ITEMS,
    RunHistory, RunItem, SessionStore,
};
pub use modality::{Modalities, Modality, ModalityError};
pub use model::{MODEL_NAME_BYTES, ModelCapabilities, ModelError, ModelLimits};
pub use permission::{
    Approved, Ask, Command, Disposition, Grant, Host, Minted, Mode, Permission, Remember,
    RuleError, Rules, Sensitivity, Settled, Target, Verdict, narrowest,
};
pub use prompt::{
    EnvironmentSection, Identity, ModelSection, PermissionsSection, Skill, SkillsSection,
    SystemPrompt, Tone, ToneError, ToolsSection, WorkspaceSection,
};
pub use prompt_cache::{
    CostAmount, PriceRate, PricingCurrency, PricingDate, PricingError, PricingQuery, PricingUnit,
    PromptCachePricing, PromptCacheRates, UsageCost, UsageRate, select_pricing,
};
pub use prompt_cache::{
    InputTokenUsage, MAX_PROVIDER_USAGE_DETAIL_LABEL_BYTES, MAX_PROVIDER_USAGE_DETAILS,
    ProviderNumericDetail, ProviderUsage, UsageError,
};
pub use prompt_cache::{
    MAX_PROMPT_CACHE_BOUNDARIES, PromptCacheBoundaryPoint, PromptCacheContentSet,
    PromptCacheProjection, PromptCacheProjectionError,
};
pub use prompt_cache::{
    MAX_PROMPT_CACHE_HANDLE_BYTES, MAX_PROMPT_CACHE_RESOURCE_WORD_BYTES,
    MAX_PROMPT_CACHE_RESOURCES, PromptCachePolicyDigest, PromptCacheResourceBinding,
    PromptCacheResourceCreate, PromptCacheResourceCreated, PromptCacheResourceDeadline,
    PromptCacheResourceError, PromptCacheResourceFact, PromptCacheResourceHandle,
    PromptCacheResourceId, PromptCacheResourceLifecycle, PromptCacheResourceOperation,
    PromptCacheResourceOwner, PromptCacheResourceRecord, PromptCacheResourceReference,
    PromptCacheResourceRemote, PromptCacheResourceState, PromptCacheResourceStore,
    PromptCacheResourceWordError,
};
pub use prompt_cache::{
    MAX_PROMPT_CACHE_MECHANISMS, PromptCacheBoundary, PromptCacheCapabilities,
    PromptCacheCapabilityWordError, PromptCacheContent, PromptCacheMechanism,
    PromptCacheMechanismCapability, PromptCacheProvenance, PromptCacheRetentionClass,
    PromptCacheSupport, PromptCacheUsageReporting, StatefulTransportCapability,
};
pub use prompt_cache::{
    MAX_PROMPT_CACHE_NAMESPACE_BYTES, MAX_PROMPT_CACHE_RETENTION_SECONDS, PromptCacheIsolation,
    PromptCacheMechanisms, PromptCacheMode, PromptCacheNamespace, PromptCachePersistentMode,
    PromptCachePolicy, PromptCachePolicyConflict, PromptCachePolicyError, PromptCachePolicySource,
    PromptCachePolicySources, PromptCachePolicyVersion, PromptCacheRetention,
};
pub use prompt_cache::{
    PromptCacheAttempt, PromptCacheEligibility, PromptCacheEncoding, PromptCacheFact,
    PromptCacheFingerprint, PromptCacheIdentity, PromptCacheIneligibleReason, PromptCacheKey,
    PromptCacheOutcome, PromptCachePlan, PromptCachePlanned, PromptCachePreparationError,
    PromptCacheRequest, PromptCacheRequestDisposition, PromptCacheRequestFact, PromptCacheRoute,
    PromptCacheScopeDigest, PromptCacheSelected, PromptCacheSelection, PromptCacheUsageFact,
};
pub use provider::{
    Attached, Calibration, Carried, Content, Delta, DeltaStream, Effort, EffortError, Provider,
    ProviderError, ProviderLimit, Request, Spend, ToolSchema,
};
pub use registry::{
    Collision, Provenance, ProvenanceError, REGISTRY_BYTES, REGISTRY_ENTRIES, Registered, Registry,
    RegistryError, RegistryGeneration, RegistryHandle, RegistryReport, RegistryRow,
    RegistrySnapshot, SOURCE_ID_BYTES, SOURCE_LABEL_BYTES, Shadow, SourceKind, SourceReceipt,
    Staged,
};
pub use revealed::Revealed;
pub use run::{Ancestry, AncestryError};
pub use sandbox::{
    Finish, Heard, MAX_SANDBOX_AUDIT_FACTS, MAX_SANDBOX_AUDIT_LIFECYCLES,
    MAX_SANDBOX_BACKEND_ID_BYTES, MAX_SANDBOX_BACKEND_WORD_BYTES, MAX_SANDBOX_COMMAND_ARGUMENTS,
    MAX_SANDBOX_COMMAND_BYTES, MAX_SANDBOX_CREDENTIAL_HANDLE_BYTES, MAX_SANDBOX_ENVIRONMENT_BYTES,
    MAX_SANDBOX_ENVIRONMENT_ENTRIES, MAX_SANDBOX_ENVIRONMENT_NAME_BYTES,
    MAX_SANDBOX_FILESYSTEM_RULES, MAX_SANDBOX_GUARDRAIL_BYTES, MAX_SANDBOX_GUARDRAIL_LAYERS,
    MAX_SANDBOX_GUARDRAIL_RULES, MAX_SANDBOX_GUARDRAIL_WORDS, MAX_SANDBOX_HOST_BYTES,
    MAX_SANDBOX_MANIFEST_BYTES, MAX_SANDBOX_MANIFEST_ENTRIES, MAX_SANDBOX_MANIFEST_FILE_BYTES,
    MAX_SANDBOX_NETWORK_RULES, MAX_SANDBOX_PATH_BYTES, MAX_SANDBOX_PATTERN_COMPONENTS,
    MAX_SANDBOX_UNREADABLE_PATTERNS, Muttered, Said, SandboxAudit, SandboxAuditError,
    SandboxAuditRecord, SandboxAuditRegistry, SandboxBackendId, SandboxBackendIdentity,
    SandboxBackendProvenance, SandboxCapabilities, SandboxCapability, SandboxCapabilityError,
    SandboxCheckpoint, SandboxCleanup, SandboxCommand, SandboxCommandPolicy, SandboxCommandRule,
    SandboxCommandStage, SandboxCredentialHandle, SandboxCredentialProjection,
    SandboxCredentialProvenance, SandboxDomainPattern, SandboxDomainPolicy, SandboxEnablement,
    SandboxEnvironment, SandboxError, SandboxFact, SandboxFactKind, SandboxFailureKind,
    SandboxFailurePhase, SandboxFeature, SandboxFilesystemAccess, SandboxFilesystemProvenance,
    SandboxFilesystemRule, SandboxGuardrailDecision, SandboxGuardrailEffect, SandboxGuardrailError,
    SandboxInspection, SandboxInvocationMode, SandboxLaunch, SandboxLifecycle, SandboxManifest,
    SandboxManifestEntry, SandboxManifestError, SandboxNetworkEndpoint, SandboxNetworkInspection,
    SandboxNetworkPolicy, SandboxNetworkProvenance, SandboxOutput, SandboxPlanInspection,
    SandboxPolicy, SandboxPolicyError, SandboxProcess, SandboxRead, SandboxRequest,
    SandboxResourceLimits, SandboxRootInspection, SandboxService, SandboxSession, SandboxSpeech,
    SandboxUnreadablePattern, SandboxUsage, SandboxViolation,
};
pub use source::{Fetch, Page, Search, SearchResult, SourceError};
pub use steer::Steer;
pub use tool::{
    Account, CallResultAcceptance, Changed, Looking, PendingCallResult, Remembered, Summary,
    TOOL_RESULT_BYTES, TOOL_RESULT_MIN_BYTES, Tool, ToolArgs, ToolCall, ToolContext, ToolError,
    ToolOutput, ToolOutputRetention, Unwatched, Watch, Wrote,
};
pub use toolset::{
    ArgumentTransform, DescribeTool, InputGuard, OutputGuard, TOOL_ARGUMENT_BYTES,
    TOOL_CALL_ID_BYTES, TOOL_NAME_BYTES, TOOL_RESOURCE_KEY_BYTES, TOOL_SCHEMA_BYTES,
    TOOL_SNAPSHOT_BYTES, TOOL_SNAPSHOT_ENTRIES, TOOL_SOURCE_ID_BYTES, TOOL_SOURCE_LABEL_BYTES,
    ToolAdmission, ToolDescriptor, ToolDescriptorError, ToolEntry, ToolExecutionMode,
    ToolGeneration, ToolHooks, ToolOutcome, ToolProvenance, ToolReceipt, ToolResourceKey,
    ToolSnapshot, ToolSourceKind, ToolSourceReceipt, Toolset, ToolsetContext, ToolsetError,
};
pub use transcript::{Attachment, Message, StopReason, ToolResult, Transcript};
pub use version::later;
pub use workspace::{PathError, WalkFiles, Workspace, WorkspacePath, written};
