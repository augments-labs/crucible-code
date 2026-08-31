//! Drives turns to completion.
//!
//! The runner streams deltas from a provider, dispatches the tool calls the
//! model asks for, feeds the results back, and repeats until the model yields
//! or the user cancels.
//!
//! It depends on `crucible-core` for every domain type and on
//! `crucible-session` for the log a turn is recorded to. Every other
//! collaborator arrives as a trait object chosen during wiring, so the loop
//! never names Anthropic, `OpenAI`, `grep`, or a renderer. Swapping any of
//! them is a change in `main.rs`.
//!
//! Two things leave this crate, and they leave by different routes. *Progress*
//! — words arriving, a tool starting, a tool finishing — goes out as events,
//! because the thread that draws is not the thread that runs. The *outcome* of
//! a turn is the return value, because the caller is what decides whether the
//! session goes on.

mod agent;
mod context;
#[cfg(test)]
mod fake;
mod outcome;
mod policy;
mod prompt_cache;
mod runner;
#[cfg(test)]
mod sample;
mod tools;

pub use agent::AgentSpec;
pub use context::RunContext;
pub use crucible_session::{
    Glimpse, PROMPTS, Pruned, Recorded, Session, SessionError, glimpse, prompts, recent, remember,
    retitle,
};
pub use outcome::{RunResult, RunStatus};
pub use policy::{Bounds, Compaction, MAXIMUM_TOOL_CONCURRENCY, Retry, RunPolicy, ToolScheduling};
pub use runner::attachments;
pub use runner::{ContextInputs, Model, PromptCacheCleanup, Runner};
pub use tools::Tools;
