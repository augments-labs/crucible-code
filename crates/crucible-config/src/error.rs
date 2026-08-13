//! Why a configuration document was refused, in words that say what to change.
//!
//! A configuration file is edited by hand, so an error here is read by someone
//! with the file open in front of them. "invalid configuration" sends them
//! looking; the file, the position and the key send them to the character.
//!
//! Positions are exact where JSON itself reports them, and located by search
//! otherwise — see [`At`]. A position that might be the wrong one is worse than
//! none, so an ambiguous key is reported without one.

use std::fmt;

/// Where in a file something was found.
///
/// JSON reports a line and column for a document it could not parse. For a key
/// it *could* parse and crucible could not accept, there is no position in the
/// parsed value at all, so one is found by searching the source for the key
/// token. That search is only trusted when it finds exactly one occurrence:
/// pointing at the wrong `"model"` in a file with two providers would send the
/// reader to a line that is perfectly correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At {
    /// Line and column, both counted from one.
    Known {
        /// Line, counted from one.
        line: usize,
        /// Column, counted from one.
        column: usize,
    },
    /// The key occurs more than once, so no position can be given for it.
    Ambiguous,
}

impl At {
    /// Finds `key` in `text`, if it appears there exactly once.
    ///
    /// The needle includes the quotes, so `env` does not match the middle of
    /// `"environment"`.
    pub(crate) fn of(key: &str, text: &str) -> Self {
        let needle = format!("\"{key}\"");
        let mut found = text.match_indices(&needle);

        let Some((offset, _)) = found.next() else {
            return Self::Ambiguous;
        };
        if found.next().is_some() {
            return Self::Ambiguous;
        }

        let before = &text[..offset];
        let line = before.matches('\n').count() + 1;
        let column = before
            .rfind('\n')
            .map_or(offset, |newline| offset - newline - 1)
            + 1;
        Self::Known { line, column }
    }
}

impl fmt::Display for At {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known { line, column } => write!(f, " at line {line}, column {column}"),
            Self::Ambiguous => Ok(()),
        }
    }
}

/// What was accepted where something else was found — the keys a block has, or
/// the answers a setting takes.
///
/// Rendered as a list rather than as a guess at what was meant. A document this
/// small has few enough keys at any level that showing all of them is both
/// shorter to compute and more use than the nearest match — the reader learns
/// what is there instead of being handed one candidate.
#[derive(Debug, Clone)]
pub struct Accepted(Box<[&'static str]>);

impl Accepted {
    pub(crate) fn new(accepted: Vec<&'static str>) -> Self {
        Self(accepted.into_boxed_slice())
    }
}

impl fmt::Display for Accepted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return write!(f, "nothing is accepted here");
        }
        write!(f, "accepted here: {}", self.0.join(", "))
    }
}

