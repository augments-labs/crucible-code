//! The catalog of reviewed agents, fetched from the one place it is published.
//!
//! The index and its detached signature are two files under [`Source::base`],
//! `index.json` and `index.json.minisig`. Both are hostile until the signature
//! verifies, so each is read within a ceiling before a byte of it is kept: the
//! index within [`INDEX_BYTES`], the signature within [`SIGNATURE_BYTES`], and
//! both together within thirty seconds. A body cut by a ceiling, a deadline or a
//! closed connection is a failure and never a shorter catalog.
//!
//! The client is limited to the host the catalog is published on and the hosts
//! in [`Source::hosts`], and a request to any other is kept back before it is
//! looked up or dialled. GitHub answers the address of a release asset with a
//! redirect to its asset storage, which the client does not follow for itself:
//! it follows one by hand, to an address over the scheme it asked with, naming
//! no user information and a host that is exactly one of [`Source::hosts`],
//! and at most four times. Any other redirect is refused, and nothing is
//! sent to where it pointed.
//!
//! Two things are done with what comes back. [`fetch`] only believes it:
//! verified under [`Source::keys`], at a serial no lower than the compiled
//! snapshot's, and not expired past the grace the reader allows. It touches no
//! state, so a one-off check cannot change what a machine believes.
//! [`refresh`] keeps it: the same bytes go to [`Trust::take_in`], which holds
//! the trust state, and which may wait for another process's change to it for
//! some seconds, so it is never called on a runtime worker. A catalog that is
//! not believed leaves what was believed as it was.
//!
//! Who asked is a [`Trigger`]. Opening `/agents`, `/login`'s agent choice or
//! running a `crucible agents` command is a request, and fetches whatever
//! `updates.check` says. The fetch beside the release check is [`Background`]:
//! it is armed only where that check is, after the first frame is on the
//! screen, and asks again no sooner than the release check's own interval
//! after the trust state was last written.
//!
//! [`Refresher`] is the owner of that background fetch for a run, shaped like
//! the release check's: it runs on the application's runtime, holds one fetch
//! at a time, and is stopped and joined within a bound when the run ends.
//!
//! [`Background`]: Trigger::Background

use std::fmt;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use crucible_external_harnesses::catalog::{
    Catalog, CatalogError, Held, INDEX_BYTES, KEYS, Keys, SIGNATURE_BYTES, SNAPSHOT, Trust,
    TrustError,
};
use crucible_http::{
    BodyError, Hold, Http, HttpError, Lookups, Origin, PlainLookups, ProxyEnv, Tls, read_limited,
};
use crucible_runtime::Cancel;
use crucible_update::ASK_AFTER;
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

/// The file in the state directory that the trust state is kept in, which
/// [`Trust`] names and this module reads the age of.
const TRUST_FILE: &str = "agent-catalog/trust.json";

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
    /// again no sooner than the release check's interval after the trust state
    /// was last written.
    Background,
}

/// Where the catalog is, who signed it and which hosts a redirect may reach.
#[derive(Debug, Clone)]
pub struct Source {
    /// The address `index.json` and `index.json.minisig` are under.
    pub base: String,
    /// The keys the signature must verify under.
    pub keys: Keys,
    /// The hosts a redirect may lead to, each matched exactly. The host of
    /// [`Source::base`] is reached whether or not it is among them.
    pub hosts: &'static [&'static str],
}

