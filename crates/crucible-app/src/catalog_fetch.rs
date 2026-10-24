//! The catalog of reviewed agents, fetched from the one place it is published.
//!
//! The index and its detached signature are two files under [`Source::base`],
//! `index.json` and `index.json.minisig`. Both are hostile until the signature
//! verifies, so each is read within a ceiling before a byte of it is kept: the
//! index within [`INDEX_BYTES`], the signature within [`SIGNATURE_BYTES`], and
//! both together within thirty seconds. A body cut by a ceiling, a deadline or
//! a closed connection is a failure and never a shorter catalog.
//!
//! The client is limited to the place the catalog is published at and the
//! places in [`Source::hosts`], each a host and a port, and a request to any
//! other is kept back before it is looked up or dialled. GitHub answers the
//! address of a release asset with a redirect to its asset storage, which the
//! client does not follow for itself: it follows one by hand, to an address
//! over the scheme it asked with, naming no user information and a host and
//! port that are exactly one of [`Source::hosts`], and at most four times. Any
//! other redirect is refused, and nothing is sent to where it pointed. The
//! proxy settings requests go out under are [`Source::proxy`]'s: this module
//! reads nothing from the environment, the binary hands it what it read.
//!
//! Two things are done with the catalog that comes back. [`fetch`] only
//! believes it: verified under [`Source::keys`], at a serial no lower than the
//! compiled snapshot's, and not expired past the grace the reader allows. It
//! touches no state, so a one-off check cannot change what a machine believes.
//! [`refresh`] keeps it: the same bytes go to [`Trust::take_in`], which holds
//! the trust state, and which may wait for another process's change to it for
//! some seconds, so it is never called on a runtime worker. A catalog that is
//! not believed leaves what was believed as it was. A file the catalog lists
//! as an asset is fetched by [`asset`], from the catalog under the name of its
//! SHA-256, within the size it is listed with and no more than the reader
//! allows an asset, and is believed only when its size and its SHA-256 are the
//! listed ones.
//!
//! Who asked is a [`Trigger`]. Opening `/agents`, `/login`'s agent choice or
//! running a `crucible agents` command is a request, and fetches whatever
//! `updates.check` says. The fetch beside the release check is [`Background`]:
//! it is armed only where that check is, after the first frame is on the
//! screen, and asks again no sooner than the release check's own interval
//! after the last time it asked, whether that attempt was believed or not.
//! That time is a stamp kept beside the trust state and written before the
//! request is made, so an attempt that fails, is cut short or is never
//! believed (no key is compiled in yet) is not made again at the next start;
//! a request is never affected by it.
//!
//! [`Refresher`] is the owner of that background fetch for a run, shaped like
//! the release check's: it runs on the application's runtime, holds one fetch
//! at a time, and is stopped and joined within a bound when the run ends.
//!
//! [`Background`]: Trigger::Background

use std::fmt;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crucible_external_harnesses::catalog::{
    ASSET_BYTES, AssetRef, Catalog, CatalogError, Held, INDEX_BYTES, KEYS, Keys, SIGNATURE_BYTES,
    SNAPSHOT, Trust, TrustError,
};
use crucible_http::{
    BodyError, Hold, Http, HttpError, Lookups, Origin, PlainLookups, ProxyEnv, Tls, read_limited,
};
use crucible_runtime::Cancel;
use crucible_update::ASK_AFTER;
use sha2::{Digest as _, Sha256};
use tokio::runtime::Handle;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// Where the catalog is published: the base of the rolling release's assets,
/// under which `index.json` and `index.json.minisig` are.
pub const PUBLISHED: &str =
    "https://github.com/augments-labs/crucible-agent-catalog/releases/download/catalog/";

/// The hosts a fetch of the catalog follows a redirect to: GitHub, and the
/// hosts it serves release assets from.
pub const HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "github-releases.githubusercontent.com",
];

/// How long a fetch of the catalog, both files and every redirect, is given.
const LIFETIME: Duration = Duration::from_secs(30);

/// The most redirects followed for one file.
const HOPS: usize = 4;

