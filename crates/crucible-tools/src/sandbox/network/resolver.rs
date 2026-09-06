//! One bounded process-wide resolver worker; cancelled commands never join DNS.
//!
//! Native name resolution can block inside the operating system. One worker and
//! a bounded queue prevent timed-out commands from accumulating resolver threads.
//! Each answer uses its own channel and command identity. A stuck resolver makes
//! later DNS requests fail closed at their deadline; it does not hold cleanup.

use std::io;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

use crucible_core::{SandboxId, SandboxNetworkEndpoint};

use super::stream::{Lifetime, POLL};

const MAX_ADDRESSES: usize = 16;
const QUEUED: usize = 16;
const DEADLINE: Duration = Duration::from_secs(5);
static WORKER: OnceLock<io::Result<mpsc::SyncSender<Query>>> = OnceLock::new();

struct Query {
    command: SandboxId,
    endpoint: SandboxNetworkEndpoint,
    life: Arc<Lifetime>,
    deadline: Instant,
    answer: mpsc::SyncSender<Answer>,
}
struct Answer {
    command: SandboxId,
    addresses: io::Result<Vec<SocketAddr>>,
}

pub(super) fn resolve(
    endpoint: SandboxNetworkEndpoint,
    command: SandboxId,
    life: &Arc<Lifetime>,
) -> io::Result<Vec<SocketAddr>> {
    life.check()?;
    if let Ok(address) = endpoint.host().parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(address, endpoint.port())]);
    }
    let sender = WORKER
        .get_or_init(start)
        .as_ref()
        .map_err(|_| unavailable())?;
    let (answer, receiver) = mpsc::sync_channel(1);
    let deadline = Instant::now() + DEADLINE;
    sender
        .try_send(Query {
            command,
            endpoint,
            life: Arc::clone(life),
            deadline,
            answer,
        })
        .map_err(|_| unavailable())?;
    loop {
        life.check()?;
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "sandbox DNS deadline exceeded",
            ));
        }
        match receiver.recv_timeout(POLL) {
            Ok(answer) if answer.command == command => return answer.addresses,
            Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => return Err(unavailable()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn start() -> io::Result<mpsc::SyncSender<Query>> {
    let (sender, receiver) = mpsc::sync_channel::<Query>(QUEUED);
    std::thread::Builder::new()
        .name("sandbox-dns".into())
        .spawn(move || {
            while let Ok(query) = receiver.recv() {
                if query.life.check().is_err() || Instant::now() >= query.deadline {
                    continue;
                }
                let addresses = (query.endpoint.host(), query.endpoint.port())
                    .to_socket_addrs()
                    .map_err(|_| unavailable())
                    .and_then(|addresses| {
                        let values: Vec<_> = addresses.take(MAX_ADDRESSES + 1).collect();
                        if values.is_empty() || values.len() > MAX_ADDRESSES {
                            Err(unavailable())
                        } else {
                            Ok(values)
                        }
                    });
                // A timeout drops this query's receiver. Its result cannot be reused
                // by another connection or command, even if the host is identical.
                let _ = query.answer.try_send(Answer {
                    command: query.command,
                    addresses,
                });
            }
        })?;
    Ok(sender)
}

fn unavailable() -> io::Error {
    io::Error::other("sandbox DNS unavailable or saturated")
}
