//! Whether there is a release newer than the one running.
//!
//! The answer drawn under the welcome comes off the disk. The question is put to
//! GitHub only once that frame is on the screen, on work the application owns;
//! its answer is what the next run draws. The request uses the shared HTTP
//! client's fixed release route, which owns its two non-secret headers and
//! accepts no caller header. The first run after a release is therefore quiet,
//! and startup never opens a socket for this check.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use crucible_http::{Http, Lookups, PlainLookups, ProxyEnv, Tls, read_limited};
use crucible_runtime::Cancel;
use crucible_types::later;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// How long a release check is given to stop when the run ends.
pub const SHUTDOWN: Duration = Duration::from_secs(2);

/// Where the newest release is named.
const LATEST: &str = "https://api.github.com/repos/augments-labs/crucible-code/releases/latest";

/// Where somebody goes to get it. Drawn beside the version, because crucible
/// installs by download and there is no command that would update it.
const FROM: &str = "https://github.com/augments-labs/crucible-code/releases";

/// What the answer is kept in, under crucible's own directory.
const REMEMBERED: &str = "release";

/// The most a cached version name may occupy, including its newline.
const CACHE_CEILING: u64 = 128;

/// How long an answer stands before it is worth asking again.
const ASK_AFTER: Duration = Duration::from_hours(24);

/// The absolute lifetime of release discovery, including its response body.
const CHECK_LIFETIME: Duration = Duration::from_secs(10);

/// How long a join waits for the notice that the check ended before it looks
/// again.
///
/// The notice is a hint rather than the fact. It is sent without the lock the
/// join holds while it waits, so a check that ended in the moment between the
/// join reading whether it had and its waiting for a notice has nobody left to
/// give that notice to, and a join that can only wait for one sleeps out its
/// whole bound for a check that had already stopped — a reader who quit a
/// minute ago still standing at the prompt for two seconds. A tenth of a second
/// is short against a two-second bound and long against the cost of looking.
const LOOKING: Duration = Duration::from_millis(10);

/// The most of a response that is read before giving up on it.
const CEILING: usize = 256 * 1024;

/// A release this machine has heard of and is not running.
#[derive(Debug, Clone)]
pub struct Newer {
    /// What it is called, without the `v` a tag carries.
    pub version: Box<str>,
    /// Where to get it.
    pub from: Box<str>,
}

/// The release check that a run owns for as long as it lasts.
///
/// It is cheap to make and inert until [`UpdateCrateReleaseCheck::runs_on`] and
/// [`UpdateCrateReleaseCheck::refresh`] are called. The value can be cloned;
/// clones share the TLS configuration, proxy policy, resolver owner and the
/// one task that may be in flight.
#[derive(Clone)]
pub struct UpdateCrateReleaseCheck(Arc<Inner>);

struct Inner {
    tls: Option<Tls>,
    plain: PlainLookups,
    proxy: ProxyEnv,
    client: OnceLock<Http>,
    runtime: OnceLock<Handle>,
    cancel: Cancel,
    running: Mutex<Option<JoinHandle<()>>>,
    ended: Arc<Condvar>,
}

/// The release check did not stop within the bound and was aborted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the release check had not stopped within {} ms and was aborted", waited.as_millis())]
pub struct Unjoined {
    waited: Duration,
}

impl fmt::Debug for UpdateCrateReleaseCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpdateCrateReleaseCheck")
            .field("runtime", &self.0.runtime.get().is_some())
            .field("client", &self.0.client.get().is_some())
            .field(
                "running",
                &lock(&self.0.running)
                    .as_ref()
                    .is_some_and(|task| !task.is_finished()),
            )
            .finish()
    }
}

