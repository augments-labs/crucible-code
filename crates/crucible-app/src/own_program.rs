//! A program the user installed themselves, run as the catalog agent an
//! `agent:` key names: what the sandbox is told of the directory it lies in.
//!
//! An own program is no release crucible pinned or reviewed, so the directory
//! it lies in is decided once, here, for the probe that admits it and for
//! every launch alike. That directory is refused when the user's home it is
//! judged against is no full path or leads through a link that cannot be
//! followed, when it is or contains that home, as written or where its links
//! lead, or the top of the file system, or when it is, lies within or contains
//! a place no agent is made able to write; the one exception is a home search
//! directory such as `~/.local/bin`, which may itself be read though it is
//! listed as one, where no other such place lies there. A directory the
//! sandbox already lets every program read gets no rule of its own, and any
//! other is given read-only, never writable. Everything in that directory is
//! readable to the agent while it runs.
//!
//! [`probe`] is what admits one: started confined, with the network closed and
//! never without the sandbox, it is sent `initialize` alone, and it is taken
//! for an agent only where it answers with the protocol version this client
//! speaks and its capabilities; its name and version are kept and never
//! compared. Where the sandbox cannot close the network it is not started.
//! What it answered is kept, bounded as the intake bounds a probe, in one
//! [`Record`] per instance under crucible's home, beside no release of the
//! install store, and [`read`] gives it back only to the instance that has
//! the same driver, `agent:` key and declared arguments and environment it
//! had when it was probed. At most [`RECORDS`] are kept, once those of
//! instances that are gone are dropped.
//!
//! A launch holds the program to its record. One whose path, size or
//! modification time changed since its probe, as a program that updates
//! itself does, or whose modification time is before 1970 and so cannot be
//! kept, is probed again in process, confined, before it is started:
//! it is started only where it is still admitted, and its record is replaced,
//! or removed with the launch refused. Every start's answer to `initialize`
//! is admitted as the probe's was, and one that differs from the record
//! replaces it.
//! The name and version a start answered, or that it did not say, are shown
//! where `told` reads the user's own program signed out, escaped as any
//! other text a program gives.

use std::ffi::OsString;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crucible_config::ExternalAgent;
use crucible_execution::Failure;
use crucible_external_harnesses::acp::Initialized;
use crucible_external_harnesses::catalog::registry::platform;
use crucible_external_harnesses::catalog::{AuthMethod, Method, Route, Variable};
use crucible_external_harnesses::catalog::{Entry, OwnEntry, Trust};
use crucible_external_harnesses::store::Source as ReleaseSource;
use crucible_external_harnesses::{
    Account, Control, Dialect, Discovery, Held, Host, Launch, Program, Reading, Refusal, Shape,
};
use crucible_sandbox::{
    SandboxFilesystemProvenance, SandboxPolicy, SandboxPolicyError, SandboxService,
};
use crucible_types::AgentKey;
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use tokio::runtime::Handle;

use crucible_tools::Mode;

use crate::backends::{AgentRow, CatalogRow, Sandboxing, Started, Unbelieved, Wire, kept_word};
use crate::launching::discovery::{Origin, discover};
use crate::launching::{
    Guarded, Overlap, PROBING, Reach, Sandbox, beneath, guarded, is_beneath, overlap, read_only,
    resolved, unclosable,
};
use crate::probing::{Hearing, Kept, Unprobed, policy, recorded};

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
    /// neither probed nor started.
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
/// lies in, for its probe and its launch alike: refused where `home` is no
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

/// The provenance the empty directory a probe runs in is given under, where
/// the program's own directory gets no rule: it is the probe's workspace, as
/// a granted directory is.
pub const EMPTY_PROVENANCE: SandboxFilesystemProvenance = SandboxFilesystemProvenance::Workspace;

/// The most probe records kept, once those of instances that are gone are
/// dropped.
pub const RECORDS: usize = 64;

/// The most bytes of one record read or written.
const RECORD_BYTES: u64 = 64 * 1024;

/// The directory under crucible's home the records are kept in.
const KEPT_IN: &str = "own-programs";

/// What a name or version the program did not give is recorded as.
pub const DID_NOT_SAY: &str = "did not say";

/// What the user's own program answered `initialize` with of itself at one
/// start: the name and version it gave, bounded as the wire bounds a name,
/// each [`DID_NOT_SAY`] where it gave none, as its record keeps them. They
/// are shown, never compared with anything.
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
    /// crucible's own home, which the records are kept under.
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

