//! The release of an agent the catalog offers, where crucible runs it from:
//! where it is kept, what a refusal says where its program is missing, and
//! [`install`], which puts it there.
//!
//! Which release that is, is what the trust state a caller loaded offers for the
//! agent's row: the newest of the catalog most lately believed and the snapshot
//! built into the binary, none that was revoked. A trust state that cannot be
//! used offers nothing, and is told to the user as that, naming its file, rather
//! than as an agent with no release.
//!
//! The release is downloaded from where it was pinned, into a file of its own
//! under the agent's directory in crucible's home, within the archive's pinned
//! size, a bound on the whole download and one on a silence in it, and is
//! unpacked only as `crucible-external-harnesses` finds it is the archive
//! pinned. Its request goes through the release owner's client, under the
//! application's hold, as every request a run makes does; it carries nothing
//! from a session. A release already in place is kept, and nothing is asked
//! for it, and an agent's entry `/login` writes names it as its executable.
//! Like [`crate::update`], [`install`] is no request of the client contract,
//! and it reads no configuration. It is the precedent the agents door
//! replaced: `crucible agents install` is a request to
//! [`crate::client::agents`], which installs, updates, rolls back and removes a
//! catalog agent as a client's request, asks before a byte is fetched, and
//! writes the `executable` of every instance it moves into the user's
//! configuration file, which its shelf's home names.
//!
//! A release the ACP Registry offers has no exact size and no catalog entry:
//! its archive is downloaded within the registry's cap, surveyed from its
//! file by [`Unpacker::survey_from`] as [`registry_artifact`] describes it,
//! and its program started once by [`probed`], in the closed sandbox with the
//! sandbox setting on and as a plain local process with it off, which records
//! what it answered `initialize` with for the platform it runs on, so the
//! receipt keeps the facts a later session rebuilds the row from.
//!
//! [`unpack_check`] is the catalog intake's way to the same unpacker an
//! install of a catalog agent goes through: it reads an artifact as the
//! catalog spells one and a download from disk, each within its bound,
//! surveys the download into a directory of its own, and gives the file list
//! the survey recorded. Given a package tree's lock and the directory its
//! tarballs were downloaded into instead, it unpacks the tree by [`NpmTree`]
//! the same way. It downloads nothing and reads no configuration.

use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crucible_external_harnesses::Discovery;
use crucible_external_harnesses::catalog::{
    ASSET_BYTES, Admits, Artifact, CatalogError, Entry, Files, Format, INDEX_BYTES, Trust,
    Unusable, Variable,
};
use crucible_external_harnesses::release::{
    NpmError, NpmTree, OwnedPin, Pin, RELEASE_BYTES, UNPACKED_BYTES, UnpackError, Unpacked,
    Unpacker, Wheels, unpack,
};
use crucible_external_harnesses::store::{
    PROBED_METHODS, PROBED_PLATFORM_BYTES, PROBED_TEXT_BYTES, Probe, ProbedKind, ProbedMethod,
};
use crucible_http::{Chunk, Chunks, Http, Lookups, Poison};
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::time::timeout;

use crate::backends::Row;
use crate::probing::Unprobed;
use crate::runtime::Unstarted;
use crate::services::{Unfinished, serving};

/// The longest a release is given to download: the largest is a third of a
/// gigabyte.
const DOWNLOAD_LIFETIME: Duration = Duration::from_mins(45);

/// The longest a download may go without a byte before it is given up.
const STALL: Duration = Duration::from_mins(1);

/// What an install put in place, or found there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The release's version.
    pub version: Box<str>,
    /// The executable crucible runs.
    pub executable: PathBuf,
    /// Whether it was already in place, so nothing was downloaded.
    pub kept: bool,
}

