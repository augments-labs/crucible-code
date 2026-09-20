//! Drives turns to completion.
//!
//! The runner streams deltas from a provider, dispatches the tool calls the
//! model asks for, feeds the results back, and repeats until the model yields
//! or the user cancels.
//!
//! It depends on `crucible-agents` for the definition a turn is taken under,
//! on `crucible-core` for every domain type, and on `crucible-attachments` for
//! the one bounded ingress an attachment's bytes come through. Every other
//! collaborator arrives as a trait object chosen during wiring — including the
//! store a turn is recorded to — so the loop never names Anthropic, `OpenAI`,
//! `grep`, a renderer, or a session file.
//!
//! Two things leave this crate, and they leave by different routes. *Progress*
//! — words arriving, a tool starting, a tool finishing — goes out as events,
//! because the thread that draws is not the thread that runs. The *outcome* of
//! a turn is the return value, because the caller is what decides whether the
//! session goes on.
//!
//! Where the names went. `Event`, `EventEnvelope`, `Post`, `Reporter` and
//! `TurnError` are this crate's, not `crucible-core`'s: a whole execution
//! event is what a turn produces, and core cannot depend on what runs one. The
//! session names this crate once re-exported — `Session`, `SessionError`,
//! `Glimpse`, `Pruned`, `Recorded`, `DisplayHistory`, `DisplayItem`, `PROMPTS`,
//! `glimpse`, `prompts`, `recent`, `remember` and `retitle` — are
//! `crucible-session`'s, and are imported from there.

mod context;
mod events;
#[cfg(test)]
mod fake;
mod outcome;
mod policy;
mod prompt_cache;
#[cfg(test)]
mod recording;
mod runner;
#[cfg(test)]
mod sample;
mod tools;

pub use context::RunContext;
pub use crucible_agents::{
    Agent, AgentBuilder, AgentContext, Availability, Decision, GuardrailError, InputGuardrail,
    Model, OutputGuardrail, Rejection, Undecided,
};
pub use events::{Event, EventEnvelope, Post, Reporter, TurnError};
pub use outcome::{RunResult, RunStatus, Turned};
pub use policy::{Bounds, Compaction, MAXIMUM_TOOL_CONCURRENCY, Retry, RunPolicy, ToolScheduling};
pub use runner::attachments;
pub use runner::{PromptCacheCleanup, RunState, Runner};
pub use tools::Tools;
