//! Trying the user's own program before an instance runs it: what a person
//! is told of it before they say yes. The yes writes the instance, and
//! nothing is started.
//!
//! A trial is found as a launch would find the program: for a catalog agent,
//! its entry's guessed name or the instance's own `executable` on the `PATH`
//! steps look on, outside crucible's install store; for Antigravity, one of
//! its commands there. Its facts name the path its links lead to, what it is
//! started with and what the sandbox lets it read, every value escaped as
//! any other text a program gives.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crucible_config::{Driver, ExternalAgent};
use crucible_external_harnesses::antigravity;
use crucible_external_harnesses::{Dialect as _, Discovery, Program, Shape};
use crucible_types::AgentKey;
use crucible_types::shown::escaped;

use super::{Places, Platform, Proposed, Root, dialect, homeless};
use crate::backends::{AgentRow, Row};
use crate::launching::discovery::{Origin, discover};
use crate::launching::programs_beside;

/// What a trial of the user's own program is found with: the user's home
/// its directory is judged against, where one is known, crucible's trust
/// state and session logs, and the `PATH` it is looked for on.
pub(crate) struct Trying {
    /// The user's home, where one is known and is a full path.
    home: Option<PathBuf>,
    /// The directory the catalog's trust state is kept under, in crucible's
    /// home.
    state: PathBuf,
    /// The directory the session logs are kept under.
    sessions: PathBuf,
    /// The `PATH` a program is looked for on.
    path: Option<OsString>,
    /// Whether the sandbox confines a launch started now.
    sandboxed: bool,
}

impl Trying {
    /// Trying with each of what it is found with; a home that is no full path
    /// is taken as none.
    pub(crate) fn new(
        home: Option<PathBuf>,
        (state, sessions): (PathBuf, PathBuf),
        path: Option<OsString>,
        sandboxed: bool,
    ) -> Self {
        Self {
            home: home.filter(|home| home.is_absolute()),
            state,
            sessions,
            path,
            sandboxed,
        }
    }

    /// The `PATH` a program is looked for on, as an environment lookup.
    fn lookup(&self) -> impl Fn(&str) -> Option<OsString> + '_ {
        |name| (name == "PATH").then(|| self.path.clone()).flatten()
    }

    /// Crucible's home, the directory the trust state lies in.
    fn crucible(&self) -> &Path {
        self.state.parent().unwrap_or(&self.state)
    }

    /// The places a directory is judged against, where the user's home is
    /// known.
    fn places(&self) -> Option<Places<'_>> {
        Some(Places {
            home: self.home.as_deref()?,
            crucible: self.crucible(),
            sessions: &self.sessions,
        })
    }

    /// Whether the sandbox confines a launch started now.
    pub(crate) fn sandboxed(&self) -> bool {
        self.sandboxed
    }

    /// The user's own program of the catalog agent `key`, run as
    /// `executable` by the instance that names it, or found by its entry's
    /// guessed name where no instance does; nothing where no entry proposes
    /// one, or it lies in the store or is not found.
    pub(crate) fn catalog(&self, key: &AgentKey, executable: Option<&str>) -> Option<Trial> {
        let proposed = Proposed::of(executable, key, &self.state, self.lookup())?.ok()?;
        let program = proposed.program.clone();
        Trial::found(
            self,
            Of::Catalog {
                key: key.clone(),
                proposed: Box::new(proposed),
            },
            &program,
        )
    }

    /// The path the program an instance runs as `executable` leads to, found
    /// as a launch finds it; nothing where it is not found.
    pub(crate) fn leads(&self, executable: &str) -> Option<PathBuf> {
        let Discovery::Found(program) =
            discover(executable, &[executable.to_owned()], self.lookup())
        else {
            return None;
        };
        std::fs::canonicalize(program.path()).ok()
    }

    /// The user's own Antigravity, `row`'s program found by one of its
    /// commands outside the install store; nothing where it is not.
    pub(crate) fn antigravity(&self, row: &'static Row) -> Option<Trial> {
        if row.driver != Driver::Antigravity {
            return None;
        }
        let built = AgentRow::Built(row);
        let Discovery::Found(program) =
            discover(&built.command(), &built.commands(), self.lookup())
        else {
            return None;
        };
        (Origin::of(&programs_beside(&self.state), program.path()) == Origin::Own)
            .then(|| Trial::found(self, Of::Antigravity(row), &program))
            .flatten()
    }
}

