//! The owner's own locks and keys, driven below the credentials that use it.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use super::*;

/// How long a test waits for something that is about to happen, before it
/// says it did not.
const PATIENCE: Duration = Duration::from_secs(5);

/// A runtime shaped as the application's: several workers and a clock.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// A home directory that exists while the test does.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("crucible-renewals-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A join whose bound ran out aborts what is left, which means reading the
/// rotations; a rotation being started holds the rotations and then counts
/// itself running. If the join still held the count while it waited for the
/// rotations, the two would wait for each other for ever, and shutdown would
/// hang rather than end within its bound.
///
/// The rotation being started is stood in for by this test holding the
/// rotations and then counting one more, the two steps `Renewals::rotation`
/// takes, on a thread whose answer is waited for within a bound.
#[test]
fn a_join_that_ran_out_does_not_hold_the_count_while_it_reads_the_rotations() {
    let renewals = Renewals::new();
    let in_flight = Running::begin(&renewals.0);
    let starting = lock(&renewals.0.rotations);
    let joining = {
        let renewals = renewals.clone();
        thread::spawn(move || renewals.join_within(Duration::ZERO))
    };
    // Long enough for the join to find its bound run out and reach the
    // rotations, which this thread holds.
    thread::sleep(Duration::from_millis(200));

    let (counted, heard) = mpsc::channel();
    let counting = {
        let inner = Arc::clone(&renewals.0);
        thread::spawn(move || {
            let running = Running::begin(&inner);
            let _ = counted.send(());
            drop(running);
        })
    };
    let begun = heard.recv_timeout(PATIENCE);
    drop(starting);
    let joined = joining.join().unwrap();
    counting.join().unwrap();
    drop(in_flight);

    assert!(
        begun.is_ok(),
        "a rotation being started could not count itself running while a join that ran out \
         waited for the rotations it held: the two wait for each other"
    );
    assert!(joined.is_err(), "the join found nothing running");
}

/// Two providers' credentials that come to the same scope — neither bound to
/// an identity is one way — are still two accounts: each renews its own
/// rotation, and neither is handed the other's tokens, whether it arrives
/// while the other's rotation is in flight or after it has ended.
#[test]
fn two_providers_under_one_scope_each_renew_their_own_rotation() {
    let runtime = runtime();
    let renewals = Renewals::new();
    renewals.runs_on(runtime.handle().clone());
    let scratch = Scratch::new("two-providers");
    let store = Store::in_home(scratch.path());
    for provider in ["openai", "moonshot"] {
        store
            .keep_subscription(
                provider,
                Tokens::new("access-old".into(), "refresh-old".into(), 1, 1),
            )
            .unwrap();
    }
    let scope = CredentialScopeId::from_digest([7; 32]);
    let due = |provider: &'static str, access: &'static str, after: Duration| Due {
        scope,
        provider,
        store: store.clone(),
        needs_refresh: |tokens, at| tokens.times().0 <= at,
        refresh: Box::new(move |_| {
            Box::pin(async move {
                tokio::time::sleep(after).await;
                Ok(Tokens::new(
                    access.into(),
                    "refresh-new".into(),
                    u64::MAX,
                    1,
                ))
            })
        }),
    };

    let first = due("openai", "access-openai", Duration::from_millis(300));
    let second = due("moonshot", "access-moonshot", Duration::ZERO);
    let (openai, moonshot) = runtime.block_on(async {
        let openai = tokio::spawn({
            let renewals = renewals.clone();
            async move { renewals.renew(first).await }
        });
        // Arrives while the other provider's rotation is in flight.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let moonshot = renewals.renew(second).await;
        (openai.await.unwrap(), moonshot)
    });

    let access = |outcome: &Outcome| {
        outcome
            .as_ref()
            .map(|tokens| tokens.access().to_owned())
            .map_err(ToString::to_string)
    };
    assert_eq!(access(&openai), Ok("access-openai".to_owned()));
    assert_eq!(
        access(&moonshot),
        Ok("access-moonshot".to_owned()),
        "one provider was handed the rotation another provider's credential started"
    );
    let text = std::fs::read_to_string(scratch.path().join("auth.json")).unwrap();
    assert_eq!(
        text.matches("refresh-new").count(),
        2,
        "each provider's rotation was not written down under that provider"
    );
}

