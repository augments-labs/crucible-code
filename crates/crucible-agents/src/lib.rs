//! What an agent *is*, as opposed to what running one does.
//!
//! A definition and only a definition: a name, what the agent is for, the
//! instructions it works under, which model answers for it, which tools it
//! declares, and the checks its input and its final answer are held to.
//! Everything an execution needs and none of this holds — the provider
//! connection, the transcript, the permission memory, the session log — belongs
//! to whatever is running the agent, because those are facts about one run
//! rather than about the agent, and two runs of the same agent must not share
//! them.
//!
//! A definition is settled once and then shared. Changing what a session is
//! asked under is building another definition and selecting it, rather than
//! writing to the one a request may already be out under: an [`Agent`] behind
//! an `Arc` cannot be rewritten by the run that is reading it, which is what
//! makes one safe to hand to two runs at once.
//!
//! Guardrails are this crate's other half. They are how a definition says what
//! it will accept and what it will stand behind, and they are deliberately
//! narrow: a check is handed [`AgentContext`], which names the run, the agent
//! and the words it is being asked about, and nothing else. A guardrail can
//! refuse; it cannot approve a tool call, publish an event, reach a session or
//! widen what the run may do.

mod agent;
mod availability;
mod guardrails;
mod instructions;
mod model;

pub use agent::{Agent, AgentBuilder};
pub use availability::Availability;
pub use guardrails::{
    AgentContext, Decision, GuardrailError, InputGuardrail, OutputGuardrail, Rejection, Undecided,
};
pub use instructions::Instructions;
pub use model::Model;
