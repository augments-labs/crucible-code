//! A program the user installed themselves, run as the catalog agent an
//! `agent:` key names: what the sandbox is told of the directory it lies in.
//!
//! An own program is used as found. Nothing is asked of it before a launch,
//! and nothing about it is kept beyond the instance that names it: trying it
//! writes that instance and starts nothing. A program that changed since it
//! was tried is started as it is now, and one that does not speak the wire
//! fails its launch with its cause.
//! A launch follows the conversation's sandbox setting. With the sandbox off
//! it is started as a plain local process, and with it on it is confined
//! under the conversation's policy, with the directory it lies in decided
//! here.
//!
//! That directory is refused when the user's home it is judged against is no
//! full path or leads through a link that cannot be followed, when it is or
//! contains that home, as written or where its links lead, or the top of the
//! file system, or when it is, lies within or contains a place no agent is
//! made able to write; the one exception is a home search directory such as
//! `~/.local/bin`, which may itself be read though it is listed as one, where
//! no other such place lies there. A directory the sandbox already lets every
//! program read gets no rule of its own, and any other is given read-only,
//! never writable. Everything in that directory is readable to the agent
//! while it runs. Every launch starts the program at the path its links lead
//! to, the one that directory is decided from, held to the file that was
//! found, so a link pointed elsewhere since it was found is refused and
//! nothing is started.
//!
//! The name and version a start answered, or that it did not say, are shown
//! where `told` reads the user's own program signed out, escaped as any
//! other text a program gives.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crucible_config::ExternalAgent;
use crucible_external_harnesses::acp::Initialized;
use crucible_external_harnesses::catalog::AuthMethod;
use crucible_external_harnesses::catalog::registry::platform;
use crucible_external_harnesses::catalog::{Entry, OwnEntry, Trust};
use crucible_external_harnesses::store::Source as ReleaseSource;
use crucible_external_harnesses::{Discovery, Program, Refusal};
use crucible_sandbox::{SandboxPolicy, SandboxPolicyError};
use crucible_types::AgentKey;
use serde_json::{Map, Value, json};

use crucible_tools::Mode;

use crate::backends::{AgentRow, CatalogRow, Sandboxing, Started, Unbelieved, Wire, kept_word};
use crate::launching::discovery::{Origin, discover};
use crate::launching::{
    Guarded, Overlap, Reach, beneath, guarded, is_beneath, overlap, read_only, resolved,
};

mod trying;

pub(crate) use trying::{Trial, Trying};

/// The home search directories an own program may lie in, though each is a
/// place no agent is made able to write: reading one is what running from it
/// takes, and it is never given writable.
pub const HOME_SEARCH: &[&str] = &[
    "bin",
    ".local/bin",
    ".cargo/bin",
    "AppData/Local/Microsoft/WindowsApps",
];

/// What the sandbox is told of the directory an own program lies in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Root {
    /// It is not given, for the reason the sentence says, and the program is
    /// not started.
    Refused(String),
    /// The sandbox already lets every program read it, so no rule is added.
    NoRule,
    /// It is given read-only.
    Grant(PathBuf),
}

/// The platform whose sandbox has or has not been checked against a
/// directory programs are commonly installed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Platform {
    /// macOS, whose sandbox has not been checked against Homebrew's links.
    MacOs,
    /// Windows, whose sandbox has not been checked against the Microsoft
    /// Store's aliases.
    Windows,
    /// Linux, whose sandbox gives a confined program a private home of its
    /// own.
    Linux,
    /// Any other.
    Other,
}

impl Platform {
    /// The platform this build runs on.
    pub(crate) const HERE: Self = if cfg!(target_os = "macos") {
        Self::MacOs
    } else if cfg!(windows) {
        Self::Windows
    } else if cfg!(target_os = "linux") {
        Self::Linux
    } else {
        Self::Other
    };

