//! `crucible agents install`, which puts the release of an agent this build
//! pinned where crucible runs it from.
//!
//! The release is downloaded from where it was pinned, into a file of its own
//! under the agent's directory in crucible's home, within the archive's pinned
//! size, a bound on the whole download and one on a silence in it, and is
//! unpacked only as `crucible-external-harnesses` finds it is the archive
//! pinned. Its request goes through the release owner's client, under the
//! application's hold, as every request a run makes does; it carries nothing
//! from a session. A release already in place is kept, and nothing is asked
//! for it. Like [`crate::update`], this is a local command rather than a
//! request of the client contract, and it reads no configuration.

use std::fs::File;
use std::io::Write as _;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crucible_external_harnesses::release::{Pin, Unpacked, unpack};
use crucible_http::{Chunk, Chunks, Http, Lookups, Poison};
use tokio::runtime::Handle;
use tokio::time::timeout;

use crate::backends::Row;
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
    pub version: &'static str,
    /// The executable crucible runs.
    pub executable: PathBuf,
    /// Whether it was already in place, so nothing was downloaded.
    pub kept: bool,
}

/// Why an install put nothing in place.
#[derive(Debug, thiserror::Error)]
pub enum Uninstalled {
    /// This build pins no release of the agent for this platform.
    #[error(
        "this build of crucible installs no release of {agent} on {} for {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )]
    Unpinned {
        /// The agent, as the configuration names it.
        agent: &'static str,
    },
    /// No client could be built to download it with.
    #[error("no HTTP client could be built to download the release")]
    Unconnected,
    /// The download could not be kept.
    #[error("the download could not be written under {}", .0.display())]
    Unwritten(PathBuf),
    /// The download did not arrive whole.
    #[error("the release did not download whole from {0}")]
    Download(&'static str),
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
/// `home`, where this build pins a release of it for this platform.
#[must_use]
pub fn executable(home: &Path, row: &Row) -> Option<PathBuf> {
    (row.pinned)().map(|pin| pin.executable(&root(home, row)))
}

/// Installs `row`'s agent under crucible's `home`, telling `progress` how many
/// of how many bytes have arrived as they do, then shuts the services it ran
/// on down.
///
/// # Errors
///
/// The first half is [`Uninstalled`] where nothing was put in place. The
/// second is [`Unfinished`] where the services' shutdown ran out of its
/// bounds, whatever the first says.
pub fn install(
    row: &Row,
    home: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> (Result<Installed, Uninstalled>, Result<(), Unfinished>) {
    let Some(pin) = (row.pinned)() else {
        let agent = row.driver.name();
        return (Err(Uninstalled::Unpinned { agent }), Ok(()));
    };
    let root = root(home, row);
    if pin.installed(&root) {
        let executable = pin.executable(&root);
        return (
            Ok(Installed {
                version: pin.version,
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
        let executable = downloaded(&runtime, &http, pin, &root, progress)?;
        Ok(Installed {
            version: pin.version,
            executable,
            kept: false,
        })
    })
}

/// Downloads `pin`'s archive through `http` into a file of its own under
/// `root`, unpacks it there and removes it, giving the executable's path.
fn downloaded(
    runtime: &Handle,
    http: &Http,
    pin: &Pin,
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
        return Err(Uninstalled::Download(pin.url));
    }
    Ok(unpack(pin, &download.0, root)?)
}

/// Writes the body at `pin`'s address into `file`; `None` where it did not
/// arrive whole within the archive's pinned size.
async fn fetched(
    http: &Http,
    pin: &Pin,
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

#[cfg(test)]
mod tests;
