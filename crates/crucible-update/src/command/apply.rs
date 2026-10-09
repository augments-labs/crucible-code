//! Putting a newer release in place of the running one, on Unix.
//!
//! The release's two files are downloaded into a directory made for this
//! update alone, owner-only and removed with it, each within its release
//! ceiling and a bound on the whole download and on a silence in it. A
//! redirect is followed only from GitHub to GitHub's own hosts, over
//! `https`, a few times at most; a loopback source is followed nowhere. What
//! was downloaded is believed only once staging finds the archive is the one
//! `SHA256SUMS` lists.

use std::ffi::OsStr;
use std::fs::{DirBuilder, File};
use std::io::{ErrorKind, Read as _, Write as _};
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use crucible_http::{Chunk, Chunks, Http};
use tokio::runtime::Handle;
use tokio::time::timeout;

use super::{Answer, Asked, Refused, SelfUpdateCommand, Source, later, newest};
use crate::install::{Limits, archive_name};
use crate::{LayoutError, ReceiptLayout, RecoverableActivation, Version};

/// The name a release lists its archives' SHA-256 under.
const CHECKSUMS: &str = "SHA256SUMS";

/// The most redirects one download follows.
const REDIRECTS: usize = 5;

/// The longest one file of a release is given to download.
const DOWNLOAD_LIFETIME: Duration = Duration::from_mins(15);

/// The longest a download may go without a byte before it is given up.
const STALL: Duration = Duration::from_mins(1);

/// How long the new `crucible --version` is given to answer.
const SAYS_WITHIN: Duration = Duration::from_secs(10);

/// How often a `crucible --version` still running is looked at.
const LOOKING: Duration = Duration::from_millis(50);

/// The most of what `crucible --version` says that is kept.
const SAID: usize = 256;

/// How many names the download directory tries before it gives up.
const ATTEMPTS: u32 = 16;

/// Says what an update would install, or installs it, for an install
/// `install.sh` made.
pub(super) fn replace(
    runtime: &Handle,
    http: &Http,
    source: &Source,
    command: &SelfUpdateCommand<'_>,
) -> Result<Answer, Refused> {
    let layout = managed(command.executable)?;
    let newest = newest(runtime, http, source)?;
    let active = layout.receipt().version().clone();
    if !later(&newest, &active) {
        return Ok(Answer::Current { running: active });
    }
    if command.asked == Asked::DryRun {
        return Ok(Answer::Proposed {
            running: active,
            newest,
        });
    }

    let held = RecoverableActivation::begin(&layout).map_err(Refused::Activation)?;
    // Another update may have finished while this one asked.
    let active = held.layout().receipt().version().clone();
    if !later(&newest, &active) {
        return Ok(Answer::Current { running: active });
    }
    let scratch = Scratch::new()?;
    let name = archive_name(&newest, held.layout().receipt().target());
    let checksums = scratch.0.join(CHECKSUMS);
    let archive = scratch.0.join(&name);
    let limits = Limits::RELEASE;
    for (file, into, ceiling) in [
        (CHECKSUMS, &checksums, limits.checksums),
        (name.as_str(), &archive, limits.archive),
    ] {
        let url = source.download(&newest, file);
        runtime.block_on(download(http, source, url, into, ceiling))?;
    }

    let staged = held
        .stage(&newest, &archive, &checksums)
        .map_err(Refused::Stage)?;
    if held.layout().receipt().broker().is_some() && staged.receipt().broker().is_none() {
        return Err(Refused::Broker);
    }
    let activated = held.activate(staged).map_err(Refused::Activation)?;
    if says(&activated.executable(), &newest) {
        return Ok(Answer::Updated {
            from: active,
            to: newest,
        });
    }
    match activated.roll_back() {
        Ok(()) => Err(Refused::RolledBack { version: newest }),
        Err(error) => Err(Refused::RollBackFailed(error)),
    }
}

/// The layout the running executable belongs to, or how it is updated
/// instead.
fn managed(executable: &Path) -> Result<ReceiptLayout, Refused> {
    match ReceiptLayout::of_executable(executable) {
        Ok(layout) => Ok(layout),
        Err(LayoutError::Unmanaged) if cargo_built(executable) => Err(Refused::Cargo),
        Err(LayoutError::Unmanaged) => Err(Refused::Unmanaged),
        Err(error) => Err(Refused::Layout(error)),
    }
}

/// Whether `executable` is one cargo built: in the `bin` of a root
/// `cargo install` keeps its records in, or under a target directory cargo
/// tagged as its own.
fn cargo_built(executable: &Path) -> bool {
    let Ok(at) = executable.canonicalize() else {
        return false;
    };
    let installed = at
        .parent()
        .filter(|bin| bin.file_name() == Some(OsStr::new("bin")))
        .and_then(Path::parent)
        .is_some_and(|root| {
            [".crates.toml", ".crates2.json"]
                .iter()
                .any(|record| root.join(record).is_file())
        });
    installed
        || at
            .ancestors()
            .skip(1)
            .any(|directory| tagged_by_cargo(&directory.join("CACHEDIR.TAG")))
}