/// Why a configuration document was refused.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("{file} could not be read: {source}")]
    Unreadable {
        /// The file, as the user would name it.
        file: Box<str>,
        /// What the operating system reported.
        source: std::io::Error,
    },

    /// The file is not JSON.
    #[error("{file} is not valid JSON at line {line}, column {column}: {problem}")]
    Malformed {
        /// The file, as the user would name it.
        file: Box<str>,
        /// Line, counted from one.
        line: usize,
        /// Column, counted from one.
        column: usize,
        /// What the parser said, without the position it repeats.
        problem: Box<str>,
    },

    /// A key crucible does not understand.
    #[error("{file}: {path} is not a setting crucible has{at} — {accepted}")]
    UnknownKey {
        /// The file, as the user would name it.
        file: Box<str>,
        /// The dotted path to the key.
        path: Box<str>,
        /// Where it is, when that can be said.
        at: At,
        /// What is accepted in its place.
        accepted: Accepted,
    },

    /// A key whose value is the wrong kind of thing.
    #[error("{file}: {path} wants {wanted}{at}")]
    WrongType {
        /// The file, as the user would name it.
        file: Box<str>,
        /// The dotted path to the key.
        path: Box<str>,
        /// What that key accepts.
        wanted: &'static str,
        /// Where it is, when that can be said.
        at: At,
    },

    /// A string that is not one of the values that key accepts.
    #[error("{file}: {path} does not accept {found}{at} — {accepted}")]
    NotAChoice {
        /// The file, as the user would name it.
        file: Box<str>,
        /// The dotted path to the key.
        path: Box<str>,
        /// What was written there.
        found: Box<str>,
        /// Where it is, when that can be said.
        at: At,
        /// What is accepted in its place.
        accepted: Accepted,
    },

    /// Text in one of the rule lists that is not a rule.
    ///
    /// The position is the key holding the list rather than the entry, since an
    /// entry has no key of its own to find in the source. The index says which
    /// one.
    #[error("{file}: {path}{at} — {problem}")]
    BadRule {
        /// The file, as the user would name it.
        file: Box<str>,
        /// The dotted path to the entry, with the index it sits at.
        path: Box<str>,
        /// Where the list is, when that can be said.
        at: At,
        /// What the rule reader said about the text.
        problem: Box<str>,
    },

    /// A directory the workspace was asked to reach, named by a relative path.
    ///
    /// Refused rather than resolved against the working directory: a file says
    /// nothing about which directory crucible will be started in, so the same
    /// entry would name a different place per invocation.
    #[error(
        "{file}: {path} must be an absolute path{at} — {found} is relative, and \
         a configuration file cannot know what it would be relative to"
    )]
    Relative {
        /// The file, as the user would name it.
        file: Box<str>,
        /// The dotted path to the entry, with the index it sits at.
        path: Box<str>,
        /// What was written there.
        found: Box<str>,
        /// Where the list is, when that can be said.
        at: At,
    },

    /// An `env` variable that is not crucible's own, in a file under the
    /// working directory.
    ///
    /// Its own variant rather than an unknown key, because the name is not a
    /// typo — `env` is a block crucible has and this is a name it will take in
    /// the file in the user's home directory. A reader told "no such setting"
    /// would go looking for a misspelling that is not there.
    ///
    /// The message names the working directory rather than either file, because
    /// both of them are refused and telling somebody to move the line to the
    /// other one would send them in a circle.
    #[error(
        "{file}: env cannot set {name}{at} — crucible cannot tell a file you \
         wrote from one that arrived with the checkout, so no file under the \
         working directory sets a variable for the commands crucible runs — \
         PATH alone decides which program each of those commands is. Only \
         crucible's own settings, which start with {namespace}, are read from \
         one. Put this in the configuration file in your home directory, or set \
         it in the shell you start crucible in"
    )]
    ProjectEnv {
        /// The file, as the user would name it.
        file: Box<str>,
        /// The variable name. A name, never the value beside it.
        name: Box<str>,
        /// Where it is, when that can be said.
        at: At,
        /// crucible's prefix, carried so the message cannot drift from it.
        namespace: &'static str,
    },

    /// A permission key that only ever loosens what crucible does unasked,
    /// written in the layer that travels with a clone.
    ///
    /// Its own variant for the same reason as the one above: the key is real
    /// and is accepted in the other two files, so "no such setting" would send
    /// the reader hunting a typo. What is wrong is where it was written.
    ///
    /// The message says which keys a checked-in file *may* state, because the
    /// reader was configuring a repository for a team and still has that to do.
    #[error(
        "{file}: {path} cannot be set here{at} — this file is checked in, so \
         what it says reaches everyone who clones this repository, and this key \
         only ever widens what crucible does without asking. A checked-in file \
         may tighten its own rules — permissions.ask and permissions.deny — and \
         may not loosen anybody's. Put this one in .crucible/config.local.json, \
         which git ignores, or in the configuration file in your home directory"
    )]
    Widening {
        /// The file, as the user would name it.
        file: Box<str>,
        /// The dotted path to the key.
        path: Box<str>,
        /// Where it is, when that can be said.
        at: At,
    },

    /// An `env` variable that crucible has already read by the time it opens a
    /// file, so a value written in one could never apply.
    ///
    /// Refused rather than ignored. Every other name in `env` does something,
    /// and a reader has no way to tell this one apart — accepting it would mean
    /// a setting that looks applied, merges like the others, and does nothing.
    #[error(
        "{file}: env cannot set {name}{at} — crucible reads it before it opens \
         any configuration file, because it is what says where the files are. \
         Set it in your shell instead"
    )]
    TooLate {
        /// The file, as the user would name it.
        file: Box<str>,
        /// The variable name.
        name: Box<str>,
        /// Where it is, when that can be said.
        at: At,
    },

    /// One of crucible's own settings, written in a file as something crucible
    /// does not take.
    ///
    /// Refused rather than read as the default. Every other name in `env` is a
    /// string on its way to a command and means whatever it says; this one has
    /// a meaning crucible fixes, and quietly falling back would be a setting
    /// that looks applied and does nothing.
    #[error("{file}: env {name}{at} is not set to an answer crucible takes — {accepted}")]
    Answer {
        /// The file, as the user would name it.
        file: Box<str>,
        /// The variable name. A name, never the value beside it.
        name: Box<str>,
        /// Where it is, when that can be said.
        at: At,
        /// What the setting takes instead.
        accepted: Accepted,
    },

    /// The same setting, set in the shell rather than in a file.
    ///
    /// No file and no position, because there is neither: this one was typed in
    /// front of the run. The name is enough to find it, and the value is left
    /// out for the reason it is left out of every message here.
    #[error("{name} is not set to an answer crucible takes — {accepted}")]
    AnswerInShell {
        /// The variable name.
        name: Box<str>,
        /// What the setting takes instead.
        accepted: Accepted,
    },

    /// A file an answer could not be written into without rewriting the rest of
    /// it.
    ///
    /// The file is somebody's to edit, so the answer to not understanding its
    /// shape is to say what to type rather than to replace it with something
    /// crucible does understand.
    #[error(
        "{file}: crucible could not change this file without rewriting what is \
         already in it. Put {written} at {at} by hand"
    )]
    Unspliceable {
        /// The file, as the user would name it.
        file: Box<str>,
        /// Where in the document the answer belongs, dotted.
        at: Box<str>,
        /// The answer, written the way JSON would hold it, so it can be pasted.
        written: Box<str>,
    },

    /// The environment says nowhere to keep crucible's own files.
    ///
    /// Not a fault in a document — it is why there is no document to read — but
    /// the same read, and one error type is what keeps the wiring above from
    /// having to hold two.
    #[error(
        "crucible has nowhere to keep its files: set HOME, or set {named} to the \
         absolute path of the directory you want it to use"
    )]
    Homeless {
        /// crucible's own variable, carried so the message cannot drift from
        /// the name that is actually read.
        named: &'static str,
    },
}
