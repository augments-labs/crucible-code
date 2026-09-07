//! Bounded loopback-to-host Unix socket byte relay.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::fd::{FromRawFd, IntoRawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const LISTEN_ADDRESS: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 31337);
const HOST_SOCKET: &str = "/run/crucible/network.sock";
const MAX_CONNECTIONS: usize = 16;
const BUFFER_SIZE: usize = 8 * 1024;
const POLL: Duration = Duration::from_millis(50);
#[cfg(not(test))]
const IDLE: Duration = Duration::from_secs(30);
#[cfg(test)]
const IDLE: Duration = Duration::from_millis(200);

pub(super) struct NetworkRelay {
    #[cfg(test)]
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    listener: Option<JoinHandle<io::Result<()>>>,
}

impl NetworkRelay {
    pub(super) fn start() -> io::Result<Self> {
        Self::start_at(LISTEN_ADDRESS, Path::new(HOST_SOCKET))
    }

    fn start_at(address: SocketAddr, host_path: &Path) -> io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        #[cfg(test)]
        let address = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let path = host_path.to_owned();
        let thread_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("sandbox-network".into())
            .spawn(move || accept(listener, path, thread_stop))?;
        Ok(Self {
            #[cfg(test)]
            address,
            stop,
            listener: Some(worker),
        })
    }

    #[cfg(test)]
    pub(super) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(super) fn stop(mut self) -> io::Result<()> {
        self.stop.store(true, Ordering::Release);
        self.listener.take().map_or(Ok(()), join_worker)
    }
}

impl Drop for NetworkRelay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.listener.take() {
            let _ = worker.join();
        }
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the listener and path are owned by the long-lived accept worker"
)]
fn accept(listener: TcpListener, host_path: PathBuf, stop: Arc<AtomicBool>) -> io::Result<()> {
    let mut workers = Vec::new();
    let mut result = Ok(());
    while !stop.load(Ordering::Acquire) {
        reap(&mut workers, &mut result);
        match listener.accept() {
            Ok((client, _)) if workers.len() < MAX_CONNECTIONS => {
                let path = host_path.clone();
                let worker_stop = Arc::clone(&stop);
                match thread::Builder::new()
                    .name("sandbox-network-relay".into())
                    .spawn(move || relay(client, &path, &worker_stop))
                {
                    Ok(worker) => workers.push(worker),
                    Err(error) => {
                        result = Err(error);
                        break;
                    }
                }
            }
            Ok((client, _)) => {
                let _ = client.shutdown(Shutdown::Both);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                result = Err(error);
                break;
            }
        }
    }
    stop.store(true, Ordering::Release);
    for worker in workers {
        if let Err(error) = join_worker(worker) {
            result = Err(error);
        }
    }
    result
}

fn reap(workers: &mut Vec<JoinHandle<io::Result<()>>>, result: &mut io::Result<()>) {
    let mut index = 0;
    while index < workers.len() {
        if workers.get(index).is_some_and(JoinHandle::is_finished) {
            let worker = workers.swap_remove(index);
            if let Err(error) = join_worker(worker) {
                *result = Err(error);
            }
        } else {
            index += 1;
        }
    }
}

