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
mod credential;
mod diff;
mod event;
mod ids;
mod modality;
mod permission;
mod prompt;
mod provider;
mod revealed;
mod run;
mod source;
mod steer;
mod tool;
mod transcript;
mod workspace;

pub use aside::Aside;
pub use ask::{Answer, Answered, Put, Question};
pub use attachable::{CEILING, KINDS, Kind, kind};
pub use cancel::Cancel;
pub use compaction::{Compacted, Compacting, RECAP, Room};
pub use credential::{
    ApiKey, Credential, CredentialError, Header, HeaderKey, Outgoing, Redactions,
};
pub use diff::{Change, Diff, Line};
pub use event::{Event, EventEnvelope, Post, Reporter, TurnError};
pub use ids::{AgentId, IdError, RunId, SessionId, ToolId, TurnId};
pub use modality::{Modalities, Modality, ModalityError};
pub use permission::{
    Approved, Ask, Command, Disposition, Grant, Host, Minted, Mode, Permission, Remember,
    RuleError, Rules, Sensitivity, Settled, Target, Verdict, narrowest,
};
pub use prompt::{Identity, Skill, SystemPrompt, Tone, ToneError};
pub use provider::{
    Attached, Calibration, Carried, Content, Delta, DeltaStream, Effort, EffortError, Provider,
    ProviderError, ProviderLimit, Request, Spend, ToolSchema,
};
pub use revealed::Revealed;
pub use run::Ancestry;
pub use source::{Fetch, Page, Search, SearchResult, SourceError};
pub use steer::Steer;
pub use tool::{
    Account, Changed, Remembered, Summary, Tool, ToolArgs, ToolCall, ToolError, ToolOutput,
    Unwatched, Watch, Wrote,
};
pub use transcript::{Attachment, Message, StopReason, ToolResult, Transcript};
pub use workspace::{PathError, WalkFiles, Workspace, WorkspacePath, written};
