//! Filesystem, search and process tools the agent can call, and the one that
//! writes down its plan.
//!
//! Its only dependency on another crucible crate is `crucible-core`, and it is
//! a sibling of `crucible-provider`: neither may reach the other. Each tool
//! implements `Tool` from core, so the runner dispatches to them without naming
//! any of them.
//!
//! Every tool takes an `Approved` as an argument rather than asking for one —
//! the grant the permission engine minted, bound to the call it was reached
//! about. So permission is impossible to forget and equally impossible to fake:
//! code that has not obtained one cannot call the operation, and a `Verdict`,
//! which any caller can construct, will not stand in for one. A read-only tool
//! is no exception. What its sensitivity buys it is a grant issued without a
//! question, not a signature that skips the token, because a tool that reported
//! the wrong sensitivity would otherwise be one that had never been asked about
//! at all.
//!
//! Every tool that takes a path asks its `Workspace` to resolve one before
//! touching it, so the containment check lives in one place rather than in five
//! copies of the same `if`. What that check refuses is a path that *resolves*
//! outside the tree: `..`, an absolute path and a symbolic link are followed
//! first and then judged by where they landed, which is why one that stays
//! inside is allowed and one that leaves is not.
//!
//! With one thing it cannot judge: a link whose target does not exist has no
//! landing place to be judged by, so creating through it is refused whichever
//! way it points. That is its own answer and not an escape, because a dangling
//! link may well point back inside.
//!
//! What a tool answers with is bounded before it leaves, because it goes into
//! the next request whole. The private `bound` module holds the one figure they
//! share and says why that figure is in bytes rather than in lines.
//!
//! `edit` and `write` answer with one thing more, which the model is never sent:
//! the change each of them made, for the reader to look at. Between them they
//! are the only code that ever holds both versions of a file, and each holds
//! them for the length of one call, so the private `changed` module reads the
//! change off there rather than leaving anything downstream to work it out again
//! from bytes it no longer has.
//!
//! They come by the two versions differently, and it shows in what each can
//! promise. `edit` is handed the old one because it has to find text in it, so a
//! change it made is a change it can always show. `write` is handed only the new
//! one and has to go and read what it is about to discard, under a bound of its
//! own — so a file too large to hold, or one that is not text at all, is
//! replaced with nothing shown rather than with a block that guessed.
//!
//! Two of these tools share one piece of knowledge and nothing else: which
//! files have been read. `write` puts down a whole file, so it refuses to
//! replace one nobody looked at, and neither tool can answer that alone. The
//! record is handed to both when they are built rather than reached for by
//! either, which keeps the binary the only place that knows they share it.
//!
//! `todo_write` touches no file at all, and that is the whole of its permission
//! story: what it changes is a value inside this process, so there is no path
//! for a rule to be written about and nobody is asked. It puts down the plan the
//! agent is working to, replacing it whole and answering with it. That plan is
//! handed to it the way the read record above is handed to the two tools that
//! share it, and for the same reason — the loop drawing it above the prompt
//! holds the other end.
//!
//! `bash` is the exception, and deliberately. It runs a shell, and a shell
//! reaches anything the user can; the workspace gives it a directory to start
//! in, not a fence. What bounds that tool is the permission engine, which is
//! why the question it asks names the program the command is about to run.

mod account;
mod args;
mod ask;
mod atomic;
mod bash;
mod bound;
mod changed;
mod edit;
mod glob;
mod grep;
mod ledger;
mod lookup;
mod plan;
mod program;
mod read;
#[cfg(test)]
mod sample;
mod summary;
mod target;
mod tree;
mod web;
mod write;

pub use account::{backgrounded, of as account};
pub use ask::AskUser;
pub use bash::{Background, Bash, Ended, MOST, Standing};
pub use edit::Edit;
pub use glob::Glob;
pub use grep::Grep;
pub use ledger::Ledger;
pub use lookup::{Held, ToolSearch};
pub use plan::{Plan, State, Task, TodoWrite};
pub use read::Read;
pub use web::{WebFetch, WebSearch};
pub use write::Write;
