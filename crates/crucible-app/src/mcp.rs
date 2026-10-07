//! What `crucible mcp list` and `crucible mcp get NAME` print.
//!
//! The servers the home configuration writes down, read the way a run reads
//! them and then only described. Nothing here resolves a program, reads a
//! variable, opens a directory or starts a process, so a list says what a
//! server *would* be started with and never whether it would start: that is
//! only known by starting it, which is the thing somebody reading this list
//! has not decided to do yet. Selecting one is [`crate::selecting`]'s, and
//! happens only on a run that names it with `--with-mcp`.
//!
//! A record is somebody's way of handing a program a key, so nothing it holds
//! is shown as it stands. A variable's value is never shown, only its name; an
//! argument is shown as [`McpServer::shown_args`] redacts it; and every
//! string from the file, names included, is cut to [`SHOWN`] bytes and written
//! with what a terminal would act on as its escape. The configuration reader
//! already bounds how many servers, arguments and variables there can be.
//!
//! Only the home file is read, as a run reads servers only from there: a
//! checkout cannot write one down, so the directory this was started in has
//! nothing to add and is not opened.

use std::fmt::Write as _;
use std::path::Path;

use crucible_config::{Home, McpServer, Settings};
use crucible_types::shown::escaped;

use crate::AppError;

/// The most bytes of any one string from the file a list shows, before it is
/// escaped.
///
/// A path or an argument longer than this is not one somebody reads to the
/// end in a terminal, and the file it came from says the rest.
pub const SHOWN: usize = 512;

/// What a cut string ends with, so it is not read as whole.
const CUT: &str = "… (cut)";

/// Every server the home configuration writes down, a line each.
///
/// # Errors
///
/// The home configuration could not be read.
pub fn list(home: &Home) -> Result<String, AppError> {
    let settings = Settings::read_home(home)?;
    Ok(listing(&settings, &crucible_config::user(home)))
}

/// How the server called `name` would be started, and nothing about whether
/// it would.
///
/// # Errors
///
/// The home configuration could not be read, or writes down no server called
/// `name`: [`AppError::NoServer`], the refusal `--with-mcp` gives the same
/// name.
pub fn get(home: &Home, name: &str) -> Result<String, AppError> {
    let settings = Settings::read_home(home)?;
    described(&settings, &crucible_config::user(home), name)
}

/// The list, from settings already read out of the file at `at`.
pub fn listing(settings: &Settings, at: &Path) -> String {
    let servers = settings.mcp_servers();
    let at = shown(&at.display().to_string());
    let mut said = String::new();
    let _ = match servers.len() {
        0 => writeln!(said, "no MCP servers written down in {at}"),
        1 => writeln!(said, "1 MCP server written down in {at}"),
        many => writeln!(said, "{many} MCP servers written down in {at}"),
    };
    if servers.is_empty() {
        return said;
    }
    let _ = writeln!(said, "{UNSTARTED}\n");
    for server in &servers {
        let _ = writeln!(said, "  {}  {}", shown(server.name()), summary(server));
    }
    let _ = writeln!(
        said,
        "\n`crucible mcp get NAME` says how one is started, with secrets left out"
    );
    said
}

/// One server, from settings already read out of the file at `at`.
///
/// # Errors
///
/// [`AppError::NoServer`] where no record is called `name`.
pub fn described(settings: &Settings, at: &Path, name: &str) -> Result<String, AppError> {
    let servers = settings.mcp_servers();
    let Some(server) = servers.iter().find(|server| server.name() == name) else {
        return Err(AppError::NoServer {
            named: name.into(),
            has: if servers.is_empty() {
                "none".into()
            } else {
                servers
                    .iter()
                    .map(McpServer::name)
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into()
            },
        });
    };

    let mut said = String::new();
    let _ = writeln!(
        said,
        "{}, written down in {}",
        shown(server.name()),
        shown(&at.display().to_string())
    );
    let _ = writeln!(said, "{UNSTARTED}\n");
    let _ = writeln!(said, "  command    {}", shown(server.command()));
    rows(
        &mut said,
        "arguments",
        server.shown_args().iter().map(|arg| shown(arg)),
    );
    if let Some(directory) = server.directory() {
        let _ = writeln!(said, "  directory  {}", shown(directory));
    }
    rows(
        &mut said,
        "env",
        server
            .env()
            .map(|(variable, _)| format!("{}={}", shown(variable), McpServer::HIDDEN)),
    );
    rows(
        &mut said,
        "envFrom",
        server
            .env_from()
            .map(|(variable, from)| format!("{} from {}", shown(variable), shown(from))),
    );
    let _ = writeln!(
        said,
        "  waits      {}s to start, {}s a request, {}s to stop",
        server.handshake().as_secs(),
        server.request().as_secs(),
        server.shutdown().as_secs()
    );
    let _ = writeln!(said, "  restarts   {}", server.restarts());
    let _ = writeln!(
        said,
        "  required   {}",
        if server.required() {
            "yes: a run that names it fails when it cannot be prepared"
        } else {
            "no: a run that names it carries on without it when it cannot be prepared"
        }
    );
    Ok(said)
}

/// What a list says about starting, once, above the servers.
const UNSTARTED: &str = "none is started unless a run names it with --with-mcp, and none was \
                         started to write this, so whether each would start is not known";

/// A server's one line: what runs, and how much it is given.
fn summary(server: &McpServer) -> String {
    let counted = |count: usize, one: &str, many: &str| match count {
        1 => format!("1 {one}"),
        count => format!("{count} {many}"),
    };
    let mut parts = vec![
        shown(server.command()),
        counted(server.args().count(), "argument", "arguments"),
    ];
    let set = server.env().count();
    if set > 0 {
        parts.push(format!("{} set", counted(set, "variable", "variables")));
    }
    let taken = server.env_from().count();
    if taken > 0 {
        parts.push(format!(
            "{} taken from your environment",
            counted(taken, "variable", "variables")
        ));
    }
    if server.required() {
        parts.push("required".to_owned());
    }
    parts.join(", ")
}

/// A labelled block of one value a line, or nothing where there are none.
fn rows(said: &mut String, label: &str, values: impl Iterator<Item = String>) {
    for (index, value) in values.enumerate() {
        let label = if index == 0 { label } else { "" };
        let _ = writeln!(said, "  {label:<9}  {value}");
    }
}

/// A string from the file as a terminal is shown it: at most [`SHOWN`] bytes
/// of it, saying so where there was more, with every character a terminal
/// would act on or hide written as its escape.
fn shown(text: &str) -> String {
    if text.len() <= SHOWN {
        return escaped(text);
    }
    let mut end = SHOWN;
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}{CUT}", escaped(text.get(..end).unwrap_or_default()))
}

#[cfg(test)]
mod tests;