/// The directory under the state directory that the trust state is kept in,
/// which [`Trust`] names and the time of the last background attempt is kept
/// beside its file in.
const DIRECTORY: &str = "agent-catalog";

/// The file in it that holds the time of the last background attempt, in whole
/// seconds since the Unix epoch.
const ASKED: &str = "asked";

/// The most of that file that is read.
const ASKED_BYTES: u64 = 32;

/// How many hostname lookups the catalog's client has in flight at once.
///
/// The runtime's blocking threads are counted in [`crate::services`], where
/// this is one more owner and the count is checked to fit.
pub(crate) const LOOKUPS: NonZeroUsize = NonZeroUsize::MIN;

/// The catalog's index, under the base.
const INDEX: &str = "index.json";

/// Its detached signature, under the base.
const SIGNATURE: &str = "index.json.minisig";

/// How long a stopped refresher waits between looks at whether its fetch has
/// ended.
const LOOKING: Duration = Duration::from_millis(10);

/// Who asked for the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A person: `/agents` opening, `/login`'s agent choice opening, or a
    /// `crucible agents` command. It asks whatever `updates.check` says.
    Opening,
    /// The fetch beside the release check, armed only where that is. It asks
    /// again no sooner than the release check's interval after the last time
    /// it asked, whether that attempt was believed or not.
    Background,
}

/// Where the catalog is, who signed it and which hosts a redirect may reach.
#[derive(Debug, Clone)]
pub struct Source {
    /// The address `index.json` and `index.json.minisig` are under.
    pub base: String,
    /// The keys the signature must verify under.
    pub keys: Keys,
    /// The places a redirect may lead to, each matched exactly by host and by
    /// port: a name alone is that host on the scheme's own port, and
    /// `name:port` is that host on that port. The host and port of
    /// [`Source::base`] are reached whether or not they are among them.
    pub hosts: &'static [&'static str],
    /// The proxy settings requests go out under.
    pub proxy: ProxyEnv,
}

impl Source {
    /// The published catalog, under the keys compiled into this build, with
    /// the proxy settings `from` names.
    #[must_use]
    pub fn published(from: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            base: PUBLISHED.to_owned(),
            keys: KEYS,
            hosts: HOSTS,
            proxy: ProxyEnv::read(from),
        }
    }

    /// The address of the file `name` under the base.
    fn address(&self, name: &str) -> String {
        format!("{}/{name}", self.base.trim_end_matches('/'))
    }
}