fn relay(client: TcpStream, path: &Path, stop: &Arc<AtomicBool>) -> io::Result<()> {
    let host = connect_unix(path, stop)?;
    client.set_nonblocking(true)?;
    host.set_nonblocking(true)?;
    let activity = Arc::new(AtomicU64::new(0));
    let finished = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let mut client_read = client.try_clone()?;
    let mut host_write = host.try_clone()?;
    let mut host_read = host.try_clone()?;
    let mut client_write = client;
    let left_activity = Arc::clone(&activity);
    let left_done = Arc::clone(&finished);
    let right_activity = Arc::clone(&activity);
    let right_done = Arc::clone(&finished);
    thread::scope(|scope| -> io::Result<()> {
        let outbound = thread::Builder::new()
            .name("sandbox-network-outbound".into())
            .spawn_scoped(scope, || {
                pump(
                    &mut client_read,
                    &mut host_write,
                    stop,
                    &left_done,
                    &left_activity,
                    started,
                )
            })
            .map_err(|_| io::Error::other(RelayCoordinatorFailure))?;
        let Ok(inbound) = thread::Builder::new()
            .name("sandbox-network-inbound".into())
            .spawn_scoped(scope, || {
                pump(
                    &mut host_read,
                    &mut client_write,
                    stop,
                    &right_done,
                    &right_activity,
                    started,
                )
            })
        else {
            finished.store(true, Ordering::Release);
            if let Err(error) = join_scoped(outbound)
                && is_fatal_relay_error(&error)
            {
                return Err(error);
            }
            return Err(io::Error::other(RelayCoordinatorFailure));
        };
        let outbound_result = join_scoped(outbound);
        if outbound_result.is_err() {
            finished.store(true, Ordering::Release);
        }
        let inbound_result = join_scoped(inbound);
        relay_results(outbound_result, inbound_result)
    })
}

fn join_worker(worker: JoinHandle<io::Result<()>>) -> io::Result<()> {
    match worker.join() {
        Ok(Err(error)) if is_fatal_relay_error(&error) => Err(error),
        Ok(_) => Ok(()),
        Err(_) => Err(io::Error::other(RelayWorkerPanic)),
    }
}

fn join_scoped<T>(worker: thread::ScopedJoinHandle<'_, io::Result<T>>) -> io::Result<T> {
    match worker.join() {
        Ok(result) => result,
        Err(_) => Err(io::Error::other(RelayWorkerPanic)),
    }
}

#[derive(Debug)]
struct RelayWorkerPanic;

impl std::fmt::Display for RelayWorkerPanic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("network relay worker panicked")
    }
}

impl std::error::Error for RelayWorkerPanic {}

#[derive(Debug)]
struct RelayCoordinatorFailure;

impl std::fmt::Display for RelayCoordinatorFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("network relay coordinator failed")
    }
}

impl std::error::Error for RelayCoordinatorFailure {}

fn is_fatal_relay_error(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|source| {
        source.downcast_ref::<RelayWorkerPanic>().is_some()
            || source.downcast_ref::<RelayCoordinatorFailure>().is_some()
    })
}

fn relay_results(outbound: io::Result<()>, inbound: io::Result<()>) -> io::Result<()> {
    if let Err(error) = outbound
        && is_fatal_relay_error(&error)
    {
        return Err(error);
    }
    if let Err(error) = inbound
        && is_fatal_relay_error(&error)
    {
        return Err(error);
    }
    Ok(())
}

