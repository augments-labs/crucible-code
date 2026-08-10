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

/// The keys accepted where an unknown one was found.
///
/// Rendered as a list rather than as a guess at what was meant. A document this
/// small has few enough keys at any level that showing all of them is both
/// shorter to compute and more use than the nearest match — the reader learns
/// what is there instead of being handed one candidate.
#[derive(Debug, Clone)]
pub struct Accepted(Box<[&'static str]>);

impl Accepted {
    pub(crate) fn new(keys: Vec<&'static str>) -> Self {
        Self(keys.into_boxed_slice())
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

    /// An `env` variable that is not crucible's own, in the layer that travels
    /// with a clone.
    ///
    /// Its own variant rather than an unknown key, because the name is not a
    /// typo — `env` is a block crucible has and this is a name it will take
    /// anywhere else. A reader told "no such setting" would go looking for a
    /// misspelling that is not there.
    #[error(
        "{file}: env cannot set {name}{at} — this file is checked in, so a value \
         written here travels to everyone who clones this repository. Only \
         crucible's own settings, which start with {namespace}, are read from a \
         checked-in file. Put this one in .crucible/config.local.json, which git \
         ignores, or in the configuration file in your home directory"
    )]
    SecretLayer {
        /// The file, as the user would name it.
        file: Box<str>,
        /// The variable name. A name, never the value beside it.
        name: Box<str>,
        /// Where it is, when that can be said.
        at: At,
        /// crucible's prefix, carried so the message cannot drift from it.
        namespace: &'static str,
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

    /// A file a rule could not be added to without rewriting the rest of it.
    ///
    /// The file is somebody's to edit, so the answer to not understanding its
    /// shape is to say what to type rather than to replace it with something
    /// crucible does understand.
    #[error(
        "{file}: crucible could not add a rule to this file without rewriting \
         what is already in it. Add {rule} to permissions.allow by hand"
    )]
    Unspliceable {
        /// The file, as the user would name it.
        file: Box<str>,
        /// The rule that was to be written, so it can be pasted.
        rule: Box<str>,
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