/// Why the catalog was not fetched, believed or kept.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// The base address is not an `http` or `https` address with a host.
    #[error("the catalog's address is not one crucible can fetch from")]
    Address,
    /// A secure connection could not be set up.
    #[error("a secure connection could not be set up")]
    Tls,
    /// A request failed, or was kept back from a host it may not reach.
    #[error(transparent)]
    Request(#[from] HttpError),
    /// The host answered with something other than the file.
    #[error("the catalog host answered {0}")]
    Status(u16),
    /// The host redirected somewhere the fetch does not follow.
    #[error("the catalog host redirected somewhere crucible does not follow")]
    Redirect,
    /// The host redirected more times than the fetch follows.
    #[error("the catalog host redirected more than {HOPS} times")]
    Redirects,
    /// A file was larger than its ceiling, or could not be read to its end.
    #[error(transparent)]
    Body(#[from] BodyError),
    /// The fetch outlived its bound.
    #[error("the catalog was not fetched within {} seconds", LIFETIME.as_secs())]
    Late,
    /// The catalog was not believed.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// The trust state did not take the catalog in.
    #[error(transparent)]
    Trust(#[from] TrustError),
    /// The thread the trust state is changed on could not be started.
    #[error("the catalog could not be kept: {0}")]
    Thread(io::Error),
    /// That thread ended without an answer.
    #[error("the catalog could not be kept: the work stopped without an answer")]
    Abandoned,
    /// The time of the attempt could not be recorded, so none was made.
    #[error("the time of the attempt could not be recorded: {0}")]
    Stamp(io::Error),
    /// The asset's hash is not a SHA-256 in lower case hexadecimal.
    #[error("the asset is not named by a SHA-256")]
    AssetName,
    /// The asset is not the size the catalog lists.
    #[error("the asset is not the size the catalog lists")]
    AssetSize,
    /// The asset's SHA-256 is not the one the catalog lists.
    #[error("the asset is not the one the catalog lists")]
    AssetHash,
}

/// The two files, as they came.
struct Downloaded {
    index: Vec<u8>,
    signature: Vec<u8>,
}

/// The catalog at `source`, believed: verified under its keys, no lower in
/// serial than the compiled snapshot, and not expired past the grace allowed.
///
/// It reads and writes no state, and checks nothing against what this machine
/// already believes: not the serial it holds, and not a revocation it has taken
/// in. What it returns is the catalog as the network gave it, for a one-off
/// check, and nothing is installed from it. [`refresh`] is what keeps a
/// catalog, and what the install door asks, then reading what is offered from
/// the trust state. `trigger` says who asked, and changes nothing about how the
/// catalog is fetched.
///
/// # Errors
///
/// [`FetchError`] naming the first thing that stopped it.
pub async fn fetch(source: &Source, trigger: Trigger) -> Result<Catalog, FetchError> {
    let _ = trigger;
    let Downloaded { index, signature } = download(source, LIFETIME).await?;
    Ok(Catalog::verify(
        &index,
        &signature,
        &source.keys,
        Held(SNAPSHOT.serial()),
        SystemTime::now(),
    )?)
}

/// Fetches the catalog at `source` and has the trust state kept under `state`
/// take it in.
///
/// A [`Trigger::Background`] request is made no sooner than the release
/// check's interval after the last one, whether that one was believed or not,
/// and records that it is being made before it is. Nothing a catalog that is
/// not believed carries reaches the trust state, and a catalog at the serial
/// already held leaves it as it was.
///
/// # Errors
///
/// [`FetchError`] naming the first thing that stopped it.
pub async fn refresh(state: &Path, source: &Source, trigger: Trigger) -> Result<(), FetchError> {
    if trigger == Trigger::Background {
        let state = state.to_owned();
        if !blocking(move || due(&state, SystemTime::now())).await?? {
            return Ok(());
        }
    }
    let Downloaded { index, signature } = download(source, LIFETIME).await?;
    keep(state.to_owned(), index, signature, source.keys).await
}

/// One asset the catalog lists, fetched from the catalog by the SHA-256 it is
/// named by and held to the size and the SHA-256 it is listed with.
///
/// The asset is the file `asset.sha256` under [`Source::base`], fetched
/// through the same client, hosts and redirect rule as the catalog, within
/// thirty seconds and no more bytes than it is listed to have, which is never
/// more than [`ASSET_BYTES`]. A name that is not a SHA-256 and a listing past
/// that ceiling are refused before a request is made; a body longer or shorter
/// than listed, or hashing to something else, is refused whole.
///
/// # Errors
///
/// [`FetchError::AssetName`], [`FetchError::AssetSize`] or
/// [`FetchError::AssetHash`] for what the asset is not, otherwise
/// [`FetchError`] naming the first thing that stopped it.
pub async fn asset(source: &Source, asset: &AssetRef) -> Result<Vec<u8>, FetchError> {
    let named = asset.sha256.len() == 64
        && asset
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !named {
        return Err(FetchError::AssetName);
    }
    if asset.size > ASSET_BYTES {
        return Err(FetchError::AssetSize);
    }
    let listed = usize::try_from(asset.size).map_err(|_| FetchError::AssetSize)?;
    let origin = Origin::of(&source.base).ok_or(FetchError::Address)?;
    let reaching = Reaching::to(source, &origin, LIFETIME)?;
    let bytes = timeout(
        LIFETIME,
        reaching.get(source.address(&asset.sha256), listed),
    )
    .await
    .map_err(|_| FetchError::Late)?
    .map_err(|error| match error {
        FetchError::Body(BodyError::TooLarge) => FetchError::AssetSize,
        other => other,
    })?;
    if bytes.len() != listed {
        return Err(FetchError::AssetSize);
    }
    if hex(&Sha256::digest(&bytes)) != *asset.sha256 {
        return Err(FetchError::AssetHash);
    }
    Ok(bytes)
}

/// `bytes` as lower case hexadecimal.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// The directory under `home` that the trust state is kept in.
#[must_use]
pub fn state_in(home: &Path) -> PathBuf {
    home.join("state")
}

/// Whether a background attempt is due at `now`, and if it is, the time it is
/// made at is recorded before it is.
///
/// One is due when none was recorded, when what was recorded cannot be read,
/// when it is dated after `now` (a clock that ran ahead must not hold a
/// revocation back for as long as it ran ahead by), and when the release
/// check's interval has passed since. The record is kept whatever the attempt
/// comes to, so a fetch that fails or is not believed is not made again at the
/// next start.
fn due(state: &Path, now: SystemTime) -> Result<bool, FetchError> {
    let recent = asked_at(state)
        .and_then(|asked| now.duration_since(asked).ok())
        .is_some_and(|since| since < ASK_AFTER);
    if recent {
        return Ok(false);
    }
    record(state, now).map_err(FetchError::Stamp)?;
    Ok(true)
}

/// The time recorded under `state` for the last background attempt, if one can
/// be read within [`ASKED_BYTES`].
fn asked_at(state: &Path) -> Option<SystemTime> {
    let mut text = String::new();
    crucible_privacy::open_read(&state.join(DIRECTORY).join(ASKED))
        .ok()?
        .take(ASKED_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    if u64::try_from(text.len()).ok()? > ASKED_BYTES {
        return None;
    }
    UNIX_EPOCH.checked_add(Duration::from_secs(text.trim().parse().ok()?))
}

/// Records `now` under `state`, in a file only the user can read, replaced
/// whole through a draft of its own beside it.
fn record(state: &Path, now: SystemTime) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = state.join(DIRECTORY);
    crucible_privacy::directory(&directory).map_err(crucible_privacy::PrivacyError::into_io)?;
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    for _ in 0..16 {
        let draft = directory.join(format!(
            ".asked.{}.{}.writing",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = match crucible_privacy::create_write(&draft) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into_io()),
        };
        let landed = writeln!(file, "{seconds}").and_then(|()| {
            drop(file);
            crucible_privacy::replace(&draft, &directory.join(ASKED))
                .map_err(crucible_privacy::PrivacyError::into_io)
        });
        if landed.is_err() {
            let _ = fs::remove_file(&draft);
        }
        return landed;
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free name for the attempt's draft",
    ))
}

/// Both files of the catalog at `source`, within `lifetime` altogether.
async fn download(source: &Source, lifetime: Duration) -> Result<Downloaded, FetchError> {
    let origin = Origin::of(&source.base).ok_or(FetchError::Address)?;
    let reaching = Reaching::to(source, &origin, lifetime)?;
    let fetching = async {
        let index = reaching.get(source.address(INDEX), INDEX_BYTES).await?;
        let signature = reaching
            .get(source.address(SIGNATURE), SIGNATURE_BYTES)
            .await?;
        Ok(Downloaded { index, signature })
    };
    timeout(lifetime, fetching)
        .await
        .map_err(|_| FetchError::Late)?
}

/// A client that reaches `limited` and nowhere else, over the proxy settings
/// it is given.
fn client(limited: Limited, proxy: ProxyEnv) -> Result<Http, FetchError> {
    let tls = Tls::new().map_err(|_| FetchError::Tls)?;
    let plain = lookups();
    Ok(Http::new(&tls, plain.clone().into(), plain, proxy).holding(Arc::new(limited)))
}

/// The one place the catalog's hostname lookups run in, whoever asked and
/// however many fetches there are: [`LOOKUPS`] lookups at a time, on blocking
/// threads of the runtime.
fn lookups() -> PlainLookups {
    static PLACE: OnceLock<PlainLookups> = OnceLock::new();
    PLACE.get_or_init(|| Lookups::plain(LOOKUPS)).clone()
}

/// What a file is fetched with and under: the client, the scheme every address
/// is asked over, the places a redirect may lead to, and the time a body is
/// given.
struct Reaching<'a> {
    client: Http,
    scheme: &'a str,
    listed: &'a [&'a str],
    within: Duration,
}

