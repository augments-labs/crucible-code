//! What a tool is, what may run one, and the proof that a call was allowed to.
//!
//! Tools are an open set — the built-in ones live in `crucible-builtins` — so
//! adding one must not edit this crate. The roster a request was admitted
//! against, the admission binding a returned call to it and the permission
//! engine that issues [`Approved`] are closed, and live here together.

mod ask;
mod permissions;
mod revealed;
mod source;
mod tool;
mod toolset;

pub use ask::Put;
// Public because this crate's public surface is written in them:
// `CallResultAcceptance::accept` takes a `CallResultReceipt`,
// `ToolContext::with_invocation` an `InvocationId`, and `DescribeTool::effect`
// answers a `ToolEffect`. A tool crate can name each here without a storage
// dependency of its own.
pub use crucible_storage::{CallResultReceipt, InvocationId, ToolEffect};
pub use permissions::{
    Approved, Ask, Command, Disposition, Grant, Host, Minted, Mode, Permission, Remember,
    RuleError, Rules, Sensitivity, Settled, Target, Verdict, narrowest,
};
pub use revealed::Revealed;
pub use source::{Fetch, Page, Search, SearchResponse, SearchResult, SourceError};
pub use tool::{
    Account, CallResultAcceptance, Looking, PendingCallResult, Remembered, Summary, Tool,
    ToolContext, ToolError, ToolOutput, Unwatched, Watch, Wrote,
};
pub use toolset::{
    ArgumentTransform, DescribeTool, InputGuard, OutputGuard, TOOL_ARGUMENT_BYTES,
    TOOL_CALL_ID_BYTES, TOOL_NAME_BYTES, TOOL_RESOURCE_KEY_BYTES, TOOL_SCHEMA_BYTES,
    TOOL_SNAPSHOT_BYTES, TOOL_SNAPSHOT_ENTRIES, TOOL_SOURCE_ID_BYTES, TOOL_SOURCE_LABEL_BYTES,
    ToolAdmission, ToolDescriptor, ToolDescriptorError, ToolEntry, ToolExecutionMode,
    ToolGeneration, ToolHooks, ToolOutcome, ToolProvenance, ToolReceipt, ToolResourceKey,
    ToolSnapshot, ToolSourceKind, ToolSourceReceipt, Toolset, ToolsetContext, ToolsetError,
};
