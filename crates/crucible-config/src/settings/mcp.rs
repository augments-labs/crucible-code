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
    /// Written by hand so `env` is named and not shown.
    ///
    /// The block is what this server is started with, so a key for it is
    /// written there and nowhere else. A derive is how one reaches a log line,
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
    /// and the reason to give it up was `env`, not the rest of the record.
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
            .field("args", args)
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

    /// What to pass it, as a reader is shown it: each argument with whatever
    /// could carry a secret replaced by [`HIDDEN`](Self::HIDDEN).
    ///
    /// [`args`](Self::args) is what the server is started with and must stay
    /// whole; this is for a list somebody reads, and could paste. A key is
    /// often given to a server on its command line, so an argument is read for
    /// the places one goes. Each word is read as the pairs it holds: every
    /// `name=value` and `name:value`, a value read again for a pair of its own
    /// (`--env=DB_PASSWORD=...`), pairs run together with `;`, `&` or `,` as a
    /// connection string or a query writes them, and a JSON object's
    /// `"key":"value"` like any other pair. Once a pair's name, a flag or a
    /// header names a secret, the rest of the argument is hidden, and the next
    /// argument too where what was hidden ends on a scheme word such as
    /// `Bearer`. A URL, wherever in a word its `scheme://` starts, is shown
    /// without its user, query and fragment, as `user:password@host` is shown
    /// without its password. The user is hidden whatever its password holds:
    /// a `;`, `,`, quote or other character that ends a URL ends one only
    /// after its user, but for the quote that closes a string the URL opens,
    /// as `"url":"https://...` opens one, and a shell's `'"'"'` or `'\''`
    /// writes a quote rather than closing one. And a word, or a piece of one,
    /// shaped like a token is hidden. A value after a flag whose name says nothing, as `-p`,
    /// is shown. More is hidden than is secret, on purpose: a value shown in
    /// error is the one that cannot be taken back.
    #[must_use]
    pub fn shown_args(&self) -> Vec<String> {
        let mut withheld = false;
        self.args
            .iter()
            .map(|arg| {
                if withheld {
                    // A scheme word is still the name of what comes after it,
                    // as `Authorization:`, `Bearer`, then the token.
                    withheld = names_next(arg);
                    return HIDDEN.to_owned();
                }
                let (shown, naming) = spoken(arg);
                withheld = naming;
                shown
            })
            .collect()
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

/// One argument as a reader is shown it, and whether it ends on the name of a
/// secret, so the next argument is that secret's value.
///
/// An argument a shell will split, as `sh -c '...'` is given, is read word by
/// word, and once a word names a secret the rest of the argument is its value.
/// What was hidden says whether the next argument is a value too: where it
/// ends on a scheme word, as `Authorization=Bearer` does, the token is next.
fn spoken(arg: &str) -> (String, bool) {
    let mut shown = String::with_capacity(arg.len());
    let mut naming = false;
    let mut at = 0;
    for piece in arg.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end();
        let gap = piece.get(word.len()..).unwrap_or_default();
        let start = at;
        at += piece.len();
        if word.is_empty() {
            shown.push_str(gap);
            continue;
        }
        if naming {
            shown.push_str(HIDDEN);
            return (shown, names_next(arg.get(start..).unwrap_or_default()));
        }
        let (said, value) = worded(word);
        shown.push_str(&said);
        match value {
            Some(value) if !value.is_empty() => {
                shown.push_str(HIDDEN);
                let from = start + word.len().saturating_sub(value.len());
                return (shown, names_next(arg.get(from..).unwrap_or_default()));
            }
            Some(_) => naming = true,
            None => {}
        }
        shown.push_str(gap);
    }
    (shown, naming)
}

/// Whether what was hidden ends on a scheme word, so the argument after it is
/// the credential that scheme names.
fn names_next(hidden: &str) -> bool {
    hidden.split_whitespace().last().is_some_and(scheme)
}

/// What separates one pair in a word from the next, or a name from its value.
const MARKS: [char; 5] = ['=', ':', ';', '&', ','];

