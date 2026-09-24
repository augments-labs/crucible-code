//! Making a connection: lookup, TCP, a proxy's tunnel and TLS, within 15 s.
//!
//! A target reached directly is looked up with the client's own [`Lookups`].
//! Through a proxy, only the proxy's host is looked up, and only with a
//! [`PlainLookups`], so a proxied request neither obeys nor raises a poison;
//! the proxy resolves the target. An `https://` proxy is itself spoken to
//! over TLS, and an `https` target's own TLS runs inside the tunnel, so the
//! proxy carries bytes it cannot read. The one deadline covers every step,
//! the proxy's lookup included, because what it promises is that a
//! connection is up within 15 s of starting to make it.
//!
//! A client makes at most [`MAX_SETUPS`] connections at once ([`Setups`]).
//! One asked for past that waits for a slot, with no clock of its own, for
//! as long as its request is waiting, and its 15 s start once it has one. A
//! connection being made for a request ends once the request is dropped or
//! has its response head, even where hyper-util carries on making it after
//! the request took an idle connection instead.
//!
//! A failure says which step it was ([`ConnectError`]), with the error that
//! step gave. Its message names no address; the `Debug` of a TCP failure
//! does, because hyper-util's error records the address it tried.

use std::error::Error;
use std::future::{Future, poll_fn};
use std::io;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use crucible_runtime::BoxFuture;
use hyper::Uri;
use hyper::http::uri::Scheme;
use hyper_util::client::legacy::connect::proxy::Tunnel;
use hyper_util::client::legacy::connect::{Connected, Connection, HttpConnector};
use hyper_util::rt::TokioIo;
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{Semaphore, watch};
use tokio_rustls::TlsConnector;
use tower_service::Service;

use crate::dns::{LookupError, Lookups, PlainLookups};
use crate::proxy::{ConnectProxy, Leg, ProxyEnv, Route, select};

/// How long a connection may take, from its lookup to its last handshake.
const TIMEOUT_CONNECT: Duration = Duration::from_secs(15);

