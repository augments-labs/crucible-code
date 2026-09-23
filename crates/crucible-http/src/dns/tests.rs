use std::io;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::str::FromStr;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use hyper_util::client::legacy::connect::dns::Name;
use tokio::time::advance;
use tower_service::Service;

use super::{Lookup, LookupError, Lookups, Poison};

/// A platform lookup that does not return until the test lets it, counting
/// how many are inside it at once.
#[derive(Default)]
pub(crate) struct Stall {
    /// Whether it lets go, how many are inside, the most ever inside, and how
    /// many were ever made.
    state: Mutex<(bool, usize, usize, usize)>,
    turn: Condvar,
}

impl Lookup for Stall {
    fn lookup(&self, _host: &str) -> io::Result<Vec<SocketAddr>> {
        let mut state = self.state.lock().unwrap();
        state.1 += 1;
        state.2 = state.2.max(state.1);
        state.3 += 1;
        while !state.0 {
            state = self.turn.wait(state).unwrap();
        }
        state.1 -= 1;
        Ok(vec![SocketAddr::from(([127, 0, 0, 1], 0))])
    }
}

impl Stall {
    fn counts(&self) -> (usize, usize, usize) {
        let state = self.state.lock().unwrap();
        (state.1, state.2, state.3)
    }
}

/// Lets every stalled lookup go when a test ends, however it ends, so the
/// runtime's blocking workers can be joined.
pub(crate) struct Release(Arc<Stall>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().0 = true;
        self.0.turn.notify_all();
    }
}

/// A platform lookup that answers every name at once with 127.0.0.1; the
/// connector fills in the port.
pub(crate) struct Answer;

impl Lookup for Answer {
    fn lookup(&self, _host: &str) -> io::Result<Vec<SocketAddr>> {
        Ok(vec![SocketAddr::from(([127, 0, 0, 1], 0))])
    }
}

pub(crate) fn stalled(count: usize, poison: Option<&Poison>) -> (Lookups, Arc<Stall>, Release) {
    let stall = Arc::new(Stall::default());
    let count = NonZeroUsize::new(count).unwrap();
    let lookups = Lookups::with(count, poison.cloned(), stall.clone());
    (lookups, Arc::clone(&stall), Release(stall))
}

fn name() -> Name {
    Name::from_str("api.test").unwrap()
}

/// Waits, on the wall clock, until `inside` lookups are running on workers.
pub(crate) async fn until_inside(stall: &Stall, inside: usize) {
    while stall.counts().0 < inside {
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub(crate) async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_stalled_lookup_fails_at_five_seconds_and_every_later_one_at_once() {
    let poison = Poison::default();
    let (mut lookups, stall, _release) = stalled(2, Some(&poison));
    let pending = tokio::spawn(lookups.call(name()));
    until_inside(&stall, 1).await;

    advance(Duration::from_millis(4_999)).await;
    settle().await;
    assert!(!pending.is_finished(), "gave up before its deadline");
    assert!(!poison.is_raised());

    advance(Duration::from_millis(1)).await;
    settle().await;
    assert!(pending.is_finished(), "still waiting at its deadline");
    assert!(matches!(pending.await.unwrap(), Err(LookupError::Stalled)));
    assert!(poison.is_raised());

    let made = stall.counts().2;
    let later = lookups.call(name()).await;
    assert!(matches!(later, Err(LookupError::Stalled)));
    assert_eq!(stall.counts().2, made, "a poisoned lookup was made");
}

#[tokio::test(start_paused = true)]
async fn lookups_in_flight_never_exceed_their_owners_count() {
    let poison = Poison::default();
    let (lookups, stall, release) = stalled(2, Some(&poison));
    let waiting: Vec<_> = (0..3)
        .map(|_| tokio::spawn(lookups.clone().call(name())))
        .collect();
    until_inside(&stall, 2).await;
    settle().await;
    assert_eq!(stall.counts(), (2, 2, 2), "a third lookup started");

    advance(Duration::from_secs(5)).await;
    settle().await;
    for one in waiting {
        assert!(matches!(one.await.unwrap(), Err(LookupError::Stalled)));
    }
    assert_eq!(stall.counts().0, 2, "a stalled lookup gave its permit back");
    assert_eq!(lookups.permits.available_permits(), 0);

    drop(release);
    while lookups.permits.available_permits() < 2 {
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(stall.counts().1, 2);
}

#[tokio::test(start_paused = true)]
async fn a_stalled_plain_lookup_keeps_no_deadline_and_takes_no_poisoned_permit() {
    let (mut plain, stall, _release) = stalled(1, None);
    let poison = Poison::default();
    let pending = tokio::spawn(plain.call(name()));
    until_inside(&stall, 1).await;

    advance(Duration::from_mins(1)).await;
    settle().await;
    assert!(!pending.is_finished(), "a plain lookup gave up on its own");
    assert!(!poison.is_raised(), "a plain lookup raised the poison");

    let mut poisoned = Lookups::with(NonZeroUsize::MIN, Some(poison.clone()), Arc::new(Answer));
    let found: Vec<_> = poisoned.call(name()).await.unwrap().collect();
    assert_eq!(found, [SocketAddr::from(([127, 0, 0, 1], 0))]);
}