/// One word as a reader is shown it, up to where a secret's value starts if
/// one does; and then what of the word is that value, empty where the value
/// is the next word.
///
/// A word is read as the pairs it holds: every `name=value` and `name:value`,
/// a value read again for a pair of its own, as `--env=DB_PASSWORD=...` holds
/// one, and pairs run together with `;`, `&` or `,`, as a connection string or
/// a query writes them. A JSON object is read the same way, its `"key":"value"`
/// a pair like any other. A URL is read as one from its `scheme://`, wherever in
/// the word that starts.
fn worded(word: &str) -> (String, Option<&str>) {
    let mut shown = String::with_capacity(word.len());
    let mut at = 0;
    while let Some(rest) = word.get(at..).filter(|rest| !rest.is_empty()) {
        let (start, end) = url(rest).unwrap_or((rest.len(), rest.len()));
        let text = rest.get(..start).unwrap_or_default();
        if let Some(cut) = paired(text, start == rest.len(), &mut shown) {
            return (shown, word.get(at + cut..));
        }
        if let Some(found) = rest.get(start..end).filter(|found| !found.is_empty()) {
            shown.push_str(&located(found));
        }
        at += end;
    }
    (shown, None)
}

/// The pairs in `text`, a word or the part of one before a URL, written to
/// `shown`; and, where a pair's name is a secret's, how far into `text` its
/// value starts.
///
/// `last` is whether `text` ends the word, where a flag or a scheme word on
/// its own names whatever comes after it.
fn paired(text: &str, last: bool, shown: &mut String) -> Option<usize> {
    let mut at = 0;
    while let Some(rest) = text.get(at..)
        && let Some(found) = rest.find(MARKS)
    {
        let (piece, marked) = rest.split_at(found);
        let (mark, after) = marked.split_at(1);
        if matches!(mark, "=" | ":") && secret(piece) {
            shown.push_str(piece);
            shown.push_str(mark);
            return Some(at + found + 1);
        }
        shown.push_str(&token(piece));
        shown.push_str(mark);
        at += found + 1;
        // `user:password@host`, with no scheme in front to say it is a URL.
        if mark == ":" {
            let value = after
                .find(MARKS)
                .map_or(after, |end| after.get(..end).unwrap_or_default());
            if let Some(sign) = value.rfind('@') {
                shown.push_str(HIDDEN);
                at += sign;
            }
        }
    }
    let tail = text.get(at..).unwrap_or_default();
    let bare = unquoted(tail);
    if last && ((bare.starts_with('-') && secret(bare)) || scheme(bare)) {
        shown.push_str(tail);
        return Some(text.len());
    }
    shown.push_str(&token(tail));
    None
}

/// A piece of a word with what is shaped like a token in it hidden, looked at
/// without the quotes and brackets a shell or JSON writes around it.
fn token(piece: &str) -> String {
    let core = piece.trim_matches(['"', '\'', '{', '}', '[', ']']);
    match piece.split_once(core) {
        Some((before, after)) if opaque(core) => format!("{before}{HIDDEN}{after}"),
        _ => piece.to_owned(),
    }
}

/// Where the first URL in `text` starts and ends: from its scheme, wherever in
/// the text that begins, to the first character a URL is not written with
/// unescaped, or a `;` or `,` that starts the next pair.
///
/// Those ends are looked for only after the user part, so a password holding
/// one is hidden whole rather than cut where it falls. The user part runs to
/// the last `@` before the authority ends: at a `/`, `?`, `#`, or the quote
/// that closes the string the URL opens, as `"url":"https://...` opens one.
/// Where that runs on into a later `@`, more is hidden than the user.
///
/// `None` where `text` holds no URL.
fn url(text: &str) -> Option<(usize, usize)> {
    const ENDS: [char; 12] = ['"', '\'', '`', '<', '>', '{', '}', '|', '\\', '^', ';', ','];
    let mut from = 0;
    loop {
        let found = from + text.get(from..)?.find("://")?;
        let head = text.get(..found).unwrap_or_default();
        let run = head
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
            .last()
            .map_or(found, |(at, _)| at);
        let letter = head
            .get(run..)
            .and_then(|scheme| scheme.find(|c: char| c.is_ascii_alphabetic()));
        let after = found + "://".len();
        if let Some(letter) = letter {
            let start = run + letter;
            let opened = text
                .get(..start)
                .and_then(|head| head.chars().next_back())
                .filter(|c| matches!(c, '"' | '\''));
            let rest = text.get(after..).unwrap_or_default();
            let user = rest
                .get(..authority(rest, opened))
                .and_then(|authority| authority.rfind('@'))
                .map_or(0, |sign| sign + 1);
            let end = rest
                .get(user..)
                .and_then(|host| host.find(ENDS))
                .map_or(text.len(), |end| after + user + end);
            return Some((start, end));
        }
        from = after;
    }
}

