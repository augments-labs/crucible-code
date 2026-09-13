//! The shared, validated data values every crucible crate exchanges.
//!
//! This crate is the root of the dependency graph and depends on no other
//! crucible crate. A type belongs here when several domains exchange the same
//! meaning without importing whatever owns the behavior behind it: an
//! identity, a message, a call, a recorded result, the persisted half of a
//! context. A type that only one owner produces belongs with that owner, and a
//! value that confers authority belongs with whatever issues it.
//!
//! What that rules out is as important as what it admits. There is no live tool
//! result here, no permission verdict, no runtime handle and no provider wire
//! object — those carry authority or behavior, and a crate that only needs to
//! read a session back should not have to compile them.

pub mod ask;
pub mod call;
pub mod context;
pub mod continuation;
pub mod diff;
pub mod ids;
pub mod modality;
pub mod output;
pub mod run;
pub mod transcript;

pub use ask::{Answer, Answered, Question};
pub use call::{
    TOOL_ARGUMENT_BYTES, TOOL_CALL_ID_BYTES, TOOL_NAME_BYTES, ToolArgs, ToolCall, ToolSchema,
};
pub use context::{ContextError, ContextPatch, ContextSnapshot, Fragment, Seen};
pub use continuation::{
    CONTINUATION_BYTES, CONTINUATION_HISTORY_BYTES, CONTINUATION_PARTS, Continuation,
    ContinuationData, ContinuationError, ContinuationPart, ContinuationScope, ProviderContinuation,
};
pub use diff::{Change, Diff, Line};
pub use ids::{
    AgentId, CredentialScopeId, IdError, ProviderAttemptId, RunId, SandboxId, SessionId, ToolId,
    TurnId,
};
pub use modality::{Modalities, Modality, ModalityError};
pub use output::{
    Changed, RecordedToolOutput, TOOL_RESULT_BYTES, TOOL_RESULT_MIN_BYTES, ToolOutcome,
    ToolOutputRetention,
};
pub use run::{Ancestry, AncestryError};
pub use transcript::{Attachment, Message, StopReason, ToolResult, Transcript};
