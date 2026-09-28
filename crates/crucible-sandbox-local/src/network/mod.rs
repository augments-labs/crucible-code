//! Host-owned per-command domain mediation, with native guest transports.
//!
//! Native backends must make this authenticated listener the workload's only
//! outbound route. A domain grant permits every nonzero TCP port at the resolved
//! endpoint, including CONNECT tunnels; it does not inspect encrypted payloads.
//! Each plain HTTP connection forwards exactly one normalized request/body.
//! Headers, queues, connections and buffers have fixed bounds. Cancellation owns
//! listeners and relay workers; the process-wide OS resolver is never joined.
//!
//! A permitted connection leaves the way crucible's own request to the same
//! host would ([`ProxyEnv::relay`]): straight to the checked address, or
//! through an `http://` proxy with a `CONNECT` to that address. Straight
//! includes past a SOCKS proxy that request connects past. The proxy is
//! asked only after the host and address are both permitted, and it hears
//! nothing of the command's own credential. An `https://` proxy, a SOCKS
//! proxy that request refuses, and an address that cannot be used are
//! refused with `502` rather than connected around, as is a proxy that
//! cannot be reached or does not open the tunnel within the handshake's
//! bound.

mod body;
mod request;
mod resolver;
mod socket;
mod stream;

use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(any(test, not(target_os = "linux")))]
use std::net::{Ipv4Addr, TcpListener};

use base64::Engine as _;
use crucible_http::{ConnectProxy, ProxyEnv, Relay};
use crucible_sandbox::{SandboxDomainPolicy, SandboxNetworkEndpoint, SandboxNetworkProvenance};
use crucible_types::SandboxId;

use socket::{Listener, Socket};
use stream::{Lifetime, POLL, Stream};

const CONNECTIONS: usize = 16;
const HANDSHAKE: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a refusal is given to reach the command, however much of the
/// handshake's bound deciding on it spent.
const ANSWER: Duration = Duration::from_secs(1);
/// How long the stop joins the listener's thread before giving it up.
///
/// A normal shutdown is milliseconds: the accept loop ticks every [`POLL`]
/// and each relay it joins is already past its own timeouts once the stop is
/// signalled. Five seconds is orders above that on a loaded machine, and well
/// inside the ten-second stop the transport awaits around a whole process
/// stop, so a mediator that still has not finished is wedged rather than
/// slow, and waiting longer would only spend the transport's own bound.
const STOP: Duration = Duration::from_secs(5);
#[cfg(test)]
const FAIL_RELAY_SPAWN: u8 = 1;
#[cfg(test)]
const PANIC_RESPONSE: u8 = 2;

/// The forms of a proxy's `userinfo` a command could print: the password, as
/// its proxy URL carries it, and the whole userinfo in base64, as its
/// authorization header does.
pub(super) fn credential_forms(userinfo: &str) -> Vec<Vec<u8>> {
    vec![
        userinfo
            .split_once(':')
            .map_or("", |(_, password)| password)
            .as_bytes()
            .to_vec(),
        base64::engine::general_purpose::STANDARD
            .encode(userinfo)
            .into_bytes(),
    ]
}

pub(super) struct Mediator {
    #[cfg(any(test, not(target_os = "linux")))]
    address: SocketAddr,
    #[cfg(test)]
    authorization: String,
    userinfo: String,
    failed: bool,
    stop: Arc<AtomicBool>,
    listener: Option<JoinHandle<io::Result<()>>>,
    #[cfg(test)]
    fault: Arc<AtomicU8>,
    #[cfg(any(target_os = "linux", all(test, unix)))]
    socket_path: Option<socket::UnixPath>,
}

impl Mediator {
    #[cfg(any(test, not(target_os = "linux")))]
    pub(super) fn tcp(
        policy: SandboxDomainPolicy,
        id: SandboxId,
        duration: Option<Duration>,
        upstream: Arc<ProxyEnv>,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let mut mediator = Self::start(Listener::Tcp(listener), policy, id, duration, upstream)?;
        mediator.address = address;
        Ok(mediator)
    }