/// How far into what follows a URL's `://` its authority runs: to the first
/// `/`, `?`, `#` or whitespace, or to `opened`, the quote the URL's string was
/// opened with, where it is not escaped.
///
/// A quote next to another quote, or before a backslash, closes nothing: that
/// is a shell writing a quote into the string, as `'"'"'` and `'\''` do, and
/// JSON never writes a closing quote beside either.
fn authority(rest: &str, opened: Option<char>) -> usize {
    let quote = |c: &char| matches!(c, '"' | '\'');
    let mut escaped = false;
    let mut before = None;
    let mut chars = rest.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let closes = !escaped
            && Some(c) == opened
            && !before.as_ref().is_some_and(quote)
            && !chars
                .peek()
                .is_some_and(|(_, next)| quote(next) || *next == '\\');
        if matches!(c, '/' | '?' | '#') || c.is_whitespace() || closes {
            return at;
        }
        escaped = c == '\\' && !escaped;
        before = Some(c);
    }
    rest.len()
}

/// A URL as a reader is shown it: scheme, host and path, with the user, the
/// query and the fragment hidden, and any path segment shaped like a token.
///
/// A user part is the one before the last `@` of the host; one holding a `/`
/// runs past where the host would end, and is told by what comes before that
/// `/` being no host and port.
fn located(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let (rest, fragment) = rest
        .split_once('#')
        .map_or((rest, None), |(rest, fragment)| (rest, Some(fragment)));
    let (rest, query) = rest
        .split_once('?')
        .map_or((rest, None), |(rest, query)| (rest, Some(query)));
    let authority = rest
        .split_once('/')
        .map_or(rest, |(authority, _)| authority);
    let hosted = hosted(authority);
    let user = authority
        .rfind('@')
        .or_else(|| if hosted { None } else { rest.rfind('@') });

    let mut shown = format!("{scheme}://");
    let after = match user {
        Some(sign) => {
            shown.push_str(HIDDEN);
            shown.push('@');
            rest.get(sign + 1..).unwrap_or_default()
        }
        None if hosted => rest,
        None => {
            shown.push_str(HIDDEN);
            rest.get(authority.len()..).unwrap_or_default()
        }
    };
    let (host, path) = after.find('/').map_or((after, ""), |at| after.split_at(at));
    shown.push_str(host);
    let segments: Vec<&str> = path
        .split('/')
        .map(|segment| if opaque(segment) { HIDDEN } else { segment })
        .collect();
    shown.push_str(&segments.join("/"));
    for (mark, part) in [('?', query), ('#', fragment)] {
        if part.is_some() {
            shown.push(mark);
            shown.push_str(HIDDEN);
        }
    }
    shown
}

/// Whether a URL's authority is a host, with a port where it has one: what
/// follows its last `:` is digits, or it is a bracketed address.
fn hosted(authority: &str) -> bool {
    authority.ends_with(']')
        || authority
            .rsplit_once(':')
            .is_none_or(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
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

/// Whether a word is shaped like a token: letters and digits run together,
/// long enough that nobody typed it as a word.
///
/// A path, a package name and a version are not: each holds a character a
/// token is not written with, or no run of letters and digits long enough.
fn opaque(word: &str) -> bool {
    const SEPARATORS: [char; 6] = ['-', '_', '.', '~', '+', '='];
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
