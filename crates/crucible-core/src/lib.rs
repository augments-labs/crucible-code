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
//! receives an already-resolved `Credential` and never learns whether it came
//! from an API key or a subscription login.

mod cancel;
mod credential;
mod event;
mod ids;
mod permission;
mod provider;
mod tool;
mod transcript;
mod workspace;

pub use cancel::Cancel;
pub use credential::{ApiKey, Credential, CredentialError, Header, HeaderKey, Outgoing};
pub use event::{Event, Post, TurnError};
pub use ids::{IdError, SessionId, ToolId, TurnId};
pub use permission::{
    Approved, Ask, Command, Disposition, Grant, Minted, Mode, Permission, Remember, RuleError,
    Rules, Sensitivity, Settled, Target, Verdict, narrowest,
};
pub use provider::{Delta, DeltaStream, Provider, ProviderError, Request, ToolSchema};
pub use tool::{Tool, ToolArgs, ToolCall, ToolError, ToolOutput};
pub use transcript::{Message, StopReason, ToolResult, Transcript};
pub use workspace::{PathError, Workspace, WorkspacePath, written};