/// Why an install put nothing in place.
#[derive(Debug, thiserror::Error)]
pub enum Uninstalled {
    /// The catalog offers no release of the agent that can be installed for
    /// this platform.
    #[error(
        "the catalog offers no release of {agent} to install on {} for {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )]
    Unpinned {
        /// The agent, as the configuration names it.
        agent: &'static str,
    },
    /// The catalog's trust state cannot be used, so no release is offered.
    #[error(
        "the catalog trust state at {} cannot be used ({why}), so no release of {agent} is \
         offered; repair or remove that file",
        .file.display()
    )]
    Untrusted {
        /// The agent, as the configuration names it.
        agent: &'static str,
        /// The file the state is kept in.
        file: PathBuf,
        /// Why it cannot be used.
        why: Unusable,
    },
    /// No client could be built to download it with.
    #[error("no HTTP client could be built to download the release")]
    Unconnected,
    /// The download could not be kept.
    #[error("the download could not be written under {}", .0.display())]
    Unwritten(PathBuf),
    /// The download did not arrive whole.
    #[error("the release did not download whole from {0}")]
    Download(Box<str>),
    /// What was downloaded is not the release pinned, or could not be
    /// unpacked.
    #[error(transparent)]
    Unpacked(#[from] Unpacked),
    /// The runtime the download runs on would not start.
    #[error(transparent)]
    Unstarted(#[from] Unstarted),
}

/// The directory `row`'s agent is kept under in crucible's `home`.
#[must_use]
pub fn root(home: &Path, row: &Row) -> PathBuf {
    home.join("programs").join(row.driver.name())
}

/// The executable crucible runs `row`'s agent as once it is installed under
/// `home`, where `trust` offers a release of it for this platform.
#[must_use]
pub fn executable(home: &Path, row: &Row, trust: &Trust) -> Option<PathBuf> {
    let offered = (row.pinned)(trust)?;
    let pin = offered.pin()?;
    Some(pin.executable(&root(home, row)))
}

/// What a refusal over `discovery` says, as the sentence and what to do next,
/// for `row`'s agent called `agent` whose command is `command`: for an agent
/// `trust` offers a release of, that what to do is to run the command that
/// installs it; for one whose releases are the catalog's while its trust state
/// cannot be used, that the state is to be repaired or removed. Nothing for a
/// program that was found.
#[must_use]
pub fn refusal(
    row: &Row,
    discovery: &Discovery,
    agent: &str,
    command: &str,
    trust: &Trust,
) -> Option<[String; 2]> {
    let [said, next] = discovery.refusal(agent, command)?;
    let next = match discovery {
        Discovery::Missing if (row.pinned)(trust).is_some() => format!(
            "Run `crucible agents install {}` in a terminal, then choose Check again.",
            row.driver.name()
        ),
        Discovery::Missing => match trust.unusable().filter(|_| row.catalogued) {
            Some(why) => format!(
                "The catalog trust state at {} cannot be used ({why}), so no release of {} is \
                 offered. Repair or remove that file, then choose Check again.",
                trust.file().display(),
                row.driver.name()
            ),
            None => next,
        },
        Discovery::NotExecutable(_) | Discovery::Found(_) => next,
    };
    Some([said, next])
}

/// Installs `row`'s agent under crucible's `home` as the release `trust`
/// offers, telling `progress` how many of how many bytes have arrived as they
/// do, then shuts the services it ran on down.
///
/// # Errors
///
/// The first half is [`Uninstalled`] where nothing was put in place. The
/// second is [`Unfinished`] where the services' shutdown ran out of its
/// bounds, whatever the first says.
pub fn install(
    row: &Row,
    home: &Path,
    trust: &Trust,
    progress: &mut dyn FnMut(u64, u64),
) -> (Result<Installed, Uninstalled>, Result<(), Unfinished>) {
    let offered = (row.pinned)(trust);
    let Some(pin) = offered.as_ref().and_then(OwnedPin::pin) else {
        return (Err(unoffered(row, trust)), Ok(()));
    };
    let root = root(home, row);
    if pin.installed(&root) {
        let executable = pin.executable(&root);
        return (
            Ok(Installed {
                version: pin.version.into(),
                executable,
                kept: true,
            }),
            Ok(()),
        );
    }
    serving(|services| {
        let runtime = services.runtime().handle()?;
        let poison = Poison::default();
        let targets = Lookups::poisoned(NonZeroUsize::MIN, &poison);
        let http = services
            .release()
            .client(targets, Arc::new(services.consent().clone()))
            .ok_or(Uninstalled::Unconnected)?;
        let executable = downloaded(&runtime, &http, &pin, &root, progress)?;
        Ok(Installed {
            version: pin.version.into(),
            executable,
            kept: false,
        })
    })
}

/// Why `row`'s agent has no release to install: its releases are the catalog's
/// and the trust state cannot be used, or this build is offered none for it.
fn unoffered(row: &Row, trust: &Trust) -> Uninstalled {
    match trust.unusable().filter(|_| row.catalogued) {
        Some(why) => Uninstalled::Untrusted {
            agent: row.driver.name(),
            file: trust.file(),
            why,
        },
        None => Uninstalled::Unpinned {
            agent: row.driver.name(),
        },
    }
}

/// Downloads `pin`'s archive through `http` into a file of its own under
/// `root`, unpacks it there and removes it, giving the executable's path.
fn downloaded(
    runtime: &Handle,
    http: &Http,
    pin: &Pin<'_>,
    root: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, Uninstalled> {
    let unwritten = || Uninstalled::Unwritten(root.to_owned());
    crucible_privacy::directory(root).map_err(|_| unwritten())?;
    let (download, file) = Download::new(root).ok_or_else(unwritten)?;
    // The timer is made on the runtime, which is the only place it can be.
    let arrived = runtime
        .block_on(async { timeout(DOWNLOAD_LIFETIME, fetched(http, pin, file, progress)).await });
    if !matches!(arrived, Ok(Some(()))) {
        return Err(Uninstalled::Download(pin.url.into()));
    }
    Ok(unpack(pin, &download.0, root)?)
}

/// Writes the body at `pin`'s address into `file`; `None` where it did not
/// arrive whole within the archive's pinned size.
async fn fetched(
    http: &Http,
    pin: &Pin<'_>,
    mut file: File,
    progress: &mut dyn FnMut(u64, u64),
) -> Option<()> {
    let response = http.get(pin.url).await.ok()?;
    // Where it was pinned is where it is downloaded from, so a redirect is
    // not followed.
    if response.status().as_u16() != 200 {
        return None;
    }
    let mut chunks = Chunks::new(response.into_body());
    let mut held: u64 = 0;
    let mut heard = Instant::now();
    while let Some(next) = chunks.next().await {
        match next.ok()? {
            Chunk::Data(bytes) => {
                held = held.checked_add(u64::try_from(bytes.len()).ok()?)?;
                if held > pin.archive {
                    return None;
                }
                file.write_all(&bytes).ok()?;
                heard = Instant::now();
                progress(held, pin.archive);
            }
            Chunk::Quiet if heard.elapsed() >= STALL => return None,
            Chunk::Quiet => {}
        }
    }
    file.sync_all().ok()
}

/// A file of this install's own under the agent's directory, removed with the
/// value.
struct Download(PathBuf);

impl Download {
    /// The file, made owner-only, and the value that removes it; nothing where
    /// it could not be made.
    fn new(root: &Path) -> Option<(Self, File)> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        let at = root.join(format!(".download-{}-{nanos:x}.zip", std::process::id()));
        // Made, not found: a name somebody else made first is not removed.
        let file = crucible_privacy::create_write(&at).ok()?;
        Some((Self(at), file))
    }
}

impl Drop for Download {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The artifact a registry archive is surveyed as, once `size` bytes of it
/// arrived: pinned by `sha256`, with no list, its unpacked ceiling
/// [`UNPACKED_BYTES`] and its decompressed ceiling [`RELEASE_BYTES`], as the
/// registry's bounds say.
#[must_use]
pub fn registry_artifact(url: &str, sha256: &str, size: u64, format: Format) -> Artifact {
    Artifact {
        url: url.into(),
        mirror: None,
        sha256: sha256.into(),
        size,
        format,
        unpacked: UNPACKED_BYTES,
        decompressed: RELEASE_BYTES,
        files: Files::Listed(Vec::new()),
        kept: Vec::new(),
        replaced_by: None,
    }
}

/// Starts the program `root` holds, as the provisional `entry` describes it,
/// held in as `mode` says, and gives what it answered as the record a receipt
/// keeps: the platform the entry installs for this machine, its `agentInfo`,
/// and the sign-in methods it offered.
///
/// The entry is read through `Entry::unsigned` before anything is started, as
/// `probing::probe` reads it, and its target for this machine is the record's
/// target, so neither an entry the catalog would refuse nor one published for
/// another platform starts anything or is recorded under a platform it is not
/// installed for.
///
/// # Errors
///
/// [`Unprobed`] saying why there is nothing to record: the entry is not one or
/// has no target for this machine, the program could not be started or did not
/// answer, or its answer had to be cut and so is not one a row can be rebuilt
/// from.
pub fn probed(
    root: &Path,
    entry: &[u8],
    mode: crate::probing::Mode,
    within: Duration,
) -> Result<Probe, Unprobed> {
    let draft = Draft::new(entry)?;
    let described = Entry::unsigned(entry).map_err(|source| Unprobed::Entry {
        path: draft.0.clone(),
        source,
    })?;
    let target = described
        .here()
        .map(|target| target.platform.clone())
        .ok_or_else(|| Unprobed::Unanswered("the entry has no target for this platform".into()))?;
    let answered = crate::probing::probe(root, &draft.0, mode, &crate::probing::local, within)?;
    record(&answered, &target)
}

/// The record the probe's `answered` JSON holds, taken for `target`.
pub(crate) fn record(answered: &str, target: &str) -> Result<Probe, Unprobed> {
    let refused = |why: &str| Unprobed::Unanswered(why.into());
    let answered: Value = serde_json::from_str(answered)
        .map_err(|_| refused("the probe's answer could not be read"))?;
    if answered.get("incomplete") == Some(&Value::Bool(true)) {
        return Err(refused("the agent answered more than the probe keeps"));
    }
    // A probe the sandbox held in with its network closed, or one the
    // sandbox setting off started as a plain local process; never one that
    // ran unconfined where the sandbox was on.
    if !matches!(
        answered.get("sandbox").and_then(Value::as_str),
        Some("closed" | "off")
    ) {
        return Err(refused(
            "the probe did not run in a sandbox that closed its network",
        ));
    }
    let info = answered.get("agentInfo");
    let name = said(info.and_then(|info| info.get("name")))
        .ok_or_else(|| refused("the agent did not say what it is"))?;
    let version = said(info.and_then(|info| info.get("version")))
        .ok_or_else(|| refused("the agent did not say what version it is"))?;
    let methods = answered
        .get("authMethods")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if methods.len() > PROBED_METHODS {
        return Err(refused("the agent answered more than a receipt keeps"));
    }
    let auth_methods = methods
        .iter()
        .map(method_of)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| refused("the agent offered a sign-in method that cannot be recorded"))?;
    let record = Probe {
        target: target.into(),
        admits: Admits {
            name: name.into(),
            version: version.into(),
        },
        auth_methods,
    };
    if fits(&record) {
        Ok(record)
    } else {
        Err(refused("the agent answered more than a receipt keeps"))
    }
}

/// Whether `record` is one the store's receipt reader accepts: its platform
/// and every text within the receipt's ceilings, no more methods than it
/// keeps, and no more arguments or variables of one than it keeps.
fn fits(record: &Probe) -> bool {
    let text = |text: &str| text.len() <= PROBED_TEXT_BYTES;
    // The names the reader requires with `kept_text`: not empty, and within
    // the receipt's one-text bound.
    let kept = |text: &str| !text.is_empty() && text.len() <= PROBED_TEXT_BYTES;
    !record.target.is_empty()
        && record.target.len() <= PROBED_PLATFORM_BYTES
        && kept(&record.admits.name)
        && kept(&record.admits.version)
        && record.auth_methods.len() <= PROBED_METHODS
        && record.auth_methods.iter().all(|method| {
            kept(&method.id)
                && kept(&method.name)
                && match &method.kind {
                    ProbedKind::Terminal {
                        command, args, env, ..
                    } => {
                        kept(command)
                            && args.len() <= PROBED_METHODS
                            && args.iter().all(|arg| text(arg))
                            && env.len() <= PROBED_METHODS
                            && env
                                .iter()
                                .all(|variable| kept(&variable.name) && text(&variable.value))
                    }
                    ProbedKind::Agent | ProbedKind::EnvVar | ProbedKind::Other => true,
                }
        })
}

/// `value` as the text the probe keeps, where it is text and not empty.
fn said(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

/// One sign-in method as the probe recorded it.
fn method_of(method: &Value) -> Option<ProbedMethod> {
    let text = |name: &str| said(method.get(name));
    let id = text("id")?;
    let name = text("name")?;
    let kind = match method.get("kind").and_then(Value::as_str) {
        Some("agent") => ProbedKind::Agent,
        Some("env_var") => ProbedKind::EnvVar,
        Some("terminal") => {
            let command = text("command")?;
            let args = method
                .get("args")?
                .as_array()?
                .iter()
                .map(|arg| Some(arg.as_str()?.into()))
                .collect::<Option<Vec<_>>>()?;
            let env = method
                .get("env")?
                .as_object()?
                .iter()
                .map(|(name, value)| {
                    Some(Variable {
                        name: name.as_str().into(),
                        value: value.as_str()?.into(),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            let tty = match method.get("tty") {
                Some(Value::Bool(needs)) => Some(*needs),
                _ => None,
            };
            ProbedKind::Terminal {
                command: command.into(),
                args,
                env,
                tty,
            }
        }
        _ => ProbedKind::Other,
    };
    Some(ProbedMethod {
        id: id.into(),
        name: name.into(),
        kind,
    })
}

/// A file holding the provisional entry while the probe reads it, removed
/// with the value.
struct Draft(PathBuf);

impl Draft {
    /// `entry` written to a file of this probe's own, owner-only, within the
    /// bound a catalog index is read at.
    fn new(entry: &[u8]) -> Result<Self, Unprobed> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let ceiling = u64::try_from(INDEX_BYTES).unwrap_or(u64::MAX);
        if u64::try_from(entry.len()).unwrap_or(u64::MAX) > ceiling {
            return Err(Unprobed::Read(Unchecked::TooLarge {
                path: std::env::temp_dir().join("the provisional entry"),
                ceiling,
            }));
        }
        let mut last = io::Error::from(io::ErrorKind::AlreadyExists);
        for _ in 0..16_u32 {
            let path = std::env::temp_dir().join(format!(
                "crucible-probe-{}-{}.json",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let unread = |source| {
                Unprobed::Read(Unchecked::Read {
                    path: path.clone(),
                    source,
                })
            };
            let mut file = match crucible_privacy::create_write(&path) {
                Ok(file) => file,
                Err(error) => {
                    let error = error.into_io();
                    if error.kind() == io::ErrorKind::AlreadyExists {
                        last = error;
                        continue;
                    }
                    return Err(unread(error));
                }
            };
            file.write_all(entry)
                .and_then(|()| file.sync_all())
                .map_err(unread)?;
            return Ok(Self(path));
        }
        Err(Unprobed::Read(Unchecked::Read {
            path: std::env::temp_dir().join("the provisional entry"),
            source: last,
        }))
    }
}

impl Drop for Draft {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Why `crucible agents unpack-check` gave no file list.
#[derive(Debug, thiserror::Error)]
pub enum Unchecked {
    /// A file it was given could not be read.
    #[error("{} could not be read", .path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// What the system said.
        #[source]
        source: io::Error,
    },
    /// A file it was given is larger than what it holds may be.
    #[error("{} is larger than {ceiling} bytes", .path.display())]
    TooLarge {
        /// The file.
        path: PathBuf,
        /// The most it may hold.
        ceiling: u64,
    },
    /// The artifact is not one as the catalog spells it.
    #[error("{} is not an artifact as the catalog spells one", .path.display())]
    Artifact {
        /// The file.
        path: PathBuf,
        /// Which field is wrong.
        #[source]
        source: CatalogError,
    },
    /// The directory to unpack into could not be made, or is already there.
    #[error("{} could not be made to unpack into", .path.display())]
    Out {
        /// The directory.
        path: PathBuf,
        /// What the system said.
        #[source]
        source: io::Error,
    },
    /// The download is not the artifact, or breaks a rule an install holds it
    /// to.
    #[error(transparent)]
    Unpacked(#[from] UnpackError),
    /// The lock is not one as the catalog keeps it, or a tarball is not the
    /// one it pins, or breaks a rule an install holds it to.
    #[error(transparent)]
    Tree(#[from] NpmError),
    /// What was recorded could not be written as JSON.
    #[error("the file list could not be written as JSON")]
    Json(#[from] serde_json::Error),
}

/// Surveys the download at `artifact`, which the catalog artifact spelled in
/// `entry` pins, into `out`, a directory only its owner may reach that this
/// makes and nothing else may already be at, by the rules an install unpacks
/// it by (a wheel's files under `site/`, as a pinned interpreter's install
/// puts them), and gives the file list it recorded as JSON, with its sizes,
/// ending in a line feed.
///
/// Where `artifact` is a directory, `entry` is a package tree's lock instead,
/// and `artifact` holds each of its tarballs under the SHA-512 it is pinned
/// by: the tree is unpacked into `out` by [`NpmTree::unpack`], and the JSON
/// gives the `files` it wrote.
///
/// `entry` is read only when it is at most [`INDEX_BYTES`], as a catalog is,
/// or [`ASSET_BYTES`] for a lock, and the download only up to the size the
/// artifact pins, so neither is held past its bound. A refusal removes `out`.
///
/// # Errors
///
/// [`Unchecked`] naming what could not be read or made, or how the download
/// is not the artifact or breaks a rule.
pub fn unpack_check(entry: &Path, artifact: &Path, out: &Path) -> Result<String, Unchecked> {
    if artifact.is_dir() {
        let tree = NpmTree::parse(&bounded(entry, ASSET_BYTES)?)?;
        made(out)?;
        let unpacked = tree
            .unpack(artifact, out)
            .map_err(Unchecked::from)
            .and_then(|files| {
                let files: Vec<_> = files
                    .iter()
                    .map(crucible_external_harnesses::catalog::File::json)
                    .collect();
                Ok(serde_json::to_string_pretty(
                    &serde_json::json!({ "files": files }),
                )?)
            });
        return recorded(out, unpacked);
    }
    let ceiling = u64::try_from(INDEX_BYTES).unwrap_or(u64::MAX);
    let text = bounded(entry, ceiling)?;
    let described = Artifact::unsigned(&text).map_err(|source| Unchecked::Artifact {
        path: entry.to_owned(),
        source,
    })?;
    let pinned = described
        .replaced_by
        .as_ref()
        .map_or(described.size, |asset| asset.size);
    let bytes = bounded(artifact, pinned)?;
    made(out)?;
    // A wheel is read as an install reads one, into `site/`.
    let survey = if described.format == Format::Wheel {
        Wheels::survey
    } else {
        Unpacker::survey
    };
    let surveyed = survey(&bytes, &described, out)
        .map_err(Unchecked::from)
        .and_then(|staged| Ok(serde_json::to_string_pretty(&staged.json())?));
    recorded(out, surveyed)
}

/// Makes `out`, which only its owner may reach where the platform says who
/// may, and which must not already be there.
fn made(out: &Path) -> Result<(), Unchecked> {
    let made = fs::DirBuilder::new();
    #[cfg(unix)]
    let made = {
        let mut made = made;
        std::os::unix::fs::DirBuilderExt::mode(&mut made, 0o700);
        made
    };
    made.create(out).map_err(|source| Unchecked::Out {
        path: out.to_owned(),
        source,
    })
}

/// What `crucible agents unpack-check` gives once it unpacked into `out`:
/// the JSON `unpacked` holds, ending in a line feed, or its refusal, once
/// `out` is removed.
fn recorded(out: &Path, unpacked: Result<String, Unchecked>) -> Result<String, Unchecked> {
    match unpacked {
        Ok(listed) => Ok(format!("{listed}\n")),
        Err(refused) => {
            let _ = fs::remove_dir_all(out);
            Err(refused)
        }
    }
}

/// What the file at `path` holds, refused when that is more than `ceiling`
/// bytes; no more than one byte past it is read.
pub(crate) fn bounded(path: &Path, ceiling: u64) -> Result<Vec<u8>, Unchecked> {
    let unread = |source| Unchecked::Read {
        path: path.to_owned(),
        source,
    };
    let mut held = Vec::new();
    File::open(path)
        .map_err(unread)?
        .take(ceiling.saturating_add(1))
        .read_to_end(&mut held)
        .map_err(unread)?;
    if u64::try_from(held.len()).unwrap_or(u64::MAX) > ceiling {
        return Err(Unchecked::TooLarge {
            path: path.to_owned(),
            ceiling,
        });
    }
    Ok(held)
}

#[cfg(test)]
mod tests;