    fn start(
        listener: Listener,
        policy: SandboxDomainPolicy,
        id: SandboxId,
        duration: Option<Duration>,
        upstream: Arc<ProxyEnv>,
    ) -> io::Result<Self> {
        let userinfo = credential()?;
        let authorization = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(&userinfo)
        );
        let stop = Arc::new(AtomicBool::new(false));
        #[cfg(test)]
        let fault = Arc::new(AtomicU8::new(0));
        let context = Context {
            policy,
            id,
            deadline: duration
                .map(|duration| {
                    Instant::now()
                        .checked_add(duration)
                        .ok_or_else(|| io::Error::other("invalid sandbox network deadline"))
                })
                .transpose()?,
            authorization: authorization.clone(),
            upstream,
            stop: Arc::clone(&stop),
            #[cfg(test)]
            fault: Arc::clone(&fault),
        };
        let worker = thread::Builder::new()
            .name("sandbox-proxy".into())
            .spawn(move || accept(listener, &Arc::new(context)))?;
        Ok(Self {
            #[cfg(any(test, not(target_os = "linux")))]
            address: (Ipv4Addr::LOCALHOST, 0).into(),
            #[cfg(test)]
            authorization,
            userinfo,
            failed: false,
            stop,
            listener: Some(worker),
            #[cfg(test)]
            fault,
            #[cfg(any(target_os = "linux", all(test, unix)))]
            socket_path: None,
        })
    }

    #[cfg(any(target_os = "linux", all(test, unix)))]
    pub(super) fn unix(
        path: &std::path::Path,
        policy: SandboxDomainPolicy,
        id: SandboxId,
        duration: Option<Duration>,
        upstream: Arc<ProxyEnv>,
    ) -> io::Result<Self> {
        let listener = socket::listen_unix(path)?;
        let owned = socket::UnixPath::bound(path)?;
        listener.set_nonblocking(true)?;
        let mut mediator = Self::start(Listener::Unix(listener), policy, id, duration, upstream)?;
        mediator.socket_path = Some(owned);
        Ok(mediator)
    }

    /// What of this command's credential is masked in its output streams.
    pub(super) fn masked(&self) -> Vec<Vec<u8>> {
        credential_forms(&self.userinfo)
    }

    /// Stops the listener and disposes the private socket pathname.
    ///
    /// The listener's thread is joined up to [`STOP`]; one still running then
    /// is detached and the stop is reported as failed cleanup rather than
    /// joined without end. The stop was already signalled and the pathname is
    /// still disposed below, so a later stop finds no listener to join and
    /// answers the recorded failure instead of waiting again.
    pub(super) fn stop(&mut self) -> io::Result<()> {
        self.stop.store(true, Ordering::Release);
        let stopped = self.listener.take().map_or(Ok(()), |worker| {
            let deadline = Instant::now() + STOP;
            while !worker.is_finished() {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "sandbox proxy listener did not stop within its bound",
                    ));
                }
                thread::sleep(POLL);
            }
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

    #[cfg(test)]
    fn inject_relay_spawn_failure(&self) {
        self.fault.store(FAIL_RELAY_SPAWN, Ordering::Release);
    }

    #[cfg(test)]
    fn inject_response_panic(&self) {
        self.fault.store(PANIC_RESPONSE, Ordering::Release);
    }

    /// Replaces the listener's thread with one that never finishes, standing
    /// in for a listener wedged past its bound. The thread it replaces is
    /// detached and exits once the stop is signalled, as a listener whose
    /// handle was lost still would.
    #[cfg(test)]
    pub(super) fn hang_listener(&mut self) {
        self.listener = Some(
            thread::Builder::new()
                .name("sandbox-hung-listener".into())
                .spawn(|| -> io::Result<()> {
                    loop {
                        thread::sleep(Duration::from_hours(1));
                    }
                })
                .expect("a hung listener thread"),
        );
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
    deadline: Option<Instant>,
    authorization: String,
    /// The proxy settings a permitted connection is routed by.
    upstream: Arc<ProxyEnv>,
    stop: Arc<AtomicBool>,
    #[cfg(test)]
    fault: Arc<AtomicU8>,
}

