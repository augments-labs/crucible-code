//! The path a call is about, worked out before the call is permitted.
//!
//! A rule is matched against the path the workspace resolved, never the text
//! the model sent: `../../etc/shadow` and `/etc/shadow` are the same file, and
//! a rule about one that is not a rule about the other is not a rule at all.
//!
//! So a path is resolved twice — once here, before anybody is asked, and once
//! in `run`, where the result is what actually gets opened. That is not a
//! redundancy waiting to be cached away. What crosses the permission boundary
//! between the two is a [`Target`], which is a name rather than a handle: its
//! text, and for a name that is not text, the path itself beside it. A tool
//! that carried a resolved handle across it would be a tool that had decided
//! what to open before anyone said yes. The second resolution is held to the
//! first, inside the workspace as well as outside it: the filesystem can answer
//! differently by the time the call runs, and a verdict about one file is not a
//! verdict about the file a name leads to now.
//!
//! Anything that does not resolve becomes [`Target::unresolved`], which no rule
//! matches. The call is asked about and then refused by the tool a moment
//! later; what this buys is that it is never *allowed* by a rule somebody wrote
//! about somewhere else, and that a yes to it is never remembered for the next.
//! A tool whose calls name no path at all says so with [`Target::pathless`]
//! instead, and is never built here.

use crucible_tools::{Approved, Sensitivity, Target};
use crucible_types::ToolArgs;
use crucible_workspace::{PathError, Workspace, WorkspacePath};

use crate::args::Args;

/// The existing path `requested` names, resolved for the open — the second of
/// the two resolutions the module doc describes, made in the tool's `run`.
///
/// Inside the workspace it is [`Workspace::existing`]'s answer, provided that
/// answer is still the file the question named — see [`held`]. A path that
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
        Ok(path) => held(workspace, approved, requested, &path).map(|()| path),
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
                Ok(_) => Err(moved(requested)),
                Err(problem) => Err(problem.to_string()),
            }
        }
        Err(problem) => Err(problem.to_string()),
    }
}

/// Whether `path`, a resolution inside the workspace made in `run`, is the
/// file the verdict in hand was reached about.
///
/// Asked before anything is read or changed, because it is the read or the
/// change the verdict permitted. A verdict about a path that did not resolve
/// when the question was put named no file, so there is nothing to hold the
/// resolution to and it is not refused here.
///
/// # Errors
///
/// Text for a failed output: the path no longer leads where the question said.
pub(crate) fn held(
    workspace: &Workspace,
    approved: &Approved,
    requested: &str,
    path: &WorkspacePath,
) -> Result<(), String> {
    agrees(approved, requested, &Target::resolved(workspace, path))
}

/// Whether the file `requested` would create is still the one the verdict in
/// hand was reached about — asked by `write` before it makes a directory,
/// where [`held`] cannot be asked yet because the path does not resolve until
/// its parents exist.
///
/// # Errors
///
/// Text for a failed output: the path no longer leads where the question said.
pub(crate) fn intends(
    workspace: &Workspace,
    approved: &Approved,
    requested: &str,
) -> Result<(), String> {
    agrees(approved, requested, &Target::intended(workspace, requested))
}

/// Whether `now`, the target a call resolves to as it runs, is the target its
/// verdict named.
fn agrees(approved: &Approved, requested: &str, now: &Target) -> Result<(), String> {
    let named = match approved.sensitivity() {
        Sensitivity::ReadOnly { target }
        | Sensitivity::ReadsOutside { target }
        | Sensitivity::MutatesFile { target } => target,
        // A verdict about running a program or reaching a host is about no
        // file, so no file is the one it named.
        Sensitivity::SpawnsProcess { .. } | Sensitivity::ReachesNetwork { .. } => {
            return Err(moved(requested));
        }
    };

    // A verdict about a path that named no file has none to hold this one to.
    // One about a call naming no path is not about a file either, so a file
    // found now is never the one it named.
    if *named == Target::unresolved() || named == now {
        Ok(())
    } else {
        Err(moved(requested))
    }
}

/// The refusal of a path that resolves somewhere other than the file the
/// question named.
fn moved(requested: &str) -> String {
    format!("{requested} no longer leads to the file the question named")
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
fn found<E>(
    workspace: &Workspace,
    resolved: Result<crucible_workspace::WorkspacePath, E>,
) -> Target {
    match resolved {
        Ok(path) => Target::resolved(workspace, &path),
        Err(_) => Target::unresolved(),
    }
}

#[cfg(test)]
mod tests {
    use crucible_tools::Target;
    use crucible_types::ToolArgs;

    use crate::sample::Sample;

    #[test]
    fn a_parent_component_to_an_existing_file_is_the_target_of_that_file() {
        let sample = Sample::new("target-parent-existing");
        sample.write("sub/kept.txt", "kept");
        sample.write("protected.txt", "kept");
        let workspace = sample.workspace();
        let through = ToolArgs::new(r#"{"path":"sub/../protected.txt"}"#);
        let direct = ToolArgs::new(r#"{"path":"protected.txt"}"#);

        let target = super::existing(&workspace, "edit", &through, "path");

        assert_ne!(target, Target::unresolved());
        assert_eq!(target, super::existing(&workspace, "edit", &direct, "path"));
    }
}
