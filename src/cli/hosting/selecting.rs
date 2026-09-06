//! Which written-down servers this run starts, and what they are started with.
//!
//! The reading of the `mcp.servers` block already happened, in configuration,
//! and produced records that resolve nothing. This is the other half: a
//! selection names some of those records, and every unresolved thing in them
//! becomes a resolved one — a bare program becomes an absolute path, a
//! variable name becomes the value crucible's own environment holds, and a
//! directory becomes a root the confinement grants.
//!
//! **Nothing is selected by default.** A machine with twenty servers written
//! down and no `--with-mcp` starts none of them, which is the same statement
//! the reader makes and is worth making twice: a document is a list of servers
//! somebody could run, and a run is the moment one of them is chosen.
//!
//! **A name nobody wrote down is a refusal, not an omission.** Somebody who
//! asked for a server by name and got a run without it would be told nothing,
//! and the first sign would be a model that could not do what it was asked.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crucible_config::{McpServer, SandboxSettings, Settings};
use crucible_core::{SandboxEnvironment, SandboxPolicy, Workspace};

use super::Chosen;
use crate::cli::Fatal;

/// The servers `named` selects, resolved against this machine.
///
/// # Errors
///
/// [`Fatal::NoServer`] where a name has no record, and [`Fatal::Server`] where
/// a record cannot be turned into something startable: a program that is not
/// on the `PATH`, a variable that is not set, a directory the confinement will
/// not take.
pub(crate) fn selected(
    named: &[String],
    settings: &Settings,
    workspace: &Workspace,
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Result<Vec<Chosen>, Fatal> {
    let records = settings.mcp_servers();
    named
        .iter()
        .map(|name| {
            let record = records
                .iter()
                .find(|record| record.name() == name.as_str())
                .ok_or_else(|| Fatal::NoServer {
                    named: name.as_str().into(),
                    has: written(&records),
                })?;
            one(record, workspace, settings.sandbox(), &lookup)
        })
        .collect()
}

/// One record, as something that can be started.
fn one(
    record: &McpServer,
    workspace: &Workspace,
    settings: &SandboxSettings,
    lookup: &impl Fn(&str) -> Option<OsString>,
) -> Result<Chosen, Fatal> {
    let refused = |problem: String| Fatal::Server {
        server: record.name().into(),
        problem: problem.into(),
    };

    let program = program(record.command(), lookup)
        .ok_or_else(|| refused(format!("no {} on the PATH", record.command())))?;
    let arguments = record.args().map(OsString::from);
    let environment = environment(record, lookup).map_err(refused)?;
    let policy = confinement(record, workspace, settings).map_err(refused)?;

    Ok(Chosen::new(record.name(), program, arguments, policy)
        .following_enablement(settings.enablement())
        .given(environment)
        .waiting(record.handshake(), record.request(), record.shutdown())
        // Named on the command line and written down as required are two
        // different statements, and this is the second one. A run says which
        // servers it wants; the record says which of them it cannot do without.
        .required(record.required())
        // A ceiling on the endings crucible can prove were harmless, not a
        // retry count: an ending with a request outstanding is refused whatever
        // this says, so a document cannot buy its way past that with a number.
        .restarting(record.restarts()))
}

/// Where the written-down command is, as an absolute path.
///
/// A path written out is taken as it stands and a bare name is looked for,
/// through the one resolver that decides which `PATH` elements count. Nothing
/// here checks that the file will execute: that is the kernel's answer and it
/// is given at the moment of starting, where it is still true.
fn program(command: &str, lookup: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let written = Path::new(command);
    if written.is_absolute() {
        return Some(written.to_path_buf());
    }
    let spelling = crucible_tools::program::spelled(command);
    crucible_tools::program::on_path(lookup, &spelling)
}

/// Everything the server is started with, and nothing else.
///
/// The whole environment, rather than crucible's own with these added: a
/// server inherits nothing it was not given a name for. `envFrom` is where a
/// secret travels, and it travels as a value read here and never written
/// anywhere — the document holds the name of a variable, which is not one.
fn environment(
    record: &McpServer,
    lookup: &impl Fn(&str) -> Option<OsString>,
) -> Result<SandboxEnvironment, String> {
    let mut entries: Vec<(Box<str>, OsString)> = record
        .env()
        .map(|(name, value)| (name.into(), OsString::from(value)))
        .collect();

    for (name, from) in record.env_from() {
        let held = lookup(from).ok_or_else(|| {
            // The name of the variable, never a value: this sentence reaches a
            // terminal, and a run that failed because a key was unset must not
            // be the thing that prints one that was.
            format!("envFrom names {from}, which is not set in crucible's own environment")
        })?;
        entries.push((name.into(), held));
    }

    SandboxEnvironment::new(
        entries
            .iter()
            .map(|(name, value)| (name.as_ref(), value.as_os_str() as &OsStr)),
    )
    .map_err(|problem| problem.to_string())
}

/// What the server may reach.
///
/// The workspace's own policy, with this run's enabled choice, with
/// the written-down directory added as a root and made the one it starts in.
/// A server is somebody else's program: it gets what a confined command gets,
/// and the directory is the only thing about that a document may move.
///
/// It moves it *outwards*: the path becomes a read-write root, and nothing here
/// asks whether it sits inside the workspace. That is deliberate and it is safe
/// for one reason, which is not in this file — the whole `mcp` block widens, so
/// only the configuration file in the home directory may state it. A checked-in
/// project file is refused at the block, before any of this is reached.
///
/// # Errors
///
/// A directory the policy will not take: one that is relative, unnormalised,
/// empty or oversized, or one that conflicts with a rule the workspace already
/// states.
fn confinement(
    record: &McpServer,
    workspace: &Workspace,
    settings: &SandboxSettings,
) -> Result<SandboxPolicy, String> {
    // The user-owned server directory is part of the initial grant. Apply
    // sandbox restrictions afterwards so it cannot reopen a protected path.
    let (reaching, directory) = match record.directory() {
        Some(written) => {
            let reaching = workspace
                .clone()
                .reaching([written])
                .map_err(|problem| problem.to_string())?;
            let resolved = reaching
                .roots()
                .last()
                .map(Path::to_path_buf)
                .ok_or_else(|| "server directory did not resolve".to_owned())?;
            (reaching, Some(resolved))
        }
        None => (workspace.clone(), None),
    };
    let standard = settings
        .enforcing_policy(&reaching)
        .map_err(|problem| problem.to_string())?;
    let Some(directory) = directory else {
        return Ok(standard);
    };
    SandboxPolicy::new(
        standard.enabled(),
        standard.filesystem().to_vec(),
        directory,
        standard.network().clone(),
        standard.limits(),
    )
    .map_err(|problem| problem.to_string())
}

/// The names the document did hold, for a sentence that says what to type.
fn written(records: &[McpServer]) -> Box<str> {
    if records.is_empty() {
        return "no servers are written down under mcp.servers".into();
    }
    records
        .iter()
        .map(McpServer::name)
        .collect::<Vec<_>>()
        .join(", ")
        .into()
}

#[cfg(test)]
mod tests;
