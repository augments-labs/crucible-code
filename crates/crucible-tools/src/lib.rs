//! Filesystem, search and process tools the agent can call.
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
//! Two of these tools share one piece of knowledge and nothing else: which
//! files have been read. `write` puts down a whole file, so it refuses to
//! replace one nobody looked at, and neither tool can answer that alone. The
//! record is handed to both when they are built rather than reached for by
//! either, which keeps the binary the only place that knows they share it.
//!
//! `bash` is the exception, and deliberately. It runs a shell, and a shell
//! reaches anything the user can; the workspace gives it a directory to start
//! in, not a fence. What bounds that tool is the permission engine, which is
//! why the question it asks names the program the command is about to run.

mod args;
mod atomic;
mod bash;
mod bound;
mod edit;
mod glob;
mod grep;
mod ledger;
mod read;
#[cfg(test)]
mod sample;
mod summary;
mod target;
mod tree;
mod write;

pub use bash::Bash;
pub use edit::Edit;
pub use glob::Glob;
pub use grep::Grep;
pub use ledger::Ledger;
pub use read::Read;
pub use write::Write;

#[cfg(test)]
mod tests {
    use crucible_core::{Cancel, Tool};

    use crate::sample::Sample;
    use crate::{Bash, Edit, Glob, Grep, Ledger, Read, Write};

    #[test]
    fn a_run_of_calls_is_counted_in_whatever_the_tool_acted_on() {
        // The word a folded row says: `6 files`, not `6 calls`. Only the tool
        // knows it — downstream of here a call is a name and a line of JSON,
        // and a table of names kept beside whatever draws the row would be a
        // second list of these tools, to fall out of step with this one the
        // first time either changed.
        let sample = Sample::new("tools-counted");
        let workspace = sample.workspace();
        let cancel = Cancel::new();
        let seen = Ledger::new();

        // The three that put a file in front of the model or change one.
        let files: [Box<dyn Tool>; 3] = [
            Box::new(Read::new(workspace.clone(), cancel.clone(), seen.clone())),
            Box::new(Write::new(workspace.clone(), seen)),
            Box::new(Edit::new(workspace.clone(), cancel.clone())),
        ];

        for tool in &files {
            assert_eq!(tool.counted(), "files", "{}", tool.name());
        }

        // And the three that do not. A shell command is not a file, and a walk
        // of the tree looking for a pattern is not one either however many it
        // opens on the way.
        assert_eq!(
            Bash::new(workspace.clone(), cancel.clone()).counted(),
            "commands"
        );
        assert_eq!(
            Grep::new(workspace.clone(), cancel.clone()).counted(),
            "searches"
        );
        assert_eq!(Glob::new(workspace, cancel).counted(), "searches");
    }
}