fn connect_unix(path: &Path, stop: &AtomicBool) -> io::Result<UnixStream> {
    let socket = rustix::net::socket_with(
        rustix::net::AddressFamily::UNIX,
        rustix::net::SocketType::STREAM,
        rustix::net::SocketFlags::CLOEXEC | rustix::net::SocketFlags::NONBLOCK,
        None,
    )?;
    let address = rustix::net::SocketAddrUnix::new(path)?;
    match rustix::net::connect(&socket, &address) {
        Ok(()) => Ok(unix_stream_from_fd(socket)),
        Err(error) if error == rustix::io::Errno::INPROGRESS => {
            let timeout: rustix::event::Timespec = POLL
                .try_into()
                .map_err(|_| io::Error::other("invalid poll timeout"))?;
            let mut pollfd = rustix::event::PollFd::new(&socket, rustix::event::PollFlags::OUT);
            let started = Instant::now();
            loop {
                if stop.load(Ordering::Acquire) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "relay cancelled",
                    ));
                }
                if started.elapsed() >= IDLE {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "host socket connection timed out",
                    ));
                }
                pollfd.clear_revents();
                rustix::event::poll(std::slice::from_mut(&mut pollfd), Some(&timeout))?;
                if pollfd.revents().intersects(
                    rustix::event::PollFlags::OUT
                        | rustix::event::PollFlags::ERR
                        | rustix::event::PollFlags::HUP
                        | rustix::event::PollFlags::NVAL,
                ) {
                    match rustix::net::sockopt::socket_error(&socket)? {
                        Ok(()) => return Ok(unix_stream_from_fd(socket)),
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn unix_stream_from_fd(socket: rustix::fd::OwnedFd) -> UnixStream {
    let raw = socket.into_raw_fd();
    // SAFETY: `socket` owns this live AF_UNIX stream descriptor, and ownership
    // transfers exactly once into UnixStream.
    unsafe { UnixStream::from_raw_fd(raw) }
}

trait ShutdownWrite {
    fn shutdown_write(&self) -> io::Result<()>;
}

impl ShutdownWrite for TcpStream {
    fn shutdown_write(&self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}

impl ShutdownWrite for UnixStream {
    fn shutdown_write(&self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "these are the two bounded sockets and their shared cancellation state"
)]
fn pump<D: Write + ShutdownWrite>(
    source: &mut impl Read,
    destination: &mut D,
    stop: &AtomicBool,
    finished: &AtomicBool,
    activity: &AtomicU64,
    started: Instant,
) -> io::Result<()> {
    let mut buffer = [0_u8; BUFFER_SIZE];
    loop {
        if stop.load(Ordering::Acquire) || finished.load(Ordering::Acquire) {
            return Ok(());
        }
        if started
            .elapsed()
            .saturating_sub(Duration::from_millis(activity.load(Ordering::Acquire)))
            >= IDLE
        {
            finished.store(true, Ordering::Release);
            return Ok(());
        }
        match source.read(&mut buffer) {
            Ok(0) => {
                if let Err(error) = destination.shutdown_write() {
                    finished.store(true, Ordering::Release);
                    return Err(error);
                }
                return Ok(());
            }
            Ok(read) => {
                mark_activity(activity, started);
                let bytes = buffer
                    .get(..read)
                    .ok_or_else(|| io::Error::other("relay read exceeded buffer"))?;
                if let Err(error) = write_all(destination, bytes, stop, finished, activity, started)
                {
                    finished.store(true, Ordering::Release);
                    return Err(error);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                finished.store(true, Ordering::Release);
                return Err(error);
            }
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the write loop shares the bounded sockets and cancellation accounting"
)]
fn write_all(
    destination: &mut impl Write,
    bytes: &[u8],
    stop: &AtomicBool,
    finished: &AtomicBool,
    activity: &AtomicU64,
    started: Instant,
) -> io::Result<()> {
    let mut last_progress = Instant::now();
    let mut written = 0;
    while written < bytes.len() {
        if stop.load(Ordering::Acquire) || finished.load(Ordering::Acquire) {
            return Ok(());
        }
        if last_progress.elapsed() >= IDLE {
            finished.store(true, Ordering::Release);
            return Ok(());
        }
        let remaining = bytes
            .get(written..)
            .ok_or_else(|| io::Error::other("relay write offset exceeded buffer"))?;
        match destination.write(remaining) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "relay closed")),
            Ok(count) => {
                written += count;
                mark_activity(activity, started);
                last_progress = Instant::now();
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn mark_activity(activity: &AtomicU64, started: Instant) {
    activity.store(
        started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        Ordering::Release,
    );
}

#[cfg(test)]
mod tests {
    use super::{NetworkRelay, pump};
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn relay_forwards_private_socket_bytes_both_directions() {
        let root = std::env::temp_dir().join(format!("crucible-network-{}", std::process::id()));
        let _ = std::fs::remove_file(&root);
        let listener = UnixListener::bind(&root).expect("host listener");
        let host = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut bytes = [0_u8; 4];
            stream.read_exact(&mut bytes).expect("read guest");
            assert_eq!(&bytes, b"ping");
            stream.write_all(b"pong").expect("write guest");
        });
        let relay = NetworkRelay::start_at((std::net::Ipv4Addr::LOCALHOST, 0).into(), &root)
            .expect("relay");
        let mut guest = std::net::TcpStream::connect(relay.address()).expect("guest");
        guest.write_all(b"ping").expect("send");
        let mut bytes = [0_u8; 4];
        guest
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        guest.read_exact(&mut bytes).expect("receive");
        assert_eq!(&bytes, b"pong");
        relay.stop().expect("stop");
        host.join().expect("host");
        let _ = std::fs::remove_file(root);
    }

    #[test]
    fn relay_preserves_response_after_client_half_close() {
        let root = std::env::temp_dir().join(format!(
            "crucible-network-half-close-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&root);
        let listener = UnixListener::bind(&root).expect("host listener");
        let host = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = Vec::new();
            stream.read_to_end(&mut request).expect("read request");
            assert_eq!(request, b"request");
            stream.write_all(b"response").expect("write response");
        });
        let relay = NetworkRelay::start_at((std::net::Ipv4Addr::LOCALHOST, 0).into(), &root)
            .expect("relay");
        let mut guest = std::net::TcpStream::connect(relay.address()).expect("guest");
        guest.write_all(b"request").expect("send request");
        guest
            .shutdown(std::net::Shutdown::Write)
            .expect("half close");
        guest
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut response = [0_u8; 8];
        guest.read_exact(&mut response).expect("read response");
        assert_eq!(&response, b"response");
        relay.stop().expect("stop");
        host.join().expect("host");
        let _ = std::fs::remove_file(root);
    }

    #[test]
    fn reset_connection_does_not_poison_listener_cleanup_or_reconnect() {
        let root =
            std::env::temp_dir().join(format!("crucible-network-reset-{}", std::process::id()));
        let _ = std::fs::remove_file(&root);
        let listener = UnixListener::bind(&root).expect("host listener");
        let (reset_done_tx, reset_done_rx) = std::sync::mpsc::channel();
        let host = thread::spawn(move || {
            let (mut reset, _) = listener.accept().expect("reset accept");
            let mut discarded = Vec::new();
            reset.read_to_end(&mut discarded).expect("reset read");
            reset_done_tx.send(()).expect("reset done");
            let (mut live, _) = listener.accept().expect("live accept");
            let mut request = [0_u8; 4];
            live.read_exact(&mut request).expect("live request");
            assert_eq!(&request, b"ping");
            live.write_all(b"pong").expect("live response");
        });
        let relay = NetworkRelay::start_at((std::net::Ipv4Addr::LOCALHOST, 0).into(), &root)
            .expect("relay");
        let reset = std::net::TcpStream::connect(relay.address()).expect("reset guest");
        reset
            .shutdown(std::net::Shutdown::Both)
            .expect("reset guest");
        drop(reset);
        reset_done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("reset relay closed");
        let mut live = std::net::TcpStream::connect(relay.address()).expect("live guest");
        live.write_all(b"ping").expect("live request");
        live.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut response = [0_u8; 4];
        live.read_exact(&mut response).expect("live response");
        assert_eq!(&response, b"pong");
        relay.stop().expect("normal cleanup");
        host.join().expect("host");
        let _ = std::fs::remove_file(root);
    }

    #[test]
    fn active_one_way_download_prevents_read_idle_expiry() {
        let root =
            std::env::temp_dir().join(format!("crucible-network-download-{}", std::process::id()));
        let _ = std::fs::remove_file(&root);
        let listener = UnixListener::bind(&root).expect("host listener");
        let host = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            for _ in 0..12 {
                stream.write_all(&[0x44_u8; 1024]).expect("download chunk");
                thread::sleep(Duration::from_millis(25));
            }
        });
        let relay = NetworkRelay::start_at((std::net::Ipv4Addr::LOCALHOST, 0).into(), &root)
            .expect("relay");
        let mut guest = std::net::TcpStream::connect(relay.address()).expect("guest");
        guest
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut received = 0;
        let mut chunk = [0_u8; 1024];
        while received < 12 * 1024 {
            guest
                .read_exact(&mut chunk)
                .expect("download remains active");
            received += chunk.len();
        }
        relay.stop().expect("stop");
        host.join().expect("host");
        let _ = std::fs::remove_file(root);
    }

    #[test]
    fn delayed_first_response_still_forwards_during_opposite_activity() {
        let root = std::env::temp_dir().join(format!(
            "crucible-network-delayed-response-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&root);
        let listener = UnixListener::bind(&root).expect("host listener");
        let (request_tx, request_rx) = std::sync::mpsc::channel();
        let host = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream.set_nonblocking(true).expect("nonblocking");
            let mut request = [0_u8; 16];
            let mut observed_request = false;
            let deadline = std::time::Instant::now() + Duration::from_millis(350);
            while std::time::Instant::now() < deadline {
                match stream.read(&mut request) {
                    Ok(_) if !observed_request => {
                        observed_request = true;
                        request_tx.send(()).expect("request observed");
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => break,
                }
                thread::sleep(Duration::from_millis(1));
            }
            stream.write_all(b"response").expect("delayed response");
            stream
                .shutdown(std::net::Shutdown::Write)
                .expect("response half-close");
            stream.set_nonblocking(false).expect("drain blocking");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("drain timeout");
            std::io::copy(&mut stream, &mut std::io::sink()).expect("drain trailing request");
        });
        let relay = NetworkRelay::start_at((std::net::Ipv4Addr::LOCALHOST, 0).into(), &root)
            .expect("relay");
        let mut guest = std::net::TcpStream::connect(relay.address()).expect("guest");
        guest.set_nonblocking(true).expect("guest nonblocking");
        let sender_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sender_stop_flag = std::sync::Arc::clone(&sender_stop);
        let mut sender = guest.try_clone().expect("sender clone");
        sender.set_nonblocking(false).expect("sender blocking");
        sender
            .set_write_timeout(Some(Duration::from_millis(50)))
            .expect("sender timeout");
        let sender = thread::spawn(move || {
            while !sender_stop_flag.load(std::sync::atomic::Ordering::Acquire) {
                if sender.write_all(b"request").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        });
        request_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("opposite activity started");
        thread::sleep(Duration::from_millis(450));
        sender_stop.store(true, std::sync::atomic::Ordering::Release);
        sender.join().expect("sender");
        guest
            .shutdown(std::net::Shutdown::Write)
            .expect("request half-close");
        guest.set_nonblocking(false).expect("guest blocking");
        guest
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut response = [0_u8; 8];
        guest.read_exact(&mut response).expect("delayed response");
        assert_eq!(&response, b"response");
        relay.stop().expect("stop");
        host.join().expect("host");
        let _ = std::fs::remove_file(root);
    }

    #[test]
    fn delayed_second_response_starts_a_fresh_write_deadline() {
        let root = std::env::temp_dir().join(format!(
            "crucible-network-delayed-second-response-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&root);
        let listener = UnixListener::bind(&root).expect("host listener");
        let (first_tx, first_rx) = std::sync::mpsc::channel();
        let (second_tx, second_rx) = std::sync::mpsc::channel();
        let host = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0_u8; 7];
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("host timeout");
            stream.read_exact(&mut request).expect("first request");
            stream.write_all(b"first___").expect("first response");
            first_tx.send(()).expect("first response sent");
            let deadline = std::time::Instant::now() + Duration::from_millis(350);
            while std::time::Instant::now() < deadline {
                stream.read_exact(&mut request).expect("opposite activity");
            }
            stream.write_all(b"second__").expect("second response");
            second_tx.send(()).expect("second response sent");
        });
        let relay = NetworkRelay::start_at((std::net::Ipv4Addr::LOCALHOST, 0).into(), &root)
            .expect("relay");
        let mut guest = std::net::TcpStream::connect(relay.address()).expect("guest");
        let sender_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sender_stop_flag = std::sync::Arc::clone(&sender_stop);
        let mut sender = guest.try_clone().expect("sender clone");
        sender
            .set_write_timeout(Some(Duration::from_millis(50)))
            .expect("sender timeout");
        let sender = thread::spawn(move || {
            while !sender_stop_flag.load(std::sync::atomic::Ordering::Acquire) {
                if sender.write_all(b"request").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        first_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first response");
        second_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("second response");
        sender_stop.store(true, std::sync::atomic::Ordering::Release);
        sender.join().expect("sender");
        guest
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("guest timeout");
        let mut responses = [0_u8; 16];
        guest.read_exact(&mut responses).expect("both responses");
        assert_eq!(&responses[..8], b"first___");
        assert_eq!(&responses[8..], b"second__");
        relay.stop().expect("stop");
        host.join().expect("host");
        let _ = std::fs::remove_file(root);
    }

    #[test]
    fn stalled_write_expires_without_external_cancellation() {
        struct Infinite;
        impl Read for Infinite {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                let count = bytes.len().min(7);
                if let Some(prefix) = bytes.get_mut(..count) {
                    prefix.fill(b'x');
                }
                Ok(count)
            }
        }

        struct Stalled;
        impl Write for Stalled {
            fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::WouldBlock.into())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl super::ShutdownWrite for Stalled {
            fn shutdown_write(&self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let activity = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let thread_finished = std::sync::Arc::clone(&finished);
        let thread_stop = std::sync::Arc::clone(&stop);
        let thread_activity = std::sync::Arc::clone(&activity);
        let worker = thread::spawn(move || {
            let _ = pump(
                &mut Infinite,
                &mut Stalled,
                &thread_stop,
                &thread_finished,
                &thread_activity,
                std::time::Instant::now(),
            );
            done_tx.send(()).expect("done");
        });
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("stalled write must expire");
        worker.join().expect("worker");
        assert!(finished.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn relay_closes_when_client_stops_reading() {
        let root = std::env::temp_dir().join(format!(
            "crucible-network-stalled-peer-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&root);
        let listener = UnixListener::bind(&root).expect("host listener");
        let (closed_tx, closed_rx) = std::sync::mpsc::channel();
        let host = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream.set_nonblocking(true).expect("nonblocking");
            let payload = [0x55_u8; 8192];
            let mut input = [0_u8; 1024];
            let mut sent = 0;
            // Opposite-direction traffic remains active while this response
            // starts late; expiry must follow write progress, not test startup.
            let write_after = std::time::Instant::now() + Duration::from_millis(300);
            loop {
                match stream.read(&mut input) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
                    Err(error) => panic!("host read failed: {error}"),
                }
                if std::time::Instant::now() >= write_after && sent < 8 * 1024 * 1024 {
                    match stream.write(&payload) {
                        Ok(count) => sent += count,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => break,
                        Err(error) => panic!("host write failed: {error}"),
                    }
                }
                if sent >= 8 * 1024 * 1024 {
                    thread::sleep(Duration::from_millis(1));
                }
            }
            closed_tx.send(()).expect("closed observer");
        });
        let relay = NetworkRelay::start_at((std::net::Ipv4Addr::LOCALHOST, 0).into(), &root)
            .expect("relay");
        let guest = std::net::TcpStream::connect(relay.address()).expect("guest");
        let sender_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sender_stop_flag = std::sync::Arc::clone(&sender_stop);
        let mut sender = guest.try_clone().expect("sender clone");
        sender.set_nonblocking(true).expect("sender nonblocking");
        let sender = thread::spawn(move || {
            let input = [0x33_u8; 1024];
            while !sender_stop_flag.load(std::sync::atomic::Ordering::Acquire) {
                match sender.write(&input) {
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });
        // Reading the client would relieve the backpressure being tested. Its
        // sender clone also makes reads nonblocking, so a read timeout cannot
        // turn a fixed sleep into evidence of closure. Observe the host peer
        // instead, keeping the client unread until the relay expires.
        let closed = closed_rx.recv_timeout(Duration::from_secs(2)).is_ok();
        sender_stop.store(true, std::sync::atomic::Ordering::Release);
        sender.join().expect("sender");
        relay.stop().expect("stop");
        host.join().expect("host");
        assert!(closed, "stalled host write must expire");
        let _ = std::fs::remove_file(root);
    }

    #[test]
    fn relay_cancellation_joins_an_active_connection() {
        let root =
            std::env::temp_dir().join(format!("crucible-network-cancel-{}", std::process::id()));
        let _ = std::fs::remove_file(&root);
        let listener = UnixListener::bind(&root).expect("host listener");
        let host = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).expect("read until close");
        });
        let relay = NetworkRelay::start_at((std::net::Ipv4Addr::LOCALHOST, 0).into(), &root)
            .expect("relay");
        let _guest = std::net::TcpStream::connect(relay.address()).expect("guest");
        thread::sleep(Duration::from_millis(100));
        relay.stop().expect("stop");
        host.join().expect("host");
        let _ = std::fs::remove_file(root);
    }

    #[test]
    fn relay_ignores_host_connection_worker_failure() {
        let root =
            std::env::temp_dir().join(format!("crucible-network-missing-{}", std::process::id()));
        let _ = std::fs::remove_file(&root);
        let relay = NetworkRelay::start_at((std::net::Ipv4Addr::LOCALHOST, 0).into(), &root)
            .expect("relay");
        let mut guest = std::net::TcpStream::connect(relay.address()).expect("guest");
        guest
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("timeout");
        let mut byte = [0_u8; 1];
        assert_eq!(guest.read(&mut byte).expect("worker closes guest"), 0);
        relay
            .stop()
            .expect("connection failure is not global cleanup failure");
    }

    #[test]
    fn relay_closes_an_idle_connection() {
        let root =
            std::env::temp_dir().join(format!("crucible-network-idle-{}", std::process::id()));
        let _ = std::fs::remove_file(&root);
        let listener = UnixListener::bind(&root).expect("host listener");
        let host = thread::spawn(move || {
            let (_stream, _) = listener.accept().expect("accept");
            thread::sleep(Duration::from_secs(1));
        });
        let relay = NetworkRelay::start_at((std::net::Ipv4Addr::LOCALHOST, 0).into(), &root)
            .expect("relay");
        let mut guest = std::net::TcpStream::connect(relay.address()).expect("guest");
        guest
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut byte = [0_u8; 1];
        assert_eq!(guest.read(&mut byte).expect("read"), 0);
        relay.stop().expect("stop");
        host.join().expect("host");
        let _ = std::fs::remove_file(root);
    }

    #[test]
    fn relay_rejects_the_seventeenth_connection() {
        let root =
            std::env::temp_dir().join(format!("crucible-network-limit-{}", std::process::id()));
        let _ = std::fs::remove_file(&root);
        let listener = UnixListener::bind(&root).expect("host listener");
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let host = thread::spawn(move || {
            let mut streams = Vec::new();
            for _ in 0..super::MAX_CONNECTIONS {
                streams.push(listener.accept().expect("accept").0);
            }
            ready_tx.send(()).expect("ready");
            release_rx.recv().expect("release");
            drop(streams);
        });
        let relay = NetworkRelay::start_at((std::net::Ipv4Addr::LOCALHOST, 0).into(), &root)
            .expect("relay");
        let mut guests = Vec::new();
        for _ in 0..super::MAX_CONNECTIONS {
            guests.push(std::net::TcpStream::connect(relay.address()).expect("guest"));
        }
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("all relays accepted");
        let mut extra = std::net::TcpStream::connect(relay.address()).expect("extra guest");
        extra
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut byte = [0_u8; 1];
        assert_eq!(extra.read(&mut byte).expect("read"), 0);
        drop(guests);
        relay.stop().expect("stop");
        release_tx.send(()).expect("release host");
        host.join().expect("host");
        let _ = std::fs::remove_file(root);
    }
}