impl<'a> Reaching<'a> {
    /// What `source` is fetched with, its base at `origin`, giving a body
    /// `within`.
    fn to(source: &'a Source, origin: &'a Origin, within: Duration) -> Result<Self, FetchError> {
        Ok(Self {
            client: client(Limited::to(origin, source.hosts), source.proxy.clone())?,
            scheme: origin.scheme(),
            listed: source.hosts,
            within,
        })
    }

    /// The body at `address`, read within `ceiling` bytes, after following at
    /// most [`HOPS`] redirects.
    async fn get(&self, address: String, ceiling: usize) -> Result<Vec<u8>, FetchError> {
        let mut address = address;
        for _ in 0..=HOPS {
            let response = self.client.get(&address).await?;
            let status = response.status().as_u16();
            match status {
                200 => {
                    return Ok(read_limited(response.into_body(), ceiling, self.within).await?);
                }
                301 | 302 | 303 | 307 | 308 => {
                    address = response
                        .headers()
                        .get("location")
                        .and_then(|location| location.to_str().ok())
                        .and_then(|location| redirect_to(location, self.scheme, self.listed))
                        .ok_or(FetchError::Redirect)?;
                }
                other => return Err(FetchError::Status(other)),
            }
        }
        Err(FetchError::Redirects)
    }
}