/// How many connections one client may be making at once.
///
/// A connection being made holds a socket and, through a proxy, the
/// proxy's credential, and a request that finds no idle connection starts
/// one; so without a bound, a burst of requests to hosts that stall is a
/// burst of sockets. The previous client made one connection at a time
/// across the whole process, and held that one slot until the response head.
/// This holds a slot only while a connection is being made, and lets four
/// be made at once, so connections that stall hold up the rest only once
/// four of them do.
pub(crate) const MAX_SETUPS: usize = 4;

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
    /// No TCP connection could be made, to the target or to its proxy. For a
    /// SOCKS proxy that was to resolve the target, or a proxy address that
    /// cannot be rebuilt without its user information, none is tried: the
    /// previous client made no connection either.
    #[error("connection failed")]
    Tcp(#[source] BoxError),
    /// TLS with an `https://` proxy failed.
    #[error("TLS setup with the proxy failed")]
    ProxyTls(#[source] io::Error),
    /// No tunnel through the proxy was opened; the source says why.
    #[error("the proxy did not open a tunnel")]
    Tunnel(#[source] BoxError),
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

/// One connection, whatever it was built from: TCP, TLS over it, or either
/// inside a tunnel.
pub(crate) struct Conn(Box<dyn Io>);

impl Conn {
    pub(crate) fn new(io: impl Io + 'static) -> Self {
        Self(Box::new(io))
    }
}

/// Connects to a target the way the environment's proxy settings say.
#[derive(Clone)]
pub(crate) struct Connector {
    tls: Tls,
    target: HttpConnector<Lookups>,
    /// Reaches a proxy itself, looking its host up with plain lookups alone.
    proxy: HttpConnector<PlainLookups>,
    env: Arc<ProxyEnv>,
}

/// A TCP connector over `lookups` that dials whatever scheme it is handed:
/// the connector above it has already decided the scheme.
fn tcp<R>(lookups: R) -> HttpConnector<R> {
    let mut tcp = HttpConnector::new_with_resolver(lookups);
    tcp.enforce_http(false);
    tcp
}

impl Connector {
    pub(crate) fn new(
        tls: &Tls,
        target: Lookups,
        proxy_host: PlainLookups,
        env: Arc<ProxyEnv>,
    ) -> Self {
        Self {
            tls: tls.clone(),
            target: tcp(target),
            proxy: tcp(proxy_host),
            env,
        }
    }

    /// Connects, and speaks TLS to an `https` target. Plaintext is for an
    /// `http` target or an `http://` proxy alone: any other target is refused
    /// before it is dialled.
    async fn connect(mut self, target: Uri) -> Result<Conn, ConnectError> {
        let verified = match (target.scheme(), target.host()) {
            (Some(scheme), _) if *scheme == Scheme::HTTP => None,
            (Some(scheme), Some(host)) if *scheme == Scheme::HTTPS && !host.is_empty() => {
                Some(host.to_owned())
            }
            _ => return Err(ConnectError::Unverifiable),
        };
        let env = Arc::clone(&self.env);
        let stream = match select(&env, &target) {
            Route::Direct => {
                let tcp = self.target.call(target).await;
                Conn::new(tcp.map_err(ConnectError::reaching)?.into_inner())
            }
            Route::Tunnel(proxy) => self.tunnel(proxy, &target).await?,
            Route::Refused => {
                let refused = io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "the proxy the environment names cannot be used",
                );
                return Err(ConnectError::Tcp(Box::new(refused)));
            }
        };
        match verified {
            Some(host) => self
                .tls
                .wrap(stream, &host)
                .await
                .map_err(ConnectError::Tls),
            None => Ok(stream),
        }
    }

    /// Opens a tunnel to `target` through `proxy`: TCP to the proxy, TLS to
    /// it when it is `https://`, then `CONNECT`.
    async fn tunnel(&mut self, proxy: &ConnectProxy, target: &Uri) -> Result<Conn, ConnectError> {
        let tcp = self.proxy.call(proxy.uri().clone()).await;
        let tcp = Conn::new(tcp.map_err(ConnectError::reaching)?.into_inner());
        let leg = match proxy.leg() {
            Leg::Tls(host) => self
                .tls
                .wrap(tcp, host)
                .await
                .map_err(ConnectError::ProxyTls)?,
            Leg::Plain => tcp,
        };
        let leg = Conn::new(HeadFirst::new(leg));
        let destination = with_port(target).map_err(|error| ConnectError::Tunnel(error.into()))?;
        let mut tunnel =
            Tunnel::new(proxy.uri().clone(), Opened(Some(leg))).with_headers(proxy.headers());
        let opened = tunnel.call(destination).await;
        Ok(opened
            .map_err(|error| ConnectError::Tunnel(error.into()))?
            .into_inner())
    }
}

/// `target`'s host with its port written out, since a tunnel otherwise
/// assumes 443 whatever the scheme.
fn with_port(target: &Uri) -> Result<Uri, hyper::http::Error> {
    let secure = target.scheme() == Some(&Scheme::HTTPS);
    let port = target.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let host = target.host().unwrap_or_default();
    Uri::builder().authority(format!("{host}:{port}")).build()
}

/// The most of a proxy's answer to `CONNECT` read ahead: the size of the
/// buffer hyper-util's tunnel reads that answer into, and so the longest head
/// it accepts.
const MAX_ANSWER: usize = 8 * 1024;

/// A connection to a proxy that hands the proxy's answer to `CONNECT` on in
/// one piece ending with its head, the way hyper-util's tunnel expects it.
///
/// The tunnel judges the bytes it has after each read: a first read shorter
/// than `HTTP/1.1 200` is taken for a refusal, and a 200 whose head does not
/// end the read is waited on until the connect deadline. The previous client
/// read on until the head was whole. So here the answer is read until
/// `\r\n\r\n` ends its head, [`MAX_ANSWER`] bytes or the end of the
/// stream, whichever comes first, and only then handed on; bytes read after
/// the head are held and handed on untouched by the reads that follow, and
/// every later read goes straight to the proxy. Nothing of the answer is
/// looked at but where its head ends. A head must end in CRLF CRLF: one that
/// ends in bare LF, which the previous client accepted, is never found to
/// end. The connection then fails at the connect deadline while the proxy
/// keeps it open, and at once when the proxy closes it or when 8 KiB arrive
/// with no CRLF CRLF, since what was read is then handed to the tunnel,
/// which refuses it.
struct HeadFirst {
    io: Conn,
    /// Whether the answer is still being read ahead.
    gathering: bool,
    /// Bytes read from the proxy and not yet handed on.
    held: Vec<u8>,
    /// How many of `held` are the answer's head, to be handed on in reads of
    /// their own before anything after it.
    head: usize,
}

impl HeadFirst {
    fn new(io: Conn) -> Self {
        Self {
            io,
            gathering: true,
            held: Vec::new(),
            head: 0,
        }
    }

    /// Reads ahead until the answer's head has ended, or can grow no more.
    fn gather(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.gathering {
            if let Some(end) = self.held.windows(4).position(|four| four == b"\r\n\r\n") {
                self.head = end + 4;
                self.gathering = false;
            } else if self.held.len() >= MAX_ANSWER {
                self.head = self.held.len();
                self.gathering = false;
            } else {
                let mut chunk = [0; 1024];
                let room = MAX_ANSWER.saturating_sub(self.held.len()).min(chunk.len());
                let mut read = ReadBuf::new(chunk.get_mut(..room).unwrap_or_default());
                ready!(Pin::new(&mut *self.io.0).poll_read(cx, &mut read))?;
                if read.filled().is_empty() {
                    // The proxy closed: hand on what it said.
                    self.head = self.held.len();
                    self.gathering = false;
                }
                self.held.extend_from_slice(read.filled());
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for HeadFirst {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        ready!(this.gather(cx))?;
        if this.held.is_empty() {
            return Pin::new(&mut *this.io.0).poll_read(cx, buf);
        }
        let part = if this.head > 0 {
            this.head
        } else {
            this.held.len()
        };
        let taken = part.min(buf.remaining());
        buf.put_slice(this.held.get(..taken).unwrap_or_default());
        this.held.drain(..taken);
        this.head = this.head.saturating_sub(taken);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for HeadFirst {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

/// The connection to a proxy, already made, handed to the tunnel as the one
/// connection it asks for, so that a failure to reach the proxy is reported
/// as the step that failed rather than as the tunnel's.
struct Opened(Option<Conn>);

impl Service<Uri> for Opened {
    type Response = TokioIo<Conn>;
    type Error = io::Error;
    type Future = std::future::Ready<io::Result<TokioIo<Conn>>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: Uri) -> Self::Future {
        let leg = self.0.take().map(TokioIo::new);
        std::future::ready(leg.ok_or_else(|| io::Error::other("the proxy connection was taken")))
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

tokio::task_local! {
    /// The request whose future is being polled, for a connection started
    /// during that poll to be bound to.
    static REQUEST: Pending;
}

/// Held by a request for as long as it is waiting for its response head:
/// every connection started for it is ended once this is dropped.
#[derive(Debug)]
pub(crate) struct Waiting(watch::Sender<()>);

/// A connection's view of the request it was started for.
#[derive(Clone)]
struct Pending(watch::Receiver<()>);

/// A connection given up because the request it was for is gone.
#[derive(Debug, thiserror::Error)]
#[error("the request this connection was being made for is gone")]
struct Abandoned;

impl Waiting {
    pub(crate) fn new() -> Self {
        Self(watch::channel(()).0)
    }

    /// Runs `poll` with this request as the one any connection it starts is
    /// for.
    pub(crate) fn during<T>(&self, poll: impl FnOnce() -> T) -> T {
        REQUEST.sync_scope(Pending(self.0.subscribe()), poll)
    }
}

impl Pending {
    /// Runs `connecting` until it ends or the request is gone, whichever is
    /// first.
    async fn unless_gone<T>(
        mut self,
        connecting: impl Future<Output = Result<T, BoxError>>,
    ) -> Result<T, BoxError> {
        let mut connecting = pin!(connecting);
        // Nothing is ever sent, so this ends only when the sender is dropped.
        let mut gone = pin!(async move { while self.0.changed().await.is_ok() {} });
        poll_fn(|cx| {
            if let Poll::Ready(made) = connecting.as_mut().poll(cx) {
                return Poll::Ready(made);
            }
            ready!(gone.as_mut().poll(cx));
            Poll::Ready(Err(Box::new(Abandoned).into()))
        })
        .await
    }
}

/// A connector that makes at most [`MAX_SETUPS`] connections at once, each
/// for as long as the request it was started for is waiting.
///
/// A connection asked for past the bound waits for a slot for as long as
/// its request is waiting, and the 15 s deadline of [`Connector`] starts
/// only once it has one. Each slot is held until its connection is made,
/// fails or is given up, so behind the production connector a slot is never
/// held for more than 15 s.
///
/// hyper-util starts a connection when a request finds none idle, and if
/// another request's connection comes free first, it takes that one and
/// carries on making its own in a task of the client's, to pool it. Here
/// that connection is ended, with its slot and anything it carries, such as
/// a proxy's credential, once the request it was started for is gone:
/// dropped, or answered on the other connection. The request is the one
/// whose poll started it ([`Waiting::during`]); one started outside any
/// request's poll, which [`Http`](crate::Http) never does, is bound to
/// nothing and runs until it is made or fails.
#[derive(Clone)]
pub(crate) struct Setups<C> {
    connector: C,
    slots: Arc<Semaphore>,
}

impl<C> Setups<C> {
    pub(crate) fn new(connector: C) -> Self {
        Self {
            connector,
            slots: Arc::new(Semaphore::new(MAX_SETUPS)),
        }
    }
}

impl<C> Service<Uri> for Setups<C>
where
    C: Service<Uri> + Clone + Send + 'static,
    C::Future: Send,
    C::Error: Into<BoxError>,
{
    type Response = C::Response;
    type Error = BoxError;
    type Future = BoxFuture<'static, Result<C::Response, BoxError>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, target: Uri) -> Self::Future {
        let slots = Arc::clone(&self.slots);
        let mut connector = self.connector.clone();
        let connecting = async move {
            // Never closed, so this waits and never fails.
            let _slot = slots.acquire_owned().await?;
            poll_fn(|cx| connector.poll_ready(cx))
                .await
                .map_err(Into::into)?;
            connector.call(target).await.map_err(Into::into)
        };
        match REQUEST.try_with(Pending::clone) {
            Ok(request) => Box::pin(request.unless_gone(connecting)),
            Err(_) => Box::pin(connecting),
        }
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
