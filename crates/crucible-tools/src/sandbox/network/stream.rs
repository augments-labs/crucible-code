//! Socket I/O with shared activity tracking, a command deadline and cancellation.

use super::socket::Socket;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub(super) const POLL: Duration = Duration::from_millis(50);
const IDLE: Duration = Duration::from_secs(30);

pub(super) struct Lifetime {
    started: Instant,
    deadline: Instant,
    idle: Duration,
    last: AtomicU64,
    command_cancelled: Arc<AtomicBool>,
    connection_cancelled: AtomicBool,
}

impl Lifetime {
    pub(super) fn new(deadline: Instant, command_cancelled: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            deadline,
            idle: IDLE,
            last: AtomicU64::new(0),
            command_cancelled,
            connection_cancelled: AtomicBool::new(false),
        })
    }

    pub(super) fn check(&self) -> io::Result<()> {
        if self.command_cancelled.load(Ordering::Acquire)
            || self.connection_cancelled.load(Ordering::Acquire)
        {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "sandbox network mediation cancelled",
            ));
        }
        let idle = self
            .started
            .elapsed()
            .saturating_sub(Duration::from_millis(self.last.load(Ordering::Relaxed)));
        if Instant::now() >= self.deadline || idle >= self.idle {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "sandbox network mediation deadline exceeded",
            ));
        }
        Ok(())
    }

    fn check_write(&self, started: Instant) -> io::Result<()> {
        if self.command_cancelled.load(Ordering::Acquire)
            || self.connection_cancelled.load(Ordering::Acquire)
        {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "sandbox network mediation cancelled",
            ));
        }
        if Instant::now() >= self.deadline || started.elapsed() >= self.idle {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "sandbox network mediation deadline exceeded",
            ));
        }
        Ok(())
    }

    pub(super) fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    fn active(&self) {
        let elapsed = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.last.fetch_max(elapsed, Ordering::Relaxed);
    }

    pub(super) fn stop(&self) {
        self.connection_cancelled.store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn for_test(
        deadline: Instant,
        command_cancelled: Arc<AtomicBool>,
        idle: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            deadline,
            idle,
            last: AtomicU64::new(0),
            command_cancelled,
            connection_cancelled: AtomicBool::new(false),
        })
    }
}

pub(super) struct Stream {
    socket: Socket,
    life: Arc<Lifetime>,
}

impl Stream {
    pub(super) fn new(socket: impl Into<Socket>, life: Arc<Lifetime>) -> io::Result<Self> {
        let socket = socket.into();
        socket.timeout(POLL)?;
        Ok(Self { socket, life })
    }

    pub(super) fn duplicate(&self) -> io::Result<Self> {
        Ok(Self {
            socket: self.socket.duplicate()?,
            life: Arc::clone(&self.life),
        })
    }

    pub(super) fn following(&mut self, life: Arc<Lifetime>) {
        self.life = life;
    }

    pub(super) fn shutdown(&self, direction: Shutdown) {
        let _ = self.socket.shutdown(direction);
    }
}

impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.life.check()?;
            match self.socket.read(bytes) {
                Ok(length) => {
                    if length > 0 {
                        self.life.active();
                    }
                    return Ok(length);
                }
                Err(error) if retry(&error) => {}
                Err(error) => return Err(error),
            }
        }
    }
}

impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let started = Instant::now();
        loop {
            self.life.check_write(started)?;
            match self.socket.write(bytes) {
                Ok(length) => {
                    if length > 0 {
                        self.life.active();
                    }
                    return Ok(length);
                }
                Err(error) if retry(&error) => {}
                Err(error) => return Err(error),
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.life.check()
    }
}

fn retry(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    #[test]
    fn stalled_write_expires_despite_fresh_reverse_activity() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            socket
                .set_write_timeout(Some(Duration::from_millis(100)))
                .expect("write timeout");
            let bytes = [b'r'; 1024];
            let deadline = Instant::now() + Duration::from_secs(4);
            while Instant::now() < deadline {
                match socket.write(&bytes) {
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::BrokenPipe
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::WouldBlock
                        ) =>
                    {
                        break;
                    }
                    Err(error) => panic!("reverse writer failed: {error}"),
                }
            }
        });

        let client = TcpStream::connect(address).expect("connect");
        let life = Lifetime::for_test(
            Instant::now() + Duration::from_secs(3),
            Arc::new(AtomicBool::new(false)),
            Duration::from_millis(200),
        );
        let mut stream = Stream::new(client, Arc::clone(&life)).expect("stream");
        let mut reverse = stream.duplicate().expect("duplicate");
        let reader = thread::spawn(move || {
            let mut bytes = [0; 1024];
            while matches!(reverse.read(&mut bytes), Ok(count) if count > 0) {}
        });

        let payload = vec![b'w'; 64 * 1024 * 1024];
        let started = Instant::now();
        let result = stream.write_all(&payload);
        let elapsed = started.elapsed();
        assert_eq!(
            result.expect_err("stalled write must expire").kind(),
            io::ErrorKind::TimedOut
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "pending write exceeded its short idle bound: {elapsed:?}"
        );

        life.stop();
        drop(stream);
        reader.join().expect("reverse reader");
        server.join().expect("reverse writer");
    }
}
