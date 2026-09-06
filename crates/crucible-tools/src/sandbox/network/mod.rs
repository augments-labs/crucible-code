//! Host-owned per-command domain mediation, with native guest transports.
//!
//! Native backends must make this authenticated listener the workload's only
//! outbound route. A domain grant permits every nonzero TCP port at the resolved
//! endpoint, including CONNECT tunnels; it does not inspect encrypted payloads.
//! Each plain HTTP connection forwards exactly one normalized request/body.
//! Headers, queues, connections and buffers have fixed bounds. Cancellation owns
//! listeners and relay workers; the process-wide OS resolver is never joined.

mod body;
mod redaction;
mod request;
mod resolver;
mod socket;
mod stream;

use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(any(test, not(target_os = "linux")))]
use std::net::TcpListener;

use base64::Engine as _;
use crucible_core::{SandboxDomainPolicy, SandboxId};

use socket::{Listener, Socket};
use stream::{Lifetime, POLL, Stream};

const CONNECTIONS: usize = 16;
const HANDSHAKE: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct Mediator {
    #[cfg(any(test, not(target_os = "linux")))]
    address: SocketAddr,
    #[cfg(test)]
    authorization: String,
    userinfo: String,
    failed: bool,
    stop: Arc<AtomicBool>,
    listener: Option<JoinHandle<io::Result<()>>>,
    #[cfg(any(target_os = "linux", all(test, unix)))]
    socket_path: Option<socket::UnixPath>,
}

impl Mediator {
    #[cfg(any(test, not(target_os = "linux")))]
    pub(super) fn tcp(
        policy: SandboxDomainPolicy,
        id: SandboxId,
        duration: Duration,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        Self::start(Listener::Tcp(listener), address, policy, id, duration)
    }

    fn start(
        listener: Listener,
        address: SocketAddr,
        policy: SandboxDomainPolicy,
        id: SandboxId,
        duration: Duration,
    ) -> io::Result<Self> {
        #[cfg(all(target_os = "linux", not(test)))]
        let _ = address;
        let userinfo = credential()?;
        let authorization = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(&userinfo)
        );
        let stop = Arc::new(AtomicBool::new(false));
        let context = Context {
            policy,
            id,
            deadline: Instant::now()
                .checked_add(duration)
                .ok_or_else(|| io::Error::other("invalid sandbox network deadline"))?,
            authorization: authorization.clone(),
            stop: Arc::clone(&stop),
        };
        let worker = thread::Builder::new()
            .name("sandbox-proxy".into())
            .spawn(move || accept(listener, &Arc::new(context)))?;
        Ok(Self {
            #[cfg(any(test, not(target_os = "linux")))]
            address,
            #[cfg(test)]
            authorization,
            userinfo,
            failed: false,
            stop,
            listener: Some(worker),
            #[cfg(any(target_os = "linux", all(test, unix)))]
            socket_path: None,
        })
    }

    #[cfg(any(target_os = "linux", all(test, unix)))]
    pub(super) fn unix(
        path: &std::path::Path,
        policy: SandboxDomainPolicy,
        id: SandboxId,
        duration: Duration,
    ) -> io::Result<Self> {
        let listener = socket::listen_unix(path)?;
        let owned = socket::UnixPath::bound(path)?;
        listener.set_nonblocking(true)?;
        let mut mediator = Self::start(
            Listener::Unix(listener),
            (Ipv4Addr::LOCALHOST, 0).into(),
            policy,
            id,
            duration,
        )?;
        mediator.socket_path = Some(owned);
        Ok(mediator)
    }

    pub(super) fn protect_output(
        &self,
        output: Box<dyn crucible_core::SandboxOutput>,
    ) -> Box<dyn crucible_core::SandboxOutput> {
        Box::new(redaction::ProtectedOutput::new(output, &self.userinfo))
    }

    pub(super) fn stop(&mut self) -> io::Result<()> {
        self.stop.store(true, Ordering::Release);
        let stopped = self.listener.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("sandbox proxy listener failed")))
        });
        #[cfg(any(target_os = "linux", all(test, unix)))]
        let cleaned = self
            .socket_path
            .as_ref()
            .map_or(Ok(()), socket::UnixPath::cleanup);
        #[cfg(not(any(target_os = "linux", all(test, unix))))]
        let cleaned: io::Result<()> = Ok(());
        self.failed |= stopped.is_err() || cleaned.is_err();
        if self.failed {
            Err(io::Error::other("sandbox network cleanup failed"))
        } else {
            Ok(())
        }
    }

    /// These bounded values travel only in the workload environment. Native
    /// adapters replace inherited proxy settings, including bypass lists.
    pub(super) fn environment(&self, endpoint: SocketAddr) -> [(&'static str, String); 8] {
        let url = format!("http://{}@{endpoint}", self.userinfo);
        [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
        ]
        .map(|name| {
            (
                name,
                if name.eq_ignore_ascii_case("NO_PROXY") {
                    String::new()
                } else {
                    url.clone()
                },
            )
        })
    }

    #[cfg(any(test, not(target_os = "linux")))]
    pub(super) const fn address(&self) -> SocketAddr {
        self.address
    }
    #[cfg(test)]
    pub(super) fn authorization(&self) -> &str {
        &self.authorization
    }
}

