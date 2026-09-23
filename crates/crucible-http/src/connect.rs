//! Making a connection: lookup, TCP and TLS, within 15 s.
//!
//! The target is looked up with the client's own [`Lookups`]. The one
//! deadline covers every step, because what it promises is that a connection
//! is up within 15 s.
//!
//! A failure says which step it was ([`ConnectError`]), with the error that
//! step gave. Its message names no address; the `Debug` of a TCP failure
//! does, because hyper-util's error records the address it tried.

use std::error::Error;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use crucible_runtime::BoxFuture;
use hyper::Uri;
use hyper::http::uri::Scheme;
use hyper_util::client::legacy::connect::{Connected, Connection, HttpConnector};
use hyper_util::rt::TokioIo;
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::TlsConnector;
use tower_service::Service;

use crate::dns::{LookupError, Lookups};

/// How long a connection may take, from its lookup to its last handshake.
const TIMEOUT_CONNECT: Duration = Duration::from_secs(15);

type BoxError = Box<dyn Error + Send + Sync>;

/// The TLS every connection is made with, and the session store it resumes
/// from; clients built with clones of one share both.
#[derive(Clone, Debug)]
pub struct Tls(Arc<ClientConfig>);

impl Tls {
    /// TLS 1.2 and 1.3 over `ring`, trusting the compiled-in Mozilla roots,
    /// with no client certificate and no ALPN.
    ///
    /// # Errors
    ///
    /// A [`rustls::Error`] if the provider refuses the protocol versions.
    pub fn new() -> Result<Self, rustls::Error> {
        Self::with_roots(rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        })
    }

    fn with_roots(roots: rustls::RootCertStore) -> Result<Self, rustls::Error> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(rustls::ALL_VERSIONS)?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Self(Arc::new(config)))
    }

    /// The production configuration, trusting `root` instead of the roots.
    #[cfg(test)]
    pub(crate) fn trusting(root: rustls::pki_types::CertificateDer<'static>) -> Self {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(root).unwrap();
        Self::with_roots(roots).unwrap()
    }

    async fn wrap(&self, io: Conn, host: &str) -> io::Result<Conn> {
        let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']'));
        let name = ServerName::try_from(bare.unwrap_or(host).to_owned())
            .map_err(|problem| io::Error::new(io::ErrorKind::InvalidInput, problem))?;
        let connector = TlsConnector::from(Arc::clone(&self.0));
        Ok(Conn::new(connector.connect(name, io).await?))
    }
}

/// A connection that was not made, by the step that failed.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// Hostname resolution stalled, now or before.
    #[error("hostname resolution stalled")]
    ResolveStalled,
    /// The hostname lookup failed; the source says how.
    #[error("hostname lookup failed")]
    Lookup(#[source] BoxError),
    /// No TCP connection could be made.
    #[error("connection failed")]
    Tcp(#[source] BoxError),
    /// TLS with the target failed.
    #[error("TLS setup failed")]
    Tls(#[source] io::Error),
    /// The target is neither `http` nor `https` with a host to verify, so it
    /// would have been spoken to in plaintext.
    #[error("request URL was invalid")]
    Unverifiable,
    /// Everything together took longer than 15 s.
    #[error("request timed out")]
    Deadline,
}

impl ConnectError {
    /// Sorts a failure to reach an address into a lookup or a TCP failure,
    /// by whether a lookup is what it came from.
    fn reaching(error: impl Into<BoxError>) -> Self {
        let error = error.into();
        let mut cause: Option<&(dyn Error + 'static)> = Some(&*error);
        while let Some(step) = cause {
            match step.downcast_ref::<LookupError>() {
                Some(LookupError::Stalled) => return Self::ResolveStalled,
                Some(_) => return Self::Lookup(error),
                None => cause = step.source(),
            }
        }
        Self::Tcp(error)
    }
}

pub(crate) trait Io: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

/// One connection, whatever it was built from: TCP, or TLS over it.
pub(crate) struct Conn(Box<dyn Io>);

impl Conn {
    pub(crate) fn new(io: impl Io + 'static) -> Self {
        Self(Box::new(io))
    }
}

/// Connects to a target directly.
#[derive(Clone)]
pub(crate) struct Connector {
    tls: Tls,
    target: HttpConnector<Lookups>,
}

impl Connector {
    pub(crate) fn new(tls: &Tls, target: Lookups) -> Self {
        let mut tcp = HttpConnector::new_with_resolver(target);
        tcp.enforce_http(false);
        Self {
            tls: tls.clone(),
            target: tcp,
        }
    }

    /// Connects, and speaks TLS to an `https` target. Plaintext is for an
    /// `http` target alone: anything else is refused before it is dialled.
    async fn connect(mut self, target: Uri) -> Result<Conn, ConnectError> {
        let verified = match (target.scheme(), target.host()) {
            (Some(scheme), _) if *scheme == Scheme::HTTP => None,
            (Some(scheme), Some(host)) if *scheme == Scheme::HTTPS && !host.is_empty() => {
                Some(host.to_owned())
            }
            _ => return Err(ConnectError::Unverifiable),
        };
        let tcp = self.target.call(target).await;
        let stream = Conn::new(tcp.map_err(ConnectError::reaching)?.into_inner());
        match verified {
            Some(host) => self
                .tls
                .wrap(stream, &host)
                .await
                .map_err(ConnectError::Tls),
            None => Ok(stream),
        }
    }
}

impl Service<Uri> for Connector {
    type Response = TokioIo<Conn>;
    type Error = ConnectError;
    type Future = BoxFuture<'static, Result<TokioIo<Conn>, ConnectError>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), ConnectError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, target: Uri) -> Self::Future {
        let connecting = self.clone().connect(target);
        Box::pin(async move {
            match tokio::time::timeout(TIMEOUT_CONNECT, connecting).await {
                Ok(connected) => connected.map(TokioIo::new),
                Err(_) => Err(ConnectError::Deadline),
            }
        })
    }
}

impl Connection for Conn {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

impl AsyncRead for Conn {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Conn {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.0).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.0).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_shutdown(cx)
    }
}