/// `location`, if a redirect may follow it: an absolute address over `scheme`
/// that names no user information and whose host and port are exactly one of
/// the places `listed` names.
///
/// The host and port are the ones the client will dial, read by the parser the
/// client parses the address with, so what is compared is what is reached.
/// Nothing that is not plain ASCII without controls or spaces is followed.
fn redirect_to(location: &str, scheme: &str, listed: &[&str]) -> Option<String> {
    if !location.is_ascii()
        || location
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return None;
    }
    let origin = Origin::of(location)?;
    if !origin.scheme().eq_ignore_ascii_case(scheme) {
        return None;
    }
    let (_, rest) = location.split_once("://")?;
    if rest.split(['/', '?', '#']).next()?.contains('@') {
        return None;
    }
    let default = default_port(scheme);
    listed
        .iter()
        .filter_map(|entry| place(entry, default))
        .any(|(host, port)| host == origin.host() && port == origin.port())
        .then(|| location.to_owned())
}

/// The port an address over `scheme` goes to when it names none.
fn default_port(scheme: &str) -> u16 {
    if scheme.eq_ignore_ascii_case("http") {
        80
    } else {
        443
    }
}

/// The host and port a listed `entry` names, with `default` where it names no
/// port: `entry` as an [`Origin`] spells a host, in lower case and without a
/// trailing dot. Nothing for one that is empty, a pattern or not a name.
fn place(entry: &str, default: u16) -> Option<(String, u16)> {
    let entry = entry.to_ascii_lowercase();
    let (host, port) = match entry.split_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok()?),
        None => (entry.as_str(), default),
    };
    let host = host.strip_suffix('.').unwrap_or(host);
    (!host.is_empty() && !host.contains(['*', ':', '[', ']'])).then(|| (host.to_owned(), port))
}

/// The places a client may reach.
#[derive(Debug)]
struct Limited(Vec<(String, u16)>);

impl Limited {
    /// The places `listed` and the one `base` is at.
    fn to(base: &Origin, listed: &[&str]) -> Self {
        let default = default_port(base.scheme());
        Self(
            std::iter::once((base.host().to_owned(), base.port()))
                .chain(listed.iter().filter_map(|entry| place(entry, default)))
                .collect(),
        )
    }
}

impl Hold for Limited {
    fn held(&self, origin: &Origin) -> Option<Box<str>> {
        (!self
            .0
            .iter()
            .any(|(host, port)| host == origin.host() && *port == origin.port()))
        .then(|| "a place the catalog is not published from".into())
    }
}

/// Has the trust state kept under `state` take in the catalog, on a thread of
/// its own.
///
/// The change waits up to five seconds for another process's, which a runtime
/// worker must not do and the runtime's blocking threads, counted to the last
/// by [`crate::services`], should not be asked for.
async fn keep(
    state: PathBuf,
    index: Vec<u8>,
    signature: Vec<u8>,
    keys: Keys,
) -> Result<(), FetchError> {
    blocking(move || {
        let now = SystemTime::now();
        Trust::load(&state, now).take_in(&index, &signature, &keys, now)
    })
    .await?
    .map_err(FetchError::Trust)
}