/// An instance of the configuration, as its probe record is held to it.
#[derive(Debug, Clone, Copy)]
pub struct Instance<'a> {
    /// Its name, exactly as the configuration spells it.
    pub name: &'a str,
    /// Its driver.
    pub driver: &'a str,
    /// The catalog entry its `agent:` names, none for a built-in driver.
    pub agent: Option<&'a str>,
    /// The arguments it is declared to start with.
    pub args: &'a [Box<str>],
    /// The environment it is declared to start with, by name and value.
    pub env: &'a [Variable],
}

impl Instance<'_> {
    /// The digest of the arguments and environment it is declared to start
    /// with, each length-prefixed so no two shapes read alike.
    fn launch(&self) -> String {
        let mut digest = Sha256::new();
        let mut field = |tag: &[u8], text: &str| {
            digest.update(tag);
            digest.update(u64::try_from(text.len()).unwrap_or(u64::MAX).to_le_bytes());
            digest.update(text.as_bytes());
        };
        for arg in self.args {
            field(b"a", arg);
        }
        for variable in self.env {
            field(b"n", &variable.name);
            field(b"v", &variable.value);
        }
        hex(&digest.finalize())
    }

    /// Where its record is kept under crucible's home `crucible`: named by a
    /// digest of its name's exact bytes, so two names that differ only in
    /// case never share one on a file system that ignores case.
    fn kept(&self, crucible: &Path) -> PathBuf {
        kept_for(crucible, self.name)
    }
}

/// Where the record of the instance `name` is kept under `crucible`.
fn kept_for(crucible: &Path, name: &str) -> PathBuf {
    let digest = Sha256::digest(name.as_bytes());
    crucible
        .join(KEPT_IN)
        .join(format!("{}.json", hex(&digest)))
}

/// `bytes` as lowercase hexadecimal.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// What a probe of one instance's own program recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The record, as it is kept.
    kept: Map<String, Value>,
}

impl Record {
    /// The text field `name`, or nothing.
    fn text(&self, name: &str) -> Option<&str> {
        self.kept.get(name).and_then(Value::as_str)
    }

    /// The name the program answered with, or [`DID_NOT_SAY`].
    #[must_use]
    pub fn name(&self) -> &str {
        self.text("name").unwrap_or(DID_NOT_SAY)
    }

    /// The version the program answered with, or [`DID_NOT_SAY`].
    #[must_use]
    pub fn version(&self) -> &str {
        self.text("version").unwrap_or(DID_NOT_SAY)
    }

    /// The canonical path of the program it was taken of.
    #[must_use]
    pub fn program(&self) -> Option<&Path> {
        self.text("program").map(Path::new)
    }

    /// Whether the program at `program`, a canonical path, is still the one
    /// probed: the same path, size and modification time. One that changed,
    /// as a program that updates itself does, is probed again before it is
    /// started, as is one whose modification time is before 1970.
    #[must_use]
    pub fn current(&self, program: &Path) -> bool {
        let Ok(Some(stamp)) = stamp(program) else {
            return false;
        };
        self.program() == Some(program) && self.kept.get("stamp") == Some(&stamp)
    }