/// Work anybody is waiting on holds one of the runtime's blocking threads at
/// a time between a rotation and a login request. A rotation waiting on the
/// store's lock holds one for as long as the lock is held elsewhere, and a
/// login request sent meanwhile has its host looked up on another, unless
/// both wait for the one place this owner's blocking work takes.
///
/// Seen by order. The rotation's lock work is kept busy for a second by
/// another process's hold on the lock, and the login request is sent a fifth
/// of the way in. The request's host is looked up before it connects, so a
/// token service that accepted the login's connection before the rotation's
/// refresh ran saw a lookup made beside the rotation's lock work. With the
/// place, the connection comes after the refresh, and after the write that
/// follows it, however slowly either side is scheduled. Counting the blocking
/// threads the runtime starts cannot say this: a job handed a thread the
/// moment another finishes finds none idle yet, and starts one without the
/// two ever running together.
#[test]
fn a_rotation_and_a_login_request_at_once_hold_one_blocking_thread_between_them() {
    let runtime = runtime();
    let renewals = Renewals::new();
    renewals.runs_on(runtime.handle().clone());

    let scratch = Scratch::new("one-blocking-thread");
    let store = Store::in_home(scratch.path());
    store
        .keep_subscription(
            "openai",
            Tokens::new("access-old".into(), "refresh-old".into(), 1, 1),
        )
        .unwrap();
    // Another process's hold on the store's lock, for long enough that the
    // rotation's lock work is still waiting when the login request is sent.
    let other = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(scratch.path().join("auth.lock"))
        .unwrap();
    other.lock().unwrap();
    let releasing = thread::spawn(move || {
        thread::sleep(Duration::from_secs(1));
        other.unlock().unwrap();
    });

    // A token service on a name rather than an address, so the login
    // request's host is looked up on a blocking thread.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let serving = thread::spawn(move || {
        use std::io::{Read as _, Write as _};
        let (mut stream, _) = listener.accept().unwrap();
        let accepted = Instant::now();
        let mut asked = [0_u8; 4096];
        let _ = stream.read(&mut asked);
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}");
        accepted
    });
    let (refreshed, refreshed_at) = mpsc::channel();

    let rotating = runtime.spawn({
        let renewals = renewals.clone();
        async move {
            renewals
                .renew(Due {
                    scope: CredentialScopeId::from_digest([9; 32]),
                    provider: "openai",
                    store,
                    needs_refresh: |tokens, at| tokens.times().0 <= at,
                    refresh: Box::new(move |_| {
                        let _ = refreshed.send(Instant::now());
                        Box::pin(async {
                            Ok(Tokens::new(
                                "access-new".into(),
                                "refresh-new".into(),
                                u64::MAX,
                                1,
                            ))
                        })
                    }),
                })
                .await
        }
    });
    thread::sleep(Duration::from_millis(200));
    let logging_in = runtime.spawn({
        let renewals = renewals.clone();
        async move {
            renewals
                .login_request(renewals.post(
                    &format!("http://localhost:{port}/token"),
                    Outgoing::new(),
                    String::new(),
                    PATIENCE,
                ))
                .await
        }
    });

    let requested = runtime.block_on(logging_in).unwrap();
    let rotated = runtime.block_on(rotating).unwrap();
    releasing.join().unwrap();
    let accepted = serving.join().unwrap();
    let refreshed_at = refreshed_at.recv_timeout(PATIENCE).unwrap();

    assert!(rotated.is_ok(), "the rotation failed: {rotated:?}");
    assert!(requested.is_ok(), "the login request failed: {requested:?}");
    assert!(
        accepted > refreshed_at,
        "the token service accepted the login request {:?} before the rotation it was sent \
         beside refreshed: its host was looked up on a second blocking thread while the \
         rotation's lock work held one",
        refreshed_at.saturating_duration_since(accepted)
    );
}

/// A login's store work is blocking work, which nothing can stop once it has
/// begun, so it keeps the owner's one place until it returns, even once the
/// login that asked for it has been dropped. Were the place let go with the
/// login, the next account work would take a second blocking thread beside
/// work still running on the first.
///
/// Seen by order: the store work runs for 300 ms, the login is aborted as it
/// begins, and a login request is sent at once. The request must not have the
/// place before the store work has returned.
#[test]
fn a_login_s_store_work_keeps_the_place_until_it_returns_after_its_login_is_dropped() {
    const WORKING: Duration = Duration::from_millis(300);
    let runtime = runtime();
    let renewals = Renewals::new();
    renewals.runs_on(runtime.handle().clone());
    let (starting, started) = mpsc::channel();
    let (finished, finished_at) = mpsc::channel();

    let storing = runtime.spawn({
        let renewals = renewals.clone();
        async move {
            renewals
                .login_store(move || {
                    let _ = starting.send(());
                    thread::sleep(WORKING);
                    let _ = finished.send(Instant::now());
                    Ok::<_, crate::AuthError>(())
                })
                .await
        }
    });
    started.recv_timeout(PATIENCE).unwrap();
    storing.abort();
    let placed_at = runtime.block_on({
        let renewals = renewals.clone();
        async move {
            renewals
                .login_request(async { Ok(Instant::now()) })
                .await
                .unwrap()
        }
    });
    let finished_at = finished_at.recv_timeout(PATIENCE).unwrap();

    assert!(
        placed_at >= finished_at,
        "a login request took the place {:?} before the dropped login's store work returned",
        finished_at.saturating_duration_since(placed_at)
    );
}