/// Whether the cache-directory tag at `tag` says cargo made it.
fn tagged_by_cargo(tag: &Path) -> bool {
    let mut said = Vec::new();
    File::open(tag)
        .and_then(|file| file.take(512).read_to_end(&mut said))
        .is_ok()
        && said.windows(5).any(|word| word == b"cargo")
}

/// A directory of this update's own, owner-only, removed with the value.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, Refused> {
        let base = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        for attempt in 0..ATTEMPTS {
            let at = base.join(format!(
                "crucible-update.{}.{nanos:x}.{attempt}",
                std::process::id()
            ));
            // Made, not found: a name somebody else made first is passed by.
            match DirBuilder::new().mode(0o700).create(&at) {
                Ok(()) => return Ok(Self(at)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(_) => break,
            }
        }
        Err(Refused::Scratch)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Downloads `url` from `source` into a new file `into`, refusing more than
/// `ceiling` bytes.
async fn download(
    http: &Http,
    source: &Source,
    url: String,
    into: &Path,
    ceiling: u64,
) -> Result<(), Refused> {
    let mut file = crucible_privacy::create_write(into).map_err(|_| Refused::Scratch)?;
    match timeout(
        DOWNLOAD_LIFETIME,
        fetched(http, source, url, &mut file, ceiling),
    )
    .await
    {
        Ok(Some(())) => Ok(()),
        Ok(None) | Err(_) => Err(Refused::Download {
            name: into
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }),
    }
}

/// Writes the body at `url` into `file`, following GitHub's redirects;
/// `None` where it did not arrive whole within `ceiling`.
async fn fetched(
    http: &Http,
    source: &Source,
    mut url: String,
    file: &mut File,
    ceiling: u64,
) -> Option<()> {
    let mut response = http.get(&url).await.ok()?;
    for _ in 0..REDIRECTS {
        if !response.status().is_redirection() {
            break;
        }
        let location = response.headers().get("location")?.to_str().ok()?;
        url = followed(source, location)?;
        response = http.get(&url).await.ok()?;
    }
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
                if held > ceiling {
                    return None;
                }
                file.write_all(&bytes).ok()?;
                heard = Instant::now();
            }
            Chunk::Quiet if heard.elapsed() >= STALL => return None,
            Chunk::Quiet => {}
        }
    }
    file.sync_all().ok()
}

/// Where a redirect to `location` may be followed: from GitHub, to an
/// `https` URL on `github.com` or a host under `githubusercontent.com`, and
/// from a loopback source nowhere.
pub(super) fn followed(source: &Source, location: &str) -> Option<String> {
    if *source != Source::GitHub {
        return None;
    }
    let uri: http::Uri = location.parse().ok()?;
    let authority = uri.authority()?;
    let host = authority.host();
    let github = host == "github.com" || host.ends_with(".githubusercontent.com");
    let port = authority.port_u16().is_none_or(|port| port == 443);
    (uri.scheme_str() == Some("https") && github && port && !authority.as_str().contains('@'))
        .then(|| location.to_owned())
}

/// Whether `executable --version` says it is `version`, within its bound.
///
/// What it says is read from a pipe as it comes, and no more than [`SAID`]
/// bytes of it are kept: a release that says more is not saying its version,
/// and is stopped there rather than read to its end or written anywhere.
fn says(executable: &Path, version: &Version) -> bool {
    let Ok(mut child) = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let said = child
        .stdout
        .take()
        .and_then(|out| listened(&mut child, out));
    // Kills nothing that has exited, and reaps what was killed.
    let _ = child.kill();
    let _ = child.wait();
    said.is_some_and(|said| said.trim_ascii_end() == format!("crucible {version}").as_bytes())
}

/// What `child` wrote to `out` once it exited with success within
/// [`SAYS_WITHIN`]; `None` where it failed, ran past that bound or said more
/// than [`SAID`] bytes.
fn listened(child: &mut Child, mut out: ChildStdout) -> Option<Vec<u8>> {
    rustix::io::ioctl_fionbio(&out, true).ok()?;
    let mut said = Vec::with_capacity(SAID + 1);
    let began = Instant::now();
    loop {
        // Looked at before the pipe is read, so all an exited child wrote is
        // in it.
        let exited = child.try_wait().ok()?;
        heard(&mut out, &mut said)?;
        match exited {
            Some(status) => return status.success().then_some(said),
            None if began.elapsed() < SAYS_WITHIN => std::thread::sleep(LOOKING),
            None => return None,
        }
    }
}

/// Adds what `out` holds now to `said`, reading no more than one byte past
/// [`SAID`]; `None` once `said` holds more than [`SAID`] bytes or the pipe
/// cannot be read.
fn heard(out: &mut ChildStdout, said: &mut Vec<u8>) -> Option<()> {
    let mut chunk = [0_u8; SAID + 1];
    loop {
        let room = SAID.saturating_add(1).saturating_sub(said.len());
        match out.read(chunk.get_mut(..room)?) {
            Ok(0) => return Some(()),
            Ok(read) => {
                said.extend_from_slice(chunk.get(..read)?);
                if said.len() > SAID {
                    return None;
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => return Some(()),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
}