/// What `work`, which waits on the disk or on another process, comes to on a
/// thread of its own.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, FetchError> {
    let (done, answer) = oneshot::channel();
    std::thread::Builder::new()
        .name("catalog-state".into())
        .spawn(move || {
            let _ = done.send(work());
        })
        .map_err(FetchError::Thread)?;
    answer.await.map_err(|_| FetchError::Abandoned)
}

/// The catalog fetch that a run owns for as long as it lasts, beside its
/// release check.
///
/// Cheap to make and inert until [`Refresher::runs_on`] and
/// [`Refresher::refresh`] are called. The value can be cloned; clones share the
/// one task that may be in flight and the one cancellation.
#[derive(Clone)]
pub struct Refresher(Arc<Inner>);

struct Inner {
    runtime: OnceLock<Handle>,
    cancel: Cancel,
    running: Mutex<Option<JoinHandle<()>>>,
}

/// The catalog fetch did not stop within the bound and was aborted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the catalog fetch had not stopped within {} ms and was aborted", waited.as_millis())]
pub struct Unjoined {
    waited: Duration,
}

impl fmt::Debug for Refresher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Refresher")
            .field("runtime", &self.0.runtime.get().is_some())
            .field(
                "running",
                &lock(&self.0.running)
                    .as_ref()
                    .is_some_and(|task| !task.is_finished()),
            )
            .finish()
    }
}

impl Default for Refresher {
    fn default() -> Self {
        Self::new()
    }
}

impl Refresher {
    /// An owner with no runtime and no fetch started.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            runtime: OnceLock::new(),
            cancel: Cancel::new(),
            running: Mutex::new(None),
        }))
    }

    /// Gives the owner the application runtime its fetch may run on. The first
    /// handle is kept.
    pub fn runs_on(&self, runtime: Handle) {
        let _ = self.0.runtime.set(runtime);
    }

    /// Fetches the catalog at `source` for the trust state under `state`, as
    /// [`Trigger::Background`] asks for it.
    ///
    /// The fetch runs on the runtime given to [`Self::runs_on`], and nothing
    /// is done before one is given. One is held at a time; a fetch already
    /// running is left alone. A fetch that fails is not reported. The next
    /// background trigger waits out the release check's interval from this
    /// attempt, and an opening trigger asks again at once.
    pub fn refresh(&self, state: &Path, source: Source) {
        let Some(runtime) = self.0.runtime.get() else {
            return;
        };
        let mut running = lock(&self.0.running);
        if running.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        running.take();
        let state = state.to_owned();
        let cancel = self.0.cancel.clone();
        *running = Some(runtime.spawn(async move {
            let _ = cancel
                .race(refresh(&state, &source, Trigger::Background))
                .await;
        }));
    }

    /// Joins the fetch within `bound`, then aborts one that did not stop.
    ///
    /// The owner first raises its cancellation, which drops a fetch still
    /// waiting on the network at its next await point. A change to the trust
    /// state that has begun is not undone, and leaves the state whole whether
    /// or not it finishes before the process ends.
    ///
    /// # Errors
    ///
    /// [`Unjoined`] where the fetch had not stopped within `bound`.
    pub fn join_within(&self, bound: Duration) -> Result<(), Unjoined> {
        self.0.cancel.request();
        let until = Instant::now().checked_add(bound);
        loop {
            let mut running = lock(&self.0.running);
            let Some(task) = running.as_ref() else {
                return Ok(());
            };
            if task.is_finished() {
                running.take();
                return Ok(());
            }
            let left = until.map_or(bound, |until| {
                until.saturating_duration_since(Instant::now())
            });
            if left.is_zero() {
                if let Some(task) = running.take() {
                    task.abort();
                }
                return Err(Unjoined { waited: bound });
            }
            drop(running);
            std::thread::sleep(left.min(LOOKING));
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.request();
        if let Some(task) = lock(&self.running).take() {
            task.abort();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