    /// The sign-in methods it offered: those it signs in to itself, and those
    /// it names a command for, which crucible never runs for a program it
    /// did not install. A method that takes a key names no variable a probe
    /// can learn, so none is kept.
    #[must_use]
    pub fn methods(&self) -> Vec<AuthMethod> {
        let text = |method: &Value, name: &str| -> Box<str> {
            method
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into()
        };
        self.kept
            .get("authMethods")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|method| {
                let how = match method.get("kind").and_then(Value::as_str) {
                    Some("agent") => Method::Agent,
                    Some("terminal") => Method::Terminal {
                        command: text(method, "command"),
                        args: method
                            .get("args")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .map(Into::into)
                            .collect(),
                        env: method
                            .get("env")
                            .and_then(Value::as_object)
                            .into_iter()
                            .flatten()
                            .map(|(name, value)| Variable {
                                name: name.as_str().into(),
                                value: value.as_str().unwrap_or_default().into(),
                            })
                            .collect(),
                        route: if method.get("tty") == Some(&Value::Bool(true)) {
                            Route::Terminal
                        } else {
                            Route::Piped
                        },
                    },
                    _ => return None,
                };
                Some(AuthMethod {
                    id: text(method, "id"),
                    name: text(method, "name"),
                    description: method
                        .get("description")
                        .and_then(Value::as_str)
                        .map(Into::into),
                    method: how,
                })
            })
            .collect()
    }

    /// The record of `instance`'s program at `program`, a canonical path,
    /// from what it answered, `said`, held in as `sandbox` says.
    ///
    /// # Errors
    ///
    /// [`Unowned`] where the sandbox did not close the network, the program
    /// refused `initialize`, or is not taken for an agent.
    fn answered(
        instance: &Instance<'_>,
        program: &Path,
        sandbox: &Sandbox,
        said: &Map<String, Value>,
    ) -> Result<Self, Unowned> {
        if *sandbox != Sandbox::Closed {
            return Err(Unowned::Unconfined);
        }
        let probed = recorded(sandbox, said).map_err(Unowned::Probe)?;
        let result = said.get("result").cloned().unwrap_or(Value::Null);
        let initialized = Initialized::own(&result)
            .map_err(|failure| Unowned::Unadmitted(failure.reason().as_str().into()))?;
        let mut clip = Kept::default();
        let mut methods = probed.get("authMethods").cloned().unwrap_or(Value::Null);
        let offered = result.get("authMethods").and_then(Value::as_array);
        if let (Value::Array(methods), Some(offered)) = (&mut methods, offered) {
            for (method, said) in methods.iter_mut().zip(offered) {
                let description = clip.text(said.get("description"));
                if let (Value::Object(method), false) = (method, description.is_null()) {
                    method.insert("description".into(), description);
                }
            }
        }
        let path = program
            .to_str()
            .ok_or_else(|| Unowned::Unnamed(program.to_owned()))?;
        let stamp = stamp(program).map_err(|why| Unowned::Unreachable(program.to_owned(), why))?;
        let said_text = |pointer: &str| {
            probed
                .pointer(pointer)
                .and_then(Value::as_str)
                .map_or_else(|| json!(DID_NOT_SAY), |text| json!(text))
        };
        let mut kept = Map::new();
        kept.insert("instance".into(), json!(instance.name));
        kept.insert("driver".into(), json!(instance.driver));
        kept.insert("agent".into(), json!(instance.agent));
        kept.insert("launch".into(), json!(instance.launch()));
        kept.insert("program".into(), json!(path));
        kept.insert("stamp".into(), stamp.unwrap_or(Value::Null));
        kept.insert("sandbox".into(), json!("closed"));
        kept.insert("protocol".into(), json!(initialized.protocol()));
        // It answered with its capabilities, or `Initialized::own` would have
        // refused it; where they are more than the intake keeps, an empty
        // object stands for them and the record says it is incomplete.
        let capabilities = if let Some(Value::Object(capabilities)) = probed.get("capabilities") {
            Value::Object(capabilities.clone())
        } else {
            clip.cut = true;
            json!({})
        };
        kept.insert("capabilities".into(), capabilities);
        kept.insert("authMethods".into(), methods);
        kept.insert("name".into(), said_text("/agentInfo/name"));
        kept.insert("version".into(), said_text("/agentInfo/version"));
        if clip.cut || probed.get("incomplete") == Some(&Value::Bool(true)) {
            kept.insert("incomplete".into(), json!(true));
        }
        Ok(Self { kept })
    }

    /// Whether it was kept for `instance` as it is now, and admits: taken in
    /// a sandbox that closed the network, of a program that answered with the
    /// protocol version this client speaks and its capabilities.
    fn holds_for(&self, instance: &Instance<'_>) -> bool {
        self.text("instance") == Some(instance.name)
            && self.text("driver") == Some(instance.driver)
            && self.kept.get("agent") == Some(&json!(instance.agent))
            && self.text("launch") == Some(instance.launch().as_str())
            && self.admits()
    }

    /// Whether what it holds admits the program it was taken of.
    fn admits(&self) -> bool {
        self.text("sandbox") == Some("closed")
            && self.kept.get("protocol") == Some(&json!(1))
            && self.kept.get("capabilities").is_some_and(Value::is_object)
    }

    /// The record kept at `path`, where one is there and reads as one.
    fn at(path: &Path) -> Option<Self> {
        let mut text = Vec::new();
        fs::File::open(path)
            .ok()?
            .take(RECORD_BYTES.saturating_add(1))
            .read_to_end(&mut text)
            .ok()?;
        if u64::try_from(text.len()).unwrap_or(u64::MAX) > RECORD_BYTES {
            return None;
        }
        match serde_json::from_slice(&text).ok()? {
            Value::Object(kept) => Some(Self { kept }),
            _ => None,
        }
    }
}

