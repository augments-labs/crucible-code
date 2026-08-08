//! Filesystem, search and process tools the agent can call.
//!
//! Depends on `crucible-core` alone, and is a sibling of `crucible-provider`:
//! neither may reach the other. Each tool implements `Tool` from core, so the
//! runner dispatches to them without naming any of them.
//!
//! Every tool takes a `Grant` as an argument rather than asking for one, and a
//! `Grant` is minted only by the permission engine — so permission is
//! impossible to forget and equally impossible to fake: code that has not
//! obtained one cannot call the operation. A read-only tool is no exception.
//! What its sensitivity buys it is a grant issued without a question, not a
//! signature that skips the token, because a tool that reported the wrong
//! sensitivity would otherwise be one that had never been asked about at all.
//!
//! Every tool that takes a path asks its `Workspace` to resolve one before
//! touching it, so the containment check lives in one place rather than in five
//! copies of the same `if`. What that check refuses is a path that *resolves*
//! outside the tree: `..`, an absolute path and a symbolic link are followed
//! first and then judged by where they landed, which is why one that stays
//! inside is allowed and one that leaves is not.
//!
//! `bash` is the exception, and deliberately. It runs a shell, and a shell
//! reaches anything the user can; the workspace gives it a directory to start
//! in, not a fence. What bounds that tool is the permission engine, which is
//! why the question it asks names the program the command is about to run.

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