    /// Why `root` is not given on this platform yet, where it is not.
    fn unrecorded(self, root: &Path) -> Option<String> {
        let shown = root.display();
        match self {
            Self::MacOs if is_beneath(root, Path::new("/opt/homebrew/bin")) => Some(format!(
                "{shown} is not given to an agent on macOS yet: whether the sandbox there can \
                 read the programs Homebrew links into it has not been checked."
            )),
            Self::Windows
                if root
                    .components()
                    .any(|part| part.as_os_str().eq_ignore_ascii_case("WindowsApps")) =>
            {
                Some(format!(
                    "{shown} is not given to an agent on Windows yet: whether the sandbox there \
                     can read and start the programs the Microsoft Store links into it has not \
                     been checked."
                ))
            }
            Self::MacOs | Self::Windows | Self::Linux | Self::Other => None,
        }
    }
}

/// What a person reading `row`'s agent signed out is told of signing it in,
/// where it runs the user's own program, as it is started where `started`
/// says on `platform`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Told {
    /// The line saying how to sign it in, in place of the one a release is
    /// told, where it differs.
    pub(crate) how: Option<String>,
    /// What is said after that line.
    pub(crate) more: Vec<String>,
}

/// What [`Told`] says of `row`'s agent started as `started` on `platform`.
///
/// The user's own Antigravity started [`homeless`](Started::homeless) is
/// told, in place of signing in at the agent, the two ways it can be signed
/// in, and on Linux alone, whose sandbox gives it a private home of its own,
/// that a sign-in made there is gone when that start ends. A catalog agent's
/// own program is told what reaches it of a sign-in: under the sandbox only
/// the places its entry names, which a registry entry names none of; with the
/// sandbox off, no variable the user exported, and on Linux no desktop
/// keyring and no relocated configuration or data directory, since it is
/// given neither the session bus nor those variables. Each is told first
/// the name and version its program `answered` with at the start that read
/// it, where that answer was heard. A release is told nothing more.
pub(crate) fn told(
    row: &AgentRow,
    started: Started,
    platform: Platform,
    answered: Option<&Introduced>,
) -> Told {
    if started.origin != Origin::Own || !own_row(row) {
        return Told::default();
    }
    let mut told = signing_in(row, started, platform);
    if let Some(answered) = answered {
        told.more.insert(0, answered.said());
    }
    told
}

/// Whether `row`'s agent run as the user's own program is held to what is
/// decided here: a catalog agent's own program and Antigravity's.
fn own_row(row: &AgentRow) -> bool {
    match row {
        AgentRow::Catalog(catalog) => catalog.is_own(),
        AgentRow::Built(built) => built.driver == crucible_config::Driver::Antigravity,
    }
}

/// What [`told`] says of signing `row`'s own program in.
fn signing_in(row: &AgentRow, started: Started, platform: Platform) -> Told {
    match row {
        AgentRow::Built(built) if built.driver == crucible_config::Driver::Antigravity => {
            if !started.homeless() {
                return Told::default();
            }
            let more = match platform {
                Platform::Linux => vec![
                    "A sign-in made under the sandbox lives on the sandbox's private home and is \
                     gone when that start ends."
                        .to_owned(),
                ],
                Platform::MacOs | Platform::Windows | Platform::Other => Vec::new(),
            };
            Told {
                how: Some(format!(
                    "Sign in: under the sandbox your own Antigravity cannot reach the sign-in it \
                     keeps in ~/.gemini. {}",
                    crucible_external_harnesses::antigravity::own_ways(started.sandbox.required)
                )),
                more,
            }
        }
        AgentRow::Built(_) => Told::default(),
        AgentRow::Catalog(catalog) if !catalog.is_own() => Told::default(),
        AgentRow::Catalog(catalog) if started.sandbox.on => Told {
            how: None,
            more: vec![match catalog.source() {
                ReleaseSource::Registry => "Under the sandbox your own program starts signed \
                                            out: its registry entry names no place it keeps a \
                                            sign-in in, so none is given to it."
                    .to_owned(),
                ReleaseSource::Catalog => "Under the sandbox your own program keeps a sign-in \
                                           only in the places its catalog entry names."
                    .to_owned(),
            }],
        },
        AgentRow::Catalog(_) => {
            let mut more = vec![
                "Your own program is given only the variables Crucible passes on and its \
                 entry's own, so a sign-in kept in a variable you exported is not seen."
                    .to_owned(),
            ];
            if platform == Platform::Linux {
                more.push(
                    "Nor is one kept in a desktop keyring or under a relocated XDG_CONFIG_HOME \
                     or XDG_DATA_HOME."
                        .to_owned(),
                );
            }
            Told { how: None, more }
        }
    }
}