impl Default for UpdateCrateReleaseCheck {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateCrateReleaseCheck {
    /// An owner with no runtime, task or client started yet.
    ///
    /// TLS and proxy settings are captured once here so a client built later
    /// has the same trust and environment decisions as the application's other
    /// client. A TLS configuration that cannot be built leaves the check
    /// silent, as the old client was when its setup failed.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            tls: Tls::new().ok(),
            plain: Lookups::plain(NonZeroUsize::MIN),
            proxy: ProxyEnv::capture(),
            client: OnceLock::new(),
            runtime: OnceLock::new(),
            cancel: Cancel::new(),
            running: Mutex::new(None),
            ended: Arc::new(Condvar::new()),
        }))
    }

    /// Gives the owner the application runtime its check may run on.
    ///
    /// The first handle is kept. Giving a second one is harmless and does not
    /// move work that is already owned by the run.
    pub fn runs_on(&self, runtime: Handle) {
        let _ = self.0.runtime.set(runtime);
    }

    /// Reads the answer left by an earlier check without starting any work.
    ///
    /// This is the startup operation. It reads a bounded cache buffer and uses
    /// its first line; it does not construct an HTTP client, ask for a runtime
    /// handle or open a socket. The file is opened through the owner of private
    /// local files, so what a name planted at that path is read as is not
    /// decided here.
    #[must_use]
    pub fn cached(&self, home: &Path, running: &str) -> Option<Newer> {
        newer(home, running)
    }

    /// Asks again when the cached answer is old enough.
    ///
    /// The request is started on the runtime given to [`Self::runs_on`]. Only
    /// one check is held at a time; a check already running is left alone. The
    /// answer is written through an exclusively created sibling and an atomic
    /// replacement, so a process that ends during the request leaves the old
    /// complete answer in place. Runtime workers poll application-owned spawned
    /// tasks, while a turn is polled on its calling thread; synchronous disk
    /// work on a worker can hold it and starve other runtime work, so this
    /// write is given to the runtime's blocking threads.
    pub fn refresh(&self, home: &Path) {
        self.refresh_at(home, LATEST);
    }

    fn refresh_at(&self, home: &Path, url: &str) {
        if !stale(home) {
            return;
        }

        let Some(runtime) = self.0.runtime.get() else {
            return;
        };
        let Some(http) = self.release_client().cloned() else {
            return;
        };
        let into = home.join(REMEMBERED);
        let url = url.to_owned();
        let cancel = self.0.cancel.clone();
        let ended = Arc::clone(&self.0.ended);
        let mut running = lock(&self.0.running);
        if running.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        running.take();

        *running = Some(runtime.spawn(async move {
            let version = asked_at(&http, &url, &cancel)
                .await
                .unwrap_or_else(|| known(&into));
            // The write is the one part of a check with no asynchronous form: it
            // creates a sibling, writes into it and flushes it to the disk.
            // Runtime workers poll application-owned spawned tasks, while a turn
            // is polled on its calling thread. Synchronous disk work on a worker
            // can hold it and starve other runtime work, so the runtime's
            // blocking threads are where this disk work belongs; `crucible-app`
            // names this among the owners of them.
            let _ = tokio::task::spawn_blocking(move || remember(&into, &version)).await;
            ended.notify_all();
        }));
    }

    /// Joins the check within `bound`, then aborts one that did not stop.
    ///
    /// The owner first raises its cancellation token, which drops an in-flight
    /// HTTP future at its next await point. A task still running when the bound
    /// passes is aborted and reported as [`Unjoined`]. This method is
    /// synchronous because the application services owner is taken apart after
    /// the run has returned, while its runtime is still running.
    ///
    /// Whether the check has stopped is read as often as `LOOKING` allows
    /// rather than only once the bound has passed, for the reason `LOOKING`
    /// gives.
    ///
    /// # Errors
    ///
    /// [`Unjoined`] where the check had not stopped within `bound`.
    pub fn join_within(&self, bound: Duration) -> Result<(), Unjoined> {
        self.0.cancel.request();
        let until = Instant::now().checked_add(bound);
        let mut running = lock(&self.0.running);

        loop {
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
            running = self
                .0
                .ended
                .wait_timeout(running, left.min(LOOKING))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Builds a client over this owner's TLS and proxy decisions, with the
    /// caller's target resolver.
    ///
    /// The returned client has its own pool. Passing the owner's plain lookup
    /// owner for proxy hosts keeps the release and application lookups within
    /// the same bounded, unpoisoned place without sharing a connection pool.
    #[must_use]
    pub fn client(&self, targets: Lookups) -> Option<Http> {
        let tls = self.0.tls.as_ref()?;
        Some(Http::new(
            tls,
            targets,
            self.0.plain.clone(),
            self.0.proxy.clone(),
        ))
    }

    fn release_client(&self) -> Option<&Http> {
        let tls = self.0.tls.as_ref()?;
        Some(self.0.client.get_or_init(|| {
            let targets = self.0.plain.clone().into();
            Http::new(tls, targets, self.0.plain.clone(), self.0.proxy.clone())
        }))
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

/// What this machine last heard, where that is newer than what is running.
///
/// One small read on the startup path and no network at all. A file that is not
/// there, cannot be read, or names something no newer is all the same answer:
/// nothing to say.
#[must_use]
pub fn newer(home: &Path, running: &str) -> Option<Newer> {
    let said = cached(&home.join(REMEMBERED))?;
    let version = said.lines().next()?.trim();

    later(version, running).then(|| Newer {
        version: version.into(),
        from: FROM.into(),
    })
}

/// The release already written down, or this one where nothing is.
fn known(at: &Path) -> Box<str> {
    let said = cached(at).unwrap_or_default();
    let version = said.lines().next().unwrap_or_default().trim();

    if version.is_empty() {
        return env!("CARGO_PKG_VERSION").into();
    }

    version.into()
}

/// Whether the answer on disk is old enough to be worth asking again.
fn stale(home: &Path) -> bool {
    let asked = std::fs::metadata(home.join(REMEMBERED))
        .and_then(|about| about.modified())
        .ok();

    asked.is_none_or(|asked| {
        SystemTime::now()
            .duration_since(asked)
            .is_ok_and(|since| since >= ASK_AFTER)
    })
}

/// What the release source says the newest release is called.
async fn asked_at(http: &Http, url: &str, cancel: &Cancel) -> Option<Box<str>> {
    let request = async {
        // The fixed release route owns the two non-secret headers GitHub asks
        // for and accepts no caller header. The program variant names the
        // application and its version because GitHub refuses a request that
        // names nothing, and a version in it makes an old build tellable from
        // a new one in somebody else's logs.
        let response = http.get_release(url).await.ok()?;
        if response.status().as_u16() != 200 {
            return None;
        }

        let body = read_limited(response.into_body(), CEILING, CHECK_LIFETIME)
            .await
            .ok()?;
        let said: serde_json::Value = serde_json::from_slice(&body).ok()?;
        let tag = said.get("tag_name")?.as_str()?;

        tagged(tag)
    };

    match cancel.race(timeout(CHECK_LIFETIME, request)).await {
        Some(Ok(version)) => version,
        _ => None,
    }
}

/// The one shape of release name this owner will keep, or nothing at all.
///
/// A raw tag containing CR or LF is refused before surrounding whitespace is
/// removed, and a tag made only of whitespace is no answer either. One leading
/// `v` is a tag's way of naming a release and is taken off; if that leaves no
/// name, it is refused. The remaining name must fit the cache line, while a
/// second leading `v` stays part of the name. Anything else is the same as an
/// answer that never came: the cache keeps whatever it already said, and the
/// version this machine is running stands only where the cache says nothing.
fn tagged(tag: &str) -> Option<Box<str>> {
    if tag.contains(['\n', '\r']) || tag.chars().all(char::is_whitespace) {
        return None;
    }

    let said = tag.trim();
    if said.is_empty() {
        return None;
    }
    let version = said.strip_prefix('v').unwrap_or(said);
    if version.is_empty() {
        return None;
    }
    (version.len() < usize::try_from(CACHE_CEILING).ok()?).then(|| version.into())
}

/// Writes the answer down, whole or not at all.
///
/// Through a file beside it and an atomic replace, because the write is given
/// to the runtime's blocking pool rather than a worker, and the process may end
/// under it: a half-written name read back next time would be a version that
/// was never released. The sibling is created exclusively, so a link planted
/// at its name redirects nothing.
fn remember(into: &Path, version: &str) {
    // Record `(cache path, thread id)` on entry to each `remember`, before the
    // temporary file or write begins. The test waits for the selected cache
    // before reading this record, so it proves the completed selected write was
    // not on the sole runtime worker. Compiled out of a shipped build.
    #[cfg(test)]
    lock(&WRITTEN_ON).push((into.to_owned(), std::thread::current().id()));

    let Some((beside, mut file)) = temporary(into) else {
        return;
    };
    let written = writeln!(file, "{version}")
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            crucible_privacy::replace(&beside, into)
                .map_err(crucible_privacy::PrivacyError::into_io)
        });
    if written.is_err() {
        let _ = std::fs::remove_file(beside);
    }
}

/// Reads the whole bounded buffer a cache is allowed to hold.
///
/// Through the owner of private local files, because this is one: the answer
/// lives under crucible's own directory, and the name it has there says nothing
/// about what a process able to write in that directory left at it. An ordinary
/// open would follow a link, open a second name, or block on a special file
/// before the byte ceiling below applied. The ceiling still applies, on the
/// handle that was proved to be one ordinary file. `newer` and `known` use the
/// first line; this read does not require the buffer to contain only one.
fn cached(at: &Path) -> Option<String> {
    let mut said = String::new();
    crucible_privacy::open_read(at)
        .ok()?
        .take(CACHE_CEILING + 1)
        .read_to_string(&mut said)
        .ok()?;
    (said.len() <= usize::try_from(CACHE_CEILING).ok()?).then_some(said)
}

/// Exclusively creates a sibling which no planted link can redirect.
fn temporary(into: &Path) -> Option<(PathBuf, File)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = into.parent().unwrap_or_else(|| Path::new(""));

    for _ in 0..32 {
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(
            ".release-writing.{}.{sequence}",
            std::process::id()
        ));
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => return Some((path, file)),
            Err(problem) if problem.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
    }

    None
}

