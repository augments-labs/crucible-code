//! MCP servers the reader has written down, read back as inert records.
//!
//! Its own module for the reason `output` has one: the document holds an
//! object and the program holds a value, and the reading of it belongs beside
//! the type it produces.
//!
//! Nothing here resolves a program, reads an environment variable, opens a
//! directory or starts a process. A record says that a server exists and how
//! one would be launched; a server runs when something selects it by name, and
//! that selection is made per agent or per run rather than by anything in this
//! file. So a machine with twenty servers written down and none selected
//! starts none of them, and a build that never reaches the selection reaches no
//! server at all.

use std::time::Duration;

use serde_json::Value;

use super::Settings;

/// The most server records read back from one document.
///
/// Far beyond a machine somebody configures by hand, and small enough that a
/// document that grew a block by accident cannot make startup walk it forever.
const SERVERS: usize = 64;

/// The most arguments read back for one server.
const ARGS: usize = 256;

/// The most environment entries read back for one server, in each block.
const VARIABLES: usize = 256;

/// What each timeout is where the record does not say, in seconds.
///
/// These are the numbers `shape` publishes as the defaults for their keys, and
/// a test walks them through this reader so the schema and the program cannot
/// drift into two answers.
const HANDSHAKE: u64 = 10;
const REQUEST: u64 = 60;
const SHUTDOWN: u64 = 5;

impl Settings {
    /// Every MCP server this document declares, in the order the reader
    /// chose.
    ///
    /// An empty list where none was written, which is the ordinary machine:
    /// crucible installs no server, so the block is absent until somebody
    /// writes one.
    #[must_use]
    pub fn mcp_servers(&self) -> Vec<McpServer> {
        let Some(servers) = self
            .value
            .get("mcp")
            .and_then(|block| block.get("servers"))
            .and_then(Value::as_object)
        else {
            return Vec::new();
        };

        servers
            .iter()
            .take(SERVERS)
            .filter_map(|(name, record)| McpServer::read(name, record))
            .collect()
    }
}

/// One server, as the document states it.
///
/// Every field was written down; none of it has been resolved. `command` is a
/// name or a path that has not been looked for, `env_from` holds the names of
/// variables that have not been read, and `directory` is a path nothing has
/// opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    name: Box<str>,
    command: Box<str>,
    args: Vec<Box<str>>,
    directory: Option<Box<str>>,
    env: Vec<(Box<str>, Box<str>)>,
    env_from: Vec<(Box<str>, Box<str>)>,
    handshake: Duration,
    request: Duration,
    shutdown: Duration,
    restarts: u32,
    required: bool,
}

impl McpServer {
    /// The identifier this server's tools are qualified by.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The program, as written: an absolute path or a bare name for `PATH`.
    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }

    /// What to pass it, applied verbatim.
    pub fn args(&self) -> impl Iterator<Item = &str> {
        self.args.iter().map(AsRef::as_ref)
    }

    /// The absolute directory to start it in, where one was written.
    #[must_use]
    pub fn directory(&self) -> Option<&str> {
        self.directory.as_deref()
    }

    /// Variables to set, as name and value.
    pub fn env(&self) -> impl Iterator<Item = (&str, &str)> {
        pairs(&self.env)
    }

    /// Variables to take from crucible's own environment, as the name the
    /// server reads and the name crucible reads.
    ///
    /// Names on both sides. Nothing has been read, so no value this returns can
    /// be a secret.
    pub fn env_from(&self) -> impl Iterator<Item = (&str, &str)> {
        pairs(&self.env_from)
    }

    /// How long the server has to agree a protocol version.
    #[must_use]
    pub const fn handshake(&self) -> Duration {
        self.handshake
    }

    /// How long one request to it may take.
    #[must_use]
    pub const fn request(&self) -> Duration {
        self.request
    }

    /// How long it is given to stop on its own.
    #[must_use]
    pub const fn shutdown(&self) -> Duration {
        self.shutdown
    }

    /// How many times it may be started again after it ends.
    #[must_use]
    pub const fn restarts(&self) -> u32 {
        self.restarts
    }

    /// Whether a run that selected it fails when it cannot be prepared.
    #[must_use]
    pub const fn required(&self) -> bool {
        self.required
    }

    /// Reads one record.
    ///
    /// `None` only where there is no command, which [`Document::parse`] has
    /// already refused for every document that reached here — so this is the
    /// same answer stated twice rather than a second policy, and the one that
    /// names the file is the one a reader meets.
    ///
    /// [`Document::parse`]: crate::document::Document::parse
    fn read(name: &str, record: &Value) -> Option<Self> {
        let command = record.get("command")?.as_str()?;

        Some(Self {
            name: name.into(),
            command: command.into(),
            args: record
                .get("args")
                .and_then(Value::as_array)
                .map(|held| {
                    held.iter()
                        .filter_map(Value::as_str)
                        .take(ARGS)
                        .map(Into::into)
                        .collect()
                })
                .unwrap_or_default(),
            directory: record
                .get("directory")
                .and_then(Value::as_str)
                .map(Into::into),
            env: block(record, "env"),
            env_from: block(record, "envFrom"),
            handshake: seconds(record, "handshakeSeconds", HANDSHAKE),
            request: seconds(record, "requestSeconds", REQUEST),
            shutdown: seconds(record, "shutdownSeconds", SHUTDOWN),
            restarts: record
                .get("restarts")
                .and_then(Value::as_u64)
                .and_then(|held| u32::try_from(held).ok())
                .unwrap_or_default(),
            required: record
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
        })
    }
}

/// One of the two environment blocks, read as ordered pairs.
fn block(record: &Value, key: &str) -> Vec<(Box<str>, Box<str>)> {
    record
        .get(key)
        .and_then(Value::as_object)
        .map(|held| {
            held.iter()
                .filter_map(|(name, written)| {
                    written
                        .as_str()
                        .map(|written| (name.as_str().into(), written.into()))
                })
                .take(VARIABLES)
                .collect()
        })
        .unwrap_or_default()
}

/// A whole number of seconds, or the default the schema publishes for the key.
fn seconds(record: &Value, key: &str, usual: u64) -> Duration {
    Duration::from_secs(record.get(key).and_then(Value::as_u64).unwrap_or(usual))
}

/// Borrowed halves of a retained pair.
fn pairs(held: &[(Box<str>, Box<str>)]) -> impl Iterator<Item = (&str, &str)> {
    held.iter()
        .map(|(name, written)| (name.as_ref(), written.as_ref()))
}

#[cfg(test)]
mod tests;