/// The modification time and size of the file at `path`, as a record keeps
/// them; none where that time is before 1970, which a record cannot keep, so
/// such a program is never read as the one probed.
fn stamp(path: &Path) -> std::io::Result<Option<Value>> {
    let meta = fs::metadata(path)?;
    let Ok(modified) = meta.modified()?.duration_since(UNIX_EPOCH) else {
        return Ok(None);
    };
    Ok(Some(json!({
        "size": meta.len(),
        "secs": modified.as_secs(),
        "nanos": modified.subsec_nanos(),
    })))
}

/// The record kept under crucible's home `crucible` for `instance` as it is
/// now: its driver, its `agent:` key and the arguments and environment it is
/// declared to start with all as they were when it was probed, taken in a
/// sandbox that closed the network, of a program that admits. None where
/// there is no such record, so a changed launch shape is probed again only
/// at the user's word, never by belief or a launch.
#[must_use]
pub fn read(crucible: &Path, instance: &Instance<'_>) -> Option<Record> {
    Record::at(&instance.kept(crucible)).filter(|record| record.holds_for(instance))
}

/// Removes the record of the instance `name`, with the instance.
///
/// # Errors
///
/// The I/O error where a record is there and could not be removed.
pub fn forget(crucible: &Path, name: &str) -> std::io::Result<()> {
    match fs::remove_file(kept_for(crucible, name)) {
        Err(why) if why.kind() != std::io::ErrorKind::NotFound => Err(why),
        Ok(()) | Err(_) => Ok(()),
    }
}

/// Keeps `record` for `instance` under `crucible`, in place of any it had,
/// once every record of an instance not among `instances` is dropped; where
/// the instances are not given, as at a launch, none is dropped.
///
/// # Errors
///
/// [`Unowned::Full`] where [`RECORDS`] are kept for other instances still,
/// else [`Unowned::Unkept`] where it could not be written.
fn keep(
    crucible: &Path,
    instance: &Instance<'_>,
    instances: Option<&[&str]>,
    record: &Record,
) -> Result<(), Unowned> {
    let dir = crucible.join(KEPT_IN);
    let unkept = |why| Unowned::Unkept(dir.clone(), why);
    fs::create_dir_all(&dir).map_err(unkept)?;
    let path = instance.kept(crucible);
    let mut others = 0_usize;
    for entry in fs::read_dir(&dir).map_err(unkept)? {
        let kept = entry.map_err(unkept)?.path();
        if kept == path {
            continue;
        }
        let owner = Record::at(&kept);
        let live = owner
            .as_ref()
            .and_then(|owner| owner.text("instance"))
            .is_some_and(|owner| instances.is_none_or(|instances| instances.contains(&owner)));
        if live {
            others = others.saturating_add(1);
        } else {
            fs::remove_file(&kept).map_err(unkept)?;
        }
    }
    if others >= RECORDS {
        return Err(Unowned::Full);
    }
    let mut text = serde_json::to_vec_pretty(&Value::Object(record.kept.clone()))
        .map_err(|_| Unowned::Unadmitted("what it answered could not be written".into()))?;
    text.push(b'\n');
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > RECORD_BYTES {
        return Err(Unowned::Unadmitted(
            "what it answered is more than a record keeps".into(),
        ));
    }
    let staged = path.with_extension("json.new");
    fs::write(&staged, &text).map_err(unkept)?;
    fs::rename(&staged, &path).map_err(unkept)
}