fn accept(listener: Listener, context: &Arc<Context>) -> io::Result<()> {
    let mut result = Ok(());
    let mut workers: Vec<JoinHandle<io::Result<()>>> = Vec::new();
    while !context.stop.load(Ordering::Acquire)
        && context
            .deadline
            .is_none_or(|deadline| Instant::now() < deadline)
    {
        let mut index = 0;
        while index < workers.len() {
            if workers.get(index).is_some_and(JoinHandle::is_finished) {
                let worker = workers.swap_remove(index);
                if !matches!(worker.join(), Ok(Ok(()))) {
                    result = Err(io::Error::other("sandbox proxy relay failed"));
                }
            } else {
                index += 1;
            }
        }
        match listener.accept() {
            Ok(socket) if workers.len() < CONNECTIONS => {
                #[cfg(test)]
                let injected = context
                    .fault
                    .compare_exchange(FAIL_RELAY_SPAWN, 0, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok();
                let context = Arc::clone(context);
                #[cfg(test)]
                let spawned = if injected {
                    let _ = socket.shutdown(Shutdown::Both);
                    Err(io::Error::other("injected sandbox relay spawn failure"))
                } else {
                    thread::Builder::new()
                        .name("sandbox-relay".into())
                        .spawn(move || serve(socket, &context))
                };
                #[cfg(not(test))]
                let spawned = thread::Builder::new()
                    .name("sandbox-relay".into())
                    .spawn(move || serve(socket, &context));
                match spawned {
                    Ok(worker) => workers.push(worker),
                    Err(error) => {
                        result = Err(error);
                        break;
                    }
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
        if !matches!(worker.join(), Ok(Ok(()))) {
            result = Err(io::Error::other("sandbox proxy relay failed"));
        }
    }
    result
}

fn serve(socket: Socket, context: &Context) -> io::Result<()> {
    let handshake = Instant::now() + HANDSHAKE;
    let life = Lifetime::new(
        Some(
            context
                .deadline
                .map_or(handshake, |deadline| deadline.min(handshake)),
        ),
        Arc::clone(&context.stop),
    );
    let stream = Stream::new(socket, Arc::clone(&life))?;
    let mut client = BufReader::with_capacity(8192, stream);
    let (request, mut origin) = match prepare(&mut client, context, &life) {
        Ok(prepared) => prepared,
        Err(refusal) => {
            let answering = Instant::now() + ANSWER;
            client.get_mut().following(Lifetime::new(
                Some(
                    context
                        .deadline
                        .map_or(answering, |deadline| deadline.min(answering)),
                ),
                Arc::clone(&context.stop),
            ));
            let _ = client.get_mut().write_all(&refusal.answer());
            client.get_ref().shutdown(Shutdown::Both);
            return Ok(());
        }
    };
    let life = Lifetime::new(context.deadline, Arc::clone(&context.stop));
    client.get_mut().following(Arc::clone(&life));
    origin.following(Arc::clone(&life));
    if request.tunnel {
        if client
            .get_mut()
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .is_err()
        {
            return Ok(());
        }
    } else {
        if request.expect_continue
            && client
                .get_mut()
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                .is_err()
        {
            return Ok(());
        }
        if origin.write_all(&request.header).is_err() {
            return Ok(());
        }
    }
    let mut response = origin.duplicate()?;
    let mut outgoing = client.get_ref().duplicate()?;
    thread::scope(|scope| {
        let response_life = Arc::clone(&life);
        #[cfg(test)]
        let fault = Arc::clone(&context.fault);
        let received = thread::Builder::new()
            .name("sandbox-response".into())
            .spawn_scoped(scope, move || {
                #[cfg(test)]
                if fault
                    .compare_exchange(PANIC_RESPONSE, 0, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    panic!("injected sandbox response failure");
                }
                if io::copy(&mut response, &mut outgoing).is_err() {
                    response_life.stop();
                }
                outgoing.shutdown(Shutdown::Write);
            });
        let received =
            received.map_err(|_| io::Error::other("sandbox proxy response could not start"))?;
        let copied = if request.tunnel {
            io::copy(&mut client, &mut origin).map(|_| ())
        } else {
            body::forward(&mut client, &mut origin, request.body)
        };
        if copied.is_err() {
            life.stop();
        }
        origin.shutdown(Shutdown::Write);
        received
            .join()
            .map_err(|_| io::Error::other("sandbox proxy response failed"))
    })?;
    client.get_ref().shutdown(Shutdown::Both);
    origin.shutdown(Shutdown::Both);
    Ok(())
}

/// Why a command's connection was answered rather than made.
#[derive(Clone, Copy)]
enum Refusal {
    /// Malformed, unauthorized or denied, or no permitted address answered.
    Denied,
    /// The proxy crucible's environment names would not carry it.
    Upstream(Upstream),
}

/// What kept the proxy crucible's environment names from carrying a
/// permitted connection.
#[derive(Clone, Copy)]
enum Upstream {
    /// It is not a usable `http://` proxy, the only kind spoken to here, nor
    /// a SOCKS one connected past.
    Unsupported,
    /// No connection to it could be made, or the request not written.
    Unreachable,
    /// It answered the `CONNECT` with this status instead of a tunnel.
    Refused(u16),
    /// It answered nothing a tunnel could be taken from within the bound.
    Unanswered,
}

impl Refusal {
    /// What the command is told: `403` for anything it asked for that was
    /// not allowed or not there, and `502` with one line saying why when the
    /// proxy it has to go through would not carry it. Neither says anything
    /// of that proxy's address or credential.
    fn answer(self) -> Vec<u8> {
        let reason = match self {
            Self::Denied => {
                return b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec();
            }
            Self::Upstream(Upstream::Unsupported) => "the proxy in crucible's environment cannot carry a sandboxed command's traffic: only a usable http:// proxy can\n".to_owned(),
            Self::Upstream(Upstream::Unreachable) => {
                "the proxy in crucible's environment could not be reached\n".to_owned()
            }
            Self::Upstream(Upstream::Refused(status)) => format!(
                "the proxy in crucible's environment refused the connection with status {status}\n"
            ),
            Self::Upstream(Upstream::Unanswered) => {
                "the proxy in crucible's environment gave no usable answer\n".to_owned()
            }
        };
        format!(
            "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reason}",
            reason.len()
        )
        .into_bytes()
    }
}

/// The request a command made and the connection that carries it, once
/// everything it asked for was checked and reached.
///
/// Nothing leaves this machine until the host and the address are both
/// permitted. The proxy settings are asked about the host the command named,
/// as crucible's own request to it would be, and a tunnel through a proxy is
/// asked for the checked address rather than that name, so what the proxy
/// reaches is what the policy permitted.
fn prepare(
    client: &mut BufReader<Stream>,
    context: &Context,
    life: &Arc<Lifetime>,
) -> Result<(request::Request, Stream), Refusal> {
    let header = read_header(client).map_err(|_| Refusal::Denied)?;
    let request = request::parse(&header, context.authorization.as_bytes(), &context.policy)
        .map_err(|_| Refusal::Denied)?;
    let relay = context.upstream.relay(request.endpoint.host());
    let addresses = resolver::resolve(request.endpoint.clone(), context.id, life)
        .map_err(|_| Refusal::Denied)?;
    let mut refusal = Refusal::Denied;
    for address in addresses {
        if life.check().is_err() {
            break;
        }
        if !context.policy.permits_address(address.ip()) {
            continue;
        }
        let reached = match relay {
            Relay::Direct => dial([address], life)
                .and_then(|origin| Stream::new(origin, Arc::clone(life)).ok())
                .ok_or(Refusal::Denied),
            Relay::Through(proxy) => {
                tunnel(proxy, address, context.id, life).map_err(Refusal::Upstream)
            }
            Relay::Unsupported => Err(Refusal::Upstream(Upstream::Unsupported)),
        };
        match reached {
            Ok(origin) if life.check().is_ok() => return Ok((request, origin)),
            Ok(_) => return Err(refusal),
            // Every other address goes to the same proxy, with the same result.
            Err(Refusal::Upstream(problem @ (Upstream::Unsupported | Upstream::Unreachable))) => {
                return Err(Refusal::Upstream(problem));
            }
            Err(problem) => refusal = problem,
        }
    }
    Err(refusal)
}

/// A connection to the first of `addresses` that answers within `life`.
fn dial(addresses: impl IntoIterator<Item = SocketAddr>, life: &Lifetime) -> Option<TcpStream> {
    for address in addresses {
        life.check().ok()?;
        let remaining = life
            .remaining()
            .unwrap_or(CONNECT_TIMEOUT)
            .min(CONNECT_TIMEOUT);
        if remaining.is_zero() {
            return None;
        }
        if let Ok(connection) = TcpStream::connect_timeout(&address, remaining) {
            return Some(connection);
        }
    }
    None
}

/// A tunnel to the permitted `target` through `proxy`, opened within `life`.
///
/// The proxy is the user's own, named by crucible's environment rather than
/// by the command, so its address is not held to the command's policy. Its
/// answer is read a byte at a time under the ceiling a command's own request
/// head has, so a far end that speaks first loses nothing to this read, and
/// only a `2xx` opens the tunnel.
///
/// The target is asked for in the form the policy checked it in: an IPv4
/// address written as IPv6 is written as IPv4, and an IPv6 address loses its
/// scope, which has no meaning on the proxy's machine and no place in an
/// authority.
fn tunnel(
    proxy: &ConnectProxy,
    target: SocketAddr,
    command: SandboxId,
    life: &Arc<Lifetime>,
) -> Result<Stream, Upstream> {
    let at =
        SandboxNetworkEndpoint::new(proxy.host(), proxy.port(), SandboxNetworkProvenance::User)
            .map_err(|_| Upstream::Unreachable)?;
    let addresses = resolver::resolve(at, command, life).map_err(|_| Upstream::Unreachable)?;
    let connection = dial(addresses, life).ok_or(Upstream::Unreachable)?;
    let mut upstream =
        Stream::new(connection, Arc::clone(life)).map_err(|_| Upstream::Unreachable)?;
    let target = SocketAddr::new(target.ip().to_canonical(), target.port());
    let mut head = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n").into_bytes();
    for (name, value) in &proxy.headers() {
        head.extend_from_slice(name.as_str().as_bytes());
        head.extend_from_slice(b": ");
        head.extend_from_slice(value.as_bytes());
        head.extend_from_slice(b"\r\n");
    }
    head.extend_from_slice(b"\r\n");
    upstream
        .write_all(&head)
        .map_err(|_| Upstream::Unreachable)?;
    let answer = read_header(&mut BufReader::with_capacity(1, &mut upstream))
        .map_err(|_| Upstream::Unanswered)?;
    let mut fields = [httparse::EMPTY_HEADER; 64];
    let mut parsed = httparse::Response::new(&mut fields);
    if parsed.parse(&answer).ok() != Some(httparse::Status::Complete(answer.len())) {
        return Err(Upstream::Unanswered);
    }
    match parsed.code {
        Some(200..=299) => Ok(upstream),
        Some(status) => Err(Upstream::Refused(status)),
        None => Err(Upstream::Unanswered),
    }
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
