//! Drives turns to completion.
//!
//! The runner streams deltas from a provider, dispatches the tool calls the
//! model asks for, feeds the results back, and repeats until the model yields
//! or the user cancels.
//!
//! It depends on `crucible-agents` for the definition a turn is taken under,
//! on the crates that own what a turn exchanges — `crucible-types` for the
//! transcript and the identities in it, `crucible-models` for the contract a
//! provider answers to, `crucible-tools` for what a tool is and what may run
//! one, `crucible-storage` for the journal a turn is resumed from — and on
//! `crucible-attachments` for the one bounded ingress an attachment's bytes
//! come through. Every other collaborator arrives as a trait object chosen
//! during wiring — including the store a turn is recorded to — so the loop
//! never names Anthropic, `OpenAI`, `grep`, a renderer, or a session file.
//!
//! Two things leave this crate, and they leave by different routes. *Progress*
//! — words arriving, a tool starting, a tool finishing — goes out as events,
//! because the thread that draws is not the thread that runs. The *outcome* of
//! a turn is the return value, because the caller is what decides whether the
//! session goes on.
//!
//! Where the names went. `Event`, `EventEnvelope`, `Post`, `Reporter` and
//! `TurnError` are this crate's: a whole execution event is what a turn
//! produces, so it belongs to the crate that runs one. The
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
    Agent, AgentBuilder, AgentContext, Availability, Decision, Declared, GuardrailError,
    InputGuardrail, Model, NameTaken, OutputGuardrail, Rejection, Unanswered, Undecided,
};
pub use events::{Event, EventEnvelope, Post, Reporter, TurnError};
pub use outcome::{RunResult, RunStatus, Turned};
pub use policy::{Bounds, Compaction, MAXIMUM_TOOL_CONCURRENCY, Retry, RunPolicy, ToolScheduling};
pub use runner::attachments;
pub use runner::{
    Breakdown, Category, PromptCacheCleanup, RunState, Runner, SessionCost, TOOL_RUNS, Totals,
};
pub use tools::Tools;