/// What the sandbox is told of the directory `program`, a canonical path,
/// lies in, for every launch of it: refused where `home` is no
/// full path or leads through a link that cannot be followed, or where that
/// directory is `home`, as written or where its links lead, contains it, is
/// `/`, or is, lies within or contains a place no agent is made able to write,
/// crucible's own home `crucible` and its session logs `sessions` among them,
/// but for a home search directory against its own listing; no rule where the
/// sandbox already reads it; else given read-only.
#[must_use]
pub fn root(program: &Path, home: &Path, crucible: &Path, sessions: &Path) -> Root {
    decided(
        program,
        home,
        &guarded(home, crucible, sessions),
        Platform::HERE,
        crucible_sandbox_local::runtime_directories(),
    )
}

/// [`root`] as it is decided on `platform`, whose sandbox already reads every
/// directory of `runtime`.
pub(crate) fn decided(
    program: &Path,
    home: &Path,
    guarded: &[Guarded],
    platform: Platform,
    runtime: &[&str],
) -> Root {
    // A relative home matches no canonical root, so every place under it
    // would be judged against nothing.
    if !home.is_absolute() {
        return Root::Refused(format!(
            "Your home, `{}`, is not a full path, so no directory can be judged against it, and \
             the directory of {} is not given to an agent.",
            home.display(),
            program.display()
        ));
    }
    // The program's directory is judged where its links lead, so the home is
    // judged both as written and where its links lead, as each guarded place
    // is; one whose links cannot be followed leads nowhere it could be judged.
    let Ok(real) = resolved(home) else {
        return Root::Refused(format!(
            "Your home, `{}`, leads through a link that cannot be followed, so no directory can \
             be judged against it, and the directory of {} is not given to an agent.",
            home.display(),
            program.display()
        ));
    };
    let homes = [home, real.as_path()];
    let Some(root) = program.parent().filter(|root| root.parent().is_some()) else {
        return Root::Refused(format!(
            "{} lies at the top of the file system, which holds every file on it, so its \
             directory is not given to an agent.",
            program.display()
        ));
    };
    let shown = root.display();
    if let Some(found) = homes.iter().find_map(|home| {
        overlap(root, home, Reach::Itself)
            .filter(|found| matches!(found, Overlap::Is | Overlap::Contains))
    }) {
        return Root::Refused(format!(
            "{shown} {} your home, which holds every file you keep, so it is not given to an \
             agent.",
            found.says()
        ));
    }
    if let Some(why) = platform.unrecorded(root) {
        return Root::Refused(why);
    }
    // A home search directory is excepted from its own listing alone, so
    // another guarded place kept exactly there still refuses it.
    let searched = HOME_SEARCH.iter().copied().find(|dir| {
        homes
            .iter()
            .any(|home| overlap(root, &beneath(home, dir), Reach::Itself) == Some(Overlap::Is))
    });
    for place in guarded {
        let excepted = searched.is_some() && place.listed == searched;
        for at in &place.at {
            match overlap(root, at, place.reach) {
                Some(Overlap::Is) if excepted => {}
                Some(found @ (Overlap::Is | Overlap::Within | Overlap::Contains)) => {
                    return Root::Refused(format!(
                        "{shown} {} {} ({}), so it is not given to an agent.",
                        found.says(),
                        place.place,
                        place.why
                    ));
                }
                Some(Overlap::Outside) | None => {}
            }
        }
    }
    if runtime.iter().any(|dir| is_beneath(root, Path::new(dir))) {
        return Root::NoRule;
    }
    Root::Grant(root.to_path_buf())
}