impl Source {
    /// The published catalog, under the keys compiled into this build.
    #[must_use]
    pub fn published() -> Self {
        Self {
            base: PUBLISHED.to_owned(),
            keys: KEYS,
            hosts: HOSTS,
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
}

/// The two files, as they came.
struct Downloaded {
    index: Vec<u8>,
    signature: Vec<u8>,
}

/// The catalog at `source`, believed: verified under its keys, no lower in
/// serial than the compiled snapshot, and not expired past the grace allowed.
///
/// It reads and writes no state; [`refresh`] is what keeps a catalog. `trigger`
/// says who asked, and changes nothing about how the catalog is fetched.
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
/// A [`Trigger::Background`] request does nothing while the trust state was
/// written less than the release check's interval ago. Nothing a catalog that
/// is not believed carries reaches the state, and a catalog at the serial
/// already held leaves it as it was.
///
/// # Errors
///
/// [`FetchError`] naming the first thing that stopped it.
pub async fn refresh(state: &Path, source: &Source, trigger: Trigger) -> Result<(), FetchError> {
    if trigger == Trigger::Background && fresh(state, SystemTime::now()) {
        return Ok(());
    }
    let Downloaded { index, signature } = download(source, LIFETIME).await?;
    keep(state.to_owned(), index, signature, source.keys).await
}

/// The directory under `home` that the trust state is kept in.
#[must_use]
pub fn state_in(home: &Path) -> PathBuf {
    home.join("state")
}

/// Whether the trust state under `state` was written less than the release
/// check's interval before `now`. A state not there, or written in the future
/// by a clock that ran ahead, is not fresh: a revocation must not wait on it.
fn fresh(state: &Path, now: SystemTime) -> bool {
    std::fs::metadata(state.join(TRUST_FILE))
        .and_then(|about| about.modified())
        .ok()
        .and_then(|kept| now.duration_since(kept).ok())
        .is_some_and(|since| since < ASK_AFTER)
}

/// Both files of the catalog at `source`, within `lifetime` altogether.
async fn download(source: &Source, lifetime: Duration) -> Result<Downloaded, FetchError> {
    let origin = Origin::of(&source.base).ok_or(FetchError::Address)?;
    let reaching = Reaching {
        client: client(Limited::to(origin.host(), source.hosts))?,
        scheme: origin.scheme(),
        listed: source.hosts,
        within: lifetime,
    };
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

/// A client that reaches `limited` and nowhere else, over the proxy settings of
/// this process.
fn client(limited: Limited) -> Result<Http, FetchError> {
    let tls = Tls::new().map_err(|_| FetchError::Tls)?;
    let plain = lookups();
    Ok(
        Http::new(&tls, plain.clone().into(), plain, ProxyEnv::capture())
            .holding(Arc::new(limited)),
    )
}

/// The one place the catalog's hostname lookups run in, whoever asked and
/// however many fetches there are: one lookup at a time, on a blocking thread
/// of the runtime.
fn lookups() -> PlainLookups {
    static PLACE: OnceLock<PlainLookups> = OnceLock::new();
    PLACE
        .get_or_init(|| Lookups::plain(NonZeroUsize::MIN))
        .clone()
}

/// What a file is fetched with and under: the client, the scheme every address
/// is asked over, the hosts a redirect may lead to, and the time a body is
/// given.
struct Reaching<'a> {
    client: Http,
    scheme: &'a str,
    listed: &'a [&'a str],
    within: Duration,
}

impl Reaching<'_> {
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
/// that names no user information and whose host is exactly one of `listed`.
///
/// The host is the one the client will dial, read by the parser the client
/// parses the address with, so what is compared is what is reached. Nothing
/// that is not plain ASCII without controls or spaces is followed.
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
    listed
        .iter()
        .filter_map(|host| listed_host(host))
        .any(|host| host == origin.host())
        .then(|| location.to_owned())
}

/// `host` as an [`Origin`] spells a host: in lower case and without a trailing
/// dot. Nothing for a host that is empty or a pattern.
fn listed_host(host: &str) -> Option<String> {
    let host = host.to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host);
    (!host.is_empty() && !host.contains('*')).then(|| host.to_owned())
}

/// The hosts a client may reach.
#[derive(Debug)]
struct Limited(Vec<String>);

impl Limited {
    /// The hosts `listed` and `host` itself.
    fn to(host: &str, listed: &[&str]) -> Self {
        Self(
            std::iter::once(host)
                .chain(listed.iter().copied())
                .filter_map(listed_host)
                .collect(),
        )
    }
}

impl Hold for Limited {
    fn held(&self, origin: &Origin) -> Option<Box<str>> {
        (!self.0.iter().any(|host| host == origin.host()))
            .then(|| "a host the catalog is not published from".into())
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
    let (done, answer) = oneshot::channel();
    std::thread::Builder::new()
        .name("catalog-trust".into())
        .spawn(move || {
            let now = SystemTime::now();
            let kept = Trust::load(&state, now).take_in(&index, &signature, &keys, now);
            let _ = done.send(kept);
        })
        .map_err(FetchError::Thread)?;
    answer.await.map_err(|_| FetchError::Abandoned)??;
    Ok(())
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
    /// running is left alone. A fetch that fails is not reported: the next
    /// trigger asks again.
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
    /// state that has begun is not undone: it finishes on its own thread, and
    /// leaves the state whole.
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
