//! The path a call is about, worked out before the call is permitted.
//!
//! A rule is matched against the path the workspace resolved, never the text
//! the model sent: `../../etc/shadow` and `/etc/shadow` are the same file, and
//! a rule about one that is not a rule about the other is not a rule at all.
//!
//! So a path is resolved twice — once here, before anybody is asked, and once
//! in `run`, where the result is what actually gets opened. That is not a
//! redundancy waiting to be cached away. What crosses the permission boundary
//! between the two is a [`Target`], which is text; a tool that carried a
//! resolved handle across it would be a tool that had decided what to open
//! before anyone said yes.
//!
//! Anything that does not resolve becomes [`Target::unresolved`], which no rule
//! matches. The call is asked about and then refused by the tool a moment
//! later; what this buys is that it is never *allowed* by a rule somebody wrote
//! about somewhere else.

use crucible_core::{Target, ToolArgs, Workspace};

use crate::args::Args;

/// The file named in `field`, which has to be there already.
pub(crate) fn existing(
    workspace: &Workspace,
    tool: &'static str,
    args: &ToolArgs,
    field: &str,
) -> Target {
    let Some(requested) = requested(tool, args, field) else {
        return Target::unresolved();
    };
    found(workspace, workspace.existing(&requested))
}

/// The file named in `field`, which may not exist yet.
pub(crate) fn creatable(
    workspace: &Workspace,
    tool: &'static str,
    args: &ToolArgs,
    field: &str,
) -> Target {
    let Some(requested) = requested(tool, args, field) else {
        return Target::unresolved();
    };
    found(workspace, workspace.creatable(&requested))
}

/// What a search covers: the directory named in `field`, or the whole workspace
/// when the call named none.
///
/// The scope is the honest answer to what the call acts on, and it is a wider
/// answer than one file. A rule written about a file below it therefore does
/// not settle the call; it is honoured during the walk instead, where the file
/// is reached — see the note on searching in the permissions documentation.
pub(crate) fn searched(
    workspace: &Workspace,
    tool: &'static str,
    args: &ToolArgs,
    field: &str,
) -> Target {
    match requested(tool, args, field) {
        Some(requested) => found(workspace, workspace.existing(&requested)),
        None => found(workspace, workspace.existing(".")),
    }
}

/// The text of a path argument, when the call carries a readable one.
fn requested(tool: &'static str, args: &ToolArgs, field: &str) -> Option<String> {
    Args::parse(tool, args)
        .ok()?
        .optional_text(field)
        .ok()?
        .map(str::to_owned)
}

/// A resolution that either landed somewhere nameable or did not.
fn found<E>(workspace: &Workspace, resolved: Result<crucible_core::WorkspacePath, E>) -> Target {
    match resolved {
        Ok(path) => Target::resolved(workspace, &path),
        Err(_) => Target::unresolved(),
    }
}
