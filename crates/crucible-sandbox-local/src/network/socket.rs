//! The two native mediator transports, with ownership of private Unix pathnames.

use std::io::{self, Read, Write};
#[cfg(any(test, not(target_os = "linux")))]
use std::net::TcpListener;
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

pub(super) enum Listener {
    #[cfg(any(test, not(target_os = "linux")))]
    Tcp(TcpListener),
    #[cfg(any(target_os = "linux", all(test, unix)))]
    Unix(std::os::unix::net::UnixListener),
}
impl Listener {
    pub(super) fn accept(&self) -> io::Result<Socket> {
        match self {
            #[cfg(any(test, not(target_os = "linux")))]
            Self::Tcp(listener) => listener.accept().map(|(socket, _)| Socket::Tcp(socket)),
            #[cfg(any(target_os = "linux", all(test, unix)))]
            Self::Unix(listener) => listener.accept().map(|(socket, _)| Socket::Unix(socket)),
        }
    }
}

pub(super) enum Socket {
    Tcp(TcpStream),
    #[cfg(any(target_os = "linux", all(test, unix)))]
    Unix(std::os::unix::net::UnixStream),
}
impl From<TcpStream> for Socket {
    fn from(socket: TcpStream) -> Self {
        Self::Tcp(socket)
    }
}
impl Socket {
    pub(super) fn timeout(&self, duration: Duration) -> io::Result<()> {
        match self {
            Self::Tcp(socket) => socket
                .set_nonblocking(false)
                .and_then(|()| socket.set_read_timeout(Some(duration)))
                .and_then(|()| socket.set_write_timeout(Some(duration))),
            #[cfg(any(target_os = "linux", all(test, unix)))]
            Self::Unix(socket) => socket
                .set_nonblocking(false)
                .and_then(|()| socket.set_read_timeout(Some(duration)))
                .and_then(|()| socket.set_write_timeout(Some(duration))),
        }
    }
    pub(super) fn duplicate(&self) -> io::Result<Self> {
        match self {
            Self::Tcp(socket) => socket.try_clone().map(Self::Tcp),
            #[cfg(any(target_os = "linux", all(test, unix)))]
            Self::Unix(socket) => socket.try_clone().map(Self::Unix),
        }
    }
    pub(super) fn shutdown(&self, direction: Shutdown) -> io::Result<()> {
        match self {
            Self::Tcp(socket) => socket.shutdown(direction),
            #[cfg(any(target_os = "linux", all(test, unix)))]
            Self::Unix(socket) => socket.shutdown(direction),
        }
    }
}
impl Read for Socket {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(socket) => socket.read(bytes),
            #[cfg(any(target_os = "linux", all(test, unix)))]
            Self::Unix(socket) => socket.read(bytes),
        }
    }
}
impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(socket) => socket.write(bytes),
            #[cfg(any(target_os = "linux", all(test, unix)))]
            Self::Unix(socket) => socket.write(bytes),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A private directory is supplied by the native preparation owner. Only this
/// newly bound socket's identity may be unlinked during cleanup.
#[cfg(any(target_os = "linux", all(test, unix)))]
pub(super) struct UnixPath {
    path: std::path::PathBuf,
    identity: (u64, u64),
}
#[cfg(any(target_os = "linux", all(test, unix)))]
impl UnixPath {
    pub(super) fn bound(path: &std::path::Path) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt as _;
        let metadata = std::fs::symlink_metadata(path)?;
        Ok(Self {
            path: path.to_owned(),
            identity: (metadata.dev(), metadata.ino()),
        })
    }
    pub(super) fn cleanup(&self) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt as _;
        match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) if (metadata.dev(), metadata.ino()) == self.identity => {
                std::fs::remove_file(&self.path)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            _ => Err(io::Error::other(
                "sandbox proxy socket identity changed before cleanup",
            )),
        }
    }
}
#[cfg(any(target_os = "linux", all(test, unix)))]
impl Drop for UnixPath {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Linux's `sockaddr_un` is much shorter than a valid staging path. Resolve a
/// private parent through its retained descriptor while binding, so no process
/// working-directory mutation or temporary global pathname is required.
#[cfg(target_os = "linux")]
pub(super) fn listen_unix(path: &std::path::Path) -> io::Result<std::os::unix::net::UnixListener> {
    use std::os::fd::AsRawFd as _;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Unix endpoint has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("Unix endpoint has no name"))?;
    let parent = std::fs::File::open(parent)?;
    let alias =
        std::path::PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name);
    std::os::unix::net::UnixListener::bind(alias)
}

#[cfg(all(test, unix, not(target_os = "linux")))]
pub(super) fn listen_unix(path: &std::path::Path) -> io::Result<std::os::unix::net::UnixListener> {
    std::os::unix::net::UnixListener::bind(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_nonblocking_sockets_observe_the_io_poll_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        // Darwin may retain the listening socket's nonblocking flag at accept.
        // Reproduce that input explicitly on every platform.
        client.set_nonblocking(true).unwrap();
        let mut socket = Socket::Tcp(client);
        socket.timeout(Duration::from_millis(50)).unwrap();
        let began = std::time::Instant::now();
        let problem = socket.read(&mut [0; 1]).unwrap_err();
        assert!(matches!(
            problem.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
        assert!(
            began.elapsed() >= Duration::from_millis(25),
            "socket retried without its I/O polling wait"
        );
    }
}