impl std::fmt::Debug for Mediator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Mediator([private per-command listener])")
    }
}

impl Drop for Mediator {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct Context {
    policy: SandboxDomainPolicy,
    id: SandboxId,
    deadline: Instant,
    authorization: String,
    stop: Arc<AtomicBool>,
}

fn accept(listener: Listener, context: &Arc<Context>) -> io::Result<()> {
    let mut result = Ok(());
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    while !context.stop.load(Ordering::Acquire) && Instant::now() < context.deadline {
        let mut index = 0;
        while index < workers.len() {
            if workers.get(index).is_some_and(JoinHandle::is_finished) {
                let worker = workers.swap_remove(index);
                if worker.join().is_err() {
                    result = Err(io::Error::other("sandbox proxy relay failed"));
                }
            } else {
                index += 1;
            }
        }
        match listener.accept() {
            Ok(socket) if workers.len() < CONNECTIONS => {
                let context = Arc::clone(context);
                if let Ok(worker) = thread::Builder::new()
                    .name("sandbox-relay".into())
                    .spawn(move || serve(socket, &context))
                {
                    workers.push(worker);
                }
            }
            Ok(socket) => {
                let _ = socket.shutdown(Shutdown::Both);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                result = Err(error);
                break;
            }
        }
    }
    context.stop.store(true, Ordering::Release);
    drop(listener);
    for worker in workers {
        if worker.join().is_err() {
            result = Err(io::Error::other("sandbox proxy relay failed"));
        }
    }
    result
}

fn serve(socket: Socket, context: &Context) {
    let life = Lifetime::new(
        context.deadline.min(Instant::now() + HANDSHAKE),
        Arc::clone(&context.stop),
    );
    let Ok(stream) = Stream::new(socket, Arc::clone(&life)) else {
        return;
    };
    let mut client = BufReader::with_capacity(8192, stream);
    let prepared = prepare(&mut client, context, &life);
    let Ok((request, origin)) = prepared else {
        let _ = client
            .get_mut()
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        client.get_ref().shutdown(Shutdown::Both);
        return;
    };
    let life = Lifetime::new(context.deadline, Arc::clone(&context.stop));
    client.get_mut().following(Arc::clone(&life));
    let Ok(mut origin) = Stream::new(origin, Arc::clone(&life)) else {
        return;
    };
    if request.tunnel {
        if client
            .get_mut()
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .is_err()
        {
            return;
        }
    } else {
        if request.expect_continue
            && client
                .get_mut()
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                .is_err()
        {
            return;
        }
        if origin.write_all(&request.header).is_err() {
            return;
        }
    }
    let Ok(mut response) = origin.duplicate() else {
        return;
    };
    let Ok(mut outgoing) = client.get_ref().duplicate() else {
        return;
    };
    thread::scope(|scope| {
        let response_life = Arc::clone(&life);
        let received = thread::Builder::new()
            .name("sandbox-response".into())
            .spawn_scoped(scope, move || {
                if io::copy(&mut response, &mut outgoing).is_err() {
                    response_life.stop();
                }
                outgoing.shutdown(Shutdown::Write);
            });
        let Ok(received) = received else {
            return;
        };
        let copied = if request.tunnel {
            io::copy(&mut client, &mut origin).map(|_| ())
        } else {
            body::forward(&mut client, &mut origin, request.body)
        };
        if copied.is_err() {
            life.stop();
        }
        origin.shutdown(Shutdown::Write);
        let _ = received.join();
    });
    client.get_ref().shutdown(Shutdown::Both);
    origin.shutdown(Shutdown::Both);
}

fn prepare(
    client: &mut BufReader<Stream>,
    context: &Context,
    life: &Arc<Lifetime>,
) -> io::Result<(request::Request, TcpStream)> {
    let header = read_header(client)?;
    let request = request::parse(&header, context.authorization.as_bytes(), &context.policy)?;
    let addresses = resolver::resolve(request.endpoint.clone(), context.id, life)?;
    for address in addresses {
        life.check()?;
        if !context.policy.permits_address(address.ip()) {
            continue;
        }
        let remaining = life.remaining().min(CONNECT_TIMEOUT);
        if remaining.is_zero() {
            break;
        }
        if let Ok(origin) = TcpStream::connect_timeout(&address, remaining) {
            life.check()?;
            return Ok((request, origin));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "sandbox proxy target unavailable or denied",
    ))
}

fn read_header(source: &mut impl io::BufRead) -> io::Result<Vec<u8>> {
    let mut header = Vec::new();
    loop {
        let start = header.len();
        let remaining = request::MAX_HEADER_BYTES.saturating_sub(start);
        source
            .take(remaining as u64 + 1)
            .read_until(b'\n', &mut header)?;
        if header.len() > request::MAX_HEADER_BYTES
            || !header.ends_with(b"\r\n")
            || header.len() == start
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid or oversized sandbox proxy header",
            ));
        }
        if header.get(start..) == Some(b"\r\n") {
            return Ok(header);
        }
    }
}

fn credential() -> io::Result<String> {
    use std::fmt::Write as _;

    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| io::Error::other("sandbox proxy randomness unavailable"))?;
    let mut value = String::from("crucible:");
    for byte in bytes {
        write!(value, "{byte:02x}")
            .map_err(|_| io::Error::other("sandbox proxy credential encoding failed"))?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests;