/// Why a program the user installed was not admitted, and nothing recorded.
#[derive(Debug, thiserror::Error)]
pub enum Unowned {
    /// The directory it lies in is not given to an agent.
    #[error("{0}")]
    Root(String),
    /// The sandbox here cannot close the network, so it was not started.
    #[error(
        "the sandbox here cannot close the network ({0}), so your own program is not probed \
         and nothing is recorded; give this machine what the sandbox needs, as step 1 of \
         \"Turning it on\" in docs/security/sandboxing.md says"
    )]
    Unclosable(String),
    /// It was held in by no sandbox that closed the network.
    #[error("the program was not held in by a sandbox that closed the network")]
    Unconfined,
    /// It cannot be followed to where it is.
    #[error("{} cannot be reached", .0.display())]
    Unreachable(PathBuf, #[source] std::io::Error),
    /// Its path is not text, which a record keeps it as.
    #[error("{} is not a path a record can keep", .0.display())]
    Unnamed(PathBuf),
    /// The empty directory the probe runs in could not be made.
    #[error("the empty directory the probe runs in could not be made")]
    Empty(#[source] std::io::Error),
    /// The sandbox policy for the probe is not a valid one.
    #[error("the sandbox policy for the probe could not be built: {0}")]
    Policy(#[from] SandboxPolicyError),
    /// This process cannot number another start.
    #[error("no control for the program")]
    Control,
    /// The host refused the start.
    #[error(transparent)]
    Refused(#[from] Refusal),
    /// It did not answer, or refused, `initialize`.
    #[error(transparent)]
    Probe(Unprobed),
    /// It is not taken for an agent.
    #[error("{0}")]
    Unadmitted(Box<str>),
    /// Records are kept for as many other instances as are kept.
    #[error("crucible keeps the probes of at most {RECORDS} of your own programs")]
    Full,
    /// The record could not be written.
    #[error("the probe record could not be kept in {}", .0.display())]
    Unkept(PathBuf, #[source] std::io::Error),
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

/// An empty directory only its owner may reach, made fresh under the system's
/// temporary directory for one probe and removed with this.
pub(crate) struct Empty {
    /// Where it is.
    path: PathBuf,
}

impl Empty {
    /// Where it is.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Makes one that was not there before.
    pub(crate) fn made() -> std::io::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("crucible-own-probe-{}-{nanos}", std::process::id()));
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700).create(&path)?;
        #[cfg(not(unix))]
        fs::DirBuilder::new().create(&path)?;
        let path = fs::canonicalize(&path)?;
        Ok(Self { path })
    }
}

impl Drop for Empty {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}

/// The policy a probe of a program whose directory is `root` starts under,
/// with the sandbox on where `enabled`: a granted directory read-only as the
/// probe's workspace, or, where it gets no rule, `empty` alone, under
/// [`EMPTY_PROVENANCE`].
///
/// # Errors
///
/// [`Unowned::Root`] for a refused directory, else [`Unowned::Policy`].
pub(crate) fn probe_policy(
    root: &Root,
    empty: &Path,
    enabled: bool,
) -> Result<SandboxPolicy, Unowned> {
    match root {
        Root::Refused(why) => Err(Unowned::Root(why.clone())),
        Root::Grant(granted) => Ok(policy(
            granted,
            SandboxFilesystemProvenance::Workspace,
            enabled,
        )?),
        Root::NoRule => Ok(policy(empty, EMPTY_PROVENANCE, enabled)?),
    }
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
/// agent is started under, with its directory decided against `places` as
/// the probe's was: a catalog agent's own program and Antigravity's. Every
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

/// Probes `instance`'s own program at `program` through `dialect`, in the
/// sandbox `service` gives on `runtime`, and keeps what it answered under
/// `places`' crucible home, among the records of `instances` alone. It is
/// started with the network closed and only the directory [`root`] gives it
/// readable, never without the sandbox, and sent `initialize` alone; it is
/// given `within` to answer.
///
/// # Errors
///
/// [`Unowned`] saying why it is not admitted; nothing is then kept.
// Each is an input of its own to the probe, and a value gathering them would
// be built at its callers and taken apart here.
#[allow(clippy::too_many_arguments)]
pub async fn probe<D: Dialect>(
    service: Arc<dyn SandboxService>,
    runtime: Handle,
    places: &Places<'_>,
    instance: &Instance<'_>,
    instances: &[&str],
    program: Program,
    dialect: D,
    within: Duration,
) -> Result<Record, Unowned> {
    let (canonical, said) = answer(service, runtime, places, program, dialect, within).await?;
    admit(
        places.crucible,
        instance,
        instances,
        &canonical,
        &Sandbox::Closed,
        &said,
    )
}

/// Probes again, as [`probe`] does, `instance`'s own program at `program`,
/// changed since its record was kept: its record is replaced by what it
/// answers now where that admits it, every other record kept as it is, and
/// removed where it does not, so the launch that asked is refused and leaves
/// no record behind.
///
/// # Errors
///
/// [`Unowned`] saying why it is no longer admitted.
// Each is an input of its own, as for `probe`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn reprobe<D: Dialect>(
    service: Arc<dyn SandboxService>,
    runtime: Handle,
    places: &Places<'_>,
    instance: &Instance<'_>,
    program: Program,
    dialect: D,
) -> Result<Record, Unowned> {
    let admitted = match answer(service, runtime, places, program, dialect, REPROBING).await {
        Ok((canonical, said)) => Record::answered(instance, &canonical, &Sandbox::Closed, &said)
            .and_then(|record| {
                keep(places.crucible, instance, None, &record)?;
                Ok(record)
            }),
        Err(why) => Err(why),
    };
    if admitted.is_err() {
        forget(places.crucible, instance.name)
            .map_err(|why| Unowned::Unkept(places.crucible.join(KEPT_IN), why))?;
    }
    admitted
}

/// How long a program probed again before a launch is given to answer: as
/// long as the intake gives a probe.
const REPROBING: Duration = crate::probing::BOUND;

/// The canonical path of `program`, and its whole answer to `initialize` alone,
/// sent through `dialect` with the program started confined as [`probe`]
/// says.
///
/// # Errors
///
/// [`Unowned`] where it was not started or did not answer.
// Each is an input of its own, as for `probe`.
#[allow(clippy::too_many_arguments)]
async fn answer<D: Dialect>(
    service: Arc<dyn SandboxService>,
    runtime: Handle,
    places: &Places<'_>,
    program: Program,
    dialect: D,
    within: Duration,
) -> Result<(PathBuf, Map<String, Value>), Unowned> {
    let canonical = fs::canonicalize(program.path())
        .map_err(|why| Unowned::Unreachable(program.path().to_owned(), why))?;
    let root = places.root(&canonical);
    if let Root::Refused(why) = root {
        return Err(Unowned::Root(why));
    }
    if let Some(reason) = unclosable(service.as_ref()).await {
        return Err(Unowned::Unclosable(reason));
    }
    let empty = Empty::made().map_err(Unowned::Empty)?;
    let policy = probe_policy(&root, empty.path(), true)?;
    let mut hearing = Hearing::new(dialect);
    let request = hearing.request(Shape {
        workspace: Some(policy.working_directory()),
        ..Shape::default()
    });
    let control = Control::new().ok_or(Unowned::Control)?;
    let launch = Launch::new(program, policy, Held::granted(PROBING)).asking(request);
    let host = Host::launch(service, runtime, &control, launch).await?;
    let frames = hearing.spoken(Account::Version).ok_or_else(|| {
        Unowned::Probe(Unprobed::Unanswered(
            "this agent speaks no initialize".into(),
        ))
    })?;
    let reading = host
        .speak(&mut hearing, Account::Version, &frames, &|_| {}, within)
        .await;
    let Some(said) = hearing.answered.take() else {
        let why = match reading {
            Reading::Unreadable(failure) => failure.reason().as_str().to_owned(),
            Reading::Version(_) | Reading::Signed(_) => "it said nothing".to_owned(),
        };
        return Err(Unowned::Probe(Unprobed::Unanswered(why.into())));
    };
    Ok((canonical, said))
}

/// Keeps, under crucible's home `crucible` and among the records of
/// `instances` alone, what `instance`'s program at `canonical` answered,
/// `said`, held in as `sandbox` says, where that admits it.
///
/// # Errors
///
/// [`Unowned`] saying why it is not admitted; nothing is then kept.
// Each is an input of its own, as for `probe`, which passes them on.
#[allow(clippy::too_many_arguments)]
pub(crate) fn admit(
    crucible: &Path,
    instance: &Instance<'_>,
    instances: &[&str],
    canonical: &Path,
    sandbox: &Sandbox,
    said: &Map<String, Value>,
) -> Result<Record, Unowned> {
    let record = Record::answered(instance, canonical, sandbox, said)?;
    keep(crucible, instance, Some(instances), &record)?;
    Ok(record)
}

/// The wire the user's own `program` of `row`'s agent is probed through, as
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

/// A catalog instance whose program is the user's own, as its record is kept
/// for it: what a launch holds that record to.
#[derive(Debug, Clone)]
pub(crate) struct Owned {
    /// Crucible's home, where the record is kept.
    crucible: PathBuf,
    /// The instance's name.
    name: Box<str>,
    /// The catalog entry its `agent:` names.
    key: Box<str>,
    /// The arguments its entry starts it with.
    args: Vec<Box<str>>,
    /// The variables its entry starts it with.
    env: Vec<Variable>,
    /// The canonical path of its program.
    canonical: PathBuf,
}

impl Owned {
    /// The instance `name` of `row`, whose program is the user's own at
    /// `program`, its record kept under `crucible`; none where `row` runs no
    /// program the user installed.
    ///
    /// # Errors
    ///
    /// [`Unowned::Unreachable`] where the program cannot be followed.
    pub(crate) fn of(
        crucible: &Path,
        name: &str,
        row: &AgentRow,
        program: &Path,
    ) -> Option<Result<Self, Unowned>> {
        let AgentRow::Catalog(catalog) = row else {
            return None;
        };
        if !catalog.is_own() {
            return None;
        }
        Some(
            fs::canonicalize(program)
                .map_err(|why| Unowned::Unreachable(program.to_owned(), why))
                .map(|canonical| Self {
                    crucible: crucible.to_owned(),
                    name: name.into(),
                    key: catalog.key().as_str().into(),
                    args: catalog.profile().args().to_vec(),
                    env: catalog.profile().env().to_vec(),
                    canonical,
                }),
        )
    }

    /// The instance, as its record is held to it.
    pub(crate) fn instance(&self) -> Instance<'_> {
        Instance {
            name: &self.name,
            driver: crucible_config::Driver::Catalog.name(),
            agent: Some(&self.key),
            args: &self.args,
            env: &self.env,
        }
    }

    /// The canonical path of its program.
    pub(crate) fn canonical(&self) -> &Path {
        &self.canonical
    }

    /// Its record, where one is kept for it as it is configured now; whether
    /// the program is still the one probed is [`Record::current`]'s to say.
    pub(crate) fn record(&self) -> Option<Record> {
        read(&self.crucible, &self.instance())
    }

    /// Replaces its record by what its program answered at a launch, `said`,
    /// where that admits it and differs from the record kept, and the program
    /// is still the one probed: the same bytes were held in a sandbox that
    /// closed the network, so the record still says so. Nothing is changed
    /// otherwise, and a record that cannot be written leaves the launch to go
    /// on as it was.
    fn answered(&self, said: &Map<String, Value>) {
        let instance = self.instance();
        let Some(kept) = self.record() else {
            return;
        };
        if !kept.current(&self.canonical) {
            return;
        }
        let Ok(now) = Record::answered(&instance, &self.canonical, &Sandbox::Closed, said) else {
            return;
        };
        if now != kept {
            let _ = keep(&self.crucible, &instance, None, &now);
        }
    }
}

/// The wire of a catalog instance whose program is the user's own, reading
/// its answer to `initialize` at every launch: the wire admits it or refuses
/// it as the probe did, and an answer that admits and differs from the
/// record replaces the record.
pub(crate) struct Recording {
    /// The wire, which says what is sent and admits what is answered.
    wire: Box<dyn Dialect>,
    /// The instance whose record it keeps.
    owned: Owned,
    /// Whether the answer to `initialize` is still to come.
    waiting: bool,
}

impl Recording {
    /// `wire`, keeping `owned`'s record.
    pub(crate) fn new(wire: Box<dyn Dialect>, owned: Owned) -> Self {
        Self {
            wire,
            owned,
            waiting: true,
        }
    }

    /// Notes `frame`, where it answers `initialize`.
    fn noted(&mut self, frame: &str) {
        if !self.waiting {
            return;
        }
        let Some(said) = initialize_answer(frame) else {
            return;
        };
        self.waiting = false;
        if said.contains_key("result") {
            self.owned.answered(&said);
        }
    }
}

impl Dialect for Recording {
    fn program(&self) -> &str {
        self.wire.program()
    }

    fn request(&mut self, shape: Shape<'_>) -> crucible_external_harnesses::Request {
        self.waiting = true;
        self.wire.request(shape)
    }

    fn open(&mut self, turn: &crucible_execution::TurnRequest) -> Result<Vec<String>, Failure> {
        self.wire.open(turn)
    }

    fn heard(&mut self, frame: &str) -> Result<Vec<crucible_external_harnesses::Act>, Failure> {
        let acts = self.wire.heard(frame)?;
        self.noted(frame);
        Ok(acts)
    }

    fn answer(
        &mut self,
        native: &str,
        decision: &crucible_execution::Decision,
    ) -> Result<String, Failure> {
        self.wire.answer(native, decision)
    }

    fn cancel(&mut self) -> Option<String> {
        self.wire.cancel()
    }

    fn account(
        &mut self,
        step: Account,
        shape: Shape<'_>,
    ) -> Option<crucible_external_harnesses::Request> {
        self.waiting = true;
        self.wire.account(step, shape)
    }

    fn read(&self, step: Account, ran: &crucible_external_harnesses::Ran) -> Reading {
        self.wire.read(step, ran)
    }

    fn spoken(&mut self, step: Account) -> Option<Vec<String>> {
        self.wire.spoken(step)
    }

    fn told(
        &mut self,
        step: Account,
        frame: &str,
    ) -> Result<crucible_external_harnesses::Told, Failure> {
        let told = self.wire.told(step, frame)?;
        self.noted(frame);
        Ok(told)
    }

    fn signing_in_with(&mut self, id: &str) -> bool {
        self.wire.signing_in_with(id)
    }
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

/// What a refusal of an own program says of `failure`.
#[must_use]
pub fn unadmitted(failure: &Failure) -> Unowned {
    Unowned::Unadmitted(failure.reason().as_str().into())
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

    /// The instance `agent` of `key` started as this proposes, as its probe
    /// record is kept for.
    fn instance<'a>(&'a self, agent: &'a ExternalAgent, key: &'a AgentKey) -> Instance<'a> {
        Instance {
            name: agent.name(),
            driver: agent.driver().name(),
            agent: Some(key.as_str()),
            args: self.own.args(),
            env: self.own.env(),
        }
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
/// `state`, and where that program was found: signed in by the methods its
/// probe record answered. Belief never probes, so an own program with no
/// record that holds for the instance as it is configured now is refused.
/// Nothing where its program is not the user's own.
///
/// # Errors
///
/// [`Unbelieved::OwnUnprobed`] where no record holds, and
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
    let home = state.parent().unwrap_or(state);
    let Some(record) = read(home, &proposed.instance(agent, key)) else {
        return Some(Err(Unbelieved::OwnUnprobed { key: key.clone() }));
    };
    Some(
        proposed
            .row(key, record.methods())
            .map(|row| (row, proposed.program)),
    )
}

/// Tries the program the user installed that the catalog instance `agent`
/// runs as, among the configured `instances`: probes it as [`probe`] does, in
/// the sandbox `sandbox` makes, on a runtime of its own, and keeps its record
/// under `places`' crucible home, the one a later launch of `agent` reads. It
/// is given `within` to answer.
///
/// # Errors
///
/// [`Unowned::NotOwn`] where `agent` runs no program the user installed, else
/// as [`probe`].
// Each is an input of its own to the probe, as for `probe`, which it passes on.
#[allow(clippy::too_many_arguments)]
pub fn tried(
    places: &Places<'_>,
    agent: &ExternalAgent,
    instances: &[&str],
    lookup: impl Fn(&str) -> Option<OsString>,
    sandbox: &dyn Fn(Handle) -> Arc<dyn SandboxService>,
    within: Duration,
) -> Result<Record, Unowned> {
    let not_own = || Unowned::NotOwn(agent.name().into());
    let key = agent.agent().ok_or_else(not_own)?;
    let state = crate::catalog_fetch::state_in(places.crucible);
    let proposed =
        Proposed::of(Some(agent.executable()), key, &state, lookup).ok_or_else(not_own)??;
    let dialect = proposed.row(key, Vec::new())?.dialect(Mode::Ask);
    let instance = proposed.instance(agent, key);
    let (record, unfinished) = crate::services::serving(|services| {
        let runtime = services
            .runtime()
            .handle()
            .map_err(|why| Unowned::Probe(why.into()))?;
        runtime.block_on(probe(
            sandbox(runtime.clone()),
            runtime.clone(),
            places,
            &instance,
            instances,
            proposed.program.clone(),
            dialect,
            within,
        ))
    });
    let record = record?;
    unfinished.map_err(|why| Unowned::Probe(why.into()))?;
    Ok(record)
}