/// What a name or version the program did not give is shown as.
pub const DID_NOT_SAY: &str = "did not say";

/// What the user's own program answered `initialize` with of itself at one
/// start: the name and version it gave, bounded as the wire bounds a name,
/// each [`DID_NOT_SAY`] where it gave none. They are shown, never compared
/// with anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Introduced {
    /// Its name.
    name: Box<str>,
    /// Its version.
    version: Box<str>,
}

impl Introduced {
    /// What `frame`, a message the program sent, says of it, where it answers
    /// `initialize` with a result that admits it.
    pub(crate) fn heard(frame: &str) -> Option<Self> {
        let said = initialize_answer(frame)?;
        let initialized = Initialized::own(said.get("result")?).ok()?;
        Some(Self {
            name: initialized.name().unwrap_or(DID_NOT_SAY).into(),
            version: initialized.version().unwrap_or(DID_NOT_SAY).into(),
        })
    }

    /// The name it gave, or [`DID_NOT_SAY`].
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The version it gave, or [`DID_NOT_SAY`].
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// What a person is told of it, its control characters escaped.
    fn said(&self) -> String {
        format!(
            "Your own program answered as {}, version {}.",
            crate::launching::quoted(&self.name),
            crate::launching::quoted(&self.version)
        )
    }
}

/// `frame` as a message, where it answers `initialize`, the first request a
/// start sends, with its result or an error.
fn initialize_answer(frame: &str) -> Option<Map<String, Value>> {
    let Ok(Value::Object(said)) = serde_json::from_str::<Value>(frame) else {
        return None;
    };
    (said.get("id") == Some(&json!(0)) && !said.contains_key("method")).then_some(said)
}

/// The places [`root`] guards: the user's home, crucible's own home and the
/// directory of its session logs.
#[derive(Debug, Clone, Copy)]
pub struct Places<'a> {
    /// The user's home.
    pub home: &'a Path,
    /// crucible's own home.
    pub crucible: &'a Path,
    /// The directory of the session logs.
    pub sessions: &'a Path,
}

impl Places<'_> {
    /// What the sandbox is told of the directory `program` lies in.
    #[must_use]
    pub fn root(&self, program: &Path) -> Root {
        root(program, self.home, self.crucible, self.sessions)
    }
}

