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

use std::fmt;
use std::time::Duration;

use serde_json::Value;

use super::Settings;
use crate::env;
use crate::shape::whole;

/// The most server records one document may write down.
///
/// Far beyond a machine somebody configures by hand, and small enough that a
/// document that grew a block by accident cannot make startup walk it forever.
///
/// Applied where the key is declared, so a document over the boundary is
/// refused by name and position while it is being read, rather than by this
/// reader, which has neither to name by the time it holds the block.
pub(crate) const SERVERS: usize = 64;

/// The most arguments one server may be given.
///
/// Applied at the declaration like the bound above, and refused there rather
/// than shortened here: a launch assembled from part of an argument list runs
/// a command nobody wrote.
pub(crate) const ARGS: usize = 256;

/// The most environment entries one server may be given, in each block.
pub(crate) const VARIABLES: usize = 256;

/// What [`McpServer::shown_args`] writes where a secret could have been.
const HIDDEN: &str = McpServer::HIDDEN;

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
#[derive(Clone, PartialEq, Eq)]
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

impl fmt::Debug for McpServer {
    /// Written by hand so `env` is named and not shown, and `args` is shown as
    /// [`shown_args`](Self::shown_args) shows it.
    ///
    /// A key for this server is written in the block it is started with, or
    /// on its command line. A derive is how one reaches a log line,
    /// an error or a panic payload without anybody having decided that it
    /// should, which is why the redaction lives in the type rather than in
    /// whatever prints it.
    ///
    /// `env_from` stays whole: both of its halves are names of variables, and a
    /// name is not a value.
    ///
    /// The record is taken apart rather than read field by field, so that a
    /// field added later is a compilation error here instead of a field this
    /// quietly stops printing. That is the one thing the derive gave for free
    /// and the reason to give it up was `env` and `args`, not the rest of the
    /// record.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            name,
            command,
            args,
            directory,
            env,
            env_from,
            handshake,
            request,
            shutdown,
            restarts,
            required,
        } = self;

        f.debug_struct("McpServer")
            .field("name", name)
            .field("command", command)
            .field("args", &shown(args.iter().map(AsRef::as_ref)))
            .field("directory", directory)
            .field("env", &env::Named(env))
            .field("env_from", env_from)
            .field("handshake", handshake)
            .field("request", request)
            .field("shutdown", shutdown)
            .field("restarts", restarts)
            .field("required", required)
            .finish()
    }
}

impl McpServer {
    /// What [`shown_args`](Self::shown_args) writes where a secret could have
    /// been, and what a reader is shown in place of a variable's value.
    pub const HIDDEN: &str = "<redacted>";

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

    /// What to pass it, as a reader is shown it: each argument exactly as
    /// written, or [`HIDDEN`](Self::HIDDEN) in its place where it could carry a
    /// secret, and never in part.
    ///
    /// [`args`](Self::args) is what the server is started with and must stay
    /// whole; this is for a list somebody reads, and could paste. A key is
    /// often given to a server on its command line, and a password inside an
    /// argument can hold any character that would end it, so a reading that
    /// hides only the password shows the rest of it wherever that reading is
    /// wrong. An argument is hidden whole instead where:
    ///
    /// - a word in it, a run of letters, digits, `-` and `_`, is a name a
    ///   secret is given under, as in `--api-key=...`, a connection string's
    ///   `Password=...`, JSON's `"apiKey"` or a query's `?token=...`, or a
    ///   scheme a credential follows, as in `Basic dXNlcjpwYXNz`;
    /// - it holds an `@` after a `:`, as `user:password@host` and a URL with a
    ///   user do;
    /// - it holds a `?` or `#` after a URL's `://`, or a URL whose host is
    ///   followed by a `:` and anything but a port number;
    /// - a run of it is shaped like a token;
    /// - or the argument before it is hidden for the first of these reasons: it
    ///   names a secret or a scheme anywhere in it, as `--api-key`,
    ///   `Authorization:`, `X-Api-Key` and `Bearer` do, however it ends.
    ///
    /// A value after a flag whose name says nothing, as `-p`, is shown. More is
    /// hidden than is secret, on purpose: a value shown in error is the one
    /// that cannot be taken back. So the argument after `--tokenizer=fast` or
    /// `keys` is hidden too, though neither names the next: reading where a
    /// name ends to tell a flag from its own value is the reading that showed
    /// a secret wherever it was wrong.
    #[must_use]
    pub fn shown_args(&self) -> Vec<String> {
        shown(self.args.iter().map(AsRef::as_ref))
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

    /// How long the server has to agree a protocol version, and each step of
    /// starting its sandbox has to answer.
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
                .and_then(whole)
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
                .collect()
        })
        .unwrap_or_default()
}

/// A whole number of seconds, or the default the schema publishes for the key.
fn seconds(record: &Value, key: &str, usual: u64) -> Duration {
    Duration::from_secs(record.get(key).and_then(whole).unwrap_or(usual))
}

/// What [`McpServer::shown_args`] shows of a list of arguments.
fn shown<'a>(args: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut follows = false;
    args.into_iter()
        .map(|arg| {
            let hidden = follows || credential(arg);
            follows = named(arg);
            if hidden {
                HIDDEN.to_owned()
            } else {
                arg.to_owned()
            }
        })
        .collect()
}

