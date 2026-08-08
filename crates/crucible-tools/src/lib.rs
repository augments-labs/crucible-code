//! Filesystem, search and process tools the agent can call.
//!
//! Depends on `crucible-core` alone, and is a sibling of `crucible-provider`:
//! neither may reach the other. Each tool implements `Tool` from core, so the
//! runner dispatches to them without naming any of them.
//!
//! Every tool that mutates a file or spawns a process takes a `Grant` as an
//! argument rather than asking for one. A `Grant` is minted only by the
//! permission engine, so permission is impossible to forget and equally
//! impossible to fake: code that has not obtained one cannot call the
//! operation.
//!
//! Every tool holds a `Workspace` and asks it for each path it touches. The
//! containment check therefore happens in one place rather than once per tool,
//! and a tool cannot reach outside the tree the agent was pointed at even if
//! the model asks it to.

mod args;
mod bash;
mod edit;
mod glob;
mod grep;
mod read;
#[cfg(test)]
mod sample;
mod tree;
mod write;

pub use bash::Bash;
pub use edit::Edit;
pub use glob::Glob;
pub use grep::Grep;
pub use read::Read;
pub use write::Write;