/// Why a program the user installed is not tried or started.
#[derive(Debug, thiserror::Error)]
pub enum Unowned {
    /// The directory it lies in is not given to an agent.
    #[error("{0}")]
    Root(String),
    /// It cannot be followed to where it is.
    #[error("{} cannot be reached", .0.display())]
    Unreachable(PathBuf, #[source] std::io::Error),
    /// Its path is not text, which the instance that runs it names it by.
    #[error("{} is not a path an instance can name", .0.display())]
    Unnamed(PathBuf),
    /// The sandbox policy for its launch is not a valid one.
    #[error("the sandbox policy for its launch could not be built: {0}")]
    Policy(#[from] SandboxPolicyError),
    /// The host refused the start.
    #[error(transparent)]
    Refused(#[from] Refusal),
    /// The instance names no program the user installed for its entry.
    #[error(transparent)]
    Unbelieved(#[from] Unbelieved),
    /// Its program lies in crucible's store, or was not found.
    #[error("{0} is not run as a program you installed")]
    NotOwn(Box<str>),
    /// An instance of the agent is configured that runs another program, and
    /// no second one is written beside it; the instance is named, quoted.
    #[error(
        "the instance {0} configured for it runs another program, and no second one is \
         written beside it"
    )]
    Beside(Box<str>),
}

/// `conversation`'s policy as a launch of a program whose directory is `root`
/// is started under: a granted directory read-only, as a store release's is,
/// and nothing added where it gets no rule.
///
/// # Errors
///
/// [`Unowned::Root`] for a refused directory, else [`Unowned::Policy`].
pub(crate) fn launch_policy(
    conversation: &SandboxPolicy,
    root: &Root,
) -> Result<SandboxPolicy, Unowned> {
    match root {
        Root::Refused(why) => Err(Unowned::Root(why.clone())),
        Root::Grant(granted) => Ok(read_only(conversation, granted)?),
        Root::NoRule => Ok(conversation.clone()),
    }
}

/// Whether [`launched`] decides the directory of `row`'s own program, under
/// `conversation`, against the user's home: only for an own row confined.
pub(crate) fn judged(conversation: &SandboxPolicy, row: &AgentRow) -> bool {
    own_row(row) && conversation.enabled()
}

/// `conversation`'s policy as a launch of the user's own `program` of `row`'s
/// agent is started under, with its directory decided against `places`: a
/// catalog agent's own program and Antigravity's. Every
/// other row's program, and every program where `conversation` has the
/// sandbox off, is started under `conversation` unchanged: the decision says
/// what a confined program may read, and an unconfined one reads it all.
///
/// # Errors
///
/// [`Unowned::Unreachable`] where the program cannot be followed, else as
/// [`launch_policy`].
pub(crate) fn launched(
    conversation: &SandboxPolicy,
    row: &AgentRow,
    program: &Path,
    places: &Places<'_>,
) -> Result<SandboxPolicy, Unowned> {
    if !judged(conversation, row) {
        return Ok(conversation.clone());
    }
    let canonical =
        fs::canonicalize(program).map_err(|why| Unowned::Unreachable(program.to_owned(), why))?;
    launch_policy(conversation, &places.root(&canonical))
}

/// The program a launch of `row`'s agent starts where `program`, as it was
/// found, is the user's own: for a catalog agent's own program and
/// Antigravity's, `program` at the path its links lead to, inside the
/// directory decided from that path; every other row's program as it was
/// found.
///
/// # Errors
///
/// [`Unowned::Unreachable`] where the program cannot be followed, and
/// [`Refusal::Changed`] where the file it leads to now, or the one found, is
/// not the one that was found.
pub(crate) fn started_at(row: &AgentRow, program: &Program) -> Result<Program, Unowned> {
    if !own_row(row) {
        return Ok(program.clone());
    }
    let canonical = fs::canonicalize(program.path())
        .map_err(|why| Unowned::Unreachable(program.path().to_owned(), why))?;
    Ok(followed(program, &canonical)?)
}

/// `program`, as it was found, at `canonical`, the path its links led to when
/// its directory was decided, so the file started is inside that directory.
/// The found program is checked again once it is followed, so a link pointed
/// elsewhere since it was found is refused and never followed to a file it
/// did not lead to.
///
/// # Errors
///
/// [`Refusal::Changed`] where the file at `canonical`, or at the path it was
/// found at, is not the one that was found.
fn followed(program: &Program, canonical: &Path) -> Result<Program, Refusal> {
    let followed = program.followed_to(canonical)?;
    program.unchanged()?;
    Ok(followed)
}

/// The wire the user's own `program` of `row`'s agent is started through, as
/// [`AgentRow::own_dialect`] makes it for a launch in the mode that asks
/// before every action, with no trust state: Antigravity's own client, whose
/// request carries the environment its driver gives it, and every other
/// row's own wire.
///
/// # Errors
///
/// Why the agent's wire cannot be made.
pub fn dialect(row: &AgentRow, program: &Path) -> Wire {
    row.own_dialect(Mode::Ask, None, program, Sandboxing::default())
}

/// Why the user's own `program` is not started where its directory would be
/// judged against a home of the user's and none is known.
pub(crate) fn homeless(program: &Program) -> Unowned {
    Unowned::Root(format!(
        "no home directory of yours is known, so whether the directory {} lies in may be \
         given to an agent cannot be judged; set HOME, or USERPROFILE on Windows, to your \
         home directory",
        program.path().display()
    ))
}

/// What an entry proposes for the program the user installed for a catalog
/// key, and where that program was found.
struct Proposed {
    /// The program, its arguments and its environment, as the entry proposes.
    own: OwnEntry,
    /// The entry it stands for, where one can be read.
    entry: Option<Entry>,
    /// The day the ACP Registry was read, for a program a registry row
    /// proposes.
    read: Option<Box<str>>,
    /// Where it was found.
    program: Program,
}

impl Proposed {
    /// What a catalog instance of `key` run as `executable` is started as,
    /// where that is found, as the entry's guessed name or as written, or,
    /// with no `executable`, as the guessed name alone, outside the install
    /// store beside the trust state in `state`: the ACP Registry's row where
    /// the cached registry lists the key, as belief judges a release, else the
    /// signed catalog's offer. Nothing where no entry proposes a program of
    /// its own, or the program lies in the store or is not found, which
    /// belief then judges as a release.
    fn of(
        executable: Option<&str>,
        key: &AgentKey,
        state: &Path,
        lookup: impl Fn(&str) -> Option<OsString>,
    ) -> Option<Result<Self, Unbelieved>> {
        if kept_word(key.as_str()) {
            return None;
        }
        let home = state.parent().unwrap_or(state);
        let unreadable = |why| Unbelieved::Unreadable {
            key: key.clone(),
            why,
        };
        let registry = crate::catalog_fetch::cached_registry(home).ok();
        let listed = registry
            .as_ref()
            .and_then(|view| Some((view, view.index.entry(key)?)));
        let (own, entry, read) = if let Some((view, listed)) = listed {
            let own = match OwnEntry::registry(listed, &platform()) {
                Ok(own) => own?,
                Err(why) => return Some(Err(unreadable(why))),
            };
            let entry = view
                .index
                .offer(key)
                .ok()
                .and_then(|offer| offer.provisional().ok());
            (own, entry, Some(view.index.read_date().into()))
        } else {
            let offer = Trust::load(state, SystemTime::now())
                .offers(Some(key))
                .into_iter()
                .next()?;
            match OwnEntry::catalog(&offer.entry) {
                Ok(own) => (own?, Some(offer.entry), None),
                Err(why) => return Some(Err(unreadable(why))),
            }
        };
        let guess = own.guess().to_owned();
        let Discovery::Found(program) = discover(
            executable.unwrap_or(&guess),
            std::slice::from_ref(&guess),
            lookup,
        ) else {
            return None;
        };
        let programs = crate::launching::programs_beside(state);
        (Origin::of(&programs, program.path()) == Origin::Own).then_some(Ok(Self {
            own,
            entry,
            read,
            program,
        }))
    }

    /// Its row, signed in by `methods`.
    fn row(&self, key: &AgentKey, methods: Vec<AuthMethod>) -> Result<CatalogRow, Unbelieved> {
        CatalogRow::own(
            key.clone(),
            &self.own,
            self.entry.as_ref(),
            self.read.clone(),
            methods,
        )
        .map_err(|why| Unbelieved::Unprofiled {
            key: key.clone(),
            why,
        })
    }
}

/// The row the catalog instance `agent` of `key` runs as where its program is
/// the user's own, as [`Proposed`] finds it beside the trust state in
/// `state`, and where that program was found, signed in by the methods the
/// entry it stands for gives, where one is read; a registry entry gives none.
/// It is used as found: nothing is asked of it before it starts, and a
/// program that does not speak the wire fails its launch with that cause.
/// Nothing where its program is not the user's own.
///
/// # Errors
///
/// [`Unbelieved::Unreadable`] or [`Unbelieved::Unprofiled`] where its entry
/// cannot be run.
pub(crate) fn believed(
    agent: &ExternalAgent,
    key: &AgentKey,
    state: &Path,
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Option<Result<(CatalogRow, Program), Unbelieved>> {
    let proposed = match Proposed::of(Some(agent.executable()), key, state, lookup)? {
        Ok(proposed) => proposed,
        Err(why) => return Some(Err(why)),
    };
    let methods = proposed
        .entry
        .as_ref()
        .map(|entry| entry.auth_methods.clone())
        .unwrap_or_default();
    Some(
        proposed
            .row(key, methods)
            .map(|row| (row, proposed.program)),
    )
}