/// Replaces each server's arguments in a printed document with what
/// [`McpServer::shown_args`] shows of them.
///
/// The document-holding types print through [`env::Redacted`], and a document
/// holds every server's arguments unread, a key among them as often as not.
/// An argument list that is not all text, which no document that parsed
/// holds, is hidden whole.
pub(crate) fn hide_args(shown_document: &mut Value) {
    let Some(servers) = shown_document
        .get_mut("mcp")
        .and_then(|block| block.get_mut("servers"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };

    for record in servers.values_mut() {
        let Some(args) = record.get_mut("args") else {
            continue;
        };
        let read: Option<Vec<&str>> = args
            .as_array()
            .and_then(|held| held.iter().map(Value::as_str).collect());
        *args = match read {
            Some(read) => Value::from(shown(read)),
            None => Value::String(HIDDEN.to_owned()),
        };
    }
}

/// Whether an argument could carry a secret, for any of the reasons
/// [`McpServer::shown_args`] gives, so that it is hidden whole.
fn credential(arg: &str) -> bool {
    named(arg) || has_user(arg) || past_host(arg) || shaped(arg)
}

/// Whether a word in `arg` is a name a secret is given under, or a scheme a
/// credential follows, as `Basic` in `Basic dXNlcjpwYXNz`, wherever in the
/// argument it is written. Such an argument is hidden, and so is the one after
/// it, which it may be naming.
fn named(arg: &str) -> bool {
    words(arg).any(|word| secret(word) || scheme(word))
}

/// The words of `arg`: its runs of letters, digits, `-` and `_`.
fn words(arg: &str) -> impl DoubleEndedIterator<Item = &str> {
    arg.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
}

/// Whether `arg` holds an `@` after a `:`: a user and its password, with a
/// scheme in front or without one, or a URL's user.
fn has_user(arg: &str) -> bool {
    arg.find(':')
        .and_then(|colon| arg.get(colon..))
        .is_some_and(|after| after.contains('@'))
}

/// Whether a URL in `arg` says more than where it is: a `?` or `#` anywhere
/// after the first `://`, or a host that is followed by a `:` and anything but
/// a port number, as `https://user:password/...` written with no `@` is.
///
/// A host runs to the first `/`, `?`, `#`, space, quote or other character a
/// URL is not written with, or `;` or `,`. Each one ends at a `/` at the
/// latest, so no host is read twice.
fn past_host(arg: &str) -> bool {
    const ENDS: [char; 15] = [
        '/', '?', '#', '"', '\'', '`', '<', '>', '{', '}', '|', '\\', '^', ';', ',',
    ];
    let Some((_, after)) = arg.split_once("://") else {
        return false;
    };
    after.contains(['?', '#'])
        || arg.match_indices("://").any(|(at, sign)| {
            let rest = arg.get(at + sign.len()..).unwrap_or_default();
            let end = rest
                .find(|c: char| ENDS.contains(&c) || c.is_whitespace())
                .unwrap_or(rest.len());
            !hosted(rest.get(..end).unwrap_or_default())
        })
}

/// Whether a run of `arg` between the characters a token is not written with
/// is shaped like one.
fn shaped(arg: &str) -> bool {
    arg.split(|c: char| !(c.is_ascii_alphanumeric() || SEPARATORS.contains(&c)))
        .any(opaque)
}

/// Whether a URL's host, with what follows it up to its path, is a host alone
/// or a host and a port: nothing after it, or a `:` and digits. A host
/// written as a bracketed address is read from its closing bracket.
fn hosted(authority: &str) -> bool {
    let after = match authority.strip_prefix('[') {
        Some(inner) => inner.split_once(']').map(|(_, after)| after),
        None => Some(
            authority
                .find(':')
                .map_or("", |colon| authority.get(colon..).unwrap_or_default()),
        ),
    };
    after.is_some_and(|after| {
        after.is_empty()
            || after
                .strip_prefix(':')
                .is_some_and(|port| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()))
    })
}

/// Whether a name is one a secret is given under: a flag, a variable, a
/// header or a query key.
fn secret(name: &str) -> bool {
    const STEMS: [&str; 13] = [
        "token",
        "key",
        "secret",
        "pass",
        "pwd",
        "auth",
        "bearer",
        "credential",
        "cookie",
        "session",
        "private",
        "signature",
        "jwt",
    ];
    // Too short to look for inside a longer word: "pat" is in "path".
    const WORDS: [&str; 1] = ["pat"];
    let name = unquoted(name).trim_start_matches('-').to_ascii_lowercase();
    !name.is_empty()
        && (STEMS.iter().any(|stem| name.contains(stem))
            || name
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|part| WORDS.contains(&part)))
}

/// Whether a word is an authorization scheme, which a credential follows.
fn scheme(word: &str) -> bool {
    ["bearer", "basic", "token"]
        .iter()
        .any(|scheme| unquoted(word).eq_ignore_ascii_case(scheme))
}

/// What a token is written with besides letters and digits.
const SEPARATORS: [char; 6] = ['-', '_', '.', '~', '+', '='];

/// Whether a word is shaped like a token: letters and digits run together,
/// long enough that nobody typed it as a word.
///
/// A path, a package name and a version are not: each holds a character a
/// token is not written with, or no run of letters and digits long enough.
fn opaque(word: &str) -> bool {
    let mixed = |part: &str| {
        part.chars().any(|c| c.is_ascii_digit()) && part.chars().any(|c| c.is_ascii_alphabetic())
    };
    word.chars()
        .all(|c| c.is_ascii_alphanumeric() || SEPARATORS.contains(&c))
        && mixed(word)
        && (word.len() >= 32
            || word
                .split(SEPARATORS)
                .any(|part| part.len() >= 16 && mixed(part)))
}

/// A word with the quotes a shell would take off its front taken off.
fn unquoted(word: &str) -> &str {
    word.trim_start_matches(['\'', '"'])
}

/// Borrowed halves of a retained pair.
fn pairs(held: &[(Box<str>, Box<str>)]) -> impl Iterator<Item = (&str, &str)> {
    held.iter()
        .map(|(name, written)| (name.as_ref(), written.as_ref()))
}

#[cfg(test)]
mod tests;