/// `(cache path, thread id)` for every `remember` entry in a test build. The
/// entry is pushed before the write; see the note where [`remember`] records it.
#[cfg(test)]
static WRITTEN_ON: Mutex<Vec<(PathBuf, std::thread::ThreadId)>> = Mutex::new(Vec::new());

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    use super::*;

    /// A directory of its own, deleted with the value.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let at = std::env::temp_dir()
                .join(format!("crucible-release-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&at);
            std::fs::create_dir_all(&at).expect("a temporary directory");

            Self(at)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// How long a read of planted state is given to answer before the test says
    /// what it was still waiting for.
    ///
    /// Not a deadline anything is measured against, and not a budget the suite
    /// spends: the read is three syscalls and the channel hands the answer over
    /// the moment it arrives. How far inside the window that happens is not
    /// something this suite knows. The harness reports elapsed time to a
    /// hundredth of a second, so a passing run is recorded as `0.00s` and what
    /// the evidence bounds is the run under that reported resolution, not a
    /// millisecond. It is the point at which a read that has stopped answering
    /// gives up and names itself, which is the whole reason it is here. Thirty
    /// seconds rather than two, the same window `crucible-auth` gives a login
    /// answering through a channel: a shared runner hands a thread out when it
    /// feels like it, and a short window buys a suite that failed on a busy
    /// machine and passed on a quiet one.
    #[cfg(unix)]
    const PATIENCE: Duration = Duration::from_secs(30);

    /// Runs `read` where this test can leave it behind, and fails by name if it
    /// has not answered within [`PATIENCE`].
    ///
    /// A read that waits for a writer nobody is going to open never comes back,
    /// so the thread running it stays where it is; abandoning it is what turns
    /// the wait into a reported failure instead of a suite that never finishes.
    /// The process ends it with the test binary, and it holds no descriptor, so
    /// nothing it is waiting on is kept open by it.
    #[cfg(unix)]
    fn answered<T: Send + 'static>(what: &str, read: impl FnOnce() -> T + Send + 'static) -> T {
        let (said, inbox) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = said.send(read());
        });

        match inbox.recv_timeout(PATIENCE) {
            Ok(answered) => answered,
            Err(timed_out) => panic!("{what}: {timed_out} within {PATIENCE:?}"),
        }
    }

    /// A loopback source that answers `status` with `body`, and the request it
    /// was asked with, so a test can read what went out.
    async fn serve(status: &str, body: Vec<u8>) -> (String, Arc<Mutex<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_owned();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&asked);
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 1024];
                let read = stream.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                if let Some(part) = chunk.get(..read) {
                    request.extend_from_slice(part);
                }
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            *heard.lock().expect("the request to be kept") = request;
            let mut response = format!(
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes();
            response.extend_from_slice(&body);
            stream.write_all(&response).await.unwrap();
        });
        (format!("http://{address}/"), asked)
    }

    /// The request head the source was asked with, as text.
    fn head(asked: &Arc<Mutex<Vec<u8>>>) -> String {
        String::from_utf8(asked.lock().expect("the request to be read").clone())
            .expect("a request head of text")
    }

    fn client() -> Http {
        let tls = Tls::new().expect("the pinned TLS configuration to build");
        let plain = Lookups::plain(NonZeroUsize::MIN);
        Http::new(&tls, plain.clone().into(), plain, ProxyEnv::capture())
    }

    #[tokio::test]
    async fn a_release_answer_is_read_from_a_bounded_json_body() {
        let (url, _) = serve("200 OK", br#"{"tag_name":"v1.2.3"}"#.to_vec()).await;

        let found = asked_at(&client(), &url, &Cancel::new()).await;

        assert_eq!(found.as_deref(), Some("1.2.3"));
    }

    /// The two constants the release source needs still go out through the
    /// fixed release route, which accepts no caller header of its own.
    #[tokio::test]
    async fn the_release_question_says_which_answer_it_wants_and_who_is_asking() {
        let (url, asked) = serve("200 OK", br#"{"tag_name":"v1.2.3"}"#.to_vec()).await;

        let found = asked_at(&client(), &url, &Cancel::new()).await;

        assert_eq!(found.as_deref(), Some("1.2.3"));
        let head = head(&asked);
        assert!(head.starts_with("GET /"), "{head}");
        assert!(
            head.contains("accept: application/vnd.github+json"),
            "{head}"
        );
        assert!(
            head.contains(&format!(
                "user-agent: crucible-code/{}",
                env!("CARGO_PKG_VERSION")
            )),
            "{head}"
        );
    }

    #[tokio::test]
    async fn a_non_success_release_answer_is_silent() {
        let (url, _) = serve(
            "503 Service Unavailable",
            br#"{"tag_name":"v1.2.3"}"#.to_vec(),
        )
        .await;

        assert!(asked_at(&client(), &url, &Cancel::new()).await.is_none());
    }

    /// The cache holds one line and a version, so a tag that is not one line is
    /// not an answer to keep: a newline in it would be read back as a
    /// different, shorter name, or refused as too long.
    #[tokio::test]
    async fn a_tag_that_is_not_one_line_is_no_answer() {
        let (url, _) = serve("200 OK", br#"{"tag_name":"1.2.3\n9.9.9"}"#.to_vec()).await;

        assert!(asked_at(&client(), &url, &Cancel::new()).await.is_none());
    }

    /// A boundary newline, a name emptied by its one `v`, and a tag made only
    /// of whitespace are all outside the one-line version shape.
    #[test]
    fn tagged_refuses_raw_line_endings_empty_names_and_whitespace_only() {
        let accepted: Vec<_> = ["\n1.2.3\n", "1.2.3\r", "v", " \t "]
            .into_iter()
            .filter(|tag| tagged(tag).is_some())
            .collect();
        assert!(accepted.is_empty(), "accepted {accepted:?}");
    }

    /// Nor one longer than the line allows: the ceiling is what the next
    /// startup reads, and a name past it is a cache the next run cannot use.
    #[tokio::test]
    async fn a_tag_longer_than_the_cache_line_is_no_answer() {
        let longest = "1".repeat(usize::try_from(CACHE_CEILING).unwrap() - 1);
        let (kept, _) = serve(
            "200 OK",
            format!(r#"{{"tag_name":"{longest}"}}"#).into_bytes(),
        )
        .await;
        let (refused, _) = serve(
            "200 OK",
            format!(r#"{{"tag_name":"{longest}1"}}"#).into_bytes(),
        )
        .await;

        assert_eq!(
            asked_at(&client(), &kept, &Cancel::new()).await.as_deref(),
            Some(longest.as_str()),
            "a name exactly the cache line's length was refused"
        );
        assert!(
            asked_at(&client(), &refused, &Cancel::new())
                .await
                .is_none()
        );
    }

    /// One leading `v` is how a tag names a release; two are not, and taking
    /// every one of them off is how `v1.2.3` and `vv1.2.3` came to mean the
    /// same thing.
    #[tokio::test]
    async fn a_tag_keeps_every_leading_v_but_one() {
        let (url, _) = serve("200 OK", br#"{"tag_name":"vv1.2.3"}"#.to_vec()).await;

        assert_eq!(
            asked_at(&client(), &url, &Cancel::new()).await.as_deref(),
            Some("v1.2.3")
        );
    }

    /// And the answer refused is no answer at all, so the run writes down the
    /// version it is running rather than a name the next one cannot read.
    ///
    /// Waited for rather than joined: a join raises the owner's cancellation
    /// first, which is the same answer this is about and would pass the test
    /// whether or not the tag had been refused.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_tag_that_is_not_one_line_leaves_the_running_version_written_down() {
        let scratch = Scratch::new("hostile-tag");
        let cache = scratch.0.join(REMEMBERED);
        let standing = format!("{}\n", env!("CARGO_PKG_VERSION"));
        let (url, _) = serve("200 OK", br#"{"tag_name":"1.2.3\n9.9.9"}"#.to_vec()).await;
        let check = UpdateCrateReleaseCheck::new();
        check.runs_on(Handle::current());

        check.refresh_at(&scratch.0, &url);

        for _ in 0..2_000 {
            if std::fs::read_to_string(&cache).ok().as_deref() == Some(standing.as_str()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(
            std::fs::read_to_string(&cache).ok().as_deref(),
            Some(standing.as_str()),
            "a tag that is not one line reached the cache"
        );
    }

    #[tokio::test]
    async fn a_release_answer_over_the_body_ceiling_is_silent() {
        let padding = "x".repeat(CEILING);
        let body = format!(r#"{{"tag_name":"v1.2.3","padding":"{padding}"}}"#);
        let (url, _) = serve("200 OK", body.into_bytes()).await;

        assert!(asked_at(&client(), &url, &Cancel::new()).await.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refresh_writes_the_answer_from_owned_work() {
        let scratch = Scratch::new("refresh");
        let (url, _) = serve("200 OK", br#"{"tag_name":"v1.2.3"}"#.to_vec()).await;
        let check = UpdateCrateReleaseCheck::new();
        check.runs_on(Handle::current());

        check.refresh_at(&scratch.0, &url);

        for _ in 0..100 {
            if std::fs::read_to_string(scratch.0.join(REMEMBERED))
                .ok()
                .as_deref()
                == Some("1.2.3\n")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(check.join_within(Duration::from_secs(1)).is_ok());
        assert_eq!(
            std::fs::read_to_string(scratch.0.join(REMEMBERED)).unwrap(),
            "1.2.3\n"
        );
    }

    /// Runtime workers poll application-owned spawned tasks; synchronous disk
    /// work on one can hold it and starve other runtime work. The write is
    /// therefore given to the runtime's blocking threads. `WRITTEN_ON` records
    /// `(cache path, thread id)` on entry to each `remember`; once the selected
    /// write has completed, this oracle proves it was not on the sole runtime
    /// worker.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn the_answer_is_written_off_the_runtime_workers() {
        let scratch = Scratch::new("write-thread");
        let cache = scratch.0.join(REMEMBERED);
        let (url, _) = serve("200 OK", br#"{"tag_name":"v1.2.3"}"#.to_vec()).await;
        let check = UpdateCrateReleaseCheck::new();
        check.runs_on(Handle::current());
        let worker = tokio::spawn(async { std::thread::current().id() })
            .await
            .expect("the worker to answer");

        check.refresh_at(&scratch.0, &url);

        // Waited for rather than joined: a join raises the owner's cancellation
        // first, which answers the question itself and would leave this reading
        // a thread that wrote the fallback rather than the release.
        for _ in 0..2_000 {
            if cache.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(
            cache.exists(),
            "the check wrote nothing to read a thread from"
        );
        assert!(check.join_within(Duration::from_secs(2)).is_ok());
        let wrote = lock(&WRITTEN_ON)
            .iter()
            .find(|(at, _)| *at == cache)
            .map(|(_, thread)| *thread);
        assert!(
            wrote.is_some_and(|thread| thread != worker),
            "the cache write ran on the only worker the runtime has: {wrote:?}"
        );
    }

    #[tokio::test]
    async fn a_fresh_cache_keeps_startup_off_the_network() {
        let scratch = Scratch::new("fresh-no-network");
        remember(&scratch.0.join(REMEMBERED), "1.2.3");
        let check = UpdateCrateReleaseCheck::new();
        check.runs_on(Handle::current());

        check.refresh_at(&scratch.0, "http://127.0.0.1:1");

        assert!(check.0.client.get().is_none());
        assert!(lock(&check.0.running).is_none());
    }

    #[test]
    fn nothing_on_disk_is_nothing_to_say() {
        let scratch = Scratch::new("silent");

        assert!(newer(&scratch.0, "0.0.9").is_none());
    }

    #[test]
    fn an_oversized_cache_is_not_retained_on_the_startup_path() {
        let scratch = Scratch::new("oversized");
        std::fs::write(
            scratch.0.join(REMEMBERED),
            "9".repeat(usize::try_from(CACHE_CEILING).unwrap() + 1),
        )
        .unwrap();

        assert!(newer(&scratch.0, "0.0.9").is_none());
        assert_eq!(
            &*known(&scratch.0.join(REMEMBERED)),
            env!("CARGO_PKG_VERSION")
        );
    }

    /// A name planted where the answer is keeps a link from being followed: the
    /// read is on the startup path, and a link there would send it, and the
    /// update notice it feeds, somewhere outside crucible's own directory.
    #[cfg(unix)]
    #[test]
    fn a_cache_that_is_a_link_out_of_the_home_is_not_read() {
        use std::os::unix::fs::symlink;

        let scratch = Scratch::new("linked-cache");
        let elsewhere = Scratch::new("linked-cache-elsewhere");
        std::fs::write(elsewhere.0.join(REMEMBERED), "9.9.9\n").unwrap();
        symlink(elsewhere.0.join(REMEMBERED), scratch.0.join(REMEMBERED)).unwrap();

        assert!(
            newer(&scratch.0, "0.0.9").is_none(),
            "a link at the cache's name was read through"
        );
        assert_eq!(
            &*known(&scratch.0.join(REMEMBERED)),
            env!("CARGO_PKG_VERSION"),
            "a link at the cache's name was read through"
        );
    }

    /// The same for a second name: one file under two names is not the private
    /// state this read is for, whoever made the other name.
    #[test]
    fn a_cache_that_has_another_name_is_not_read() {
        let scratch = Scratch::new("aliased-cache");
        let cache = scratch.0.join(REMEMBERED);
        std::fs::write(&cache, "9.9.9\n").unwrap();
        std::fs::hard_link(&cache, scratch.0.join("release-again")).unwrap();

        assert!(
            newer(&scratch.0, "0.0.9").is_none(),
            "a file with another name was read as the cache"
        );
    }

    /// A directory is not one ordinary file, and a read of it is refused
    /// rather than left to fail on the first read of its bytes.
    #[test]
    fn a_cache_that_is_not_a_file_is_not_read() {
        let scratch = Scratch::new("directory-cache");
        std::fs::create_dir(scratch.0.join(REMEMBERED)).unwrap();

        assert!(newer(&scratch.0, "0.0.9").is_none());
        assert_eq!(
            &*known(&scratch.0.join(REMEMBERED)),
            env!("CARGO_PKG_VERSION")
        );
    }

    /// A pipe standing where the answer is kept waits for a writer that is not
    /// coming, and this read is on the startup path, so one planted there by a
    /// process able to write in this directory would hold the run before it drew
    /// anything. It is refused instead: the owner of private local files opens
    /// the name without waiting for a peer, and the ordinary-file proof is what
    /// then refuses what opened.
    ///
    /// Both reads run where this test can leave them, under [`PATIENCE`]. A
    /// blocked read is a syscall that nothing outside it can interrupt, so the
    /// bound is the only thing that ends one, and a bound held outside the test
    /// would leave the suite waiting instead of reporting.
    #[cfg(unix)]
    #[test]
    fn a_cache_that_is_a_pipe_is_refused_without_waiting_for_a_writer() {
        let scratch = Scratch::new("pipe-cache");
        let into = scratch.0.clone();
        let made = std::process::Command::new("mkfifo")
            .arg(into.join(REMEMBERED))
            .status()
            .expect("mkfifo is available on Unix");
        assert!(made.success());

        let (said, stood) = answered(
            "the reads of a pipe standing where the answer is kept",
            move || (newer(&into, "0.0.9"), known(&into.join(REMEMBERED))),
        );

        assert!(
            said.is_none(),
            "a pipe at the cache's name was read as a newer release"
        );
        assert_eq!(
            &*stood,
            env!("CARGO_PKG_VERSION"),
            "a pipe at the cache's name was read as the remembered release"
        );
    }

    /// An ordinary file under the home is still what this read opens: the
    /// refusals above are about what the name is, not about the path.
    #[test]
    fn an_ordinary_cache_under_the_home_is_still_read() {
        let scratch = Scratch::new("ordinary-cache");
        remember(&scratch.0.join(REMEMBERED), "0.1.0");

        let found = newer(&scratch.0, "0.0.9").expect("a newer release");

        assert_eq!(&*found.version, "0.1.0");
        assert_eq!(
            cached(&scratch.0.join(REMEMBERED)).as_deref(),
            Some("0.1.0\n")
        );
    }

    #[test]
    fn a_release_remembered_is_read_back_and_drawn() {
        let scratch = Scratch::new("heard");
        remember(&scratch.0.join(REMEMBERED), "0.1.0");

        let found = newer(&scratch.0, "0.0.9").expect("a newer release");

        assert_eq!(&*found.version, "0.1.0");
        assert_eq!(&*found.from, FROM);
    }

    #[test]
    fn the_release_being_run_is_not_an_update() {
        let scratch = Scratch::new("current");
        remember(&scratch.0.join(REMEMBERED), "0.0.9");

        assert!(newer(&scratch.0, "0.0.9").is_none());
    }

    #[test]
    fn an_answer_just_written_is_not_asked_for_again() {
        let scratch = Scratch::new("fresh");
        remember(&scratch.0.join(REMEMBERED), "0.0.9");

        assert!(!stale(&scratch.0), "a file written now is not stale");
    }

    #[test]
    fn a_release_already_known_survives_a_question_that_could_not_be_asked() {
        let scratch = Scratch::new("offline");
        remember(&scratch.0.join(REMEMBERED), "9.9.9");

        assert_eq!(&*known(&scratch.0.join(REMEMBERED)), "9.9.9");
    }

    #[test]
    fn a_machine_that_has_never_heard_an_answer_stands_on_this_release() {
        let scratch = Scratch::new("unheard");

        assert_eq!(
            &*known(&scratch.0.join(REMEMBERED)),
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn a_machine_that_has_never_asked_is_stale() {
        let scratch = Scratch::new("never");

        assert!(stale(&scratch.0));
    }

    #[test]
    fn a_cached_startup_never_builds_a_network_client() {
        let scratch = Scratch::new("startup-no-network");
        remember(&scratch.0.join(REMEMBERED), "9.9.9");
        let check = UpdateCrateReleaseCheck::new();

        let found = check
            .cached(&scratch.0, "0.0.9")
            .expect("the cached release");

        assert_eq!(&*found.version, "9.9.9");
        assert!(check.0.client.get().is_none());
        assert!(check.0.runtime.get().is_none());
    }

    /// A cache old enough to be asked again is the ordinary case on a machine
    /// that has run longer than a day, and startup still only reads: the check
    /// is armed behind the first frame, so nothing here has built a client,
    /// taken a runtime or started a task, whatever the cache says.
    #[test]
    fn an_answer_old_enough_to_ask_again_is_still_only_read_at_startup() {
        let scratch = Scratch::new("stale-no-network");
        let check = UpdateCrateReleaseCheck::new();

        let found = check.cached(&scratch.0, "0.0.9");

        assert!(found.is_none(), "nothing was heard of to be newer");
        assert!(stale(&scratch.0), "this is the state that asks again");
        assert!(check.0.client.get().is_none(), "startup built a client");
        assert!(check.0.runtime.get().is_none(), "startup took a runtime");
        assert!(
            lock(&check.0.running).is_none(),
            "startup started the check it would ask with"
        );
    }

    struct Dropped(Arc<AtomicBool>);

    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    /// A check that ends while the join is waiting for the notice is joined,
    /// and joined promptly: the notice is a hint and the joiner looks again
    /// within its bound, because a notice sent in the moment between the joiner
    /// reading whether the check had ended and its waiting for one has nobody
    /// left to give it to.
    ///
    /// The task is put in the slot directly, as the abort test below does, and
    /// says nothing when it ends — which is the whole of the case: nothing
    /// about a check that has stopped is obliged to announce it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_check_that_finishes_during_the_join_is_joined_promptly() {
        let check = UpdateCrateReleaseCheck::new();
        check.runs_on(Handle::current());
        let task = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(20)).await;
        });
        *lock(&check.0.running) = Some(task);

        let began = Instant::now();
        let joined = check.join_within(SHUTDOWN);

        assert!(
            joined.is_ok(),
            "a check that stopped was not joined: {joined:?}"
        );
        assert!(
            began.elapsed() < SHUTDOWN / 2,
            "a check that stopped during the join was slept out: {:?}",
            began.elapsed()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exit_aborts_a_stalled_check_within_its_bound() {
        let check = UpdateCrateReleaseCheck::new();
        check.runs_on(Handle::current());
        let dropped = Arc::new(AtomicBool::new(false));
        let held = Dropped(Arc::clone(&dropped));
        let task = tokio::spawn(async move {
            let _held = held;
            std::future::pending::<()>().await;
        });
        *lock(&check.0.running) = Some(task);

        let began = Instant::now();
        let result = check.join_within(Duration::from_millis(20));

        assert!(result.is_err(), "a stalled check was reported joined");
        assert!(began.elapsed() < Duration::from_secs(1));
        for _ in 0..100 {
            if dropped.load(Ordering::Acquire) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(
            dropped.load(Ordering::Acquire),
            "the bounded shutdown returned without stopping the task"
        );
    }
}
