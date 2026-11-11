//! A program the user installed themselves, run as the catalog agent an
//! `agent:` key names: what the sandbox is told of the directory it lies in.
//!
//! An own program is no release crucible pinned or reviewed, so the directory
//! it lies in is decided once, here, for the probe that admits it and for
//! every launch alike. That directory is refused when it is the user's home or
//! the top of the file system, or is, lies within or contains a place no agent
//! is made able to write; the one exception is a home search directory such as
//! `~/.local/bin`, which may itself be read. A directory the sandbox already
//! lets every program read gets no rule of its own, and any other is given
//! read-only, never writable. Everything in that directory is readable to the
//! agent while it runs.
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

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use crucible_execution::Failure;
use crucible_external_harnesses::acp::Initialized;
use crucible_external_harnesses::catalog::{AuthMethod, Method, Route, Variable};
use crucible_external_harnesses::{
    Account, Control, Dialect, Held, Host, Launch, Program, Reading, Refusal, Shape,
};
use crucible_sandbox::{
    SandboxFilesystemProvenance, SandboxPolicy, SandboxPolicyError, SandboxService,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use tokio::runtime::Handle;

use crucible_tools::Mode;

use crate::backends::{AgentRow, Wire};
use crate::launching::{
    Guarded, Overlap, PROBING, Reach, Sandbox, beneath, guarded, is_beneath, overlap, read_only,
    unclosable,
};
use crate::probing::{Hearing, Kept, Unprobed, policy, recorded};

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
    /// Any other.
    Other,
}

impl Platform {
    /// The platform this build runs on.
    pub(crate) const HERE: Self = if cfg!(target_os = "macos") {
        Self::MacOs
    } else if cfg!(windows) {
        Self::Windows
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
            Self::MacOs | Self::Windows | Self::Other => None,
        }
    }
}

/// What the sandbox is told of the directory `program`, a canonical path,
/// lies in, for its probe and its launch alike: refused where that directory
/// is `home`, contains it, is `/`, or is, lies within or contains a place no
/// agent is made able to write, crucible's own home `crucible` and its session
/// logs `sessions` among them, but for a home search directory itself; no
/// rule where the sandbox already reads it; else given read-only.
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
    let Some(root) = program.parent().filter(|root| root.parent().is_some()) else {
        return Root::Refused(format!(
            "{} lies at the top of the file system, which holds every file on it, so its \
             directory is not given to an agent.",
            program.display()
        ));
    };
    let shown = root.display();
    if let Some(found @ (Overlap::Is | Overlap::Contains)) = overlap(root, home, Reach::Itself) {
        return Root::Refused(format!(
            "{shown} {} your home, which holds every file you keep, so it is not given to an \
             agent.",
            found.says()
        ));
    }
    if let Some(why) = platform.unrecorded(root) {
        return Root::Refused(why);
    }
    let searched = HOME_SEARCH
        .iter()
        .any(|dir| overlap(root, &beneath(home, dir), Reach::Itself) == Some(Overlap::Is));
    for place in guarded {
        for at in &place.at {
            match overlap(root, at, place.reach) {
                Some(Overlap::Is) if searched => {}
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
    /// started.
    #[must_use]
    pub fn current(&self, program: &Path) -> bool {
        let Ok(stamp) = stamp(program) else {
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
        kept.insert("stamp".into(), stamp);
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
/// them.
fn stamp(path: &Path) -> std::io::Result<Value> {
    let meta = fs::metadata(path)?;
    let modified = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Ok(json!({
        "size": meta.len(),
        "secs": modified.as_secs(),
        "nanos": modified.subsec_nanos(),
    }))
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
/// once every record of an instance not among `instances` is dropped.
///
/// # Errors
///
/// [`Unowned::Full`] where [`RECORDS`] are kept for other instances still,
/// else [`Unowned::Unkept`] where it could not be written.
fn keep(
    crucible: &Path,
    instance: &Instance<'_>,
    instances: &[&str],
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
            .is_some_and(|owner| instances.contains(&owner));
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
        let mut made = fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut made, 0o700);
        made.create(&path)?;
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
    admit(
        places.crucible,
        instance,
        instances,
        &canonical,
        &Sandbox::Closed,
        &said,
    )
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
    keep(crucible, instance, instances, &record)?;
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
    row.own_dialect(Mode::Ask, None, program)
}

/// What a refusal of an own program says of `failure`.
#[must_use]
pub fn unadmitted(failure: &Failure) -> Unowned {
    Unowned::Unadmitted(failure.reason().as_str().into())
}