/// Whose own program a trial is of.
enum Of {
    /// A catalog agent's, as its entry proposes it.
    Catalog {
        /// The agent.
        key: AgentKey,
        /// What its entry proposes.
        proposed: Box<Proposed>,
    },
    /// Antigravity's, whose row this is.
    Antigravity(&'static Row),
}

/// The user's own program of an agent, found where a launch would find it,
/// and what the sandbox is told of the directory it lies in.
pub(crate) struct Trial {
    /// Whose it is.
    of: Of,
    /// The path its links lead to.
    canonical: PathBuf,
    /// What the sandbox is told of the directory it lies in.
    root: Root,
    /// Whether the sandbox confines a launch started now, so that what it
    /// is told of the directory is judged at all.
    sandboxed: bool,
}

impl Trial {
    /// `of`'s program at `program`, judged against `trying`'s places; nothing
    /// where its links cannot be followed. A launch with the sandbox off
    /// judges nothing of the directory, so neither does a trial then.
    fn found(trying: &Trying, of: Of, program: &Program) -> Option<Self> {
        let canonical = std::fs::canonicalize(program.path()).ok()?;
        let root = trying.places().map_or_else(
            || Root::Refused(homeless(program).to_string()),
            |places| places.root(&canonical),
        );
        Some(Self {
            of,
            canonical,
            root,
            sandboxed: trying.sandboxed,
        })
    }

    /// The path its links lead to.
    pub(crate) fn canonical(&self) -> &Path {
        &self.canonical
    }

    /// Why the directory it lies in is given to no agent, where it is not
    /// and the sandbox confines a launch started now.
    pub(crate) fn refused(&self) -> Option<&str> {
        match &self.root {
            Root::Refused(why) if self.sandboxed => Some(why),
            Root::Refused(_) | Root::NoRule | Root::Grant(_) => None,
        }
    }

    /// An instance named `name` that runs it by the path its links lead to,
    /// as the user file would state it once written, with no profile
    /// directory: the program keeps its sign-in where it always does.
    pub(crate) fn agent(&self, name: &str) -> Option<ExternalAgent> {
        let executable = self.canonical.to_str()?;
        match &self.of {
            Of::Catalog { key, .. } => {
                ExternalAgent::unwritten(name, Driver::Catalog, executable, None, Some(key))
            }
            Of::Antigravity(_) => {
                ExternalAgent::unwritten(name, Driver::Antigravity, executable, None, None)
            }
        }
    }

    /// The fact that names the path its links lead to.
    pub(crate) fn program(&self) -> String {
        format!("Program: {}.", shown(&self.canonical))
    }

    /// The path its links lead to, as a person is shown it.
    pub(crate) fn shown(&self) -> String {
        shown(&self.canonical)
    }

    /// What a person is told of it before trying it: the path its links lead
    /// to, the catalog key it is run for, the arguments and environment it
    /// is started with, what it can read, the home it starts with under the
    /// sandbox on Linux, and for Antigravity whose sign-in it uses.
    pub(crate) fn facts(&self, trying: &Trying) -> Vec<String> {
        let mut facts = vec![self.program()];
        let (arguments, environment): (Vec<String>, Vec<String>) = match &self.of {
            Of::Catalog { key, proposed } => {
                facts.push(format!("Key: {}.", escaped(key.as_str())));
                (
                    proposed.own.args().iter().map(|arg| escaped(arg)).collect(),
                    proposed
                        .own
                        .env()
                        .iter()
                        .map(|variable| {
                            format!("{}={}", escaped(&variable.name), escaped(&variable.value))
                        })
                        .collect(),
                )
            }
            Of::Antigravity(row) => (
                dialect(&AgentRow::Built(row), &self.canonical)
                    .map(|mut wire| wire.request(Shape::default()).arguments)
                    .unwrap_or_default()
                    .iter()
                    .map(|arg| escaped(arg))
                    .collect(),
                antigravity::own_environment()
                    .into_iter()
                    .map(|(name, value)| format!("{name}={}", escaped(&value)))
                    .collect(),
            ),
        };
        facts.push(listing("Arguments", &arguments, " "));
        facts.push(listing("Environment", &environment, ", "));
        facts.push(match &self.root {
            Root::Refused(_) if !self.sandboxed => format!(
                "With the sandbox off it starts as a plain process that reads what you can, so \
                 nothing is judged of {}.",
                shown(self.canonical.parent().unwrap_or(&self.canonical))
            ),
            Root::Refused(why) => why.clone(),
            Root::NoRule => format!(
                "{} is already readable inside the sandbox, so nothing is added to what it can \
                 read.",
                shown(self.canonical.parent().unwrap_or(&self.canonical))
            ),
            Root::Grant(directory) => format!(
                "Everything in {} is readable to it, read-only, at every launch under the \
                 sandbox, where the network is open to its vendor.",
                shown(directory)
            ),
        });
        if Platform::HERE == Platform::Linux {
            facts.push(
                "Under the sandbox on Linux it starts with a private home of its own, and a \
                 sign-in made there is gone when that start ends."
                    .to_owned(),
            );
        }
        if let Of::Antigravity(_) = self.of {
            facts.push(antigravity::own_note(trying.sandboxed).to_owned());
        }
        facts
    }
}

/// `path`, as text a program gave is shown.
fn shown(path: &Path) -> String {
    escaped(&path.display().to_string())
}

/// What a fact says of the `items` it is `named`, joined by `between`, or
/// that there are none.
fn listing(named: &str, items: &[String], between: &str) -> String {
    if items.is_empty() {
        format!("{named}: none.")
    } else {
        format!("{named}: {}.", items.join(between))
    }
}
