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
use std::process::{Command, Stdio};
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

/// The most of what `crucible --version` says that is read.
const SAID: u64 = 256;

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
    if says(&activated.executable(), &newest, &scratch.0) {
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
/// What it says goes to a file in `scratch` rather than a pipe, so a release
/// that writes without end fills nothing this process holds.
fn says(executable: &Path, version: &Version, scratch: &Path) -> bool {
    let said_at = scratch.join("version");
    let Ok(out) = crucible_privacy::create_write(&said_at) else {
        return false;
    };
    let Ok(mut child) = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let began = Instant::now();
    let succeeded = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if began.elapsed() < SAYS_WITHIN => std::thread::sleep(LOOKING),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    };
    let mut said = Vec::new();
    succeeded
        && File::open(&said_at)
            .and_then(|file| file.take(SAID).read_to_end(&mut said))
            .is_ok()
        && said.trim_ascii_end() == format!("crucible {version}").as_bytes()
}
