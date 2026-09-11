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

use crucible_core::{Approved, PathError, Sensitivity, Target, ToolArgs, Workspace, WorkspacePath};

use crate::args::Args;

/// The existing path `requested` names, resolved for the open — the second of
/// the two resolutions the module doc describes, made in the tool's `run`.
///
/// Inside the workspace it is [`Workspace::existing`]'s answer. A path that
/// escapes is followed only when the verdict in hand was reached about a read
/// that leaves the workspace, and only to the very file the question named:
/// the filesystem can answer differently now than it did when the question was
/// put — a link retargeted in between — and the verdict does not stretch to
/// the new answer.
///
/// # Errors
///
/// Text for a failed output: the path did not resolve, the verdict was not
/// about an outside read, or the path no longer leads where the question said.
pub(crate) fn opened(
    workspace: &Workspace,
    approved: &Approved,
    requested: &str,
) -> Result<WorkspacePath, String> {
    match workspace.existing(requested) {
        Ok(path) => Ok(path),
        Err(problem @ PathError::Escapes { .. }) => {
            let named = match approved.sensitivity() {
                Sensitivity::ReadsOutside { target } => target,
                Sensitivity::ReadOnly { .. }
                | Sensitivity::MutatesFile { .. }
                | Sensitivity::SpawnsProcess { .. }
                | Sensitivity::ReachesNetwork { .. } => return Err(problem.to_string()),
            };

            match workspace.outside(requested) {
                Ok(path) if Target::outside(&path) == *named => Ok(path),
                Ok(_) => Err(format!(
                    "{requested} no longer leads to the file the question named"
                )),
                Err(problem) => Err(problem.to_string()),
            }
        }
        Err(problem) => Err(problem.to_string()),
    }
}

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

/// What reading the file named in `field` amounts to: an ordinary read where
/// the workspace contains it, a read that leaves the workspace where it
/// resolves outside every reached directory.
///
/// The whole sensitivity rather than a target, because which variant a read is
/// depends on where the path led — and only the resolution can say.
pub(crate) fn reads(
    workspace: &Workspace,
    tool: &'static str,
    args: &ToolArgs,
    field: &str,
) -> Sensitivity {
    match requested(tool, args, field) {
        Some(requested) => led(workspace, &requested),
        None => Sensitivity::ReadOnly {
            target: Target::unresolved(),
        },
    }
}

/// What a search covers, as [`reads`] answers it: the directory named in
/// `field`, or the whole workspace when the call named none.
///
/// The scope is the honest answer to what the call acts on, and it is a wider
/// answer than one file. A rule written about a file below it therefore does
/// not settle the call; it is honoured during the walk instead, where the file
/// is reached — see the note on searching in the permissions documentation.
pub(crate) fn searches(
    workspace: &Workspace,
    tool: &'static str,
    args: &ToolArgs,
    field: &str,
) -> Sensitivity {
    led(
        workspace,
        requested(tool, args, field).as_deref().unwrap_or("."),
    )
}

/// Where one requested path led: inside, outside, or nowhere.
fn led(workspace: &Workspace, requested: &str) -> Sensitivity {
    match workspace.existing(requested) {
        Ok(path) => Sensitivity::ReadOnly {
            target: Target::resolved(workspace, &path),
        },
        Err(PathError::Escapes { .. }) => match workspace.outside(requested) {
            Ok(path) => Sensitivity::ReadsOutside {
                target: Target::outside(&path),
            },
            Err(_) => Sensitivity::ReadOnly {
                target: Target::unresolved(),
            },
        },
        Err(_) => Sensitivity::ReadOnly {
            target: Target::unresolved(),
        },
    }
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
    Target::intended(workspace, &requested)
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
